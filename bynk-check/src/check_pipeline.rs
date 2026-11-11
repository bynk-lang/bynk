//! The shared per-unit/per-file resolve-check core, used by both
//! `bynk-emit`'s `check_unit_files` (`Mode::Build` and `Mode::Analyse`) and
//! this crate's own [`crate::analysis::analyse_project`].
//!
//! P4.1 (#1115), the same `extract, don't duplicate` move as
//! [`crate::project_model`]: `check_unit_files`'s per-file body is identical
//! for both modes except for four `record_analyse_types` call sites (the
//! error-path exits) and the final "Analyse mode always stops here, Build
//! mode falls through to `certify`+`emit_unit`" branch. This module owns
//! everything up to (not including) that branch — [`check_file_core`]
//! returns `Some(TypedCommons)` only on the fully-clean, non-blocked path, so
//! a caller that wants to emit knows exactly when it may. The four
//! error-path recordings are unconditional here now (previously gated on
//! `mode == Mode::Analyse`) — behaviour-preserving for the `Mode::Build`
//! caller, which never took that branch anyway (`mode == Mode::Analyse`
//! gated it), and whose `exprs` sink `compile_project`'s `ProjectOutput`
//! never exposes.
//!
//! What stayed in `bynk-emit`: `Mode` itself (meaningless here — this
//! crate's own entry point has exactly one behaviour), `certify`+`emit_unit`
//! (real emission), and the decision of *whether* to record the clean-path
//! types (each caller does that itself with the `Some(TypedCommons)` this
//! module hands back — `bynk-emit`'s `Mode::Build` caller skips it,
//! `Mode::Analyse` and this crate's own entry point both call
//! [`record_analyse_types`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::checker::{self, TypedCommons, Types};
use crate::context_checks::{
    check_context_constraints, check_context_declarations, check_handler_constraints,
};
use crate::expr_types::ExprTypeSink;
use crate::hints::HintSink;
use crate::index::RefSink;
use crate::locals::LocalsSink;
use crate::project_model::{ErrorSink, UnitInfo};
use crate::requirements::RequirementSink;
use crate::resolver::{self, MethodTable as ResolverMethodTable, ResolvedCommons};
use crate::symbols::{ConsumedType, UnitTable, build_cross_context_info, combined_types_for};
use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{CommonsItem, ExprId, FnName, TypeDecl};

/// Record a file's (possibly partial) expression types into the Analyse-mode
/// sink. Called at every per-file exit in the check loop so `.`-member
/// completion and signature help get the receiver's type even when a later
/// check phase errors for the file (ADR 0094). A no-op-shaped wrapper,
/// factored out so the four error-path exits (now unconditional, see this
/// module's own doc comment) and every clean-path caller share one call.
pub fn record_analyse_types(
    exprs: &mut ExprTypeSink,
    source_path: &Path,
    synthetic: bool,
    types: &HashMap<ExprId, checker::TypedExpr>,
) {
    exprs.enter_file(source_path, synthetic);
    exprs.record_file(types);
}

/// The four parallel per-project maps `build_cross_context_info`/
/// `combined_types_for` need (their own general, map-based signature — see
/// [`UnitCheckCtx`]'s own doc comment for why the per-file core materialises
/// them from `unit_info` rather than changing that signature).
type CrossContextViews = (
    HashMap<String, UnitTable>,
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<String>>,
    HashMap<String, HashMap<String, String>>,
);

/// The project's `CrossContextViews`, built at most once and shared by every
/// context/adapter unit's [`UnitCheckCtx`] (#3229).
///
/// The views deep-clone every unit's table, bodies included, so building them
/// once per unit made a check cost units × project size: quadratic, and about
/// 85% of a 200-context `bynk check` (clone plus drop). `unit_info` is fixed for
/// the whole per-unit loop, so every unit's views were identical copies. Create
/// one cache per project, before the loop, and pass it to every
/// [`prepare_unit_check_ctx`]; it is filled on first use, so a project with no
/// context or adapter never pays for the clone.
#[derive(Default)]
pub struct CrossContextViewsCache(std::cell::OnceCell<Arc<CrossContextViews>>);

impl CrossContextViewsCache {
    fn get(&self, unit_info: &BTreeMap<String, UnitInfo>) -> Arc<CrossContextViews> {
        let views = self
            .0
            .get_or_init(|| {
                let unit_tables = unit_info
                    .iter()
                    .map(|(n, i)| (n.clone(), i.table.clone()))
                    .collect();
                let unit_uses = unit_info
                    .iter()
                    .map(|(n, i)| (n.clone(), i.uses.clone()))
                    .collect();
                let unit_consumes = unit_info
                    .iter()
                    .map(|(n, i)| (n.clone(), i.consumes.clone()))
                    .collect();
                let unit_consumes_aliases = unit_info
                    .iter()
                    .map(|(n, i)| (n.clone(), i.aliases.clone()))
                    .collect();
                Arc::new((unit_tables, unit_uses, unit_consumes, unit_consumes_aliases))
            })
            .clone();
        // The cache is built for one `unit_info` and reused for the whole
        // per-unit loop; a different map would silently get the first one's views.
        // This catches a map of a different size, not a same-sized different one.
        debug_assert_eq!(
            views.0.len(),
            unit_info.len(),
            "CrossContextViewsCache reused with a unit_info of a different size (it is built for one map)"
        );
        views
    }
}

