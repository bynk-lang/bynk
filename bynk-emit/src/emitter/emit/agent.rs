//! Agents: `emit_agent`, the entry point that assembles an agent's Durable Object class, with its
//! constructor, its state record and the zero entries that seed it.

use super::{
    AgentDispatchFacts, AgentLoadFns, AgentPersistFns, AgentStoreLayout, GivenDeps, UniqueIndex,
    WsDepsRebuild, WsOpenHost, agent_key_codec_name, agent_reads_key, classify_agent_store,
    consumed_type_ns, emit_agent_dispatch, emit_agent_factory, emit_agent_handlers,
    emit_agent_history_driver, emit_agent_load_commit_state, emit_agent_state_persistence,
    emit_ws_do_method, ws_close_do_method_name, ws_message_do_method_name, ws_open_do_method_name,
    ws_open_hosts_for,
};
use crate::emit_model::EmitProjectCtx;
use crate::emitter::ctx::{BodyMode, ModuleCtx};
use crate::emitter::internal_fault::FaultSite;
use crate::emitter::lower_ctx::LowerCtx;
use crate::emitter::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use crate::emitter::zero_value::zero_expr as zero_value_expr;
use crate::emitter::{BuildTarget, Pre, UnitFacts, agent_registry_name, compact_iife};
use crate::ts_build::{binary, call, ident, member, str_lit};
use bynk_check::checker::TypedCommons;
use bynk_model::{StoreFieldIr, StoreKindIr};
use bynk_syntax::ast::{AgentDecl, Expr, RecordField, Refinement, TypeDecl, TypeRef};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A state field's zero value. The checker admits a field with no initialiser
/// only when its type has a zero (`bynk.agents.non_zeroable_state_field`), so none is a missing
/// fact (1A.34), reported at the field.
fn state_field_zero(
    f: &RecordField,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> bynk_ts::TsExpr {
    zero_value_ts(&f.type_ref, f.refinement.as_ref(), &commons.types).unwrap_or_else(|| {
        let site = FaultSite {
            sink: &ctx.runtime_use,
            span: f.span,
        };
        site.missing_fact("zero value for a state field")
    })
}

/// The TypeScript zero-value expression for a state field, or `None` if its
/// type is not zeroable (#3165: the checker decides, `zero_value` renders).
fn zero_value_ts(
    type_ref: &TypeRef,
    inline: Option<&Refinement>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<bynk_ts::TsExpr> {
    bynk_check::checker::zero_value(type_ref, inline, types).map(|z| zero_value_expr(&z))
}

/// The entries of an agent's fresh-state record, one per state field: a `Cell` field's initialiser
/// (or its zero), then an empty handle for each `Map`, held map, `Set`, `Cache` and `Log`.
fn agent_zero_entries(
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsObjectEntry> {
    let tys = UnitFacts::of(ctx).render(commons);
    let AgentStoreLayout {
        store_field_ty,
        effective_fields,
        held_maps,
        store_map_fields,
        store_map_indexes,
        set_field_names,
        store_cache_fields,
        store_log_fields,
        ..
    } = layout;
    let mut entries: Vec<bynk_ts::TsObjectEntry> = effective_fields
        .iter()
        .map(|f| {
            let val = match &f.init {
                Some(init) => state_field_init_value(init, commons, ctx),
                None => state_field_zero(f, commons, ctx),
            };
            bynk_ts::TsObjectEntry::prop(f.name.name.clone(), val)
        })
        .collect();
    // A fresh `store Map`/`Set`/`Cache`/`Log` is an empty handle (#3254): insertion-ordered
    // whatever its keys. A `Map` carries the indexers of its `@indexed(by:)` fields, so its
    // handle keeps the posting lists itself (v0.93, ADR 0118).
    for (name, _) in store_map_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Map(_, v)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Map in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        let mut indexed: Vec<&String> = store_map_indexes
            .get(&name.name)
            .map(|fs| fs.iter().collect())
            .unwrap_or_default();
        indexed.sort();
        let args = if indexed.is_empty() {
            Vec::new()
        } else {
            vec![bynk_ts::TsExpr::array(
                indexed
                    .into_iter()
                    .map(|f| {
                        map_indexer(
                            f,
                            layout
                                .composite_indexes
                                .contains(&(name.name.clone(), f.clone())),
                            layout
                                .unique_indexes
                                .iter()
                                .find(|u| u.map == name.name && u.field == *f),
                        )
                    })
                    .collect(),
            )]
        };
        entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            store_handle_init("__StoreMap", Some(ts_ty_to_ts_type(value_ty, tys)), args),
        ));
    }
    // A fresh held `Map[K, Connection]` (a `K → connId` handle) is empty too.
    for (name, _) in held_maps {
        entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            store_handle_init(
                "__StoreMap",
                Some(bynk_ts::TsType::named("string")),
                Vec::new(),
            ),
        ));
    }
    for name in set_field_names {
        entries.push(bynk_ts::TsObjectEntry::prop(
            name.to_string(),
            store_handle_init("__StoreSet", None, Vec::new()),
        ));
    }
    for (name, _, _) in store_cache_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Cache(_, v, _)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Cache in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            store_handle_init(
                "__StoreCache",
                Some(ts_ty_to_ts_type(value_ty, tys)),
                Vec::new(),
            ),
        ));
    }
    for (name, _, _) in store_log_fields {
        let elem_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Log(t, _)) => *t,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Log in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            store_handle_init(
                "__StoreLog",
                Some(ts_ty_to_ts_type(elem_ty, tys)),
                Vec::new(),
            ),
        ));
    }
    entries
}

