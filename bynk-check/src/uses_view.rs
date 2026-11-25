//! `uses` with a braced list, an alias and qualified names (#4045).
//!
//! A unit's `uses` clauses say which of a commons' public names it sees and under
//! what spelling: the whole commons (`uses lib.y`), a selection with renames
//! (`uses lib.y { Status as YStatus, describe }`), or an alias that is the
//! qualifier of a name (`uses lib.y as Y`, then `Y.Status`). The compiler reaches
//! a declaration by one string, its name, so this module gives each unit a *view*
//! of every commons whose names are not simply its own: a copy of the commons'
//! public table with each selected or qualified declaration filed under the
//! spelling the unit uses and every type reference inside it renamed to match.
//! The composers that read a used table (`compose_unit_symbols` and the asset,
//! embed and constant ones) read the view instead, so they need no rules of their
//! own.
//!
//! Two used commons that declare one name are legal until the name is referenced
//! bare (`bynk.uses.ambiguous_name`, [`UnitTable::ambiguous_uses`]); a name a
//! `uses` clause renames or qualifies cannot clash.
//!
//! One declaration is reached by one spelling in a unit, because a type's identity
//! is its name. Reading it by a second spelling (`Y.Status` beside a selected
//! `Status`) is `bynk.uses.second_spelling`, never a second type.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::ast::{
    ConsumesItem, FnDecl, FnName, Ident, QualifiedRef, TypeBody, TypeDecl, TypeRef, UsesDecl,
};
use bynk_syntax::error::CompileError;

use crate::resolver::MethodTable;
use crate::symbols::{UnitTable, local_value_kind, rename_consumed_decl, rename_type_ref};

/// One `uses` clause as the views read it.
#[derive(Debug, Clone)]
pub struct UseEntry {
    pub target: String,
    pub alias: Option<String>,
    pub selected: Option<Vec<ConsumesItem>>,
    pub qualified: Vec<QualifiedRef>,
    /// Index of the file the clause is in, for attributing a diagnostic.
    pub file: usize,
}

impl UseEntry {
    /// The entry of a parsed clause, in the file at `file`.
    pub fn of(u: &UsesDecl, file: usize) -> Self {
        UseEntry {
            target: u.target.joined(),
            alias: u.alias.as_ref().map(|a| a.name.clone()),
            selected: u.selected.clone(),
            qualified: u.qualified_refs.clone(),
            file,
        }
    }

    fn is_plain(&self) -> bool {
        self.alias.is_none() && self.selected.is_none() && self.qualified.is_empty()
    }
}

/// The views of one unit, and the errors building them found.
#[derive(Default)]
pub struct UnitViews {
    pub views: BTreeMap<String, UnitTable>,
    pub ambiguous: BTreeMap<String, Vec<String>>,
    /// `(file index, error)`.
    pub errors: Vec<(usize, CompileError)>,
}