/// v0.29.4: `build_cross_context_info` (and its `combined_types_for` helper)
/// is a general map-based function — the test-emission path calls it with
/// *synthetic* harness maps, not `unit_info` — so it keeps its parallel-map
/// signature. The per-file core only has `unit_info`, so this materialises
/// the four views that one call needs, once per unit ahead of the file loop
/// — but only for a context/adapter, `build_cross_context_info`'s only
/// caller. `UnitTable` owns every declaration body in the unit, so for every
/// other unit kind (including the seven injected first-party commons) this
/// would otherwise be a whole-project deep clone, performed and discarded,
/// once per unit.
pub struct UnitCheckCtx {
    cross_context_views: Option<Arc<CrossContextViews>>,
    /// #907: the exact set of type names `emit_context_rebrands` rebrands for
    /// this unit — names brought in via `uses` of a *commons* specifically
    /// (not a local declaration, and not a type surfaced via `consumes`,
    /// which `imported_from_kind` tags `UnitKind::Context` in
    /// `merge_consumed_exports` and which the emitter never rebrands).
    pub uses_commons_type_names: HashSet<String>,
    /// #1710: the declarations recovery skipped in the files this unit can see
    /// (its own, and the units it `uses` and `consumes`). A reference to one is
    /// a known name, not an unknown one (#1663's Decision B), so its
    /// unknown-name echo is not reported. One level of `uses`, matching
    /// `compose_unit_symbols`. A suite sees its target's skipped names because
    /// it carries its target's unit name; it is in no unit's `uses`/`consumes`.
    pub visible_broken_names: Vec<String>,
    /// #1807: the types this unit's table carries only because an imported
    /// declaration reaches them (`close_reachable_types`), each with its
    /// owning commons. Resolvable, never nameable: the naming gate
    /// (`resolver::check_hidden_type_names`) runs beside the resolver.
    pub hidden_types: BTreeMap<String, String>,
    /// #1824: the reached types this unit's own declarations shadow, which
    /// `shadow_uses` checks the unit's bodies against.
    pub shadowed: crate::project_model::ShadowedReaches,
    /// S3 (#1963): the project's `[unstable]` policy, one per project.
    pub policy: Arc<crate::stability::StabilityPolicy>,
    /// #2618: the `private` declarations of the commons this unit `uses`,
    /// which it cannot see; a reference to one is reported as
    /// `bynk.visibility.private_item` rather than as an unknown name
    /// (`resolver::rewrite_private_references`).
    pub private_imports: HashMap<String, resolver::PrivateImport>,
    /// #3070: the schemas the project's D1 migrations build, which each
    /// literal D1 statement is prepared against. `None` when the project has no
    /// migrations or the check is off.
    pub d1_sql: Option<Arc<crate::d1_sql::D1Sql>>,
}

/// Build the per-unit prelude [`check_file_core`] shares across every file
/// in the unit — see [`UnitCheckCtx`]'s own doc comment.
#[allow(clippy::too_many_arguments)]
pub fn prepare_unit_check_ctx(
    name: &str,
    kind: UnitKind,
    broken: &crate::project_model::BrokenDeclNames,
    unit_info: &BTreeMap<String, UnitInfo>,
    views: &CrossContextViewsCache,
    combined_types: &HashMap<String, Arc<TypeDecl>>,
    imported_from_kind: &HashMap<String, UnitKind>,
    reached_types: &crate::project_model::ReachedTypes,
    policy: &Arc<crate::stability::StabilityPolicy>,
) -> UnitCheckCtx {
    let cross_context_views = if kind == UnitKind::Context || kind == UnitKind::Adapter {
        Some(views.get(unit_info))
    } else {
        None
    };
    let uses_commons_type_names: HashSet<String> = imported_from_kind
        .keys()
        .filter(|n| {
            crate::resolver::compute_is_uses_commons_type(imported_from_kind, combined_types, n)
        })
        .cloned()
        .collect();
    let visible_broken_names: Vec<String> = std::iter::once(name)
        .chain(
            unit_info
                .get(name)
                .into_iter()
                .flat_map(|i| i.uses.iter().chain(i.consumes.iter()).map(String::as_str)),
        )
        .filter_map(|u| broken.get(u))
        .flatten()
        .cloned()
        .collect();
    UnitCheckCtx {
        cross_context_views,
        uses_commons_type_names,
        visible_broken_names,
        hidden_types: reached_types.hidden.clone(),
        shadowed: reached_types.shadowed.clone(),
        policy: policy.clone(),
        private_imports: unit_info
            .get(name)
            .map(|info| {
                resolver::private_imports_in(
                    &info.table,
                    info.uses
                        .iter()
                        .filter_map(|t| unit_info.get(t).map(|u| (t, &u.table))),
                )
            })
            .unwrap_or_default(),
        d1_sql: None,
    }
}

/// A hasher that `Debug` output can be streamed into, so a value is hashed
/// without first being rendered to a `String`.
struct DebugHasher(std::collections::hash_map::DefaultHasher);

impl std::fmt::Write for DebugHasher {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        use std::hash::Hash;
        s.hash(&mut self.0);
        Ok(())
    }
}

impl DebugHasher {
    fn feed(&mut self, label: &str, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        use std::hash::Hash;
        label.hash(&mut self.0);
        let _ = write!(self, "{value:?}");
    }

    /// A map in key order: `HashMap` iteration order is per-instance, so two
    /// equal maps must not hash differently.
    fn feed_map<'a, K: Ord + std::fmt::Debug + 'a, V: std::fmt::Debug + 'a>(
        &mut self,
        label: &str,
        entries: impl IntoIterator<Item = (&'a K, &'a V)>,
    ) {
        let mut entries: Vec<_> = entries.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        self.feed(label, &entries);
    }

    fn feed_set<'a, T: Ord + std::fmt::Debug + 'a>(
        &mut self,
        label: &str,
        items: impl IntoIterator<Item = &'a T>,
    ) {
        let mut items: Vec<_> = items.into_iter().collect();
        items.sort();
        self.feed(label, &items);
    }
}

