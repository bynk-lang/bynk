//! Agent store layout, keys and wire: how an agent's persisted state is classified (`classify_agent_store`),
//! the lazy-read specs, and the key and wire codecs the state record and its persistence share.

use super::persist_json_input;
use crate::emit_model::EmitProjectCtx;
use crate::emitter::serialisation;
use crate::emitter::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use crate::emitter::{BuildTarget, CheckedProgram, Ty, UnitFacts, lower_store_field_shape_ir};
use crate::ts_build::{call, ident, member, str_lit};
use bynk_check::checker::{TyId, TypedCommons, Types};
use bynk_model::{StoreFieldIr, StoreKindIr};
use bynk_syntax::ast::{
    AgentDecl, BaseType, ExprKind, Ident, RecordField, StoreField, TypeBody, TypeDecl, TypeRef,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// True when a type has a boundary deserialiser (ADR 0124 rehydration gate). A
/// non-codec type never legally reaches a `store` position, but a `Cell[()]`
/// could, so the gate skips anything `deserialise_expr` would reject.
pub(super) fn is_codecable(t: &TypeRef) -> bool {
    matches!(
        t,
        TypeRef::Base(..)
            | TypeRef::Named(..)
            | TypeRef::Option(..)
            | TypeRef::List(..)
            | TypeRef::Result(..)
            | TypeRef::Map(..)
    )
}

/// An agent's `store` fields with the checker's resolved kind (1A.33): the kind is read from
/// the [`StoreFieldIr`], and the declaration's own type references stay the AST ones the
/// codec and key checks below take. `state` is built from `a.store_fields` in order
/// (`agent_state_ir`), so the two zip one to one.
fn store_kinds<'a, 'b>(
    a: &'a AgentDecl,
    state: &'b [StoreFieldIr],
) -> impl Iterator<Item = (&'a StoreField, &'b StoreKindIr)> {
    debug_assert_eq!(a.store_fields.len(), state.len());
    a.store_fields.iter().zip(state.iter().map(|ir| &ir.kind))
}

/// The checker's resolved `store` fields of an agent, in declaration order.
pub(crate) fn agent_state_ir(a: &AgentDecl, program: &CheckedProgram) -> Vec<StoreFieldIr> {
    a.store_fields
        .iter()
        .map(|f| lower_store_field_shape_ir(f, program))
        .collect()
}

/// Whether an agent emits a rehydration gate, and so the `commitState` that encodes through it.
/// Mirrors the per-field validation the gate builds. The `rehydrationViolation` import follows
/// from the tree (1A.36b-1), not from this.
pub(crate) fn agent_gates_rehydration(
    a: &AgentDecl,
    state: &[StoreFieldIr],
    types: &HashMap<String, Arc<TypeDecl>>,
) -> bool {
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` persists `K → connId` and
    // is rehydrated like any map — no value check (the connId is opaque) but the
    // textual `K` key is validated. The `Map` arm already counts that via the key,
    // so held maps need no special handling here.
    store_kinds(a, state).any(|(f, kind)| match kind {
        StoreKindIr::Cell(_) | StoreKindIr::Log(..) => {
            f.kind.args.first().is_some_and(is_codecable)
        }
        StoreKindIr::Map(..) | StoreKindIr::Cache(..) => {
            f.kind.args.get(1).is_some_and(is_codecable)
                || f.kind
                    .args
                    .first()
                    .is_some_and(|k| type_base_is_string(k, types))
        }
        StoreKindIr::Set(_) => f
            .kind
            .args
            .first()
            .is_some_and(|t| type_base_is_string(t, types)),
    })
}

/// Whether an agent's collections can be read lazily (#3189): it has a `Map`, `Set`, `Cache` or
/// `Log` field and none is a held `Map` (a held map keeps live connections inline in the record).
/// `loadState` still reads a wholesale record, or one a migration step covers, whole.
fn agent_reads_lazily(state: &[StoreFieldIr], tys: &Types, a: &AgentDecl) -> bool {
    held_map_fields(a, state, tys).is_empty()
        && state
            .iter()
            .any(|f| !matches!(f.kind, StoreKindIr::Cell(_)))
}

/// The `[lo, from]` schema-version ranges the migration steps cover, as the runtime's
/// `__checkStoredSchemaVersion` and `__readStored` take them.
pub(super) fn step_ranges_expr(
    step_plans: &[bynk_check::schema_registry::MigrationStep],
) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Array {
        items: step_plans
            .iter()
            .map(|s| bynk_ts::TsExpr::Array {
                items: vec![
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(s.lo.to_string())),
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(s.from.to_string())),
                ],
                multiline: false,
            })
            .collect(),
        multiline: false,
    }
}

/// The per-field decode and encode closures the storage-backed handles of a lazily read agent
/// are attached with (#3189): a stored value decodes when an entry is first read (so a corrupt
/// one faults when touched), and a written value encodes as it is staged. Returns the const's
/// name, or `None` when the agent does not read lazily.
pub(super) fn emit_lazy_field_spec(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) -> Option<String> {
    if !agent_reads_lazily(layout.state, commons.tys(), a) {
        return None;
    }
    let agent_name = &a.name.name;
    // A refined textual `Map` key or `Set` element is checked as it is read back, as the whole-record
    // gate checks every one at load (a key the program supplies is typed and is not checked).
    let key_check = |key_ty: Option<&TypeRef>, path: &str| -> Vec<bynk_ts::TsObjectEntry> {
        let Some(k) = key_ty.filter(|k| type_base_is_string(k, &commons.types)) else {
            return Vec::new();
        };
        let d = serialisation::deserialise_expr(
            k,
            &persist_json_input(ident("__k")),
            path,
            &ctx.runtime_use,
        );
        let body = vec![
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("__r".to_string()),
                None,
                d,
                None,
            ),
            rehydration_guard(agent_name),
        ];
        vec![bynk_ts::TsObjectEntry::prop(
            "checkKey",
            lazy_codec_arrow("__k", "string", body),
        )]
    };
    let codec = |ty: &TypeRef, path: &str, value_ts: &bynk_ts::TsType| {
        lazy_codec_entries(ty, path, value_ts, agent_name, ctx)
    };
    let mut spec: Vec<bynk_ts::TsExpr> = Vec::new();
    for (name, v) in &layout.store_map_fields {
        let ts = layout.map_value_ts.get(&name.name).unwrap_or_else(|| {
            panic!(
                "bynk internal error: no value type recorded for store Map `{}`",
                name.name
            )
        });
        let mut extra = codec(v, &name.name, ts);
        extra.extend(key_check(store_field_key_type(a, &name.name), &name.name));
        spec.push(lazy_field_entry(&name.name, "map", extra));
    }
    for (name, elem) in &layout.store_set_fields {
        spec.push(lazy_field_entry(
            &name.name,
            "set",
            key_check(Some(elem), &name.name),
        ));
    }
    for (name, v, _) in &layout.store_cache_fields {
        let ts = match layout.store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Cache(_, v_ty, _)) => {
                ts_ty_to_ts_type(*v_ty, UnitFacts::of(ctx).render(commons))
            }
            _ => ts_type_ref_to_ts_type(v, None),
        };
        spec.push(lazy_field_entry(
            &name.name,
            "cache",
            codec(v, &name.name, &ts),
        ));
    }
    for (name, v, _) in &layout.store_log_fields {
        let ts = layout.log_value_ts.get(&name.name).unwrap_or_else(|| {
            panic!(
                "bynk internal error: no element type recorded for store Log `{}`",
                name.name
            )
        });
        spec.push(lazy_field_entry(
            &name.name,
            "log",
            codec(v, &name.name, ts),
        ));
    }
    let name = format!("__lazyFieldsOf{agent_name}");
    stmts.push(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(name.clone()),
        None,
        bynk_ts::TsExpr::AsConst(Box::new(bynk_ts::TsExpr::array(spec))),
        None,
    ));
    Some(name)
}

/// One entry of a spec array: `{ name: "x", kind: "map", ..extra }`.
fn lazy_field_entry(name: &str, kind: &str, extra: Vec<bynk_ts::TsObjectEntry>) -> bynk_ts::TsExpr {
    let mut entries = vec![
        bynk_ts::TsObjectEntry::prop("name", str_lit(name)),
        bynk_ts::TsObjectEntry::prop("kind", str_lit(kind)),
    ];
    entries.extend(extra);
    bynk_ts::TsExpr::object_entries(entries)
}

/// `(<param>: <ty>) => { <body> }`, the compact one-line arrow of a spec's codec.
fn lazy_codec_arrow(param: &str, ty: &str, body: Vec<bynk_ts::TsStmt>) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Arrow {
        params: vec![bynk_ts::TsParam::new(
            param.to_string(),
            Some(bynk_ts::TsType::named(ty)),
        )],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Block(body)),
    }
}

/// `if (__r.tag === "Err") throw __rehydrationViolation("<agent>", __r.error);`
fn rehydration_guard(agent_name: &str) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::if_stmt(
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::StrictEq,
            left: Box::new(member(ident("__r"), "tag")),
            right: Box::new(str_lit("Err")),
        },
        bynk_ts::TsStmt::throw_stmt(
            bynk_ts::TsExpr::Call {
                callee: Box::new(ident("__rehydrationViolation")),
                args: vec![str_lit(agent_name), member(ident("__r"), "error")],
            },
            None,
        ),
        None,
    )
}

/// The `decode` and `encode` entries of a spec field (#3189): a stored value decodes when an
/// entry is first read (so a corrupt one faults when touched), and a written value encodes as it
/// is staged. `value_ts` is the field's value type: the encoder takes `unknown` and narrows it
/// once. A type with no wire form has neither.
fn lazy_codec_entries(
    ty: &TypeRef,
    path: &str,
    value_ts: &bynk_ts::TsType,
    agent_name: &str,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsObjectEntry> {
    if !is_codecable(ty) {
        return Vec::new();
    }
    let d = serialisation::deserialise_expr(
        ty,
        &persist_json_input(ident("__w")),
        path,
        &ctx.runtime_use,
    );
    let e = serialisation::serialise_expr(ty, &ident("__v"), &ctx.runtime_use);
    let decode = lazy_codec_arrow(
        "__w",
        "unknown",
        vec![
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("__r".to_string()),
                None,
                d,
                None,
            ),
            rehydration_guard(agent_name),
            bynk_ts::TsStmt::return_stmt(Some(member(ident("__r"), "value")), None),
        ],
    );
    let encode = lazy_codec_arrow(
        "__u",
        "unknown",
        vec![
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("__v".to_string()),
                None,
                bynk_ts::TsExpr::As {
                    expr: Box::new(ident("__u")),
                    ty: value_ts.clone(),
                },
                None,
            ),
            bynk_ts::TsStmt::return_stmt(Some(e), None),
        ],
    );
    vec![
        bynk_ts::TsObjectEntry::prop("decode", decode),
        bynk_ts::TsObjectEntry::prop("encode", encode),
    ]
}

/// The agent's `store Map[K, V]` fields whose value is a held `Connection`, as
/// `(name, value-type)`. The value type is the held `Connection[F]` itself (so
/// its TS rendering is `Connection<F>`).
pub(crate) fn held_map_fields<'a>(
    a: &'a AgentDecl,
    state: &[StoreFieldIr],
    tys: &Types,
) -> Vec<(&'a Ident, &'a TypeRef)> {
    // 1A.33: a held map is a `Map` whose resolved value type is a held `Connection` (looking
    // through `Option`/`Effect`, as the checker's held rule does), not an AST head-name test.
    store_kinds(a, state)
        .filter_map(|(f, kind)| match kind {
            StoreKindIr::Map(_, v) if held_frame_ty(*v, tys).is_some() => {
                Some((&f.name, &f.kind.args[1]))
            }
            StoreKindIr::Map(..)
            | StoreKindIr::Cell(_)
            | StoreKindIr::Set(_)
            | StoreKindIr::Cache(..)
            | StoreKindIr::Log(..) => None,
        })
        .collect()
}

/// #1187's Agent state-field slice 2c: a held `Map[K, V]`'s own frame type
/// `F`, resolved at the `TyId` level — `V` may be a bare `Connection[F]`, or
/// `F` may sit behind an `Option`/`Effect` wrapper (`Map[K,
/// Option[Connection[F]]]` is checker-legal: `type_ref_is_held`,
/// `bynk-check/src/context_checks.rs`, recurses through both to decide
/// storage admission). Mirrors that recursion at the `TyId` level rather
/// than reusing `Ty::is_held()`/`Ty::held_inner()` (`bynk-check/src/
/// checker.rs`): neither recurses past a bare `Connection` today (both are
/// otherwise dead code — no caller anywhere in this workspace), so using
/// them as-is here would silently reintroduce the same Option-blind gap
/// this slice fixes.
///
/// Tested at the `Types`/`TyId` level directly ([`held_frame_ty_tests`]),
/// not through a full end-to-end fixture: a real `store x: Map[K,
/// Option[Connection[F]]]` program certifies (`bynk check` exits 0), but
/// `bynk compile` panics before reaching this function at all —
/// `bynk-check::wire::codec_suffix`'s `TypeRef::Connection(..) =>
/// unreachable!(...)` arm, reached because whatever builds this project's
/// wire/codec instantiation table walks every store field's value type
/// looking for boundary-codec names, `Option`-recursing into a held
/// `Connection` it should have excluded. Confirmed pre-existing and
/// unrelated to this slice (reproduces identically on `main`, before this
/// function existed) — a real `bynk-check` bug, out of scope for this
/// emitter-only cutover; worth its own follow-up issue.
fn held_frame_ty(ty: TyId, tys: &Types) -> Option<TyId> {
    match &*tys.get(ty) {
        Ty::Connection(inner) => Some(*inner),
        Ty::Option(inner) | Ty::Effect(inner) => held_frame_ty(*inner, tys),
        Ty::Error
        | Ty::Never
        | Ty::Base(_)
        | Ty::Named { .. }
        | Ty::Result(..)
        | Ty::HttpResult(_)
        | Ty::QueueResult
        | Ty::HttpOutcome(_)
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::TokenClaims
        | Ty::List(_)
        | Ty::Map(..)
        | Ty::Set(_)
        | Ty::Query(_)
        | Ty::Stream(_)
        | Ty::TestConnection(_)
        | Ty::ValidationError
        | Ty::DecodeError
        | Ty::Unit
        | Ty::Actor(_)
        | Ty::ActorSum(_)
        | Ty::Fn { .. }
        | Ty::Var(_) => None,
    }
}

/// [`held_frame_ty`]'s `TypeRef`-level mirror, and `held_maps_ts`'s own
/// fallback when the frame's `TyId` doesn't resolve (a held field's own
/// type reference is never checker-validated, only its shape — see that
/// call site's own doc comment). Same `Connection`/`Option`/`Effect`
/// recursion as `type_ref_is_held` (`bynk-check/src/context_checks.rs`),
/// applied here at the AST level instead of just to a bare `Connection` the
/// way this function's own pre-#1187 shape did — so the fallback path
/// renders the same frame type the `TyId` path would have, not the whole
/// `Option<Connection<F>>`/`Effect<Connection<F>>` wrapper.
fn held_frame_ty_ref(t: &TypeRef) -> &TypeRef {
    if let TypeRef::Connection(inner, _) | TypeRef::Option(inner, _) | TypeRef::Effect(inner, _) = t
    {
        held_frame_ty_ref(inner)
    } else {
        t
    }
}

#[cfg(test)]
mod type_base_is_tests {
    use super::*;
    use bynk_syntax::span::Span;

    #[test]
    fn a_base_type_and_an_alias_over_it_match_only_their_own_base() {
        let int = TypeRef::Base(BaseType::Int, Span::default());
        let string = TypeRef::Base(BaseType::String, Span::default());
        let types = HashMap::new();
        assert!(type_base_is_int(&int, &types) && !type_base_is_string(&int, &types));
        assert!(type_base_is_string(&string, &types) && !type_base_is_int(&string, &types));
        let list = TypeRef::List(Box::new(int), Span::default());
        assert!(!type_base_is_int(&list, &types) && !type_base_is_string(&list, &types));
    }
}

#[cfg(test)]
mod held_frame_ty_tests {
    use super::*;

    #[test]
    fn unwraps_a_bare_connection() {
        let tys = Types::new();
        let frame = tys.intern(Ty::Base(BaseType::String));
        let conn = tys.intern(Ty::Connection(frame));
        assert!(matches!(held_frame_ty(conn, &tys), Some(f) if f == frame));
    }

    /// The shape a naive, non-recursive port of `Ty::held_inner()` would
    /// get wrong — the same gap this slice found in the pre-existing
    /// `held_maps_ts` construction (a bare `match v { TypeRef::Connection
    /// (inner, _) => …, _ => ts_type_ref(v) }` that rendered the whole
    /// `Option<Connection<F>>` wrapper instead of unwrapping to `F`).
    #[test]
    fn unwraps_through_option_and_effect_like_type_ref_is_held_does() {
        let tys = Types::new();
        let frame = tys.intern(Ty::Base(BaseType::String));
        let conn = tys.intern(Ty::Connection(frame));
        let opt = tys.intern(Ty::Option(conn));
        assert!(matches!(held_frame_ty(opt, &tys), Some(f) if f == frame));
        let eff = tys.intern(Ty::Effect(conn));
        assert!(matches!(held_frame_ty(eff, &tys), Some(f) if f == frame));
        // `Effect[Option[Connection[F]]]` — nested wrapping, both layers unwrap.
        let opt_eff = tys.intern(Ty::Effect(opt));
        assert!(matches!(held_frame_ty(opt_eff, &tys), Some(f) if f == frame));
    }

    #[test]
    fn a_non_held_type_returns_none_even_wrapped() {
        let tys = Types::new();
        let s = tys.intern(Ty::Base(BaseType::String));
        assert!(held_frame_ty(s, &tys).is_none());
        let opt_s = tys.intern(Ty::Option(s));
        assert!(held_frame_ty(opt_s, &tys).is_none());
    }
}

/// The key type (`K`) of a two-argument store field (`Map`/`Cache`) named
/// `field`, used by the rehydration gate to validate textual keys (ADR 0124).
pub(super) fn store_field_key_type<'a>(a: &'a AgentDecl, field: &str) -> Option<&'a TypeRef> {
    a.store_fields
        .iter()
        .find(|f| f.name.name == field && f.kind.args.len() == 2)
        .map(|f| &f.kind.args[0])
}

/// #4037: the `Map`, `Set` and `Cache` fields of `a` whose key is not a scalar (`String`, `Int` or
/// a refined or opaque type over one), and the `@indexed(by:)` fields (as `(map, field)`) of the same
/// kind. Their entries and postings are keyed by the canonical encoding, not `String(k)`.
pub(super) fn composite_store_keys(
    a: &AgentDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> (HashSet<String>, HashSet<(String, String)>) {
    let scalar = |t: &TypeRef| bynk_check::context_checks::type_ref_is_scalar_store_key(t, types);
    let mut keys = HashSet::new();
    let mut indexes = HashSet::new();
    for f in &a.store_fields {
        let head = f.kind.head.name.as_str();
        let Some(key) = f
            .kind
            .args
            .first()
            .filter(|_| matches!(head, "Map" | "Set" | "Cache"))
        else {
            continue;
        };
        if !scalar(key) {
            keys.insert(f.name.name.clone());
        }
        let record = match f.kind.args.get(1) {
            Some(TypeRef::Named(v)) if head == "Map" => match types.get(&v.name).map(|d| &d.body) {
                Some(TypeBody::Record(r)) => Some(r),
                _ => None,
            },
            _ => None,
        };
        let Some(record) = record else { continue };
        for arg in f
            .annotations
            .iter()
            .filter(|an| matches!(an.name.name.as_str(), "indexed" | "unique"))
            .flat_map(|an| &an.args)
        {
            if let ExprKind::Ident(by) = &arg.value.kind
                && let Some(rf) = record.fields.iter().find(|rf| rf.name.name == by.name)
                && !scalar(&rf.type_ref)
            {
                indexes.insert((f.name.name.clone(), by.name.clone()));
            }
        }
    }
    (keys, indexes)
}

/// #4038: one `@unique(by: field)` of the `Map` `map`: the index is enforced at commit.
#[derive(Debug, Clone)]
pub(crate) struct UniqueIndex {
    pub(crate) map: String,
    pub(crate) field: String,
    /// `Some(inner_composite)` when the field is an `Option`: a `None` is not indexed, and the
    /// posting is keyed by the `Some` payload (its canonical encoding when `inner_composite`).
    pub(crate) optional: Option<bool>,
    /// The annotation's `by:` argument, for the violation's location.
    pub(crate) span: bynk_syntax::span::Span,
}

/// #4038: the `@unique(by:)` indexes of `a`, in declaration order.
pub(super) fn unique_store_indexes(
    a: &AgentDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Vec<UniqueIndex> {
    let scalar = |t: &TypeRef| bynk_check::context_checks::type_ref_is_scalar_store_key(t, types);
    let mut out = Vec::new();
    for f in a.store_fields.iter().filter(|f| f.kind.head.name == "Map") {
        let record = if let Some(TypeRef::Named(v)) = f.kind.args.get(1)
            && let Some(TypeBody::Record(r)) = types.get(&v.name).map(|d| &d.body)
        {
            r
        } else {
            continue;
        };
        for arg in f
            .annotations
            .iter()
            .filter(|an| an.name.name == "unique")
            .flat_map(|an| &an.args)
        {
            let ExprKind::Ident(by) = &arg.value.kind else {
                continue;
            };
            let Some(rf) = record.fields.iter().find(|rf| rf.name.name == by.name) else {
                continue;
            };
            let optional = if let TypeRef::Option(inner, _) = &rf.type_ref {
                Some(!scalar(inner))
            } else {
                None
            };
            out.push(UniqueIndex {
                map: f.name.name.clone(),
                field: by.name.clone(),
                optional,
                span: arg.value.span,
            });
        }
    }
    out
}

/// True when a type's base is `String` — directly, or through a named refined /
/// opaque alias. A textual key persists as its own string in a storage `Record`,
/// so the rehydration gate can validate it; a non-textual key persists as a
/// `String(k)` structural key, whose refinement validation is deferred (ADR 0124).
pub(super) fn type_base_is_string(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    type_base_is(t, types, BaseType::String)
}

/// True when `t` is `want` or a refined/opaque type over it.
fn type_base_is(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>, want: BaseType) -> bool {
    match t {
        TypeRef::Base(b, _) => *b == want,
        TypeRef::Named(id) => matches!(
            types.get(&id.name).map(|d| &d.body),
            Some(TypeBody::Refined { base, .. }) | Some(TypeBody::Opaque { base, .. })
                if *base == want
        ),
        // Every other shape has no base type.
        TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::List(..)
        | TypeRef::Map(..)
        | TypeRef::Set(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..)
        | TypeRef::App { .. } => false,
    }
}

/// True when `t` is `Int` or a refined/opaque type over `Int`.
pub(super) fn type_base_is_int(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    type_base_is(t, types, BaseType::Int)
}

/// v0.119: the `.call` variant tag for a handler — its name with the first letter
/// upper-cased (`spend` → `Spend`). Must match the checker's synthesised sum
/// variant (`tests_emit::history_variant_name`), which the reader matches with
/// `is` / `match`.
pub(super) fn history_variant_tag(handler: &str) -> String {
    let mut c = handler.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => handler.to_string(),
    }
}

/// #1818: whether a handler of `a` reads the agent's own key as `self.<key>`.
/// Only such an agent carries its key (a `__key` field, set by the bundle
/// factory and, on workers, sent with each call), so every other agent's output
/// is unchanged.
pub(crate) fn agent_reads_key(a: &AgentDecl) -> bool {
    a.handlers.iter().any(|h| {
        let mut found = false;
        bynk_model::walk_block_exprs(&h.body, &mut |e| {
            if let ExprKind::FieldAccess { receiver, field } = &e.kind
                && matches!(&receiver.kind, ExprKind::Ident(id) if id.name == "self")
                && field.name == a.key_name.name
            {
                found = true;
            }
        });
        found
    })
}

/// #1818: `this.__key = __decodeAgentKey(<codec>, <json>) as <K>;`, setting the
/// key of an agent that reads it at an entry point: the agent dispatch, a
/// forwarded WebSocket upgrade, or a waking WebSocket message or close.
pub(super) fn agent_key_assign(
    codec: &str,
    key_ty: &bynk_ts::TsType,
    json: bynk_ts::TsExpr,
) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::assign(
        member(ident("this"), "__key"),
        bynk_ts::TsExpr::As {
            expr: Box::new(call(ident("__decodeAgentKey"), vec![ident(codec), json])),
            ty: key_ty.clone(),
        },
        None,
    )
}

/// #1818: `globalThis.Object.assign(new <Agent>(state), { __key: key })`, the bundle factory's
/// instance that learns its key.
pub(super) fn object_assign_key(agent: &str) -> bynk_ts::TsExpr {
    call(
        member(member(ident("globalThis"), "Object"), "assign"),
        vec![
            bynk_ts::TsExpr::New {
                callee: Box::new(ident(agent)),
                args: vec![ident("state")],
            },
            bynk_ts::TsExpr::object(vec![("__key".to_string(), ident("key"))]),
        ],
    )
}

/// #1818: the name of an agent's key codec, emitted on workers for an agent
/// that reads its key.
pub(crate) fn agent_key_codec_name(agent: &str) -> String {
    format!("__{agent}Key")
}

/// #1678: the name of an agent's wire table, `__<Agent>Wire`.
pub(super) fn agent_wire_name(agent: &str) -> String {
    format!("__{agent}Wire")
}

/// #1678 (runtime-semantics track S11): an agent's wire table on the `workers`
/// target, `const __<Agent>Wire: AgentWire = { <method>: { args, result } }`.
/// Each position's codec is the boundary codec's serialise/deserialise pair for
/// its type, the same helpers a cross-context call uses; the codec closure
/// (`agent_call_boundary_roots`) emits them in this module. A position with no
/// wire form passes through (`AGENT_WIRE_PASS`). The DO stub's proxy and the
/// DO's `fetch` both read it.
/// #1818: `const __<Agent>Key = <codec>`, the key type's wire codec, shared by
/// the caller's proxy (which encodes the key) and the DO (which decodes it).
pub(super) fn agent_key_codec(
    a: &AgentDecl,
    ru: &crate::emitter::runtime_use::RuntimeUse,
) -> bynk_ts::TsStmt {
    // Exported: a WebSocket edge in `compose.ts` encodes the key it forwards
    // with the upgrade.
    bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: agent_key_codec_name(&a.name.name),
            ty: None,
            init: agent_wire_codec(&a.key_type, ru),
        })),
        None,
    )
}

