//! 1B.2b (#4028): the declaration-emission tests that drove the driver's in-memory pipeline
//! (capability docs, the project pipeline's type/event loops, `OpSig` emission), moved from
//! `bynk-emit` (`emitter/emit.rs`).

#[path = "../support/emit_pipeline.rs"]
mod emit_pipeline;
use emit_pipeline::emit_bundle;

/// Review of #1358, finding 2: the printer-side test
/// (`bynk-ts::printer::tests::prints_a_generic_documented_interface_method`)
/// proves `TsTypeMember::Method.doc` renders correctly, but not that
/// `emit_capability`'s own `doc: op.documentation.clone()` wiring
/// actually threads a real op's doc comment there — the one piece of
/// this slice's own conversion with no real fixture in the corpus
/// exercising it. Closes that gap directly: a real op-level doc
/// comment, generic op, through the real emission pipeline.
#[test]
fn a_generic_capability_ops_own_doc_comment_reaches_the_interface() {
    // `capability` only parses inside a `context` (`emit_source`'s own
    // single-file path is `commons`-only) — drives `compile_in_memory`
    // directly instead, the same fs-free seam `emit_bundle` uses one
    // layer up.
    let src = r#"
context demo

capability Clock {
  ---
  Returns the current time.
  ---
  fn now[T](unit: T) -> T
}
"#;
    let out = match bynk_driver::project::compile_in_memory(
        src,
        bynk_check::project_model::BuildTarget::Bundle,
        Default::default(),
    ) {
        Ok(out) => out,
        Err(_) => panic!("fixture should compile:\n{src}"),
    };
    let ts = out
        .artefacts
        .docs
        .iter()
        .find(|(path, _)| path.to_string_lossy().contains("demo"))
        .map(|(_, doc)| doc.text())
        .expect("the context's own module should be in the output");
    assert!(
        ts.contains(
            "export interface Clock {\n  \
                 /**\n   \
                 * Returns the current time.\n   \
                 */\n  \
                 now<T>(unit: T): T;\n\
                 }\n"
        ),
        "{ts}"
    );
}

/// Exercises `emit_project`'s own Type/Event loops (Decision A/B, #1188) —
/// a separate call site from `emit_source`'s single-file `emit()` path
/// above, going through the full multi-file project pipeline instead.
#[test]
fn record_and_sum_types_emit_identically_through_the_project_pipeline() {
    let ts = emit_bundle(
        r#"
type Order = { id: String, total: Int }
type PaymentError = enum { Declined, InsufficientFunds }
"#,
    );
    assert!(
        ts.contains(
            "export interface Order {\n  readonly id: string;\n  readonly total: number;\n}\n"
        ),
        "{ts}"
    );
    assert!(
        ts.contains(
            "export type PaymentError =\n    { readonly tag: \"Declined\" }\n  | \
                 { readonly tag: \"InsufficientFunds\" };\n"
        ),
        "{ts}"
    );
}

/// Exercises `emit_project`'s **Event** mirror loop specifically (review
/// on #1190) — the one call site whose correctness this slice's own
/// `type_shape_for` leans on without a fallback: that `TypedCommons::types`
/// already holds an entry keyed `e.name.name` for every `CommonsItem::
/// Event`, the same table an ordinary declared type resolves through
/// (Decision B, #1188's own doc comment on `type_shape_for`). The prior
/// test above only reaches the Type loop; `emit_bundle` can't express an
/// event at all (it wraps its body in `commons app.bundle`, and `event`
/// outside a `context` is `bynk.event.outside_context`), so this drives
/// `project::compile_in_memory` directly with a context source instead.
#[test]
fn event_type_emits_through_the_project_pipelines_event_mirror_loop() {
    let out = bynk_driver::project::compile_in_memory(
        "context demo {\n  event Notified = { id: String }\n}\n",
        bynk_check::project_model::BuildTarget::Bundle,
        Default::default(),
    )
    .unwrap_or_else(|_| panic!("event-in-context fixture should compile"));
    let ts = out
        .artefacts
        .docs
        .iter()
        .find(|(path, _)| path.to_string_lossy().contains("demo"))
        .map(|(_, doc)| doc.text())
        .unwrap_or_else(|| panic!("the demo context's own module should be in the output"));
    assert!(
        ts.contains("export interface Notified {\n  readonly id: string;\n}\n"),
        "{ts}"
    );
}

// Differential coverage for `emit_capability` reading `bynk-emit::ir::OpSig`
// (built by `lower_capability_item_ir`/`lower_op_sig_ir`) instead of
// walking `CapabilityOp::params`/`return_type` `TypeRef`s directly (P6.x,
// #1193, slice 3 of #1187). `STORE_FIXTURE` covers a multi-op capability, a
// named return/param type (`Order`), and — new relative to #1188's own
// corpus — a generic op (`get[T]`), whose `Effect[Option[T]]` return type
// exercises `ts_ty`'s `Ty::Var` arm the way no pre-existing capability
// fixture's own emitted-TS assertions did. `capability` is a context-only
// construct (`bynk.capability.outside_context`), so — like the Event mirror
// test above — this drives `project::compile_in_memory` directly with a
// context source rather than `emit_source`/`emit_bundle`.
const STORE_FIXTURE: &str = r#"
context demo {
  type Order = { id: String, total: Int }

  capability Store {
    fn get[T](key: String) -> Effect[Option[T]]
    fn put(key: String, value: Order) -> Effect[()]
    fn count() -> Effect[Int]
  }
}
"#;

fn emit_store_context(src: &str) -> String {
    let out = bynk_driver::project::compile_in_memory(
        src,
        bynk_check::project_model::BuildTarget::Bundle,
        Default::default(),
    )
    .unwrap_or_else(|_| panic!("capability fixture should compile:\n{src}"));
    out.artefacts
        .docs
        .iter()
        .find(|(path, _)| path.to_string_lossy().contains("demo"))
        .map(|(_, doc)| doc.text())
        .unwrap_or_else(|| panic!("the demo context's own module should be in the output"))
}

/// A non-generic op's param/return types (`Order`, a named record type,
/// and `Effect[()]`/`Effect[Int]`, both base-type-adjacent) render
/// identically to `ts_type_ref`'s pre-cutover output.
#[test]
fn named_and_base_return_types_render_through_ts_ty() {
    let ts = emit_store_context(STORE_FIXTURE);
    assert!(
        ts.contains("put(key: string, value: Order): globalThis.Promise<void>;\n"),
        "{ts}"
    );
    assert!(
        ts.contains("count(): globalThis.Promise<number>;\n"),
        "{ts}"
    );
}

/// The generic op: `get[T]`'s own rigid type variable renders bare
/// (`ts_ty`'s `Ty::Var` arm) inside the `Effect[Option[T]]` return type's
/// `Option<…>` wrapper — `<T>` on the interface method itself still
/// comes from `op.type_params` (`ts_type_params`), not `sig.type_params`
/// (Decision A/B, #1193).
#[test]
fn generic_op_renders_its_rigid_type_var_and_type_param_list() {
    let ts = emit_store_context(STORE_FIXTURE);
    assert!(
        ts.contains("get<T>(key: string): globalThis.Promise<Option<T>>;\n"),
        "{ts}"
    );
}

#[test]
fn interface_and_injection_token_frame_the_ops() {
    let ts = emit_store_context(STORE_FIXTURE);
    assert!(ts.contains("export interface Store {\n"), "{ts}");
    assert!(
        ts.contains("export const __StoreToken: symbol = globalThis.Symbol(\"Store\");\n"),
        "{ts}"
    );
}