/// One type's methods as digests: its name, then instance and static `(name, digest)`.
type MethodDigests<'a> = (&'a String, Vec<(&'a String, u64)>, Vec<(&'a String, u64)>);

/// The digest of each distinct shared declaration, computed once per analysis
/// round (#3243). Units share their `uses` targets' declarations as `Arc`s, so
/// the same few types and functions are part of hundreds of files' digests;
/// rendering each of them once per file was most of the digest's cost.
///
/// Keyed by address, so an address must never be reused for another declaration
/// while an entry for it exists. That is guaranteed by construction, not by
/// argument: each entry **holds a clone of the `Arc`** it was computed from, so
/// the allocation cannot be freed, and its address cannot be handed to a new
/// declaration, for as long as the cache lives (one refcount bump per distinct
/// declaration). Even a temporary `Arc` built and dropped mid-round cannot be
/// followed by a stale hash. The cache is also never kept across rounds: the
/// memo drops it when the round ends (and asserts it is empty when the next
/// begins), so it does not keep a round's declarations alive, and the parse
/// cache's fresh declarations are never confused with an earlier round's.
#[derive(Default)]
pub struct DeclDigests {
    types: HashMap<usize, (Arc<TypeDecl>, u64)>,
    fns: HashMap<usize, (Arc<bynk_syntax::ast::FnDecl>, u64)>,
}

impl DeclDigests {
    /// How many distinct declarations have been hashed.
    pub(crate) fn len(&self) -> usize {
        self.types.len() + self.fns.len()
    }
}

fn debug_digest(value: &dyn std::fmt::Debug) -> u64 {
    use std::hash::Hasher;
    let mut h = DebugHasher(std::collections::hash_map::DefaultHasher::new());
    h.feed("decl", value);
    h.0.finish()
}

impl DeclDigests {
    fn ty(&mut self, decl: &Arc<TypeDecl>) -> u64 {
        self.types
            .entry(Arc::as_ptr(decl) as usize)
            .or_insert_with(|| (Arc::clone(decl), debug_digest(&**decl)))
            .1
    }

    fn func(&mut self, decl: &Arc<bynk_syntax::ast::FnDecl>) -> u64 {
        self.fns
            .entry(Arc::as_ptr(decl) as usize)
            .or_insert_with(|| (Arc::clone(decl), debug_digest(&**decl)))
            .1
    }

    /// A map of type declarations as `(name, digest)` in key order.
    fn type_map<'a>(
        &mut self,
        entries: impl IntoIterator<Item = (&'a String, &'a Arc<TypeDecl>)>,
    ) -> Vec<(&'a String, u64)> {
        let mut v: Vec<_> = entries.into_iter().map(|(k, d)| (k, self.ty(d))).collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v
    }

    /// Each type's instance and static methods as `(name, digest)`, in key order.
    fn methods<'a>(
        &mut self,
        methods: &'a HashMap<String, ResolverMethodTable>,
    ) -> Vec<MethodDigests<'a>> {
        crate::symbols::sorted_methods(methods)
            .into_iter()
            .map(|(name, instance, statics)| {
                let instance = instance
                    .into_iter()
                    .map(|(n, d)| (n, self.func(d)))
                    .collect();
                let statics = statics
                    .into_iter()
                    .map(|(n, d)| (n, self.func(d)))
                    .collect();
                (name, instance, statics)
            })
            .collect()
    }

    fn fn_map<'a>(
        &mut self,
        entries: impl IntoIterator<Item = (&'a String, &'a Arc<bynk_syntax::ast::FnDecl>)>,
    ) -> Vec<(&'a String, u64)> {
        let mut v: Vec<_> = entries
            .into_iter()
            .map(|(k, d)| (k, self.func(d)))
            .collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v
    }
}