/// One wire position's codec: `{ enc, dec }` from the type's boundary codec, or
/// the pass-through for a type with no wire form.
fn agent_wire_codec(t: &TypeRef, ru: &crate::emitter::runtime_use::RuntimeUse) -> bynk_ts::TsExpr {
    if serialisation::agent_wire_passes_through(t) {
        bynk_ts::TsExpr::Ident("__AGENT_WIRE_PASS".to_string())
    } else {
        bynk_ts::TsExpr::object(vec![
            (
                "enc".to_string(),
                serialisation::serialise_ref_via(t, "", ru),
            ),
            (
                "dec".to_string(),
                serialisation::deserialise_ref_via(t, "", ru),
            ),
        ])
    }
}

pub(super) fn agent_wire_table(
    a: &AgentDecl,
    ru: &crate::emitter::runtime_use::RuntimeUse,
) -> bynk_ts::TsStmt {
    let codec = |t: &TypeRef| {
        if serialisation::agent_wire_passes_through(t) {
            bynk_ts::TsExpr::Ident("__AGENT_WIRE_PASS".to_string())
        } else {
            bynk_ts::TsExpr::object(vec![
                (
                    "enc".to_string(),
                    serialisation::serialise_ref_via(t, "", ru),
                ),
                (
                    "dec".to_string(),
                    serialisation::deserialise_ref_via(t, "", ru),
                ),
            ])
        }
    };
    let methods = a
        .handlers
        .iter()
        .map(|h| {
            let name = h
                .method_name
                .as_ref()
                .map(|m| m.name.clone())
                .unwrap_or_else(|| "call".to_string());
            let args =
                bynk_ts::TsExpr::array(h.params.iter().map(|p| codec(&p.type_ref)).collect());
            (
                name,
                bynk_ts::TsExpr::object(vec![
                    ("args".to_string(), args),
                    ("result".to_string(), codec(&h.return_type)),
                ]),
            )
        })
        .collect();
    let mut table = bynk_ts::TsExpr::object(methods);
    if let bynk_ts::TsExpr::Object { multiline, .. } = &mut table {
        *multiline = true;
    }
    bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::ConstDecl {
            name: agent_wire_name(&a.name.name),
            ty: Some(bynk_ts::TsType::named("__AgentWire")),
            init: table,
        },
        None,
    )
}