/// Every public name of `t`, by name: types, functions, assets, embeds and
/// constants share one namespace in a unit.
fn public_names(t: &UnitTable) -> Vec<String> {
    let mut v: Vec<String> = t
        .types
        .keys()
        .chain(t.fns.keys())
        .chain(t.assets.keys())
        .chain(t.embeds.keys())
        .chain(t.consts.keys())
        .cloned()
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The spelling under which a declaration `name` of commons `target` is reached
/// when it has no bare one: through the first alias, else the full name.
fn qualified_key(target: &str, entries: &[&UseEntry], name: &str) -> String {
    let qualifier = entries
        .iter()
        .find_map(|e| e.alias.clone())
        .unwrap_or_else(|| target.to_string());
    format!("{qualifier}.{name}")
}

/// Build the views of `unit` from its `uses` clauses. `table_of` answers a used
/// commons' table; `local` is the unit's own, whose declarations win over every
/// imported name.
pub fn build_unit_views<'a>(
    entries: &[UseEntry],
    local: &UnitTable,
    table_of: impl Fn(&str) -> Option<&'a UnitTable>,
) -> UnitViews {
    let mut out = UnitViews::default();
    let mut targets: Vec<&str> = Vec::new();
    for e in entries {
        if !targets.contains(&e.target.as_str()) {
            targets.push(&e.target);
        }
    }
    // The bare names each clause brings in: key, declared name.
    let mut bare: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut bare_of: HashMap<&str, Vec<(String, String)>> = HashMap::new();
    for t in &targets {
        let Some(used) = table_of(t) else { continue };
        let used = used.importable();
        let names = public_names(&used);
        let mut mine: Vec<(String, String)> = Vec::new();
        for e in entries.iter().filter(|e| e.target == *t) {
            match (&e.alias, &e.selected) {
                (Some(_), _) => {}
                (None, Some(items)) => {
                    for item in items {
                        if !names.contains(&item.name.name) {
                            out.errors.push((e.file, unknown_name(t, &item.name)));
                            continue;
                        }
                        let key = item.alias.as_ref().unwrap_or(&item.name).name.clone();
                        mine.push((key, item.name.name.clone()));
                    }
                }
                (None, None) => mine.extend(names.iter().map(|n| (n.clone(), n.clone()))),
            }
        }
        for (key, orig) in &mine {
            let providers = bare.entry(key.clone()).or_default();
            if !providers.iter().any(|(pt, po)| pt == t && po == orig) {
                providers.push((t.to_string(), orig.clone()));
            }
        }
        bare_of.insert(t, mine);
    }
    // A bare name two commons bring in is ambiguous where referenced, unless the
    // unit declares it itself, which wins.
    // Types and values compose into separate tables, so a bare name has a
    // first provider of each.
    let as_type = |t: &str, o: &str| table_of(t).is_some_and(|u| u.types.contains_key(o));
    let as_value = |t: &str, o: &str| {
        table_of(t).is_some_and(|u| {
            u.fns.contains_key(o)
                || u.assets.contains_key(o)
                || u.embeds.contains_key(o)
                || u.consts.contains_key(o)
        })
    };
    let mut winners: HashMap<(String, bool), (String, String)> = HashMap::new();
    for (key, providers) in &bare {
        for p in providers {
            if as_type(&p.0, &p.1) {
                winners
                    .entry((key.clone(), true))
                    .or_insert_with(|| p.clone());
            }
            if as_value(&p.0, &p.1) {
                winners
                    .entry((key.clone(), false))
                    .or_insert_with(|| p.clone());
            }
        }
        // A declaration of the unit's own hides the imports of its kind: a local
        // type or function the types, any local value the values.
        let hides_types = local.types.contains_key(key) || local.fns.contains_key(key);
        let hides_values = local_value_kind(local, key).is_some();
        let effective = providers
            .iter()
            .filter(|(t, o)| (as_type(t, o) && !hides_types) || (as_value(t, o) && !hides_values))
            .count();
        if effective > 1 {
            let forms = providers
                .iter()
                .map(|(t, o)| {
                    let es: Vec<&UseEntry> = entries.iter().filter(|e| e.target == *t).collect();
                    qualified_key(t, &es, o)
                })
                .collect();
            out.ambiguous.insert(key.clone(), forms);
        }
    }
    let needs_view = |t: &str| {
        entries.iter().any(|e| e.target == t && !e.is_plain())
            || bare
                .values()
                .any(|ps| ps.len() > 1 && ps.iter().any(|(pt, _)| pt == t))
    };
    for t in &targets {
        if !needs_view(t) {
            continue;
        }
        let Some(used) = table_of(t) else { continue };
        let es: Vec<&UseEntry> = entries.iter().filter(|e| e.target == *t).collect();
        let mine: Vec<(String, String)> = bare_of
            .get(t)
            .map(|m| {
                m.iter()
                    .filter(|(k, o)| {
                        // An ambiguous value is composed under no bare spelling, so a
                        // bare reference finds nothing and reports the ambiguity; an
                        // ambiguous type stays in (its first provider's), and the
                        // naming gate reports each place it is written.
                        // Unless the unit also writes it qualified: the qualified
                        // spelling is then the type, and a second one would be another.
                        (!out.ambiguous.contains_key(k)
                            || (table_of(t).is_some_and(|u| u.types.contains_key(o))
                                && !es.iter().any(|e| e.qualified.iter().any(|r| r.name == *o))))
                            && [true, false].into_iter().any(|ty| {
                                (if ty { as_type(t, o) } else { as_value(t, o) })
                                    && winners
                                        .get(&(k.clone(), ty))
                                        .is_some_and(|(wt, wo)| wt == t && wo == o)
                            })
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let (view, errors) = build_view(t, &used.importable(), &es, &mine, &out.ambiguous);
        out.errors.extend(errors);
        out.views.insert(t.to_string(), view);
    }
    out
}

/// `bynk.uses.unknown_name`.
fn unknown_name(target: &str, name: &Ident) -> CompileError {
    CompileError::new(
        "bynk.uses.unknown_name",
        name.span,
        format!("commons `{target}` declares no public `{}`", name.name),
    )
    .with_note("a braced `uses` list names a type, function, constant, asset or embed the commons declares and does not mark `private`")
}

/// The view of `used` (commons `target`, public names only) for a unit whose
/// clauses for it are `entries`, with `bare` the (key, name) pairs it brings in
/// bare and wins.
fn build_view(
    target: &str,
    used: &UnitTable,
    entries: &[&UseEntry],
    bare: &[(String, String)],
    ambiguous: &BTreeMap<String, Vec<String>>,
) -> (UnitTable, Vec<(usize, CompileError)>) {
    let mut errors: Vec<(usize, CompileError)> = Vec::new();
    // Qualified names read: declared name -> its qualified spelling, checked.
    let mut qualified: BTreeMap<String, String> = BTreeMap::new();
    let bare_key_of = |name: &str| bare.iter().find(|(_, o)| o == name).map(|(k, _)| k.clone());
    for e in entries {
        for r in &e.qualified {
            if !public_names(used).contains(&r.name) {
                errors.push((e.file, unknown_name_at(target, r)));
                continue;
            }
            let canonical = qualified_key(target, entries, &r.name);
            let other_bare = bare_key_of(&r.name).filter(|k| !ambiguous.contains_key(k));
            if let Some(bare_key) = other_bare {
                errors.push((e.file, second_spelling(r, &bare_key)));
            } else if r.text != canonical {
                errors.push((e.file, second_spelling(r, &canonical)));
            } else {
                qualified.insert(r.name.clone(), canonical);
            }
        }
    }
    // The key each declared name is filed under: its bare one, else its
    // qualified one. A type's references to the commons' other types are
    // renamed to these.
    let key_of = |name: &str| -> String {
        bare_key_of(name)
            .or_else(|| qualified.get(name).cloned())
            .unwrap_or_else(|| qualified_key(target, entries, name))
    };
    let mut wanted: HashSet<String> = bare.iter().map(|(_, o)| o.clone()).collect();
    wanted.extend(qualified.keys().cloned());
    // A declaration that is wanted needs every type it mentions, whatever
    // spelling those take, so it checks (a hidden type is reachable only by the
    // declarations that name it).
    let mut types_needed: Vec<String> = wanted
        .iter()
        .filter(|n| used.types.contains_key(*n))
        .cloned()
        .collect();
    for n in &wanted {
        if let Some(f) = used.fns.get(n) {
            types_needed.extend(fn_type_names(f));
        }
        if let Some(c) = used.consts.get(n) {
            types_needed.extend(type_ref_names(&c.ty));
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(n) = types_needed.pop() {
        if !used.types.contains_key(&n) || !seen.insert(n.clone()) {
            continue;
        }
        types_needed.extend(decl_type_names(&used.types[&n]));
        if let Some(mt) = used.methods.get(&n) {
            for f in mt.instance.values().chain(mt.statics.values()) {
                types_needed.extend(fn_type_names(f));
            }
        }
    }
    let renames: HashMap<String, String> =
        used.types.keys().map(|n| (n.clone(), key_of(n))).collect();

    let mut view = UnitTable {
        kind: used.kind,
        ..UnitTable::default()
    };
    let file = |name: &str, key: String, view: &mut UnitTable| {
        if let Some(d) = used.types.get(name) {
            let decl = rename_consumed_decl(d, &key, &renames);
            view.types.insert(key.clone(), decl);
            if let Some(mt) = used.methods.get(name) {
                view.methods
                    .insert(key.clone(), rename_methods(mt, &key, &renames));
            }
        }
        if let Some(f) = used.fns.get(name) {
            view.fns.insert(
                key.clone(),
                rename_fn(f, FnName::Free(rekey(name, &key, f)), &renames),
            );
        }
        if let Some(a) = used.assets.get(name) {
            let mut a = a.clone();
            a.name.name = key.clone();
            view.assets.insert(key.clone(), a);
        }
        if let Some(e) = used.embeds.get(name) {
            let mut e = e.clone();
            e.name.name = key.clone();
            view.embeds.insert(key.clone(), e);
        }
        if let Some(c) = used.consts.get(name) {
            let mut c = c.clone();
            c.name.name = key.clone();
            rename_type_ref(&mut c.ty, &renames);
            view.consts.insert(key.clone(), c);
        }
        if key != name {
            view.use_origin.insert(key, name.to_string());
        }
    };
    for (key, orig) in bare {
        file(orig, key.clone(), &mut view);
    }
    for (orig, key) in &qualified {
        file(orig, key.clone(), &mut view);
    }
    for n in &seen {
        let key = key_of(n);
        if !view.types.contains_key(&key) {
            file(n, key, &mut view);
        }
    }
    (view, errors)
}

fn rekey(name: &str, key: &str, f: &FnDecl) -> Ident {
    let mut id = f.name.ident().clone();
    if id.name == name {
        id.name = key.to_string();
    }
    id
}

fn unknown_name_at(target: &str, r: &QualifiedRef) -> CompileError {
    CompileError::new(
        "bynk.uses.unknown_name",
        r.span,
        format!("commons `{target}` declares no public `{}`", r.name),
    )
}

/// `bynk.uses.second_spelling`: `r` reaches a declaration the unit already
/// reaches as `keep`.
fn second_spelling(r: &QualifiedRef, keep: &str) -> CompileError {
    CompileError::new(
        "bynk.uses.second_spelling",
        r.span,
        format!(
            "`{}` is the same declaration this unit reaches as `{keep}`",
            r.text
        ),
    )
    .with_note("a unit reads each declaration by one spelling; use `{keep}`, or change the `uses` clauses so only one reaches it".replace("{keep}", keep))
}

fn rename_methods(mt: &MethodTable, key: &str, renames: &HashMap<String, String>) -> MethodTable {
    let on = |f: &Arc<FnDecl>| {
        let name = match &f.name {
            FnName::Method {
                type_name: _,
                method_name,
            } => FnName::Method {
                type_name: Ident {
                    name: key.to_string(),
                    span: f.name.ident().span,
                },
                method_name: method_name.clone(),
            },
            other => other.clone(),
        };
        rename_fn(f, name, renames)
    };
    MethodTable {
        instance: mt
            .instance
            .iter()
            .map(|(k, f)| (k.clone(), on(f)))
            .collect(),
        statics: mt.statics.iter().map(|(k, f)| (k.clone(), on(f))).collect(),
    }
}

/// `f` named `name`, with the types its signature names renamed by `renames`.
fn rename_fn(f: &Arc<FnDecl>, name: FnName, renames: &HashMap<String, String>) -> Arc<FnDecl> {
    let mut out = (**f).clone();
    out.name = name;
    for p in &mut out.params {
        rename_type_ref(&mut p.type_ref, renames);
    }
    rename_type_ref(&mut out.return_type, renames);
    Arc::new(out)
}

fn fn_type_names(f: &FnDecl) -> Vec<String> {
    let mut v: Vec<String> = f
        .params
        .iter()
        .flat_map(|p| type_ref_names(&p.type_ref))
        .collect();
    v.extend(type_ref_names(&f.return_type));
    v
}

fn decl_type_names(d: &TypeDecl) -> Vec<String> {
    match &d.body {
        TypeBody::Record(r) => r
            .fields
            .iter()
            .flat_map(|f| type_ref_names(&f.type_ref))
            .collect(),
        TypeBody::Sum(s) => s
            .variants
            .iter()
            .flat_map(|v| v.payload.iter().flat_map(|f| type_ref_names(&f.type_ref)))
            .collect(),
        TypeBody::Refined { .. } | TypeBody::Opaque { .. } => Vec::new(),
    }
}

/// Every user type `r` names.
fn type_ref_names(r: &TypeRef) -> Vec<String> {
    let mut out = Vec::new();
    crate::project_model::type_ref_names(r, &HashSet::new(), &mut out);
    out
}

/// Attach the views of every unit to its table.
pub fn attach_use_views(
    entries: &HashMap<String, Vec<UseEntry>>,
    unit_tables: &mut HashMap<String, UnitTable>,
) -> Vec<(String, usize, CompileError)> {
    let mut errors = Vec::new();
    let mut names: Vec<&String> = entries.keys().collect();
    names.sort();
    let mut built: Vec<(String, UnitViews)> = Vec::new();
    for name in names {
        let es = &entries[name];
        if es.is_empty() {
            continue;
        }
        let Some(local) = unit_tables.get(name) else {
            continue;
        };
        let views = build_unit_views(es, local, |t| unit_tables.get(t));
        built.push((name.clone(), views));
    }
    for (name, views) in built {
        for (file, e) in views.errors {
            errors.push((name.clone(), file, e));
        }
        if let Some(table) = unit_tables.get_mut(&name) {
            table.use_views = views.views;
            table.ambiguous_uses = views.ambiguous;
        }
    }
    errors
}

/// Phase 6: attach every unit's `uses` views, reporting what building them found
/// (`bynk.uses.unknown_name`, `bynk.uses.second_spelling`).
pub fn phase_use_views(
    unit_uses: &HashMap<String, Vec<String>>,
    parsed: &[bynk_project::ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    unit_tables: &mut HashMap<String, UnitTable>,
    errors: &mut crate::project_model::ErrorSink,
) {
    let mut entries: HashMap<String, Vec<UseEntry>> = HashMap::new();
    for (name, indices) in groups {
        let valid = unit_uses.get(name).cloned().unwrap_or_default();
        for &i in indices {
            for u in parsed[i].uses() {
                if valid.contains(&u.target.joined()) {
                    entries
                        .entry(name.clone())
                        .or_default()
                        .push(UseEntry::of(u, i));
                }
            }
        }
    }
    for (_, file, e) in attach_use_views(&entries, unit_tables) {
        errors.push_for(Some(&parsed[file].identity_path()), e);
    }
}

/// Record each bare name two used commons both bring in as a shadowed import
/// whose reference is `bynk.uses.ambiguous_name`, so the reference sites that
/// report a hidden import report it ([`UnitTable::ambiguous_uses`]).
pub fn add_ambiguous_imports(
    table: &UnitTable,
    out: &mut HashMap<String, crate::resolver::ShadowedImport>,
) {
    for (name, forms) in &table.ambiguous_uses {
        let is_type = table.use_views.values().any(|v| v.types.contains_key(name));
        // The ambiguity is the better answer where a local value also hides one of
        // the imports: the reference is unresolved either way.
        out.insert(
            name.clone(),
            crate::resolver::ShadowedImport {
                unit: forms.first().cloned().unwrap_or_default(),
                imported: if is_type { "type" } else { "ambiguous" },
                local: "ambiguous",
                local_span: bynk_syntax::span::Span::default(),
                ambiguous: forms.clone(),
            },
        );
    }
}