/// #3243 S2.3b: a digest of **everything [`check_file_core`] reads**, so a file's
/// checked output can be kept from one language-server round to the next and
/// replayed when this value is unchanged.
///
/// It is a content hash (rendered `Debug`, which includes every span and
/// `FileId`), not a pointer: the parse cache hands out freshly built decls each
/// round, so identity does not survive. Span-inclusive on purpose: a diagnostic
/// may carry a label that points into another file at an offset, and an
/// upstream edit that moves a declaration moves that offset.
///
/// The unit's own table is not rendered: `phase_symbol_tables` builds it from
/// the unit's own files, whose text is hashed (`unit_files_digest`), so only the
/// parts filled in from other units are taken
/// (`UnitTable::cross_unit_facts`).
///
/// **Adding an input to `check_file_core` or to [`UnitCheckCtx`] means adding it
/// here.** [`UnitCheckCtx`] and [`UnitTable`] are destructured exhaustively, so a
/// new field of either is a compile error until it is classified;
/// `check_file_core`'s own parameters are the same list as this function's.
#[allow(clippy::too_many_arguments)]
pub fn file_check_digest(
    name: &str,
    kind: UnitKind,
    pf: &ParsedFile,
    unit_files_digest: u64,
    unit_info: &BTreeMap<String, UnitInfo>,
    combined_types: &HashMap<String, Arc<TypeDecl>>,
    combined_fns: &HashMap<String, Arc<bynk_syntax::ast::FnDecl>>,
    combined_methods: &HashMap<String, ResolverMethodTable>,
    local_names: &HashSet<String>,
    local_methods_for_type: &HashMap<String, Vec<bynk_syntax::ast::FnDecl>>,
    consumed_types: &HashMap<String, ConsumedType>,
    imported_from: &HashMap<String, String>,
    ctx: &UnitCheckCtx,
    decls: &mut DeclDigests,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = DebugHasher(std::collections::hash_map::DefaultHasher::new());
    // The file, and the unit's other files: a label may point into a sibling.
    h.feed(
        "file",
        &(
            pf.identity_path(),
            pf.abs_path(),
            pf.kind(),
            pf.is_synthetic(),
        ),
    );
    pf.source().hash(&mut h.0);
    h.feed("unit", &(name, kind));
    unit_files_digest.hash(&mut h.0);
    // The composed symbol space.
    h.feed("types", &decls.type_map(combined_types.iter()));
    h.feed("fns", &decls.fn_map(combined_fns.iter()));
    let methods = decls.methods(combined_methods);
    h.feed("methods", &methods);
    h.feed_set("local_names", local_names.iter());
    h.feed_map("local_methods", local_methods_for_type.iter());
    h.feed_map("consumed", consumed_types.iter());
    h.feed_map("imported_from", imported_from.iter());
    // What `check_file_core` reads of `unit_info`.
    if let Some(info) = unit_info.get(name) {
        h.feed_map("flattened", info.flattened.iter());
        h.feed("cross_unit_facts", &info.table.cross_unit_facts());
    }
    h.feed_map(
        "assets",
        crate::assets::compose_unit_assets(name, unit_info).iter(),
    );
    h.feed_map(
        "embeds",
        crate::embeds::compose_unit_embeds(name, unit_info).iter(),
    );
    h.feed_map(
        "consts",
        crate::consts::compose_unit_consts(name, unit_info).iter(),
    );
    h.feed_map(
        "shadowed_imports",
        crate::project_model::shadowed_imports(name, unit_info).iter(),
    );
    // The unit's prelude. Exhaustive: a new field must be named here.
    let UnitCheckCtx {
        cross_context_views,
        uses_commons_type_names,
        visible_broken_names,
        hidden_types,
        shadowed,
        policy,
        private_imports,
        d1_sql,
    } = ctx;
    if let Some((unit_tables, unit_uses, unit_consumes, unit_consumes_aliases)) =
        cross_context_views.as_deref()
    {
        // `check_file_core` reads the views only through these two derivations.
        let mut cci = build_cross_context_info(
            name,
            unit_consumes,
            unit_consumes_aliases,
            unit_uses,
            unit_tables,
        );
        cci.flattened_caps = unit_info[name].flattened.clone();
        // The consumed type tables are the bulk of it, and are shared `Arc`s:
        // digest them once per round and render the rest.
        let consumed = std::mem::take(&mut cci.consumed_types);
        let mut by_unit: Vec<_> = consumed
            .iter()
            .map(|(unit, types)| (unit, decls.type_map(types.iter())))
            .collect();
        by_unit.sort_by(|a, b| a.0.cmp(b.0));
        h.feed("consumed_types", &by_unit);
        // Maps of maps: its own `Debug` would print hash order.
        h.feed("cross_context", &crate::oracle::cross_context(&cci));
        h.feed(
            "subscriber_types",
            &decls.type_map(combined_types_for(name, unit_tables, unit_uses).iter()),
        );
    }
    h.feed_set("uses_commons_type_names", uses_commons_type_names.iter());
    h.feed("visible_broken_names", visible_broken_names);
    h.feed("hidden_types", hidden_types);
    h.feed("shadowed", &shadowed.sorted());
    h.feed("policy", &**policy);
    h.feed_map("private_imports", private_imports.iter());
    // `D1Sql`'s `Debug` prints only database names; its content hash covers
    // every migration the check read the schemas from.
    h.feed("d1_sql", &d1_sql.as_ref().map(|d| d.digest()));
    h.0.finish()
}

/// The clean-path output of [`check_file_core`]: the typed, fully-checked
/// unit plus the per-file cross-context info that produced it — a
/// `Mode::Build` caller needs both to reach `certify`+`emit_unit` (`emit_unit`
/// takes `cross_context_for_file` as its own argument, so this avoids making
/// the caller recompute it from `ctx`/`unit_info` a second time).
pub struct FileCheckResult {
    pub typed: TypedCommons,
    pub cross_context: resolver::CrossContextInfo,
}