/// #2272: the `path:line:col` of a declaration's name in its `.bynk` file, as a
/// violation reports it. The path is the project-relative identity the test runner
/// uses, so an editor resolves it the same way it resolves an `expect`.
pub(super) fn declaration_location(
    ctx: &EmitProjectCtx,
    source_text: &str,
    span: bynk_syntax::span::Span,
) -> String {
    let (line, col) = bynk_syntax::span::line_col(source_text, span.start);
    format!("{}:{line}:{col}", ctx.identity_path.replace('\\', "/"))
}

/// How an agent's `store` fields classify, read once from the agent's
/// declaration and the checker's resolved [`StoreFieldIr`]s (#3167): the `Cell`
/// fields that form the state record, the plain and held `Map`s, the `Set`s,
/// `Cache`s and `Log`s, and the per-field facts (TS types, TTLs, retain windows,
/// secondary indexes) the state record, its load/commit and the handler
/// lowering each need. Computed once so those steps share one classification.
pub(crate) struct AgentStoreLayout<'a> {
    /// The checker's resolved `store` fields, in declaration order (1A.33).
    pub(crate) state: &'a [StoreFieldIr],
    /// Every `store Map` whose value is a held `Connection`, on any target. `held_maps` is
    /// this on Workers and empty elsewhere, where a held map is not persisted by connection id.
    pub(crate) all_held_maps: Vec<(&'a Ident, &'a TypeRef)>,
    pub(crate) store_field_ty: HashMap<&'a str, &'a StoreKindIr>,
    pub(crate) effective_fields: Vec<RecordField>,
    pub(crate) is_workers: bool,
    pub(crate) held_maps: Vec<(&'a Ident, &'a TypeRef)>,
    pub(crate) held_maps_ts: HashMap<String, bynk_ts::TsType>,
    pub(crate) store_map_fields: Vec<(&'a Ident, &'a TypeRef)>,
    pub(crate) map_names: HashSet<String>,
    pub(crate) map_value_ts: HashMap<String, bynk_ts::TsType>,
    pub(crate) store_map_indexes: HashMap<String, Vec<String>>,
    pub(crate) store_set_fields: Vec<(&'a Ident, &'a TypeRef)>,
    pub(crate) set_field_names: Vec<&'a str>,
    pub(crate) set_names: HashSet<String>,
    pub(crate) store_cache_fields: Vec<(&'a Ident, &'a TypeRef, i64)>,
    pub(crate) cache_ttls: HashMap<String, i64>,
    pub(crate) store_log_fields: Vec<(&'a Ident, &'a TypeRef, Option<i64>)>,
    pub(crate) log_retains: HashMap<String, Option<i64>>,
    pub(crate) log_value_ts: HashMap<String, bynk_ts::TsType>,
    /// #4037: the fields whose key is not a scalar, and the `(map, field)` indexes over a non-scalar
    /// field (see [`composite_store_keys`]).
    pub(crate) composite_keys: HashSet<String>,
    pub(crate) composite_indexes: HashSet<(String, String)>,
    /// #4038: the `@unique(by:)` indexes, enforced at commit.
    pub(crate) unique_indexes: Vec<UniqueIndex>,
}

