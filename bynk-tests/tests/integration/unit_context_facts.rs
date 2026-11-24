//! The two per-unit facts lowering reads without being handed them by the body:
//! the unit's source (#2620, for `unreachable`/`todo`/`detach` locations) and
//! its hidden-type spellings (#1927). Both are fields of the unit's
//! `EmitProjectCtx` (1A.19), no longer thread-locals, so these tests prove each
//! body kind still reads them, that the single-file path still has no identity,
//! and that a unit emits the same on every thread.

use bynk_check::project_model::BuildTarget;

/// Every document emitted for `files`, joined.
fn emit(files: &[(&str, &str)], target: BuildTarget) -> String {
    let out = match bynk_driver::project::compile_files_in_memory(
        files,
        target,
        Default::default(),
        true,
    ) {
        Ok(out) => out,
        Err(failure) => panic!(
            "fixture should compile, got:\n{}",
            failure
                .flatten()
                .iter()
                .map(|e| format!("{}: {}\n", e.category, e.message))
                .collect::<String>()
        ),
    };
    out.artefacts
        .docs
        .values()
        .map(|d| d.text())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `path:line:col` of the first `needle` in `src`, as the emitter reports it.
fn location(path: &str, src: &str, needle: &str) -> String {
    let at = src.find(needle).unwrap_or_else(|| panic!("no `{needle}`"));
    let (line, col) = bynk_syntax::span::line_col(src, at);
    format!("{path}:{line}:{col}")
}

/// A `unreachable("..")` in each body kind that has its own construction site
/// names its own `path:line:col`.
#[test]
fn a_fault_names_its_location_in_every_body_kind() {
    let src = r#"context demo.faults

type Box = { n: Int }

fn Box.method(self) -> Int {
  unreachable("method")
}

fn free(n: Int) -> Int {
  unreachable("free")
}

capability Files {
  fn write(name: String) -> Effect[Int]
}

provides Files = MemFiles {
  fn write(name: String) -> Effect[Int] {
    unreachable("provider")
  }
}

agent Counter {
  key name: String

  store hits: Cell[Int]

  on call bump() -> Effect[Int] {
    unreachable("agent")
  }
}

service api {
  on call(n: Int) -> Effect[Int] by Caller {
    unreachable("service")
  }
}
"#;
    let path = "demo/faults.bynk";
    let text = emit(&[(path, src)], BuildTarget::Workers);
    for kind in ["method", "free", "provider", "agent", "service"] {
        let want = location(path, src, &format!("unreachable(\"{kind}\")"));
        let line = text
            .lines()
            .find(|l| l.contains(&format!("\"{want}\"")) && l.contains("__fault"))
            .unwrap_or_else(|| panic!("{kind}: no `__fault` naming {want} in:\n{text}"));
        assert!(line.contains(&format!("\"{kind}\"")), "{kind}: {line}");
    }
}

/// A `detach` names its own site in the runtime report.
#[test]
fn a_detach_names_its_location() {
    let src = "context demo.jobs\n\ncapability Logger {\n  fn info(msg: String) -> Effect[()]\n}\n\nprovides Logger = MemLogger {\n  fn info(msg: String) -> Effect[()] {\n    Effect.pure(())\n  }\n}\n\nservice api {\n  on call(n: Int) -> Effect[Int] by Caller given Logger {\n    do Logger.info(\"hi\").detach()\n    Effect.pure(n)\n  }\n}\n";
    let path = "demo/jobs.bynk";
    let text = emit(&[(path, src)], BuildTarget::Workers);
    let want = location(path, src, "Logger.info(\"hi\").detach()");
    assert!(
        text.lines()
            .any(|l| l.contains("__bynkDetach(") && l.contains(&format!("\"{want}\""))),
        "no `__bynkDetach` naming {want} in:\n{text}"
    );
}

/// The single-file path has no identity: a fault there is `offset N`, on purpose.
#[test]
fn the_single_file_path_still_names_an_offset() {
    let src = "commons t\n\nfn f(n: Int) -> Int {\n  unreachable(\"x\")\n}\n";
    let ts = bynk_emit::compile(src, "t.bynk").expect("fixture should compile");
    let at = src.find("unreachable(\"x\")").unwrap();
    assert!(
        ts.contains(&format!("\"offset {at}\"")),
        "no `offset {at}` in:\n{ts}"
    );
}

const LIB: &str = "commons t.lib\n\ntype Cart = { items: Int }\n\nfn carts() -> List[Cart] {\n  [Cart { items: 2 }, Cart { items: 0 }]\n}\n\nfn isFull(c: Cart) -> Bool {\n  c.items > 0\n}\n";

const SHOP: &str = "context t.shop\n\nuses t.lib\n\nagent Cart {\n  key id: String\n\n  store hits: Cell[Int]\n\n  on call ping() -> Effect[Int] {\n    1\n  }\n}\n\nfn fullCarts() -> Int {\n  carts().filter((c) => isFull(c)).length()\n}\n";

/// A type a unit's own value hides is spelled as an inline import type wherever
/// the lowering names it, and a unit that hides nothing spells it bare.
#[test]
fn a_hidden_type_is_spelled_as_an_import_type() {
    let hidden = emit(
        &[("t/lib.bynk", LIB), ("t/shop.bynk", SHOP)],
        BuildTarget::Bundle,
    );
    assert!(
        hidden.contains("(__x: import(\"./lib.js\").Cart) =>"),
        "{hidden}"
    );
    // The same lambda in a unit with no agent named `Cart` annotates the bare name.
    let plain_shop = SHOP.replace("agent Cart", "agent Basket");
    let plain = emit(
        &[("t/lib.bynk", LIB), ("t/shop.bynk", &plain_shop)],
        BuildTarget::Bundle,
    );
    assert!(plain.contains("(__x: Cart) =>"), "{plain}");
    assert!(!plain.contains("import(\"./lib.js\").Cart"), "{plain}");
}

/// Nothing is shared between threads: the same unit emitted on two threads, at
/// once and repeatedly, gives the same bytes as on this one.
#[test]
fn a_unit_emits_the_same_on_every_thread() {
    let files = [("t/lib.bynk", LIB), ("t/shop.bynk", SHOP)];
    let here = emit(&files, BuildTarget::Bundle);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                s.spawn(|| {
                    (0..3)
                        .map(|_| emit(&files, BuildTarget::Bundle))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            for out in h.join().expect("emission thread panicked") {
                assert_eq!(out, here);
            }
        }
    });
}