/// The shared resolve+check+context-checks core for one file, factored out
/// of `check_unit_files` (see this module's own doc comment). Returns
/// `Some(FileCheckResult)` only on the fully-clean, non-blocked path — the
/// signal a `Mode::Build` caller uses to know it may proceed to
/// `certify`+`emit_unit`. Every error/blocked exit records best-effort
/// partial types unconditionally (see [`record_analyse_types`]) and returns
/// `None`.
#[allow(clippy::too_many_arguments)]
pub fn check_file_core(
    name: &str,
    kind: UnitKind,
    pf: &ParsedFile,
    unit_info: &BTreeMap<String, UnitInfo>,
    combined_types: &HashMap<String, Arc<TypeDecl>>,
    combined_fns: &HashMap<String, Arc<bynk_syntax::ast::FnDecl>>,
    combined_methods: &HashMap<String, ResolverMethodTable>,
    local_names: &HashSet<String>,
    local_methods_for_type: &HashMap<String, Vec<bynk_syntax::ast::FnDecl>>,
    consumed_types: &HashMap<String, ConsumedType>,
    imported_from: &HashMap<String, String>,
    ctx: &UnitCheckCtx,
    errors: &mut ErrorSink,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    exprs: &mut ExprTypeSink,
    requirements: &mut RequirementSink,
    tys: &Arc<Types>,
) -> Option<FileCheckResult> {
    let mut emit_items: Vec<CommonsItem> = Vec::new();
    let types_in_this_file: HashSet<String> = pf
        .items()
        .iter()
        .filter_map(|it| match it {
            CommonsItem::Type(t) => Some(t.name.name.clone()),
            // Events track, slice 0 (spine #936): an `event` shares the
            // `types` namespace, so a multi-file context's method dispatch
            // treats its name the same as a `type`'s.
            CommonsItem::Event(e) => Some(e.name.name.clone()),
            _ => None,
        })
        .collect();
    for item in pf.items() {
        match item {
            CommonsItem::Type(t) => {
                emit_items.push(CommonsItem::Type(t.clone()));
            }
            CommonsItem::Fn(f) => match &f.name {
                FnName::Free(_) => emit_items.push(CommonsItem::Fn(f.clone())),
                FnName::Method { type_name, .. } => {
                    if types_in_this_file.contains(&type_name.name) {
                        emit_items.push(CommonsItem::Fn(f.clone()));
                    }
                }
            },
            CommonsItem::Capability(c) => {
                emit_items.push(CommonsItem::Capability(c.clone()));
            }
            CommonsItem::Provider(p) => {
                emit_items.push(CommonsItem::Provider(p.clone()));
            }
            CommonsItem::Service(s) => {
                emit_items.push(CommonsItem::Service(s.clone()));
            }
            CommonsItem::Agent(a) => {
                emit_items.push(CommonsItem::Agent(a.clone()));
            }
            CommonsItem::Actor(a) => {
                // Actors emit no standalone TS, but are carried so the
                // emitter can read their schemes for the verification seam.
                emit_items.push(CommonsItem::Actor(a.clone()));
            }
            CommonsItem::Messages(m) => {
                emit_items.push(CommonsItem::Messages(m.clone()));
            }
            CommonsItem::Event(e) => {
                emit_items.push(CommonsItem::Event(e.clone()));
            }
            CommonsItem::Asset(a) => {
                emit_items.push(CommonsItem::Asset(a.clone()));
            }
            CommonsItem::Embed(e) => {
                emit_items.push(CommonsItem::Embed(e.clone()));
            }
            CommonsItem::Const(c) => {
                emit_items.push(CommonsItem::Const(c.clone()));
            }
        }
    }
    for type_name in &types_in_this_file {
        if let Some(methods) = local_methods_for_type.get(type_name) {
            for m in methods {
                let already = emit_items.iter().any(|it| match it {
                    CommonsItem::Fn(existing) => match &existing.name {
                        FnName::Method {
                            type_name: t,
                            method_name: n,
                        } => match &m.name {
                            FnName::Method {
                                type_name: t2,
                                method_name: n2,
                            } => t.name == t2.name && n.name == n2.name,
                            _ => false,
                        },
                        _ => false,
                    },
                    _ => false,
                });
                if !already {
                    emit_items.push(CommonsItem::Fn(m.clone()));
                }
            }
        }
    }

    // Synthesize a "Commons-shaped" view of this file's items so we can
    // drive the existing resolver/checker without duplication.
    let synthetic_commons = pf.as_synthetic_commons(emit_items);

    // Cross-context info (v0.6) for contexts: consumed contexts, aliases,
    // services, and types. Computed once below; reused for the resolver,
    // checker, and (in `bynk-emit`) the emitter. v0.18: adapters get it too,
    // so an external provider's `given` resolves against the adapter's
    // flattened consumed capabilities (spec §4.5).
    let cross_context_for_file =
        if let Some((unit_tables, unit_uses, unit_consumes, unit_consumes_aliases)) =
            ctx.cross_context_views.as_deref()
        {
            let mut cci = build_cross_context_info(
                name,
                unit_consumes,
                unit_consumes_aliases,
                unit_uses,
                unit_tables,
            );
            cci.flattened_caps = unit_info[name].flattened.clone();
            cci
        } else {
            resolver::CrossContextInfo::default()
        };

    // Events slice 3a (#972): this unit's own local + direct-`uses` types
    // (deliberately narrower than `combined_types`, which also merges
    // `consumes`) — the same view `emit_consumed_context_helpers` (#973)
    // builds for a *subscriber* regenerating this unit's own event codecs
    // cross-context. `check_context_declarations` uses it to validate an
    // event field default is constructible in that narrower view, not just
    // this unit's own wider one.
    let subscriber_visible_types: HashMap<String, Arc<TypeDecl>> =
        if let Some((unit_tables, unit_uses, _, _)) = ctx.cross_context_views.as_deref() {
            combined_types_for(name, unit_tables, unit_uses)
        } else {
            HashMap::new()
        };

    // `ResolvedCommons::new` derives `local_type_names`/`event_type_names`
    // from this unit's own pre-merge table (`unit_info[name].table`), not
    // `combined_types` (already local+uses+consumes merged) — same
    // distinction the caller's `local_names` exists for. `Events.emit[E]`
    // additionally needs "is this specifically an event" on top of
    // owner-only emission (an ordinary local type must not pass as an emit
    // target just because it's locally declared), hence the separate
    // `events` table. Both are empty for a unit absent from `unit_info`.
    let empty_types = HashMap::new();
    let empty_events = HashMap::new();
    let local_table = unit_info.get(name).map(|i| &i.table);
    let local_types = local_table.map(|t| &t.types).unwrap_or(&empty_types);
    let local_events = local_table.map(|t| &t.events).unwrap_or(&empty_events);
    let empty_fns = HashMap::new();
    let local_fns = local_table.map(|t| &t.fns).unwrap_or(&empty_fns);

    let resolved = ResolvedCommons::new(
        Arc::new(synthetic_commons),
        combined_types.clone(),
        local_types,
        combined_fns.clone(),
        combined_methods.clone(),
        HashMap::new(),
        local_events,
        cross_context_for_file.clone(),
        // ADR 0116 D6: provenance for the `bynk.list` deprecation lint.
        imported_from.clone(),
        kind == UnitKind::Context,
        ctx.uses_commons_type_names.clone(),
    )
    .with_local_fns(local_fns)
    // #1883, slice 1a: this unit's assets plus its `uses`d commons' ([DECISION G]).
    .with_assets(crate::assets::compose_unit_assets(name, unit_info))
    // #2387, slice 1: likewise this unit's embeds and its `uses`d commons'.
    .with_embeds(crate::embeds::compose_unit_embeds(name, unit_info))
    // #2617: likewise this unit's constants and its `uses`d commons'.
    .with_consts(crate::consts::compose_unit_consts(name, unit_info))
    // #1925: imports hidden behind a local declaration of another kind.
    .with_shadowed_imports(crate::project_model::shadowed_imports(name, unit_info))
    // S3 (#1963): which names in scope are a first-party extensible enum,
    // decided here, where the unit's full provenance is known.
    .with_extensible_enums(
        imported_from
            .iter()
            .filter(|(n, unit)| crate::firstparty::is_extensible(unit, n))
            .map(|(n, _)| n.clone())
            .collect(),
    )
    // S3 (#1963): the project's policy, and this unit's blocked imports. A
    // first-party unit's own bodies are the library's business, never the
    // project's opt-in, so they check under a policy that blocks nothing.
    .with_stability(if pf.is_synthetic() {
        crate::stability::UnitStability {
            policy: Arc::new(crate::stability::StabilityPolicy::permissive()),
            blocked_imports: HashMap::new(),
        }
    } else {
        crate::stability::UnitStability {
            policy: ctx.policy.clone(),
            blocked_imports: ctx.policy.blocked_imports(imported_from),
        }
    });
    refs.enter_file(&pf.identity_path(), name, pf.is_synthetic());
    // v0.27: synthetic and test/integration files record no hints — neither
    // surfaces in an editor (the `assemble_index` rule).
    hints.enter_file(
        &pf.identity_path(),
        pf.is_synthetic() || matches!(pf.kind(), UnitKind::Test | UnitKind::Integration),
    );
    // v0.31: locals serve completion/navigation in test files too — only
    // synthetic (toolchain-injected) files are muted.
    locals.enter_file(&pf.identity_path(), pf.is_synthetic());
    // v0.99: capability requirements follow the inlay-hint muting rule —
    // synthetic and test/integration files surface none in an editor.
    requirements.enter_file(
        &pf.identity_path(),
        pf.is_synthetic() || matches!(pf.kind(), UnitKind::Test | UnitKind::Integration),
    );
    // #1663 (Decision A): a resolve error no longer stops the file before the
    // checker. Every declaration is still checked; the checker's diagnostics
    // in a declaration the resolver rejected are its echoes and are dropped
    // (`without_resolve_echoes`), and the file still fails here.
    let mut resolve_errors = resolver::resolve_file_record(&resolved, refs).err();
    // #1807: a hidden type written in this file is an unknown type, and is a
    // resolve error like any other (Decision A drops the checker's echoes).
    let hidden_named =
        resolver::check_hidden_type_names(&resolved.commons.items, &ctx.hidden_types);
    if !hidden_named.is_empty() {
        resolve_errors
            .get_or_insert_with(Vec::new)
            .extend(hidden_named);
    }
    // #1927: a `uses`-imported type a local value hides may not be written
    // either, and its errors are resolve errors the same way.
    let shadowed_named = resolver::check_shadowed_type_names(
        &resolved.commons.items,
        &resolved.shadowed_imports,
        &resolved.types,
    );
    if !shadowed_named.is_empty() {
        resolve_errors
            .get_or_insert_with(Vec::new)
            .extend(shadowed_named);
    }
    let item_spans: Vec<bynk_syntax::span::Span> =
        resolved.commons.items.iter().map(|i| i.span()).collect();
    // The unit's own span is this file's, even when it has no items of its own
    // (only methods on a sibling file's type, or only `uses`).
    let own_file = resolved.commons.span.file;
    // The unit's own span can carry no file identity; an item's always does.
    requirements.set_file_id(
        item_spans
            .iter()
            .map(|sp| sp.file)
            .find(|f| *f != bynk_syntax::span::FileId::UNKNOWN)
            .unwrap_or(own_file),
    );
    let rc = checker::check_record_in(resolved, tys, refs, hints, locals, requirements);
    if let Some(resolve_errors) = &resolve_errors {
        // #1710: a reference to a declaration recovery skipped (here or in a
        // file this unit can see) is a known name; its echo isn't reported. It
        // still counts as a resolve error below (Decision A).
        let (shown, _hidden) =
            resolver::split_broken_decl_echoes(resolve_errors.clone(), &ctx.visible_broken_names);
        errors.extend_for(
            Some(&pf.identity_path()),
            resolver::rewrite_private_references(shown, &ctx.private_imports),
        );
    }
    // Every diagnostic past the resolver goes out through this: the checker's
    // errors and warnings, and the declaration stages' (`ours`, below).
    // Decision A drops the checker's follow-ons inside a declaration the
    // resolver rejected; #1710 then drops echoes of a declaration recovery
    // skipped (a method, a capability, an actor, a consumed context's service)
    // that this unit can see. The checker reports those under its own codes,
    // in any unit kind, so the split runs here, not only in the stages.
    let unechoed = |errs: Vec<bynk_syntax::CompileError>| {
        let errs = match &resolve_errors {
            Some(r) => resolver::without_resolve_echoes(errs, r, &item_spans),
            None => errs,
        };
        resolver::rewrite_private_references(
            resolver::split_broken_decl_echoes(errs, &ctx.visible_broken_names).0,
            &ctx.private_imports,
        )
    };
    // #1663: the declaration stages walk the whole unit's handlers, so a file's
    // pass also meets another file's faults — and attributes them to this
    // file, at a position in the wrong source. That file's own pass reports
    // them; here, keep only this file's diagnostics (or unlocated ones), and
    // decide whether a stage failed this file from those alone.
    let ours = |errs: Vec<bynk_syntax::CompileError>| -> Vec<bynk_syntax::CompileError> {
        unechoed(
            errs.into_iter()
                .filter(|e| e.span.file == own_file || e.span == bynk_syntax::span::Span::default())
                .collect(),
        )
    };
    // #1663: whether this file has already failed (a resolve or type error).
    // The later stages still check its other declarations; the file returns
    // no result at the end.
    let mut failed = resolve_errors.is_some();
    let typed = match rc.result {
        Ok(t) => {
            // v0.89 (ADR 0117): a unit that checks clean may still carry
            // non-failing warnings — push them into the (severity-aware)
            // sink, where they are classified as warnings and never gate.
            if !t.warnings.is_empty() {
                errors.extend_for(Some(&pf.identity_path()), unechoed(t.warnings.clone()));
            }
            t
        }
        Err(errs) => {
            errors.extend_for(Some(&pf.identity_path()), unechoed(errs));
            // #1663 (Decision A): a type error in one declaration must not
            // hide the next stage's checks of the others (a service handler's
            // body is typed in `check_context_declarations`).
            failed = true;
            // ADR 0094: the program as checked so far carries the
            // best-effort types of every expression that did check. It is
            // not recorded here: checking goes on, and whichever exit below
            // the file reaches records it once, with the handler bodies
            // `check_context_declarations` types in the meantime.
            rc.typed_despite_errors?
        }
    };

    // Run the context-specific checks: forbidden construction, private-type
    // references.
    if kind == UnitKind::Context {
        let context_check_errs = ours(check_context_constraints(
            &typed,
            consumed_types,
            local_names,
            tys,
        ));
        if !context_check_errs.is_empty() {
            errors.extend_for(Some(&pf.identity_path()), context_check_errs);
            record_analyse_types(
                exprs,
                &pf.identity_path(),
                pf.is_synthetic(),
                &typed.expr_types,
            );
            return None;
        }
    }

    // v0.5: check capability/provider/service/agent declarations. v0.18:
    // adapters run these too — an external provider's `given` resolves
    // through the same path as a bodied provider's (the service/agent
    // checks are vacuous for adapters, which have none).
    let mut typed = typed;
    let unit_table_owned = unit_info.get(name).map(|i| i.table.clone());
    if (kind == UnitKind::Context || kind == UnitKind::Adapter)
        && let Some(table) = unit_table_owned.as_ref()
    {
        let decl_errs = ours(check_context_declarations(
            &mut typed,
            table,
            &cross_context_for_file,
            kind == UnitKind::Context,
            &ctx.uses_commons_type_names,
            &subscriber_visible_types,
            refs,
            hints,
            locals,
            requirements,
            tys,
        ));
        if !decl_errs.is_empty() {
            // ADR 0117: a warning-severity declaration diagnostic (e.g. the
            // `@indexed` hygiene hints) must not block emission — only an
            // error does. Partition first, then gate on error severity
            // alone.
            let blocks_emission = decl_errs.iter().any(|e| {
                matches!(
                    bynk_syntax::Severity::for_error(e),
                    bynk_syntax::Severity::Error
                )
            });
            errors.extend_for(Some(&pf.identity_path()), decl_errs);
            if blocks_emission {
                // ADR 0094: handler bodies are typed here — surface their
                // best-effort types even when a declaration check (e.g. a
                // service/agent wiring error) fails for the file.
                record_analyse_types(
                    exprs,
                    &pf.identity_path(),
                    pf.is_synthetic(),
                    &typed.expr_types,
                );
                return None;
            }
            // Warnings only: the declarations are valid — fall through.
        }
    }

    // #1700: the context constraints again, over the handler and provider
    // bodies `check_context_declarations` has just typed.
    if kind == UnitKind::Context {
        let handler_errs = ours(check_handler_constraints(
            &typed,
            consumed_types,
            local_names,
            tys,
        ));
        if !handler_errs.is_empty() {
            errors.extend_for(Some(&pf.identity_path()), handler_errs);
            record_analyse_types(
                exprs,
                &pf.identity_path(),
                pf.is_synthetic(),
                &typed.expr_types,
            );
            return None;
        }
    }

    // #1824: a value crossing between this unit's code and an imported
    // position that names a type the unit shadows. Last, over every body the
    // stages above have typed.
    let shadow_errs = ours(crate::shadow_uses::check_shadowed_uses(
        &typed,
        &crate::shadow_uses::ShadowScope {
            unit: name,
            shadowed: &ctx.shadowed,
            combined_types,
            combined_fns,
            combined_methods,
            imported_from,
        },
    ));
    if !shadow_errs.is_empty() {
        errors.extend_for(Some(&pf.identity_path()), shadow_errs);
        failed = true;
    }

    // #3070: each literal D1 statement of this context, prepared against the
    // schema its binding's migrations build.
    if kind == UnitKind::Context
        && let (Some(d1_sql), Some(table)) = (&ctx.d1_sql, unit_table_owned.as_ref())
    {
        let sql_errs = ours(d1_sql.check_unit(name, table, &typed.callees, Some(pf.source())));
        if !sql_errs.is_empty() {
            errors.extend_for(Some(&pf.identity_path()), sql_errs);
            failed = true;
        }
    }

    // #1663: every stage has now checked past the earlier errors; the file
    // still fails on them.
    if failed {
        record_analyse_types(
            exprs,
            &pf.identity_path(),
            pf.is_synthetic(),
            &typed.expr_types,
        );
        return None;
    }
    Some(FileCheckResult {
        typed,
        cross_context: cross_context_for_file,
    })
}

