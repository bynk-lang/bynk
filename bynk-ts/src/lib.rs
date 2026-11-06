//! The TypeScript tree and printer (phase 7 of the compiler trajectory,
//! `design/bynk-compiler-trajectory.md`). Emission produces nodes
//! ([`TsProgram`]/[`TsStmt`]); [`printer::print`] is the only code in the
//! compiler that writes a character (R7.3). The crate also holds the
//! `PredKind` → check-condition mapping ([`pred_condition_and_message`]) and the one
//! TypeScript string escaper ([`escape_ts_literal`]), shared by the emitter, the
//! checker's wire defaults and the language server (crate-hygiene A2).
//!
//! **Invariant:** this crate depends on `bynk-syntax` **only** (for
//! [`bynk_syntax::span::Span`], reused unchanged rather than redefined —
//! `bynk-emit`'s own `TypeRef`/`Ir`
//! machinery is not visible here, and never will be: a function taking one
//! would not compile, the dependency isn't present, by design). `cargo tree
//! -p bynk-ts` shows `bynk-syntax` and nothing else. Matches the same
//! load-bearing shape `bynk_driver::render`'s own module doc states for itself
//! (`bynk-driver/src/render.rs`).
//!
//! **What exists here today (P7.5, #1307 → P7.8, #1313).** The reference's
//! own §7.1 sketch names four tree enums (`TsStmt`/`TsExpr`/`TsType`/
//! `TsDecl`) with real variants (`Const`, `Let`, `Binary`, `Named`, `Class`,
//! …), but only as a variant-name list, not a field-level design. P7.5
//! built only [`TsStmt`]'s `Verbatim` escape hatch ([`VerbatimOrigin`]-
//! tagged text, Q2, `design/tracks/the-typescript-tree.md` §3.2). P7.8
//! (#1313) adds the rest — [`TsExpr`]/[`TsType`]/[`TsDecl`], plus real
//! `TsStmt` kinds — but not the sketch's full variant list: only what
//! `bynk-emit/src/emitter/events_fanout.rs` (Arc C's real next file —
//! P7.8's own accepted proposal corrected the track doc's stale schedule,
//! `design/tracks/the-typescript-tree.md` §6/§9) concretely needs, grounded
//! against that file's own real shape. `bynk-emit` still builds no
//! [`TsProgram`] beyond `Verbatim` — Arc C's own first slice is what starts
//! converting real emission into real nodes, file by file; this crate
//! exists so that conversion has somewhere to land, following R10.3's own
//! "carve prospectively, at the moment the dependency appears" precedent
//! (`bynk-strip`'s own carve is the control case cited for this).

mod lint;
mod name_closure;
mod predicate;
mod printer;
mod program;
mod source_map;

pub use lint::{Violation, verbatim_violations};
pub use name_closure::{
    ModuleExports, NamespaceMemberUse, module_exports, namespace_member_uses, type_only_value_uses,
    unbound_names, value_imports,
};
pub use predicate::{
    decimal_range_condition, decimal_signed_coef, escape_ts_literal, pred_condition_and_message,
    pred_condition_and_message_for,
};
pub use printer::{
    Printed, print, print_class_method, print_class_method_and_merge, print_expr,
    print_multi_source, print_object_entry, print_object_entry_and_merge, print_stmt,
    print_stmt_and_merge, print_type,
};
pub use program::{
    ExportedName, TsArrowBody, TsBinaryOp, TsBindingName, TsClassCtor, TsClassField, TsClassMethod,
    TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsProgram, TsStmt, TsSwitchCase, TsType,
    TsTypeMember, TsUnaryOp, VerbatimOrigin,
};
pub use source_map::SourceMapBuilder;