/// Classify an agent's `store` fields (see [`AgentStoreLayout`]).
pub(super) fn classify_agent_store<'a>(
    a: &'a AgentDecl,
    state: &'a [StoreFieldIr],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> AgentStoreLayout<'a> {
    let tys = UnitFacts::of(ctx).render(commons);
    // #1187's Agent state-field slice: the checker-resolved TyId for each
    // Cell/Map/Cache/Log field's element type, keyed by field name — read
    // once here so the state-interface block below can render through
    // `ts_ty` (TyId) instead of `ts_type_ref` (raw AST) without repeating
    // this lookup at every one of its four sites.
    let store_field_ty: HashMap<&str, &StoreKindIr> =
        state.iter().map(|f| (f.field.as_str(), &f.kind)).collect();
    let is_store_agent = true;
    // v0.81 (storage track, ADR 0109): an agent's `Cell` fields ARE its state
    // record, so the whole state machinery (interface, zero factory, load/commit,
    // invariant gate) derives the record fields from the cells. Each `Cell[T]`
    // field becomes a `T`-typed record field carrying the cell's initialiser.
    // Handler bodies lower as bare reads / `:=` over `__state`, with an implicit
    // commit at handler end.
    // P6.53 (design/tracks/the-ir.md §6b): membership reads `state`'s own
    // typed `StoreKindIr::Cell` instead of a string comparison against
    // `f.kind.head.name` — a user type genuinely named `Cell` (with a
    // different arity or an unrelated shape) cannot silently pass this
    // check when the field's own already-checked kind says otherwise. The
    // field's own `TypeRef`/`init` stay AST-typed (rendering parameters
    // outside this slice's scope).
    let effective_fields: Vec<RecordField> = a
        .store_fields
        .iter()
        .filter(|f| {
            matches!(
                store_field_ty.get(f.name.name.as_str()),
                Some(StoreKindIr::Cell(_))
            )
        })
        .map(|f| RecordField {
            annotations: Vec::new(),
            documentation: None,
            trivia: Default::default(),
            name: f.name.clone(),
            type_ref: f.kind.args[0].clone(),
            refinement: None,
            init: f.init.clone(),
            span: f.span,
        })
        .collect();
    // v0.82 (ADR 0110): `store Map[K, V]` fields are also state-record fields, but
    // persisted as a JSON-serialisable `Record<string, V>` (the value `Map` is a
    // JS `Map`, which does not serialise). Collected separately from `Cell` fields
    // since their TS type and zero differ; the working record is committed by the
    // same flush. `(name, V)`.
    // v0.104/v0.105 (real-time track slice 3b): on Workers, a `store Map[K,
    // Connection]` holds live sockets that cannot themselves be JSON-persisted.
    // v0.105 (slice 3b-ii) persists the **connection id** instead: the held map is a
    // durable `Record<string(K), connId>` in the state record, and each `Connection`
    // is re-resolved from its connId via the platform's hibernatable-socket API, so a
    // stored connection survives DO eviction (§2.9.6). The held maps are kept *out*
    // of `store_map_fields` (their value type and entry lowering differ from a plain
    // `Map[K, V]`) but are emitted into the state interface / zero / rehydration
    // key-check / load-commit as string records, and they trigger the commit flush
    // like any other persisted field. (Bundle keeps held maps in the in-memory test
    // state record of `TestConnection`s — its tested behaviour — so the connId
    // representation is Workers-only.)
    let is_workers = matches!(ctx.target, BuildTarget::Workers);
    let all_held_maps = held_map_fields(a, state, commons.tys());
    let held_maps: Vec<(&Ident, &TypeRef)> = if is_workers {
        all_held_maps.clone()
    } else {
        Vec::new()
    };
    let held_map_names: HashSet<String> = held_maps.iter().map(|(n, _)| n.name.clone()).collect();
    // The held map's lowering needs the connection's **frame type** `F` (for
    // `resolveConnection<F>`), not the `Connection<F>` wrapper — extract it.
    // #1187's Agent state-field slice 2c: the field's own membership in
    // `held_maps` still comes from `held_map_fields`'s AST-based
    // `type_ref_is_held` (unchanged — its caller, `write_header`
    // (`emitter.rs`), has no `CheckedProgram` in scope, so this and that
    // predicate stay deliberately parallel, not unified), but the frame
    // type itself is now read off `state`'s already-resolved `TyId` via
    // `held_frame_ty`, through `ts_ty` — fixing a real, previously
    // uncovered gap: the old `match v { TypeRef::Connection(inner, _) =>
    // ts_type_ref(inner), _ => ts_type_ref(v) }` only ever unwrapped a
    // *bare* `Connection`; a `Map[K, Option[Connection[F]]]` value — legal
    // per the checker's own `type_ref_is_held`, which recurses through
    // `Option`/`Effect` (`bynk-check/src/context_checks.rs`) — fell into
    // the `_` arm and rendered the whole `Option<Connection<F>>` wrapper,
    // not `F`. No existing fixture exercised this shape to catch it.
    let held_maps_ts: HashMap<String, bynk_ts::TsType> = held_maps
        .iter()
        .map(|(n, v)| {
            // Review of #1187's slice 2c: no checker pass validates a store
            // field's own type reference (only its shape —
            // `resolve_store_field_ty`'s own doc comment, `ir/lower.rs`,
            // names this explicitly), so `store x: Map[K, Connection[Bogus]]`
            // certifies today with an unresolvable frame type; the whole
            // `Connection[Bogus]` then fails to resolve too
            // (`resolve_type_ref_in`'s `?` propagation, `bynk-check/src/
            // checker.rs`), so `StoreKindIr::Map`'s own value `TyId` falls
            // back to `Ty::Unit` — `held_frame_ty` legitimately returns
            // `None` on a certified program here, not an internal
            // inconsistency. Fall back to the AST-level frame type (still
            // Option/Effect-unwrapped, via `held_frame_ty_ref`) instead of
            // panicking, mirroring `resolve_store_field_ty`'s own posture
            // for the identical reason.
            let ts = store_field_ty
                .get(n.name.as_str())
                .and_then(|kind| kind.map_value())
                .and_then(|v_ty| held_frame_ty(v_ty, tys.types()))
                .map(|frame_ty| ts_ty_to_ts_type(frame_ty, tys))
                .unwrap_or_else(|| ts_type_ref_to_ts_type(held_frame_ty_ref(v), None));
            (n.name.clone(), ts)
        })
        .collect();
    // P6.53 (design/tracks/the-ir.md §6b): typed membership, same reasoning
    // as `effective_fields` above.
    let store_map_fields: Vec<(&Ident, &TypeRef)> = if is_store_agent {
        a.store_fields
            .iter()
            .filter(|f| {
                matches!(
                    store_field_ty.get(f.name.name.as_str()),
                    Some(StoreKindIr::Map(..))
                )
            })
            .filter(|f| !held_map_names.contains(&f.name.name))
            .map(|f| (&f.name, &f.kind.args[1]))
            .collect()
    } else {
        Vec::new()
    };
    let map_names: HashSet<String> = store_map_fields
        .iter()
        .map(|(n, _)| n.name.clone())
        .collect();
    // Review of #1460: a plain map's own value type `V`, mirroring
    // `held_maps_ts`'s exact construction (above) — needed so
    // `lower_query_method`'s collection-kernel sites can narrow a plain map's
    // own value scan lifted into the general query vocabulary the same way a
    // real `store Query[T]` field already can.
    let map_value_ts: HashMap<String, bynk_ts::TsType> = store_map_fields
        .iter()
        .map(|(n, v)| {
            let ts = store_field_ty
                .get(n.name.as_str())
                .and_then(|kind| kind.map_value())
                .map(|v_ty| ts_ty_to_ts_type(v_ty, tys))
                .unwrap_or_else(|| ts_type_ref_to_ts_type(v, None));
            (n.name.clone(), ts)
        })
        .collect();
    // v0.93 (ADR 0118): `store Map[K, V] @indexed(by: f, …)` — each `by:` field
    // gets a maintained secondary index. `map name → [field, …]` (a deduped,
    // declaration-ordered list). The keys are validated against `V` in
    // `project::validate`; here we only read the surface to drive emission.
    // #1187's Agent state-field slice 2b: which `Map` fields have an
    // `@indexed(by: …)` key list, IR-sourced — `StoreFieldIr::indexed` is
    // already the deduped, declaration-order key list
    // `store_field_kind_and_indexed` built from the same annotations this
    // used to re-walk here. Deliberately still keyed and filtered exactly
    // like the AST walk it replaces: no held-map exclusion (a held
    // `Map[K, Connection]` is an ordinary `StoreKindIr::Map` as far as the
    // checker/IR are concerned — "held" is purely this function's own
    // downstream Workers-mode concern, applied separately to `store_map_fields`
    // above), so a held map's own indexes are still emitted here exactly as
    // before.
    let store_map_indexes: HashMap<String, Vec<String>> = if is_store_agent {
        state
            .iter()
            .filter(|f| matches!(f.kind, StoreKindIr::Map(..)) && !f.indexed.is_empty())
            .map(|f| (f.field.clone(), f.indexed.clone()))
            .collect()
    } else {
        HashMap::new()
    };
    // v0.83 (ADR 0110): `store Set[T]` fields are state-record fields too,
    // persisted as a JSON-serialisable `Record<string, boolean>` (a JS `Set`
    // does not serialise). `(name, T)`; the element type is unused in the TS
    // representation but kept for symmetry with maps — needed only for the
    // rehydration check below, which still validates against a raw `TypeRef`
    // (`serialisation.rs`'s own `TypeRef`-driven boundary, out of this
    // slice's scope).
    // P6.53 (design/tracks/the-ir.md §6b): typed membership, same reasoning
    // as `effective_fields` above.
    let store_set_fields: Vec<(&Ident, &TypeRef)> = if is_store_agent {
        a.store_fields
            .iter()
            .filter(|f| {
                matches!(
                    store_field_ty.get(f.name.name.as_str()),
                    Some(StoreKindIr::Set(_))
                )
            })
            .map(|f| (&f.name, &f.kind.args[0]))
            .collect()
    } else {
        Vec::new()
    };
    // #1187's Agent state-field slice 2b: which fields are Sets, IR-sourced
    // (declaration order preserved — `state` is built from `a.store_fields`
    // in order) — every consumer below except the rehydration check, which
    // alone needs `store_set_fields`'s own `TypeRef`.
    let set_field_names: Vec<&str> = state
        .iter()
        .filter(|f| matches!(f.kind, StoreKindIr::Set(_)))
        .map(|f| f.field.as_str())
        .collect();
    let set_names: HashSet<String> = set_field_names.iter().map(|s| s.to_string()).collect();
    // v0.87 (ADR 0113): `store Cache[K, V] @ttl(d)` fields — a value record plus
    // a per-entry expiry instant. `(name, V, ttl-millis)`; the ttl is the field's
    // `@ttl` Duration literal (validated by the checker).
    // P6.53 (design/tracks/the-ir.md §6b): `ttl` reads `state`'s own
    // already-computed `StoreKindIr::Cache(_, _, ttl)` (`store_field_ty`,
    // built above from `lower_store_field_shape_ir` →
    // `store_field_kind_and_indexed` → `duration_millis_annotation`)
    // instead of re-walking `f.annotations`/`ExprKind::DurationLit` a
    // second time — the two walks could diverge and emit a wrong TTL with
    // nothing to catch it. `?` on a miss preserves the original's own
    // defensive drop (an internal-consistency signal, not a real-world
    // case: every `Cache`-kind field in `a.store_fields` has a matching
    // `StoreKindIr::Cache` entry by construction).
    let store_cache_fields: Vec<(&Ident, &TypeRef, i64)> = if is_store_agent {
        a.store_fields
            .iter()
            .filter_map(|f| {
                let StoreKindIr::Cache(_, _, ttl) = store_field_ty.get(f.name.name.as_str())?
                else {
                    return None;
                };
                Some((&f.name, &f.kind.args[1], *ttl))
            })
            .collect()
    } else {
        Vec::new()
    };
    let cache_ttls: HashMap<String, i64> = store_cache_fields
        .iter()
        .map(|(n, _, ttl)| (n.name.clone(), *ttl))
        .collect();
    // v0.95 (ADR 0121): `store Log[T] [@retain(d)]` fields — an ordered array of
    // `{ t, v }` entries. `(name, T, optional retain-millis)`; the retain (from
    // `@retain`) prunes on append.
    // P6.53 (design/tracks/the-ir.md §6b): same dedup as `store_cache_fields`
    // above — `retain` reads `StoreKindIr::Log(_, retain)` instead of
    // re-walking `f.annotations` a second time. Every `Log`-kind field in
    // `a.store_fields` has a matching `StoreKindIr::Log` entry by
    // construction; `.and_then` (not `?`) preserves the original's own
    // per-field presence (every filtered field stays, `retain` is simply
    // `None` when the field has no `@retain`).
    let store_log_fields: Vec<(&Ident, &TypeRef, Option<i64>)> = if is_store_agent {
        a.store_fields
            .iter()
            .filter_map(|f| {
                let StoreKindIr::Log(_, retain) = store_field_ty.get(f.name.name.as_str())? else {
                    return None;
                };
                Some((&f.name, &f.kind.args[0], *retain))
            })
            .collect()
    } else {
        Vec::new()
    };
    let log_retains: HashMap<String, Option<i64>> = store_log_fields
        .iter()
        .map(|(n, _, r)| (n.name.clone(), *r))
        .collect();
    // Review of #1460: a Log's own element type `T`, same reason and
    // construction as `map_value_ts` above.
    let log_value_ts: HashMap<String, bynk_ts::TsType> = store_log_fields
        .iter()
        .map(|(n, v, _)| {
            let ts = store_field_ty
                .get(n.name.as_str())
                .and_then(|kind| kind.log_element())
                .map(|elem_ty| ts_ty_to_ts_type(elem_ty, tys))
                .unwrap_or_else(|| ts_type_ref_to_ts_type(v, None));
            (n.name.clone(), ts)
        })
        .collect();
    let (composite_keys, composite_indexes) = composite_store_keys(a, &commons.types);
    let unique_indexes = unique_store_indexes(a, &commons.types);
    AgentStoreLayout {
        state,
        all_held_maps,
        store_field_ty,
        effective_fields,
        is_workers,
        held_maps,
        held_maps_ts,
        store_map_fields,
        map_names,
        map_value_ts,
        store_map_indexes,
        store_set_fields,
        set_field_names,
        set_names,
        store_cache_fields,
        cache_ttls,
        store_log_fields,
        log_retains,
        log_value_ts,
        composite_keys,
        composite_indexes,
        unique_indexes,
    }
}