#[cfg(test)]
mod tests {
    use super::{Arc, BTreeMap, CrossContextViewsCache, DeclDigests};
    use crate::testkit::analyse;

    /// #3243 S2.4a: the digest cache is keyed by address, so it must keep every
    /// `Arc` it hashed alive: a dropped declaration's address could otherwise be
    /// reused by a new one and be handed the old hash.
    #[test]
    fn the_declaration_digests_keep_what_they_hashed_alive() {
        let source = "commons shapes\n\ntype Id = String where NonEmpty\n";
        let tokens = bynk_syntax::lexer::tokenize(source).expect("lexes");
        let commons = bynk_syntax::parser::parse(&tokens, source).expect("parses");
        let decl = commons
            .items
            .iter()
            .find_map(|i| match i {
                bynk_syntax::ast::CommonsItem::Type(t) => Some(t.clone()),
                _ => None,
            })
            .expect("a type");
        let temporary = Arc::new(decl);
        let weak = Arc::downgrade(&temporary);
        let mut digests = DeclDigests::default();
        let first = digests.ty(&temporary);
        // The caller's handle goes away, as a temporary `Arc` built mid-round would.
        drop(temporary);
        assert!(
            weak.upgrade().is_some(),
            "the cache must still hold the declaration its address is keyed by"
        );
        assert_eq!(digests.len(), 1);
        // Only dropping the cache (the end of the round) releases it.
        drop(digests);
        assert!(weak.upgrade().is_none());
        let _ = first;
    }