/// `new <handle><<ty>>(<args>)`, a fresh store handle: the type argument is the value (or element)
/// type, and a `Set` carries none.
fn store_handle_init(
    handle: &str,
    type_arg: Option<bynk_ts::TsType>,
    args: Vec<bynk_ts::TsExpr>,
) -> bynk_ts::TsExpr {
    let callee = ident(handle);
    bynk_ts::TsExpr::New {
        callee: Box::new(match type_arg {
            Some(ty) => bynk_ts::TsExpr::TypeApp {
                callee: Box::new(callee),
                type_args: vec![ty],
            },
            None => callee,
        }),
        args,
    }
}

/// `{ index: "<field>", of: (__v) => String(__v.<field>) }`: how a `Map` handle derives the posting
/// key of one `@indexed(by:)` field (its canonical encoding when the field is not a scalar, #4037).
/// A `@unique(by:)` field (#4038) also carries `unique: true`, and when it is an `Option` its `of` is
/// the `Some` payload's key, or `undefined` for a `None`, which is not indexed.
fn map_indexer(field: &str, composite: bool, unique: Option<&UniqueIndex>) -> bynk_ts::TsExpr {
    let key = |x: bynk_ts::TsExpr, composite: bool| {
        call(
            ident(if composite {
                "__bynkKeyEncode"
            } else {
                "String"
            }),
            vec![x],
        )
    };
    let value = member(ident("__v"), field);
    let derived = match unique.and_then(|u| u.optional) {
        Some(inner_composite) => bynk_ts::TsExpr::Conditional {
            test: Box::new(binary(
                bynk_ts::TsBinaryOp::StrictEq,
                member(value.clone(), "tag"),
                str_lit("Some"),
            )),
            consequent: Box::new(key(member(value, "value"), inner_composite)),
            alternate: Box::new(ident("undefined")),
        },
        None => key(value, composite),
    };
    let mut entries = vec![
        ("index".to_string(), str_lit(field)),
        (
            "of".to_string(),
            bynk_ts::TsExpr::Arrow {
                params: vec![bynk_ts::TsParam::new("__v".to_string(), None)],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(derived))),
            },
        ),
    ];
    if unique.is_some() {
        entries.push((
            "unique".to_string(),
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true)),
        ));
    }
    bynk_ts::TsExpr::object(entries)
}

