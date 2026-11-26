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
//! A declaration read by several spellings (`Status`, `Y.Status`, `lib.y.Status`) is
//! filed in the view under each, every one naming the declaration by one key, so the
//! spellings are one type, one function, one constant (Decision C of #4045).

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
    /// Where the clause names its commons, for a fix that edits the clause.
    pub target_span: bynk_syntax::span::Span,
}

/// One commons that declares a bare name two used commons share, with what a fix
/// needs: the spelling that picks it, and where the plain clause that brought it in
/// can be edited.
#[derive(Debug, Clone)]
pub struct AmbiguousProvider {
    /// The qualified spelling, `lib.y.Status` or `Y.Status`.
    pub form: String,
    /// The commons' name.
    pub target: String,
    /// An alias that would name it, `Y` for `lib.y`.
    pub alias: String,
    /// The name as the commons declares it.
    pub name: String,
    /// The end of the commons' name in a clause that brings in the whole commons,
    /// where `as Y` or `{ Status }` would go; `None` for any other clause.
    pub clause_end: Option<bynk_syntax::span::Span>,
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
            target_span: u.target.span,
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
    pub ambiguous: BTreeMap<String, Vec<AmbiguousProvider>>,
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
    // The (bare name, commons) pairs a braced list selects. A name a selection
    // brings in outranks the same name from a whole-commons clause, so `uses lib.y
    // { Status }` is the way to say which `Status` is meant.
    let mut selected_by: HashSet<(String, String)> = HashSet::new();
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
                        selected_by.insert((key.clone(), t.to_string()));
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
    for (key, all_providers) in &bare {
        // Where a braced list selects the name, only the selections compete for it.
        let selecting: Vec<(String, String)> = all_providers
            .iter()
            .filter(|(t, _)| selected_by.contains(&(key.clone(), t.clone())))
            .cloned()
            .collect();
        let providers = if selecting.is_empty() {
            all_providers
        } else {
            &selecting
        };
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
            // A suggested alias must not itself clash: with an alias the unit already
            // writes, a name it declares, a name the clauses bring in bare, or the
            // alias suggested for another provider of this name.
            let taken = |c: &str| {
                entries.iter().any(|e| e.alias.as_deref() == Some(c))
                    || local.types.contains_key(c)
                    || local.fns.contains_key(c)
                    || local_value_kind(local, c).is_some()
                    || bare.contains_key(c)
            };
            let mut forms: Vec<AmbiguousProvider> = Vec::new();
            for (t, o) in providers {
                let es: Vec<&UseEntry> = entries.iter().filter(|e| e.target == *t).collect();
                let chosen: Vec<&str> = forms.iter().map(|f| f.alias.as_str()).collect();
                forms.push(ambiguous_provider(t, o, &es, &|c| {
                    taken(c) || chosen.contains(&c)
                }));
            }
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

/// The alias to suggest for `target`: its last segment capitalised (`lib.y` →
/// `Y`), then the segments before it joined on (`LibY`), then a number, taking the
/// first that `taken` does not refuse.
fn suggested_alias(target: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let capitalised = |seg: &str| {
        let mut chars = seg.chars();
        chars.next().map_or(String::new(), |c| {
            c.to_uppercase().collect::<String>() + chars.as_str()
        })
    };
    let segments: Vec<&str> = target.split('.').collect();
    for from in (0..segments.len()).rev() {
        let candidate: String = segments[from..].iter().map(|s| capitalised(s)).collect();
        if !taken(&candidate) {
            return candidate;
        }
    }
    let base: String = segments.iter().map(|s| capitalised(s)).collect();
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|c| !taken(c))
        .unwrap_or(base)
}

fn ambiguous_provider(
    target: &str,
    name: &str,
    entries: &[&UseEntry],
    taken: &dyn Fn(&str) -> bool,
) -> AmbiguousProvider {
    let alias = suggested_alias(target, taken);
    AmbiguousProvider {
        form: qualified_key(target, entries, name),
        target: target.to_string(),
        alias,
        name: name.to_string(),
        clause_end: entries
            .iter()
            .find(|e| e.alias.is_none() && e.selected.is_none())
            .map(|e| {
                bynk_syntax::span::Span::new_in(
                    e.target_span.file,
                    e.target_span.end,
                    e.target_span.end,
                )
            }),
    }
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
    ambiguous: &BTreeMap<String, Vec<AmbiguousProvider>>,
) -> (UnitTable, Vec<(usize, CompileError)>) {
    let mut errors: Vec<(usize, CompileError)> = Vec::new();
    // Qualified names read: declared name -> each qualified spelling written, in
    // `uses` clause order. Every spelling of a declaration resolves to the one declaration
    // (Decision C of #4045): filed under each, all of them naming it by one key.
    let mut qualified: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let bare_key_of = |name: &str| bare.iter().find(|(_, o)| o == name).map(|(k, _)| k.clone());
    for e in entries {
        for r in &e.qualified {
            if !public_names(used).contains(&r.name) {
                errors.push((e.file, unknown_name_at(target, r)));
                continue;
            }
            let spellings = qualified.entry(r.name.clone()).or_default();
            if !spellings.contains(&r.text) {
                spellings.push(r.text.clone());
            }
        }
    }
    // The key each declared name is named by: its bare one, else the first
    // qualified spelling written (in clause order), else the unit's canonical qualified one. A type's
    // references to the commons' other types are renamed to these.
    let key_of = |name: &str| -> String {
        bare_key_of(name)
            .filter(|k| !ambiguous.contains_key(k))
            .or_else(|| qualified.get(name).and_then(|v| v.first().cloned()))
            .or_else(|| bare_key_of(name))
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
    // `file(name, spelling, view)` files declaration `name` under `spelling`, the
    // declaration itself named by its one key (`key_of`), so two spellings are one
    // type, one function, one constant.
    let file = |name: &str, spelling: String, view: &mut UnitTable| {
        let key = key_of(name);
        if let Some(d) = used.types.get(name) {
            let decl = rename_consumed_decl(d, &key, &renames);
            view.types.insert(spelling.clone(), decl);
            if let Some(mt) = used.methods.get(name) {
                view.methods
                    .insert(spelling.clone(), rename_methods(mt, &key, &renames));
            }
        }
        if let Some(f) = used.fns.get(name) {
            view.fns.insert(
                spelling.clone(),
                rename_fn(f, FnName::Free(rekey(name, &key, f)), &renames),
            );
        }
        if let Some(a) = used.assets.get(name) {
            let mut a = a.clone();
            a.name.name = key.clone();
            view.assets.insert(spelling.clone(), a);
        }
        if let Some(e) = used.embeds.get(name) {
            let mut e = e.clone();
            e.name.name = key.clone();
            view.embeds.insert(spelling.clone(), e);
        }
        if let Some(c) = used.consts.get(name) {
            let mut c = c.clone();
            c.name.name = key.clone();
            rename_type_ref(&mut c.ty, &renames);
            view.consts.insert(spelling.clone(), c);
        }
        if spelling != name {
            view.use_origin.insert(spelling, name.to_string());
        }
    };
    for (key, orig) in bare {
        file(orig, key.clone(), &mut view);
    }
    for (orig, spellings) in &qualified {
        for spelling in spellings {
            file(orig, spelling.clone(), &mut view);
        }
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
/// (`bynk.uses.unknown_name`).
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
        let forms = forms.clone();
        // The ambiguity is the better answer where a local value also hides one of
        // the imports: the reference is unresolved either way.
        out.insert(
            name.clone(),
            crate::resolver::ShadowedImport {
                unit: forms.first().map(|f| f.target.clone()).unwrap_or_default(),
                imported: if is_type { "type" } else { "ambiguous" },
                local: "ambiguous",
                local_span: bynk_syntax::span::Span::default(),
                ambiguous: forms,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    const X: (&str, &str) = (
        "lib/x.bynk",
        "commons lib.x\n\ntype Status = | Open | Shut\n\nfn describe(s: Status) -> String {\n  \"x\"\n}\n\nconst Limit: Int = 1\n",
    );
    const Y: (&str, &str) = (
        "lib/y.bynk",
        "commons lib.y\n\ntype Status = | On | Off\n\nfn describe(s: Status) -> String {\n  \"y\"\n}\n\nconst Limit: Int = 2\n",
    );

    fn app(body: &str) -> crate::testkit::Analysed {
        analyse(&[X, Y, ("app/z.bynk", &format!("commons app.z\n\n{body}"))])
    }

    /// #4045: two used commons that declare one name are legal until it is written.
    #[test]
    fn two_whole_uses_sharing_a_name_pass_until_it_is_written() {
        app("uses lib.x\nuses lib.y\n\nfn f() -> Int {\n  1\n}\n").assert_clean();
    }

    #[test]
    fn writing_a_shared_name_bare_is_ambiguous_for_types_values_and_constants() {
        for body in [
            "uses lib.x\nuses lib.y\n\nfn f(s: Status) -> Int {\n  1\n}\n",
            "uses lib.x\nuses lib.y\n\nfn f() -> Int {\n  Limit\n}\n",
        ] {
            app(body).assert_reports("bynk.uses.ambiguous_name");
        }
        let a = app("uses lib.x\nuses lib.y\n\nfn f(s: Status) -> Int {\n  1\n}\n");
        let msg = a.render();
        assert!(
            msg.contains("lib.x.Status") && msg.contains("lib.y.Status"),
            "{msg}"
        );
    }

    /// An alias qualifies, and bare names of the other commons still resolve.
    #[test]
    fn an_alias_reaches_the_commons_names_qualified() {
        app("uses lib.x\nuses lib.y as Y\n\nfn f(s: Status, t: Y.Status) -> String {\n  Y.describe(t)\n}\n\nfn g() -> Int {\n  Y.Limit\n}\n")
            .assert_clean();
    }

    /// A braced list selects and renames.
    #[test]
    fn a_braced_list_selects_and_renames() {
        app("uses lib.x\nuses lib.y { Status as YStatus, describe as describeY }\n\nfn f(s: Status, t: YStatus) -> String {\n  describeY(t)\n}\n")
            .assert_clean();
        // Unlisted names are not in scope.
        app("uses lib.y { Status }\n\nfn f() -> Int {\n  Limit\n}\n")
            .assert_reports("bynk.resolve.unknown_name");
    }

    /// A commons' full name qualifies a name the clauses leave ambiguous bare.
    #[test]
    fn the_full_commons_name_qualifies() {
        app("uses lib.x\nuses lib.y\n\nfn f(s: lib.x.Status, t: lib.y.Status) -> String {\n  lib.y.describe(t)\n}\n")
            .assert_clean();
    }

    #[test]
    fn a_selected_name_that_does_not_exist_is_unknown() {
        app("uses lib.y { Nope }\n\nfn f() -> Int {\n  1\n}\n")
            .assert_reports("bynk.uses.unknown_name");
        app("uses lib.y as Y\n\nfn f() -> Int {\n  Y.Nope\n}\n")
            .assert_reports("bynk.uses.unknown_name");
    }

    /// Every spelling of one declaration is that declaration (Decision C of #4045):
    /// one type, so a value of one spelling is a value of the other.
    #[test]
    fn every_spelling_of_one_declaration_is_one_type() {
        app("uses lib.y { Status }\n\nfn f(a: Status, b: lib.y.Status) -> Status {\n  b\n}\n\nfn g(a: lib.y.Status) -> Status {\n  a\n}\n")
            .assert_clean();
        app("uses lib.y as Y\n\nfn f(a: Y.Status) -> lib.y.Status {\n  a\n}\n\nfn g(a: lib.y.Status) -> Y.Status {\n  a\n}\n")
            .assert_clean();
        app("uses lib.y { Status }\nuses lib.y as Y\n\nfn f(a: Status, b: Y.Status, c: lib.y.Status) -> Bool {\n  a == b && b == c\n}\n")
            .assert_clean();
    }

    /// A match over a value reads its variants whichever spelling typed it.
    #[test]
    fn a_match_works_across_spellings() {
        app("uses lib.y { Status }\n\nfn f(s: lib.y.Status) -> Int {\n  match s {\n    On => 1,\n    Off => 0,\n  }\n}\n")
            .assert_clean();
    }

    /// A function and a constant reached by two spellings are one declaration.
    #[test]
    fn a_function_and_a_constant_read_by_two_spellings_are_each_one() {
        app("uses lib.y\n\nfn f(s: Status) -> String {\n  lib.y.describe(s)\n}\n\nfn k() -> Int {\n  Limit + lib.y.Limit\n}\n")
            .assert_clean();
    }

    /// A one-segment commons name is no qualifier: `keys.map(…)` is a method call.
    #[test]
    fn a_one_segment_commons_name_is_not_a_qualifier() {
        let a = analyse(&[
            ("keys.bynk", "commons keys\n\nfn one() -> Int {\n  1\n}\n"),
            (
                "app.bynk",
                "commons app\n\nuses keys\n\nfn f(keys: List[Int]) -> List[Int] {\n  keys.map((k: Int) => k)\n}\n",
            ),
        ]);
        a.assert_clean();
    }

    /// The types a selected declaration mentions come along under their own
    /// names, so it still checks, and a local declaration wins over an import.
    #[test]
    fn a_selected_declaration_keeps_the_types_it_mentions() {
        app("uses lib.x\nuses lib.y { describe as describeY }\n\nfn f(t: lib.y.Status) -> String {\n  describeY(t)\n}\n")
            .assert_clean();
    }

    /// A reference through a rename or a qualifier is a reference to the
    /// declaration in the commons it comes from, so navigation, hover and
    /// semantic tokens reach it as they do a bare one.
    #[test]
    fn a_renamed_or_qualified_reference_resolves_to_the_declaration() {
        let files = [
            X,
            Y,
            (
                "lib/billing.bynk",
                "commons lib.billing\n\ntype Amount = Int where Positive\n",
            ),
            (
                "app/z.bynk",
                "commons app.z\n\nuses lib.x\nuses lib.y { Status as YStatus }\nuses lib.billing as B\n\n\
                 fn f(s: Status, t: YStatus, c: B.Amount) -> Int {\n  1\n}\n",
            ),
        ];
        let a = analyse(&files);
        a.assert_clean();
        // How many of a declaration's references are in the importing file.
        let refs_in_app = |unit: &str, name: &str| {
            a.analysis
                .index
                .symbols
                .iter()
                .find(|(k, _)| k.unit == unit && k.name == name)
                .map_or(0, |(_, e)| {
                    e.refs
                        .iter()
                        .filter(|r| r.path.ends_with("app/z.bynk"))
                        .count()
                })
        };
        assert_eq!(
            refs_in_app("lib.y", "Status"),
            1,
            "the rename reaches lib.y's Status"
        );
        assert_eq!(
            refs_in_app("lib.billing", "Amount"),
            1,
            "the alias reaches lib.billing's Amount"
        );
        assert_eq!(
            refs_in_app("lib.x", "Status"),
            1,
            "the bare name still reaches lib.x's"
        );
    }

    const Z: (&str, &str) = ("lib/w.bynk", "commons lib.w\n\ntype Status = | Up | Down\n");

    fn categories_of(a: &crate::testkit::Analysed) -> Vec<String> {
        a.analysis
            .errors
            .iter()
            .map(|e| e.error.category.to_string())
            .collect()
    }

    /// The suggestions on the first `bynk.uses.ambiguous_name` of `a`, as (title,
    /// edits).
    fn fixes_of(
        a: &crate::testkit::Analysed,
    ) -> Vec<(String, Vec<(bynk_syntax::span::Span, String)>)> {
        let err = a
            .analysis
            .errors
            .iter()
            .find(|e| e.error.category == "bynk.uses.ambiguous_name")
            .expect("the ambiguity is reported");
        assert!(
            err.error
                .suggestions
                .iter()
                .all(|s| { s.applicability == bynk_syntax::error::Applicability::MaybeIncorrect }),
            "each fix may change what other names mean"
        );
        err.error
            .suggestions
            .iter()
            .map(|s| (s.message.clone(), s.edits.clone()))
            .collect()
    }

    /// `src` with `edits` applied, last first so the offsets stay true.
    fn apply(src: &str, edits: &[(bynk_syntax::span::Span, String)]) -> String {
        let mut edits = edits.to_vec();
        edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
        let mut out = src.to_string();
        for (span, text) in edits {
            out.replace_range(span.start..span.end, &text);
        }
        out
    }

    /// Apply every fix offered for `body` to the unit and analyse the result with
    /// `others`, asserting each one leaves no `bynk.uses.ambiguous_name`.
    fn every_fix_resolves(body: &str, others: &[(&str, &str)]) -> Vec<String> {
        let src = format!("commons app.z\n\n{body}");
        let mut files: Vec<(&str, &str)> = others.to_vec();
        files.push(("app/z.bynk", &src));
        let a = analyse(&files);
        let fixes = fixes_of(&a);
        assert!(!fixes.is_empty(), "no fix offered");
        let mut titles = Vec::new();
        for (title, edits) in fixes {
            let fixed = apply(&src, &edits);
            let mut files: Vec<(&str, &str)> = others.to_vec();
            files.push(("app/z.bynk", &fixed));
            let after = analyse(&files);
            assert!(
                !categories_of(&after)
                    .iter()
                    .any(|c| c == "bynk.uses.ambiguous_name"),
                "`{title}` leaves the name ambiguous:\n{fixed}\n{}",
                after.render()
            );
            titles.push(title);
        }
        titles
    }

    /// #4045: the fixes on `bynk.uses.ambiguous_name`, which are edits of the plain
    /// `uses` clause that brought a commons in. None is applied by `--fix`, since
    /// each changes what the file's other names mean; each, applied, settles the name.
    #[test]
    fn the_ambiguity_offers_fixes_that_settle_it() {
        let titles = every_fix_resolves(
            "uses lib.x\nuses lib.y\n\nfn f(s: Status) -> Int {\n  1\n}\n",
            &[X, Y],
        );
        for want in ["alias `lib.x` as `X`", "alias `lib.y` as `Y`"] {
            assert!(titles.iter().any(|t| t == want), "{want}: {titles:?}");
        }
        assert!(
            titles
                .iter()
                .any(|t| t.starts_with("select `Status` from `lib.x`"))
                && titles
                    .iter()
                    .any(|t| t.starts_with("select `Status` from `lib.y`")),
            "{titles:?}"
        );
    }

    /// With three commons declaring the name, aliasing one leaves two, so only the
    /// selections are offered, and they settle it.
    #[test]
    fn three_providers_are_settled_by_selecting_not_by_aliasing_one() {
        let titles = every_fix_resolves(
            "uses lib.x\nuses lib.y\nuses lib.w\n\nfn f(s: Status) -> Int {\n  1\n}\n",
            &[X, Y, Z],
        );
        assert!(titles.iter().all(|t| t.starts_with("select")), "{titles:?}");
        assert_eq!(titles.len(), 3, "{titles:?}");
    }

    /// A provider whose clause is already a selection has nothing to edit, so it
    /// gets no fix; two selections of one name are still ambiguous.
    #[test]
    fn a_provider_that_is_already_a_selection_gets_no_fix() {
        let a = app(
            "uses lib.x { Status }\nuses lib.y { Status }\n\nfn f(s: Status) -> Int {\n  1\n}\n",
        );
        a.assert_reports("bynk.uses.ambiguous_name");
        let err = a
            .analysis
            .errors
            .iter()
            .find(|e| e.error.category == "bynk.uses.ambiguous_name")
            .expect("reported");
        assert!(
            err.error.suggestions.is_empty(),
            "{:?}",
            err.error.suggestions
        );
        // Beside a whole `uses`, one selection settles it.
        app("uses lib.x { Status }\nuses lib.y\n\nfn f(s: Status) -> Int {\n  1\n}\n")
            .assert_clean();
    }

    /// A suggested alias never clashes with an alias the unit writes or a name it
    /// declares, nor repeats for the other provider.
    #[test]
    fn a_suggested_alias_avoids_the_names_already_taken() {
        let a = analyse(&[
            X,
            (
                "app/y.bynk",
                "commons app.y\n\ntype Status = | Hot | Cold\n",
            ),
            (
                "app/z.bynk",
                "commons app.z\n\nuses lib.x\nuses app.y\nuses lib.w as Y\n\ntype X = Int\n\nfn f(s: Status) -> Int {\n  1\n}\n",
            ),
            Z,
        ]);
        let titles: Vec<String> = fixes_of(&a).into_iter().map(|(t, _)| t).collect();
        // `X` is a local type and `Y` an alias already in use.
        assert!(
            titles.iter().any(|t| t == "alias `lib.x` as `LibX`"),
            "{titles:?}"
        );
        assert!(
            titles.iter().any(|t| t == "alias `app.y` as `AppY`"),
            "{titles:?}"
        );
    }

    /// #4397: a sum's `embeds E as V` clause names a type, so a `?` that converts an `E`
    /// error still does when the unit reads the sum and `E` by a commons' full name.
    #[test]
    fn an_embeds_clause_follows_the_spelling_a_qualified_sum_is_read_by() {
        let io = |unit: &str, variant: &str| {
            format!(
                "commons {unit}\n\ntype IoError = enum {{ {variant} }}\n\ntype AppError =\n  | Io(reason: IoError)\n  | Other\n  embeds IoError as Io\n"
            )
        };
        let (x, y) = (io("lib.x", "Disk"), io("lib.y", "Net"));
        analyse(&[
            ("lib/x.bynk", &x),
            ("lib/y.bynk", &y),
            (
                "app/z.bynk",
                "commons app.z\n\nuses lib.x\nuses lib.y\n\nfn embed(r: Result[Int, lib.x.IoError]) -> Result[Int, lib.x.AppError] {\n  let n = r?\n  Ok(n)\n}\n",
            ),
        ])
        .assert_clean();
    }
}