    /// #3229: every unit shares one set of views; a per-unit rebuild deep-cloned
    /// the whole project once per context, which made a check quadratic.
    #[test]
    fn the_cross_context_views_are_built_once_and_shared() {
        let cache = CrossContextViewsCache::default();
        let unit_info = BTreeMap::new();
        let first = cache.get(&unit_info);
        let second = cache.get(&unit_info);
        assert!(Arc::ptr_eq(&first, &second));
    }

    const PAYMENT: &str = "context shop.payment\n\nservice charge {\n  on call(n: Int) -> Effect[Int] {\n    Effect.pure(n)\n  }\n}\n";

    fn consumer(name: &str, arg: &str) -> String {
        format!(
            "context shop.{name}\n\nconsumes shop.payment\n\nservice place {{\n  on call() -> Effect[Int] {{\n    let n <- shop.payment.charge({arg})\n    Effect.pure(n)\n  }}\n}}\n"
        )
    }

    /// #3239 review: every context resolves its `consumes` through the one shared
    /// set of views, so two consumers of the same context both check, and a
    /// mistake in one is reported without disturbing the other.
    #[test]
    fn two_consumers_of_one_context_share_the_views_without_interference() {
        let ok = analyse(&[
            ("shop/payment.bynk", PAYMENT),
            ("shop/orders.bynk", &consumer("orders", "1")),
            ("shop/billing.bynk", &consumer("billing", "2")),
        ]);
        ok.assert_clean();

        let broken = analyse(&[
            ("shop/payment.bynk", PAYMENT),
            ("shop/orders.bynk", &consumer("orders", "1")),
            ("shop/billing.bynk", &consumer("billing", "\"two\"")),
        ]);
        let errors = &broken.analysis.errors;
        assert!(
            !errors.is_empty(),
            "a wrongly typed cross-context call in one consumer must be reported"
        );
        // Every error is in the consumer that made the call; the other consumer
        // and the context they share stay clean.
        assert!(
            errors.iter().all(|e| e
                .source_path
                .as_deref()
                .is_some_and(|p| p.ends_with("billing.bynk"))),
            "errors must be attributed to billing.bynk only: {:?}",
            errors.iter().map(|e| &e.source_path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_file_with_a_type_error_still_records_its_other_expression_types() {
        // ADR 0094: completion and signature help need a receiver's type even
        // when another declaration in the same file fails to check.
        let a = analyse(&[(
            "demo.bynk",
            "commons demo\n\nfn good(s: String) -> Int { s.length() }\n\nfn bad() -> Int { \"x\" }\n",
        )]);
        assert!(
            !a.error_categories().is_empty(),
            "the fixture must fail to check"
        );
        assert_eq!(a.type_at("demo.bynk", "s.length()"), "Int");
    }

    #[test]
    fn a_call_to_a_declaration_that_failed_to_parse_is_not_an_unknown_name() {
        // #1710: a broken declaration stays a known name, so its callers report
        // nothing extra; only the parse error itself is reported.
        let a = analyse(&[(
            "demo.bynk",
            "commons demo\n\nfn broken() -> Int { 1 + }\n\nfn user() -> Int { broken() }\n",
        )]);
        assert!(
            !a.error_categories().is_empty(),
            "the parse error is reported"
        );
        assert!(
            !a.categories().contains(&"bynk.resolve.unknown_function"),
            "a broken declaration's caller must not report it unknown:\n{}",
            a.render()
        );
    }

    #[test]
    fn a_clean_file_s_types_are_recorded_on_the_clean_path() {
        let a = analyse(&[(
            "demo.bynk",
            "commons demo\n\nfn f(s: String) -> Int { s.length() }\n",
        )]);
        a.assert_clean();
        assert_eq!(a.type_at("demo.bynk", "s.length()"), "Int");
    }
}
