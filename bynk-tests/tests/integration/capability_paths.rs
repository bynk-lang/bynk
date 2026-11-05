//! #2469: the checker records one shortest witness per `(given entry,
//! capability)` and computes the full set of chains on demand. These tests drive
//! `diagnose_project` over one-file projects and inspect
//! `ProjectDiagnostics::capability_paths` — direct, transitive, multi-path and
//! cyclic shapes.

use std::fs;

use bynk_check::capability_paths::{CapabilityGraph, PathStep};
use bynk_ide::diagnose_project;

const PRELUDE: &str = "context shop\n\
    \n\
    capability Files { fn write(name: String) -> Effect[()] }\n\
    provides Files = MemFiles { fn write(name: String) -> Effect[()] { Effect.pure(()) } }\n\
    \n";

/// Analyse `body` (after the prelude) as a one-file project; return the text and
/// the file's capability graph.
fn graph_for(tag: &str, body: &str) -> (String, CapabilityGraph) {
    let text = format!("{PRELUDE}{body}");
    let root = std::env::temp_dir().join(format!("bynk_cappaths_{}_{tag}", std::process::id()));
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("shop.bynk"), &text).unwrap();
    let result = diagnose_project(
        &src,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(src.clone())),
    );
    let graph = result
        .capability_paths
        .values()
        .next()
        .cloned()
        .unwrap_or_default();
    let _ = fs::remove_dir_all(&root);
    (text, graph)
}

/// Resolve a step's site to the text it covers, for readable assertions.
fn site_text<'a>(text: &'a str, step: &PathStep) -> &'a str {
    let s = step.site();
    &text[s.start..s.end]
}

/// The given entry whose span text is `Files` at the `nth` `given Files`.
fn given_entry(text: &str, nth: usize) -> bynk_syntax::span::Span {
    let at = text
        .match_indices("given Files")
        .nth(nth)
        .unwrap_or_else(|| panic!("no `given Files` #{nth}"))
        .0;
    bynk_syntax::span::Span::new(at + "given ".len(), at + "given Files".len())
}

const CHAIN: &str = "agent Index {\n\
    \x20 key name: String\n\
    \x20 store hits: Cell[Int]\n\
    \x20 on call withdraw(version: String) -> Effect[()] given Files {\n\
    \x20   Files.write(version)\n\
    \x20 }\n\
    }\n\
    \n\
    agent Front {\n\
    \x20 key name: String\n\
    \x20 store hits: Cell[Int]\n\
    \x20 on call go(version: String) -> Effect[()] {\n\
    \x20   let index = Index(self.name)\n\
    \x20   index.withdraw(version)\n\
    \x20 }\n\
    }\n\
    \n";

/// A handler that calls the capability itself: the witness is the one direct
/// call, and there is exactly one path.
#[test]
fn direct_use_is_a_one_step_path() {
    let (text, graph) = graph_for("direct", CHAIN);
    let entry = given_entry(&text, 0);
    let w = graph.witness(entry).expect("a witness for Index.withdraw");
    assert_eq!(w.capability, "Files");
    assert_eq!(w.declaration, "Index.withdraw");
    assert!(w.path.is_direct());
    assert_eq!(site_text(&text, &w.path.steps[0]), "Files.write(version)");
    let (_, paths, truncated) = graph.all_paths(entry).unwrap();
    assert_eq!(paths.len(), 1);
    assert!(!truncated);
}

/// A provider that reaches `Files` only through two agent-handler calls: the
/// witness names each call, then the use.
#[test]
fn transitive_use_walks_the_agent_calls() {
    let body = format!(
        "{CHAIN}capability Takedown {{ fn take(name: String) -> Effect[()] }}\n\
         provides Takedown = BucketTakedown given Files {{\n\
         \x20 fn take(name: String) -> Effect[()] {{\n\
         \x20   Front(name).go(\"1.0.0\")\n\
         \x20 }}\n\
         }}\n"
    );
    let (text, graph) = graph_for("transitive", &body);
    let w = graph
        .witness(given_entry(&text, 1))
        .expect("a witness for the provider's `given Files`");
    assert_eq!(w.declaration, "BucketTakedown");
    let kinds: Vec<String> = w
        .path
        .steps
        .iter()
        .map(|s| match s {
            PathStep::Call { agent, handler, .. } => format!("call {agent}.{handler}"),
            PathStep::Use { .. } => "use".to_string(),
        })
        .collect();
    assert_eq!(kinds, ["call Front.go", "call Index.withdraw", "use"]);
    assert_eq!(site_text(&text, &w.path.steps[0]), "go");
    assert_eq!(site_text(&text, &w.path.steps[2]), "Files.write(version)");
}