/// The lowered, static initialiser of a state field. A static initialiser lowers to a pure
/// expression (no setup statements). #1029 review: if any appear, wrap them in an IIFE rather
/// than splicing them into a comma sequence. The comma form only ever worked for
/// expression-shaped hoists: a `const` or an `if` operand does not parse, and T2.1 made a
/// statement-shaped hoist reachable here for the first time, since a value-position `if` that
/// hoists now yields `let …; if (…) {…}` where it used to yield a self-contained arrow. An IIFE is
/// sound for this position specifically: a static initialiser has no enclosing function to
/// `return` out of, so the arrow cannot swallow a control transfer the way it would in a handler
/// body.
fn state_field_init_value(
    init: &Expr,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> bynk_ts::TsExpr {
    let mut pre = Pre::new();
    let mut module = ModuleCtx::for_unit(commons, ctx);
    module.target = ctx.target;
    module.agent_method_givens = ctx.agent_method_givens.clone();
    module.agent_called_contexts = ctx.agent_called_contexts.clone();
    module.event_schema_versions = ctx.event_schema_versions.clone();
    module.consumed_type_ns = consumed_type_ns(ctx);
    module.consumed_type_declared = ctx.consumed_type_aliases.clone();
    module.consumed_context_types = ctx.consumed_context_types();
    let mut icx = LowerCtx::new(module, BodyMode::StaticInit);
    icx.local_agents = ctx.local_agents.clone();
    let expr = pre.lower_node(init, &mut icx);
    if pre.is_empty() {
        expr
    } else {
        compact_iife(pre.ts_stmts(), expr)
    }
}

/// The agent's collection-field description const (`__entryFieldsOf<Agent>`, #3189). The one
/// spelling: the declaration ([`AgentStateNames::fields_const`]) and the inspect branch read it
/// through here (1A.39b, Decision C).
pub(super) fn entry_fields_const(agent: &str) -> String {
    format!("__entryFieldsOf{agent}")
}

/// The names of an agent's state registry and fresh-state factory.
struct AgentStateNames {
    /// The per-agent state registry (`__agentStates…`).
    registry: String,
    /// The fresh-state factory (`__zeroOf<Agent>State`).
    zero_fn: String,
    /// The description of the agent's collection fields the per-entry storage
    /// codec reads (`__entryFieldsOf<Agent>`, #3189).
    fields_const: String,
}

/// The agent's state record, as the TypeScript the rest of the agent reads: the
/// `<Agent>State` interface, the per-agent state registry, and the fresh-state
/// factory (#3167). Pushes those three declarations, in that order, onto `stmts`.
/// Returns the registry's and the factory's names, which `loadState`, the
/// rehydration step and the test reset refer to.
fn emit_agent_state_record(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) -> AgentStateNames {
    let tys = UnitFacts::of(ctx).render(commons);
    let state_ty = format!("{}State", a.name.name);
    let AgentStoreLayout {
        store_field_ty,
        effective_fields,
        held_maps,
        store_map_fields,
        store_map_indexes,
        set_field_names,
        store_cache_fields,
        store_log_fields,
        ..
    } = layout;
    // 1) State record type.
    //
    // Arc C, slice 19 (#1367): the first of step (9)'s own proposed 5-6
    // sub-slices (design/tracks/the-typescript-tree.md §6's own grounding
    // pass) — converts this block alone to a real `bynk_ts::TsDecl::Interface`
    // (already exists, gained `type_params`/per-member `readonly` by #1339,
    // no new algebra gap needed). Each field's own `readonly {name}: {ty};`
    // line is a real `TsTypeMember::Prop`; `ts_ty_to_ts_type` (the real-node
    // sibling of the plain-`String` `ts_ty` this block called before) already
    // exists and is already used by `emit_service`/`emit_make_surface` for
    // the identical purpose. Three compound member types, one load-bearing
    // trap among them: `Cache`'s `Record<string, { v: T; exp: number }>` and
    // `Log`'s `Array<{ t: number; v: T }>` are real `TsType::named_with_args`
    // over a real inline `TsType::Object` — no opaque text anywhere in this
    // block. The posting-list's own `Record<string, string[]>` is the third,
    // and it must use `TsType::Array { readonly: false, .. }` (postfix `[]`
    // syntax), NOT `named_with_args("Array", ..)` like `Cache`/`Log`
    // themselves use two blocks down — semantically identical, textually
    // distinct; using the generic form here would silently break zero-diff.
    // See its own construction below for the actual choice, not just this
    // note.
    let mut members: Vec<bynk_ts::TsTypeMember> = Vec::new();
    for f in effective_fields {
        let cell_ty = match store_field_ty.get(f.name.name.as_str()) {
            Some(StoreKindIr::Cell(t)) => *t,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Cell in this \
                 function's own state reader, found {other:?}",
                f.name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: f.name.name.clone(),
            ty: ts_ty_to_ts_type(cell_ty, tys),
            optional: false,
            readonly: true,
        });
    }
    for (name, _) in store_map_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Map(_, v)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Map in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreMap",
                vec![ts_ty_to_ts_type(value_ty, tys)],
            ),
            optional: false,
            readonly: true,
        });
    }
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` persists `K → connId`.
    for (name, _) in held_maps {
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreMap",
                vec![bynk_ts::TsType::named("string")],
            ),
            optional: false,
            readonly: true,
        });
    }
    for name in set_field_names {
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.to_string(),
            ty: bynk_ts::TsType::named("__StoreSet"),
            optional: false,
            readonly: true,
        });
    }
    // v0.93 (ADR 0118): an `@indexed(by: f)` posting list lives inside the `Map`'s handle
    // (#3254), not in a sibling state field; the wire form still carries it.
    for (name, _, _) in store_cache_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Cache(_, v, _)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Cache in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreCache",
                vec![ts_ty_to_ts_type(value_ty, tys)],
            ),
            optional: false,
            readonly: true,
        });
    }
    for (name, _, _) in store_log_fields {
        let elem_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Log(t, _)) => *t,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Log in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreLog",
                vec![ts_ty_to_ts_type(elem_ty, tys)],
            ),
            optional: false,
            readonly: true,
        });
    }
    // #1486: `no_blank_before` when this agent's own doc comment is the
    // preceding statement — see `emit_free_fn`'s own identical note. The
    // explicit `TsStmt::blank(None)` push this used to carry is gone —
    // `bynk_ts::print` itself now supplies the one blank line between
    // adjacent real statements.
    let mut state_interface = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Interface {
            name: state_ty.clone(),
            type_params: Vec::new(),
            members,
        })),
        None,
    );
    state_interface.no_blank_before = a.documentation.is_some();
    stmts.push(state_interface);
    // v0.9.2: per-agent state registry (bundle mode + `bynk test`) and the
    // zero-value factory used to initialise a fresh key's state.
    //
    // Arc C, slice 20 (#1369): the second of step (9)'s own proposed 5-6
    // sub-slices — the registry `const` and the zero-factory `function`
    // build real nodes; the zero-record's own per-field VALUES stay
    // opaque where they genuinely must (a `Cell` field with an initialiser
    // lowers through `LowerCtx`, `lower.rs`'s own permanently-excluded
    // general expression lowering — the same "carry an unconverted
    // sibling's already-formed JS text as `TsExpr::Ident`" pattern
    // #1355's own `emit_message_entry_renderer` call already established),
    // but every EMPTY-container field (`Map`/`Set`/`Cache`/held-map/
    // posting-list `{}`, `Log`'s `[]`) is fully real — `TsExpr::object(
    // vec![])`/`TsExpr::array(vec![])`, no opacity needed there at all.
    let registry = agent_registry_name(&a.name.name);
    let zero_fn = format!("__zeroOf{}State", a.name.name);
    let registry_const = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::ConstDecl {
            name: registry.clone(),
            ty: None,
            init: bynk_ts::TsExpr::New {
                callee: Box::new(bynk_ts::TsExpr::Ident("__StateRegistry".to_string())),
                args: Vec::new(),
            },
        },
        None,
    );
    stmts.push(registry_const);
    // v0.11: build the fresh-state record. A field with an explicit initialiser
    // lowers its (static) expression; a field without one uses the v0.9.2
    // implicit zero. A fresh store handle is empty.
    let zero_entries = agent_zero_entries(layout, commons, ctx);
    // #1486: `no_blank_before: true`, unconditional — no blank line wanted
    // between `registry_const` and this function, matching the
    // pre-conversion hand-written text this crate's own zero-diff
    // discipline requires preserving exactly (see `TsStmt::no_blank_
    // before`'s own doc, added specifically for this real, already-shipped
    // pairing). The explicit `TsStmt::blank(None)` push this used to carry
    // afterward is also gone — `bynk_ts::print` itself now supplies the one
    // blank line before whatever follows.
    let mut zero_fn_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Function {
            name: zero_fn.clone(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: Some(bynk_ts::TsType::named(state_ty.clone())),
            body: vec![bynk_ts::TsStmt::return_stmt(
                Some(bynk_ts::TsExpr::object_entries(zero_entries)),
                None,
            )],
            is_async: false,
            // One line when the record fits on it: a genuinely multi-line `Cell` initialiser
            // makes the printer fall back to the ordinary multi-line block form (1A.39a).
            inline: true,
        },
        None,
    );
    zero_fn_decl.no_blank_before = true;
    stmts.push(zero_fn_decl);
    // Per-entry storage (#3189): which fields are collections, and which index
    // posting records belong to which `Map`. The runtime codec splits a state record
    // by this on commit and joins it back on load.
    let fields_const = entry_fields_const(&a.name.name);
    let mut spec: Vec<String> = Vec::new();
    // A held `Map[K, Connection]` keeps its connection ids inline in the state record:
    // its values are live connections, not stored data, so it is not a per-entry field.
    // (On the bundle target `held_maps` is empty, so ask the declaration.)
    let held_decls = &layout.all_held_maps;
    let held_names: HashSet<&str> = held_decls.iter().map(|(n, _)| n.name.as_str()).collect();
    for (name, _) in store_map_fields
        .iter()
        .filter(|(n, _)| !held_names.contains(n.name.as_str()))
    {
        let indexes: Vec<String> = store_map_indexes
            .get(&name.name)
            .map(|fs| {
                let mut fs: Vec<&String> = fs.iter().collect();
                fs.sort();
                fs.into_iter()
                    .map(|f| {
                        // #4038: a `@unique` index says so, so the recorded index set tells it from `@indexed`.
                        let unique = if layout
                            .unique_indexes
                            .iter()
                            .any(|u| u.map == name.name && &u.field == f)
                        {
                            ", unique: true"
                        } else {
                            ""
                        };
                        format!(
                            "{{ index: \"{f}\", recordField: \"{}__idx_{f}\"{unique} }}",
                            name.name
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if indexes.is_empty() {
            spec.push(format!("{{ name: \"{}\", kind: \"map\" }}", name.name));
        } else {
            spec.push(format!(
                "{{ name: \"{}\", kind: \"map\", indexes: [{}] }}",
                name.name,
                indexes.join(", ")
            ));
        }
    }
    for name in set_field_names {
        spec.push(format!("{{ name: \"{name}\", kind: \"set\" }}"));
    }
    for (name, _, _) in store_cache_fields {
        spec.push(format!("{{ name: \"{}\", kind: \"cache\" }}", name.name));
    }
    for (name, _, _) in store_log_fields {
        spec.push(format!("{{ name: \"{}\", kind: \"log\" }}", name.name));
    }
    stmts.push(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(fields_const.clone()),
        None,
        bynk_ts::TsExpr::VerbatimExpr(
            format!("[{}] as const", spec.join(", ")),
            bynk_ts::VerbatimOrigin::Emit,
        ),
        None,
    ));
    AgentStateNames {
        registry,
        zero_fn,
        fields_const,
    }
}

/// The Durable Object constructor: `state` alone, or with the Worker's `env` when the agent
/// rebuilds deps inside the object (#527), in which case the `private __env` field joins `fields`.
fn agent_constructor(
    needs_env_ctor: bool,
    fields: &mut Vec<bynk_ts::TsClassField>,
) -> bynk_ts::TsClassCtor {
    let this_assign = |prop: &str, value: &str| {
        bynk_ts::TsStmt::assign(
            bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                property: prop.to_string(),
            },
            bynk_ts::TsExpr::Ident(value.to_string()),
            None,
        )
    };
    let state = bynk_ts::TsParam::new(
        "state",
        Some(bynk_ts::TsType::named("__DurableObjectState")),
    );
    if !needs_env_ctor {
        return bynk_ts::TsClassCtor {
            params: vec![state],
            body: vec![this_assign("state", "state")],
        };
    }
    fields.push(bynk_ts::TsClassField::new("__env", bynk_ts::TsType::named("unknown")).private());
    bynk_ts::TsClassCtor {
        params: vec![
            state,
            bynk_ts::TsParam::optional("env", Some(bynk_ts::TsType::named("unknown"))),
        ],
        body: vec![this_assign("state", "state"), this_assign("__env", "env")],
    }
}

/// Emit one agent: the state interface, registry, zero factory, persistence functions and
/// the Durable Object class, each a `bynk_ts` node. The DO class's fields, constructor and
/// every method are nodes too, so the printer alone records the source map (#1482 returned a
/// `Vec<bynk_ts::TsStmt>` in place of an `out: &mut String`; 1A.32b made the class a real
/// `TsDecl::class`). The test-support history driver is emitted alongside it. The caller
/// (`emitter.rs`) merges every returned node into its module map via `print_stmt_and_merge`,
/// as `emit_free_fn`, `emit_provider` and `emit_service` do.
pub(crate) fn emit_agent(
    a: &AgentDecl,
    state: &[StoreFieldIr],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    source_text: &str,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    if let Some(doc) = a.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let layout = classify_agent_store(a, state, commons, ctx);
    let AgentStateNames {
        registry,
        zero_fn,
        fields_const,
    } = emit_agent_state_record(a, &layout, commons, ctx, &mut stmts);
    let AgentPersistFns {
        rehydrate_fn,
        encode_fn,
        migrate_fn,
        hydrate_fn,
        has_rehydrate,
        has_collections,
        has_encode,
        step_plans,
        agent_versions,
        lazy_fields,
    } = emit_agent_state_persistence(a, &layout, &zero_fn, commons, ctx, &mut stmts);
    let is_workers = layout.is_workers;
    // 2) Durable Object class: fields, constructor and every method are nodes, so the
    // printer alone records the source map (#1482 gave the carrier a map of its own).
    let reads_key = agent_reads_key(a);
    let mut fields = vec![bynk_ts::TsClassField::new(
        "state",
        bynk_ts::TsType::named("__DurableObjectState"),
    )];
    if reads_key {
        // #1818: set before any handler runs, by the bundle factory or the
        // workers `fetch` dispatch.
        fields.push(
            bynk_ts::TsClassField::new("__key", ts_type_ref_to_ts_type(&a.key_type, None))
                .definite(),
        );
    }
    // #527: an agent whose methods take `given` capabilities rebuilds those
    // deps *inside* the DO (providers cannot cross the JSON wire), and some
    // providers take the Worker `env` — workerd passes it as the DO
    // constructor's second argument. Bundle mode constructs with `state`
    // only and never uses the fetch path, so the parameter stays optional.
    let given_deps_expr: Option<GivenDeps> = ctx.agent_given_deps.get(&a.name.name).map(|expr| {
        (
            expr.clone(),
            ctx.agent_given_deps_names
                .get(&a.name.name)
                .cloned()
                .unwrap_or_default(),
        )
    });
    // Events track, slice 0 (spine #936): the same wire problem #527 already
    // solved for capability providers hits `deps.__eventsDispatch` too — it is
    // a function, so it does not survive the DO's JSON wire either. An agent
    // whose own handler body emits directly (`body_emits_directly`'s
    // per-handler test, mirrored here at the whole-agent level) needs its
    // fetch dispatch to rebuild it from `env.EVENTS_FANOUT` exactly as a
    // `given` provider is rebuilt, so it takes the same env-carrying
    // constructor.
    let agent_uses_emit = a
        .handlers
        .iter()
        .any(|h| bynk_model::block_uses_emit(&h.body, &commons.callees));
    // #1960: likewise `deps.env` itself — its Service Bindings and DO
    // namespaces are dead plain objects once the caller's deps cross the
    // wire, so an agent whose handlers call a consumed context's service or
    // construct an agent reads them from its own `env` instead.
    let agent_reaches_env = ctx.target == BuildTarget::Workers
        && a.handlers
            .iter()
            .any(|h| bynk_model::block_reaches_worker_env(&h.body, &commons.callees));
    // #3109: likewise the `from websocket` bodies this agent's Durable Object
    // hosts (`on open` / `on message` / `on close`): they run here, so any
    // Service Binding or DO namespace they reach comes from this object's `env`.
    let ws_hosts_reach_env = ctx.target == BuildTarget::Workers
        && ws_open_hosts_for(&a.name.name, commons, &ctx.local_agents, &ctx.actors)
            .iter()
            .any(|h| h.reaches_env(commons));
    // The `/_bynk/agent/` RPC branch rebuilds on its own condition; the ws
    // hosts only widen the constructor (and the ws entry points).
    let rpc_rebuilds_env = given_deps_expr.is_some() || agent_uses_emit || agent_reaches_env;
    let needs_env_ctor = rpc_rebuilds_env || ws_hosts_reach_env;
    let constructor = agent_constructor(needs_env_ctor, &mut fields);
    let mut methods = Vec::new();
    emit_agent_load_commit_state(
        &mut methods,
        a,
        &layout.effective_fields,
        &AgentLoadFns {
            zero_fn: &zero_fn,
            fields_const: &fields_const,
            rehydrate_fn: &rehydrate_fn,
            encode_fn: &encode_fn,
            migrate_fn: &migrate_fn,
            hydrate_fn: &hydrate_fn,
            has_rehydrate,
            has_collections,
            has_encode,
            step_plans: &step_plans,
            agent_versions: &agent_versions,
            lazy_fields: lazy_fields.as_ref(),
        },
        commons,
        ctx,
        source_text,
    );
    emit_agent_handlers(&mut methods, a, &layout, commons, ctx);
    // v0.104 (real-time track slice 3b): the `from websocket` `on open` handlers
    // whose connection transfers to *this* agent are hosted in this Durable Object
    // (DECISION A) — the upgrade is authenticated at the edge then forwarded here,
    // where the socket is accepted and the body runs as a `this`-self-call.
    let mut ws_open_hosts: Vec<WsOpenHost<'_>> = if is_workers {
        ws_open_hosts_for(&a.name.name, commons, &ctx.local_agents, &ctx.actors)
    } else {
        Vec::new()
    };
    if reads_key {
        for host in &mut ws_open_hosts {
            host.agent_key = Some((
                agent_key_codec_name(&a.name.name),
                ts_type_ref_to_ts_type(&a.key_type, None),
            ));
        }
    }
    for host in &mut ws_open_hosts {
        host.rebuild = WsDepsRebuild {
            given: given_deps_expr.clone(),
            uses_emit: agent_uses_emit,
            reaches_env: host.reaches_env(commons),
        };
    }
    for host in &ws_open_hosts {
        emit_ws_do_method(
            &mut methods,
            a,
            host,
            host.handler,
            &ws_open_do_method_name(host.service),
            commons,
            ctx,
        );
        if let Some(m) = host.message {
            emit_ws_do_method(
                &mut methods,
                a,
                host,
                m,
                &ws_message_do_method_name(host.service),
                commons,
                ctx,
            );
        }
        if let Some(c) = host.close {
            emit_ws_do_method(
                &mut methods,
                a,
                host,
                c,
                &ws_close_do_method_name(host.service),
                commons,
                ctx,
            );
        }
    }
    emit_agent_dispatch(
        &mut methods,
        a,
        &AgentDispatchFacts {
            reads_key,
            given_deps_expr: &given_deps_expr,
            rpc_rebuilds_env,
            agent_uses_emit,
            agent_reaches_env,
            agent_versions: &agent_versions,
        },
        &ws_open_hosts,
        commons,
        ctx,
    );
    // #2270: what follows the last method belongs to the agent, not to that method's
    // final body line.
    let class = bynk_ts::TsDecl::class(a.name.name.as_str(), fields, Some(constructor), methods)
        .ending_at(a.span);
    stmts.push(bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(class)),
        None,
    ));
    // #3932: `@busyAfter(d)` lowers this class's queue wait; the runtime reads it where the
    // class is wrapped, on every target and tier.
    if let Some(ms) = bynk_check::agent_busy::busy_after_ms(a) {
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__agentBusyAfter".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Ident(a.name.name.clone()),
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(ms.to_string())),
                ],
            },
            None,
        ));
    }
    emit_agent_factory(a, ctx, reads_key, &registry, &mut stmts);
    emit_agent_history_driver(
        a,
        &AgentLoadFns {
            zero_fn: &zero_fn,
            fields_const: &fields_const,
            rehydrate_fn: &rehydrate_fn,
            encode_fn: &encode_fn,
            migrate_fn: &migrate_fn,
            hydrate_fn: &hydrate_fn,
            has_rehydrate,
            has_collections,
            has_encode,
            step_plans: &step_plans,
            agent_versions: &agent_versions,
            // The history driver reads and commits whole.
            lazy_fields: None,
        },
        &registry,
        commons,
        ctx,
        &mut stmts,
    );
    stmts
}
