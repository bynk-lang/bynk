//! Suite declarations: the local `fn`s, `type`s, `const`s and late codecs a test suite declares, which
//! `testgen` calls (`emit_suite_fn`, `emit_suite_decls`).

use super::{emit_const_in, emit_free_fn_in, emit_type_in};
use crate::emitter::ctx::ModuleCtx;
use crate::emitter::{RuntimeUse, UnitFacts};
use bynk_check::checker::{TyId, TypedCommons};
use bynk_syntax::ast::{ConstDecl, FnDecl, TypeDecl, TypeRef};

/// #2834: a suite-local helper `fn` — `emit_free_fn` without the `export`,
/// since it is declared inside a test runner and called from its body.
pub fn emit_suite_fn(
    f: &FnDecl,
    commons: &TypedCommons,
    runtime_use: &RuntimeUse,
) -> Vec<bynk_ts::TsStmt> {
    // A suite is only ever built for `bynk test`, which guards every contract.
    // A suite helper cannot declare `given`, so it has no capability types to name.
    let empty = bynk_check::resolver::CrossContextInfo::default();
    emit_free_fn_in(
        f,
        commons,
        true,
        runtime_use,
        &empty,
        false,
        true,
        UnitFacts::detached(),
    )
}

/// #3066: a suite's own `type` and `const` declarations, as TypeScript for the
/// body of one test runner: the types (an interface and its companion object, or
/// a union), their JSON codecs (so `select[T]`, `Json.encode`/`decode` and the
/// boundary paths find `__serialise_T` / `__deserialise_T` in scope), and the
/// constants (cached-function declarations, like a unit's). They are declared
/// local to the runner, so another suite declaring the same name cannot collide
/// and nothing here reaches the deployable. `typed` is the suite's view with the
/// suite's types and constants already merged in; `qual` qualifies a target
/// type a suite type's codec mentions, as the module-level codecs do.
///
/// Returns the statements with the `export` modifiers dropped (a function-local declaration
/// cannot carry one). The driver prints them, at its own test-harness boundary.
pub fn emit_suite_decls(
    types: &[TypeDecl],
    consts: &[ConstDecl],
    typed: &TypedCommons,
    qual: &std::collections::HashMap<String, String>,
    runtime_use: &RuntimeUse,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
    for t in types {
        let Some(def) = typed.types.get(&t.name.name) else {
            continue;
        };
        let shape = bynk_model::lower_suite_type_shape_ir(def, typed);
        stmts.extend(emit_type_in(
            t,
            &shape,
            typed,
            "",
            runtime_use,
            UnitFacts::detached(),
        ));
    }
    if !types.is_empty() {
        // The type itself plus the wrappers a test reads one through: a
        // `select[T]` is a `List[T]`, a `firstAs[T]` an `Option[T]`.
        let roots = suite_base_codec_roots(types);
        let (names, insts) = super::serialisation::collect_codec_closure(&roots, &typed.types);
        stmts.extend(super::serialisation::decls_as_stmts_block(
            super::serialisation::emit_helpers_for_owner_qualified(
                &names,
                &typed.types,
                "",
                qual,
                runtime_use,
            ),
        ));
        stmts.extend(super::serialisation::decls_as_stmts(
            super::serialisation::emit_generic_helpers_qualified(
                &insts,
                &typed.types,
                qual,
                runtime_use,
            ),
        ));
    }
    let empty = bynk_check::resolver::CrossContextInfo::default();
    for c in consts {
        stmts.extend(emit_const_in(
            c,
            ModuleCtx::new(typed, &empty, runtime_use, UnitFacts::detached()),
        ));
    }
    stmts
        .into_iter()
        .map(bynk_ts::TsStmt::without_export)
        .collect()
}

/// The roots a suite's local codecs always cover: each non-generic type plus the
/// `List`/`Option` wrappers a test reads one through (a `select[T]` is a
/// `List[T]`, a `firstAs[T]` an `Option[T]`).
fn suite_base_codec_roots(types: &[TypeDecl]) -> Vec<TypeRef> {
    let mut roots: Vec<TypeRef> = Vec::new();
    for t in types.iter().filter(|t| t.type_params.is_empty()) {
        let named = TypeRef::Named(t.name.clone());
        let span = t.name.span;
        roots.push(TypeRef::List(Box::new(named.clone()), span));
        roots.push(TypeRef::Option(Box::new(named.clone()), span));
        roots.push(named);
    }
    roots
}

/// #3111: the codecs a runner's body reached for beyond the always-present
/// ones ([`emit_suite_decls`]): a suite type under another generic (`Result`,
/// a user generic, nested containers) at a `Json` boundary. `reached` is what
/// the runner's lowering noted; only roots that name a suite type are served
/// here (the module-level closure serves the rest). Returns the function
/// declarations, with `export` dropped, to be spliced ahead of the body; empty
/// when the body reached nothing new.
pub fn emit_suite_late_codecs(
    types: &[TypeDecl],
    typed: &TypedCommons,
    qual: &std::collections::HashMap<String, String>,
    runtime_use: &RuntimeUse,
    reached: &[TyId],
) -> Vec<bynk_ts::TsStmt> {
    if types.is_empty() || reached.is_empty() {
        return Vec::new();
    }
    let names: std::collections::HashSet<String> =
        types.iter().map(|t| t.name.name.clone()).collect();
    let extra: Vec<TypeRef> = reached
        .iter()
        .filter_map(|t| super::ty_to_type_ref(*t, typed.tys()))
        .filter(|r| super::serialisation::root_names_any(r, &names))
        .collect();
    if extra.is_empty() {
        return Vec::new();
    }
    let (base_names, base_insts) =
        super::serialisation::collect_codec_closure(&suite_base_codec_roots(types), &typed.types);
    let (all_names, all_insts) = super::serialisation::collect_codec_closure(&extra, &typed.types);
    let new_names: Vec<String> = all_names
        .into_iter()
        .filter(|n| !base_names.contains(n))
        .collect();
    let base_inst_names: std::collections::HashSet<String> =
        base_insts.iter().map(|i| i.ts_name()).collect();
    let new_insts: Vec<_> = all_insts
        .into_iter()
        .filter(|i| !base_inst_names.contains(&i.ts_name()))
        .collect();
    let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
    stmts.extend(super::serialisation::decls_as_stmts_block(
        super::serialisation::emit_helpers_for_owner_qualified(
            &new_names,
            &typed.types,
            "",
            qual,
            runtime_use,
        ),
    ));
    stmts.extend(super::serialisation::decls_as_stmts(
        super::serialisation::emit_generic_helpers_qualified(
            &new_insts,
            &typed.types,
            qual,
            runtime_use,
        ),
    ));
    stmts
        .into_iter()
        .map(bynk_ts::TsStmt::without_export)
        .collect()
}
