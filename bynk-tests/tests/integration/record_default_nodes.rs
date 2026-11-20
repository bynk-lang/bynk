//! 1A.31 (#3732): a record literal that omits defaulted fields lowers to an `Object` node. A
//! project is emitted through the driver and the exact lines are pinned, with the text-exit
//! counter read where it is real (see `text_exit_counters.rs`). The tree and the node builder have
//! their own unit tests in `bynk-check/src/wire_default.rs` and `bynk-emit/src/emitter/default_value.rs`.
use bynk_emit::emitter::text_counters;

/// `(text exits, every emitted document joined)` for the one-context project `src` on Workers.
fn mem(src: &str) -> (usize, String) {
    text_counters::reset();
    let out = bynk_driver::project::compile_files_in_memory(
        &[("ops/jobs.bynk", src)],
        bynk_check::project_model::BuildTarget::Workers,
        Default::default(),
        true,
    )
    .unwrap_or_else(|e| {
        panic!(
            "should compile:\n{src}\n{}",
            bynk_driver::render::render_project_errors(&e.flatten())
        )
    });
    let text = out
        .artefacts
        .docs
        .values()
        .map(|d| d.text())
        .collect::<Vec<_>>()
        .join("\n");
    (text_counters::exits(), text)
}

fn assert_line(text: &str, line: &str) {
    assert!(
        text.lines().any(|l| l.trim_start() == line),
        "missing line {line:?} in\n{text}"
    );
}

const OPTIONS: &str = r#"context ops.jobs

type Mode = enum { Fast, Slow }
type Retry = { times: Int = 3, backoff: Float = 1.5 }
type Options = {
  url: String,
  mode: Mode = Fast,
  retry: Retry = Retry { },
  tags: List[String] = ["a", "b"],
  limit: Option[Int] = None,
  seen: Set[String] = Set.empty(),
  index: Map[String, Int] = Map.empty(),
  delta: Int = -5,
  ok: Result[Int, String] = Ok(1),
}

fn plain(url: String) -> Options { Options { url } }
fn named(url: String, n: Int) -> Options { Options { url, limit: Some(n) } }
fn lam(xs: List[String]) -> List[Options] { xs.map((u) => Options { url: u }) }
"#;

/// A record that omits its defaulted fields is a node, so no construction leaves as text, and each
/// default prints the in-memory value it always did.
#[test]
fn a_record_that_omits_defaulted_fields_prints_as_a_node_with_no_text_exit() {
    let (exits, text) = mem(OPTIONS);
    assert_eq!(exits, 0, "no text exit:\n{text}");
    let defaults = r#"mode: { tag: "Fast" as const }, retry: { times: 3, backoff: 1.5 }, tags: ["a", "b"], limit: { tag: "None" as const }, seen: new globalThis.Set(), index: new globalThis.Map(), delta: -5, ok: { tag: "Ok" as const, value: 1 } }"#;
    for line in [
        format!("return {{ url, {defaults};"),
        r#"return { url, limit: Some(n), mode: { tag: "Fast" as const }, retry: { times: 3, backoff: 1.5 }, tags: ["a", "b"], seen: new globalThis.Set(), index: new globalThis.Map(), delta: -5, ok: { tag: "Ok" as const, value: 1 } };"#.to_string(),
    ] {
        assert_line(&text, &line);
    }
    assert!(
        text.contains(&format!("=> ({{ url: u, {defaults})")),
        "a record literal as a lambda body keeps its brackets:\n{text}"
    );
}