/// Two routes to the same use: the witness is the shorter, and the on-demand
/// set lists both, shortest first.
#[test]
fn multi_path_keeps_the_shortest_and_lists_all_on_demand() {
    let body = format!(
        "{CHAIN}capability Takedown {{ fn take(name: String) -> Effect[()] }}\n\
         provides Takedown = BucketTakedown given Files {{\n\
         \x20 fn take(name: String) -> Effect[()] {{\n\
         \x20   let _ <- Front(name).go(\"1.0.0\")\n\
         \x20   Index(name).withdraw(\"1.0.0\")\n\
         \x20 }}\n\
         }}\n"
    );
    let (text, graph) = graph_for("multi", &body);
    let entry = given_entry(&text, 1);
    let w = graph.witness(entry).unwrap();
    assert_eq!(w.path.steps.len(), 2, "direct agent call is the shortest");
    let (cap, paths, truncated) = graph.all_paths(entry).unwrap();
    assert_eq!(cap, "Files");
    assert!(!truncated);
    let lens: Vec<usize> = paths.iter().map(|p| p.steps.len()).collect();
    assert_eq!(lens, [2, 3]);
    assert_eq!(paths[0], w.path, "the witness is the first of the full set");
}

/// A call cycle terminates, and the cycle's own back-edge is not a path.
#[test]
fn call_cycles_terminate() {
    let body = "agent Ping {\n\
        \x20 key name: String\n\
        \x20 store hits: Cell[Int]\n\
        \x20 on call a(v: String) -> Effect[()] given Files {\n\
        \x20   let _ <- Files.write(v)\n\
        \x20   Pong(self.name).b(v)\n\
        \x20 }\n\
        }\n\
        \n\
        agent Pong {\n\
        \x20 key name: String\n\
        \x20 store hits: Cell[Int]\n\
        \x20 on call b(v: String) -> Effect[()] {\n\
        \x20   Ping(self.name).a(v)\n\
        \x20 }\n\
        }\n";
    let (text, graph) = graph_for("cycle", body);
    let (_, paths, _) = graph.all_paths(given_entry(&text, 1 - 1)).unwrap();
    assert_eq!(paths.len(), 1);
    assert!(paths[0].is_direct());
}

/// A `given` the body never reaches has no witness (the entry is unused, which
/// `bynk.given.unused_capability` reports separately).
#[test]
fn an_unused_given_has_no_witness() {
    let body = "agent Idle {\n\
        \x20 key name: String\n\
        \x20 store hits: Cell[Int]\n\
        \x20 on call noop() -> Effect[()] given Files {\n\
        \x20   Effect.pure(())\n\
        \x20 }\n\
        }\n";
    let (text, graph) = graph_for("unused", body);
    let entry = given_entry(&text, 0);
    assert!(graph.witness(entry).is_none());
    let (_, paths, _) = graph.all_paths(entry).unwrap();
    assert!(paths.is_empty());
}

/// Handlers that all call each other have factorially many simple paths; the
/// witness and an unused entry must not enumerate them. (Unbudgeted, this
/// test does not finish.)
#[test]
fn a_dense_call_cycle_does_not_blow_up() {
    const N: usize = 9;
    let mut body = String::new();
    for i in 0..N {
        let calls: Vec<String> = (0..N)
            .filter(|j| *j != i)
            .map(|j| format!("A{j}(self.name).h{j}(v)"))
            .collect();
        // `Effect` blocks: every call but the last binds with `let _ <-`.
        let (last, rest) = calls.split_last().unwrap();
        let lets: String = rest
            .iter()
            .map(|c| format!("\x20   let _ <- {c}\n"))
            .collect();
        let (given, used) = if i == 0 {
            (" given Files", "\x20   let _ <- Files.write(v)\n")
        } else {
            ("", "")
        };
        body.push_str(&format!(
            "agent A{i} {{\n\x20 key name: String\n\x20 store hits: Cell[Int]\n\
             \x20 on call h{i}(v: String) -> Effect[()]{given} {{\n{used}{lets}\x20   {last}\n\x20 }}\n}}\n\n"
        ));
    }
    // An unused entry on a handler inside the cycle's reach.
    body.push_str(
        "agent Idle {\n\x20 key name: String\n\x20 store hits: Cell[Int]\n\
         \x20 on call noop() -> Effect[()] given Files {\n\x20   A1(self.name).h1(\"x\")\n\x20 }\n}\n",
    );
    let (text, graph) = graph_for("dense", &body);
    // The cycle's first handler uses `Files`: one direct path.
    let (_, paths, _) = graph.all_paths(given_entry(&text, 0)).unwrap();
    assert_eq!(paths.len(), 1);
    // `Idle` reaches it through `A1` in a shortest chain of two calls... and
    // every other route is a distinct simple path, bounded by the cap.
    let idle = given_entry(&text, 1);
    let w = graph.witness(idle).expect("Idle reaches Files via A1");
    assert_eq!(w.path.steps.len(), 3);
    let (_, all, truncated) = graph.all_paths(idle).unwrap();
    assert!(all.len() <= bynk_check::capability_paths::MAX_PATHS);
    assert!(truncated, "dense graph exceeds the path cap");
    assert_eq!(all[0], w.path);
}
