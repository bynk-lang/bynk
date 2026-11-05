//! Storage-track `store`/`Cell` checking: valid `Cell` usage compiles cleanly,
//! and each misuse raises its specific diagnostic.
//!
//! Agents need a `context`, which the single-file `compile` API does not accept,
//! so each case is compiled as a one-file project in a temp directory.

use std::fs;

use bynkc::{ProjectFailure, compile_project};

/// Compile `bynk` as `context shop` (one-file project) and return its diagnostic
/// category codes (empty on success).
fn codes(tag: &str, bynk: &str) -> Vec<String> {
    let root = std::env::temp_dir().join(format!("bynk_store_{}_{tag}", std::process::id()));
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("shop.bynk"), bynk).unwrap();
    let out = match compile_project(&bynk_testkit::compile_options_single(src.clone())) {
        Ok(_) => Vec::new(),
        Err(f) => ProjectFailure::flatten(f)
            .iter()
            .map(|e| e.category.to_string())
            .collect(),
    };
    let _ = fs::remove_dir_all(&root);
    out
}

/// A minimal agent in `context shop` whose handler body is `body`, around the
/// store `field`. Used by the misuse tests.
fn agent_with(field: &str, body: &str) -> String {
    format!(
        "context shop\n\nagent A {{\n  key id: String\n  {field}\n  \
         on call f(p: Int) -> Effect[()] {{ {body}\n    Effect.pure(()) }}\n}}\n"
    )
}

#[test]
fn valid_cell_agent_compiles_cleanly() {
    // Bare `Cell` reads (`count`, the `count >= 0` invariant), two `:=` writes,
    // and a read handler — all valid. No diagnostics.
    let src = "context shop\n\
        \n\
        agent Counter {\n\
        \x20 key id: String\n\
        \x20 store count: Cell[Int] = 0\n\
        \x20 store flag: Cell[Bool] = false\n\
        \n\
        \x20 invariant nonneg: count >= 0\n\
        \n\
        \x20 on call setTo(n: Int) -> Effect[()] {\n\
        \x20   count := n\n\
        \x20   flag := true\n\
        \x20   Effect.pure(())\n\
        \x20 }\n\
        \x20 on call read() -> Effect[Int] {\n\
        \x20   count\n\
        \x20 }\n\
        }\n";
    assert_eq!(
        codes("valid", src),
        Vec::<String>::new(),
        "a valid Cell agent must compile with no diagnostics"
    );
}

#[test]
fn assign_self_reference_is_rejected() {
    let cs = codes(
        "selfref",
        &agent_with("store count: Cell[Int] = 0", "count := count + 1"),
    );
    assert!(
        cs.contains(&"bynk.cell.self_reference".to_string()),
        "{cs:?}"
    );
}

#[test]
fn assign_type_mismatch_is_rejected() {
    let cs = codes(
        "mismatch",
        &agent_with("store count: Cell[Int] = 0", "count := \"x\""),
    );
    assert!(
        cs.contains(&"bynk.types.type_mismatch".to_string()),
        "{cs:?}"
    );
}

#[test]
fn assign_to_non_cell_target_is_rejected() {
    // `p` is a handler parameter, not a store cell.
    let cs = codes(
        "nontarget",
        &agent_with("store count: Cell[Int] = 0", "p := 1"),
    );
    assert!(
        cs.contains(&"bynk.cell.invalid_target".to_string()),
        "{cs:?}"
    );
}

#[test]
fn unsupported_kind_is_gated() {
    // `Queue` is a known kind but not yet functional.
    let cs = codes("queue", &agent_with("store q: Queue[Int]", ""));
    assert!(
        cs.contains(&"bynk.store.kind_unsupported".to_string()),
        "{cs:?}"
    );
}

#[test]
fn unknown_kind_is_rejected() {
    let cs = codes("widget", &agent_with("store w: Widget[Int]", ""));
    assert!(
        cs.contains(&"bynk.store.unknown_kind".to_string()),
        "{cs:?}"
    );
}

#[test]
fn cell_arity_is_checked() {
    let cs = codes("arity", &agent_with("store c: Cell[Int, Int] = 0", ""));
    assert!(cs.contains(&"bynk.store.kind_arity".to_string()), "{cs:?}");
}

// -- v0.98 Cell.update (ADR 0125) --

#[test]
fn valid_cell_update_compiles_cleanly() {
    // `update((c) => c + 1)` binds as `Effect[()]` (the `let _ <-` discards it),
    // and the bare-name read in the tail is the cell value — no diagnostics.
    let cs = codes(
        "cellupd",
        &agent_with("store n: Cell[Int] = 0", "let _ <- n.update((c) => c + 1)"),
    );
    assert_eq!(
        cs,
        Vec::<String>::new(),
        "a valid Cell.update agent must compile clean: {cs:?}"
    );
}

#[test]
fn cell_unknown_op_is_rejected() {
    let cs = codes(
        "cellop",
        &agent_with("store n: Cell[Int] = 0", "let _ <- n.frobnicate()"),
    );
    assert!(cs.contains(&"bynk.store.unknown_op".to_string()), "{cs:?}");
}

#[test]
fn cell_update_arg_type_is_checked() {
    // The combiner must be `(Int) -> Int`; a body of the wrong type is rejected.
    let cs = codes(
        "cellarg",
        &agent_with("store n: Cell[Int] = 0", "let _ <- n.update((c) => \"x\")"),
    );
    assert!(
        cs.contains(&"bynk.types.lambda_mismatch".to_string())
            || cs.contains(&"bynk.types.argument_mismatch".to_string()),
        "{cs:?}"
    );
}

// -- v0.83 storage Map (ADR 0110) --

#[test]
fn valid_map_agent_compiles_cleanly() {
    let cs = codes(
        "mapok",
        &agent_with("store m: Map[String, Int]", "let _ <- m.put(\"k\", p)"),
    );
    assert_eq!(
        cs,
        Vec::<String>::new(),
        "a valid Map agent must compile clean: {cs:?}"
    );
}

#[test]
fn map_unknown_op_is_rejected() {
    let cs = codes(
        "mapop",
        &agent_with("store m: Map[String, Int]", "let _ <- m.frobnicate(\"k\")"),
    );
    assert!(cs.contains(&"bynk.store.unknown_op".to_string()), "{cs:?}");
}

#[test]
fn map_op_arg_type_is_checked() {
    // The key type is `String`; passing an `Int` key is a mismatch.
    let cs = codes(
        "mapkey",
        &agent_with("store m: Map[String, Int]", "let _ <- m.put(5, 5)"),
    );
    assert!(
        cs.contains(&"bynk.types.argument_mismatch".to_string()),
        "{cs:?}"
    );
}

// -- v0.84 storage Set (ADR 0110) --

#[test]
fn valid_set_agent_compiles_cleanly() {
    let cs = codes(
        "setok",
        &agent_with("store s: Set[Int]", "let _ <- s.add(p)"),
    );
    assert_eq!(
        cs,
        Vec::<String>::new(),
        "a valid Set agent must compile clean: {cs:?}"
    );
}

#[test]
fn set_unknown_op_is_rejected() {
    let cs = codes(
        "setop",
        &agent_with("store s: Set[Int]", "let _ <- s.frobnicate(p)"),
    );
    assert!(cs.contains(&"bynk.store.unknown_op".to_string()), "{cs:?}");
}

#[test]
fn set_op_arg_type_is_checked() {
    // The element type is `Int`; adding a `String` is a mismatch.
    let cs = codes(
        "setelem",
        &agent_with("store s: Set[Int]", "let _ <- s.add(\"x\")"),
    );
    assert!(
        cs.contains(&"bynk.types.argument_mismatch".to_string()),
        "{cs:?}"
    );
}

// -- v0.85 storage annotations (ADR 0111) --

#[test]
fn annotation_indexed_on_map_is_functional_and_validates_keys() {
    // v0.93 (ADR 0118): `@indexed` is no longer gated — it is functional and
    // validates its `by:` keys against the map's value type. On a non-record
    // value (`Int`), `by: id` is not a field, so it reports `unknown_key` —
    // never `annotation_unsupported` (the slice has landed).
    let cs = codes(
        "annidx",
        &agent_with("store m: Map[String, Int] @indexed(by: id)", ""),
    );
    assert!(cs.contains(&"bynk.index.unknown_key".to_string()), "{cs:?}");
    assert!(
        !cs.contains(&"bynk.store.annotation_unsupported".to_string()),
        "`@indexed` is functional now, not gated: {cs:?}"
    );
}

#[test]
fn annotation_indexed_well_formed_is_accepted() {
    // A `by:` naming a value-keyable field of the value record compiles cleanly
    // (the `unused` hygiene hint is a non-failing warning, not an error).
    let src = "context shop\n\n\
        type R = { id: String, orderId: String }\n\n\
        agent A {\n  key k: String\n  \
        store m: Map[String, R] @indexed(by: orderId)\n  \
        on call f(p: Int) -> Effect[()] { Effect.pure(()) }\n}\n";
    let cs = codes("annidxok", src);
    assert!(
        !cs.iter().any(|c| c.starts_with("bynk.index.")),
        "a well-formed `@indexed` must not error: {cs:?}"
    );
}

#[test]
fn annotation_unknown_name_is_rejected() {
    let cs = codes(
        "annunk",
        &agent_with("store m: Map[String, Int] @frobnicate(1)", ""),
    );
    assert!(
        cs.contains(&"bynk.store.unknown_annotation".to_string()),
        "{cs:?}"
    );
}

#[test]
fn annotation_on_wrong_kind_is_rejected() {
    // `@ttl` belongs on `Cache`, not `Map` — a kind mismatch, and the mismatch
    // wins over the unsupported gate (we stop at the first failure).
    let cs = codes(
        "annkind",
        &agent_with("store m: Map[String, Int] @ttl(5.minutes)", ""),
    );
    assert!(
        cs.contains(&"bynk.store.annotation_kind_mismatch".to_string()),
        "{cs:?}"
    );
    assert!(
        !cs.contains(&"bynk.store.annotation_unsupported".to_string()),
        "kind mismatch must short-circuit the unsupported gate: {cs:?}"
    );
}

#[test]
fn ttl_on_cache_field_compiles_clean() {
    // v0.87: `@ttl` is functional on a `Cache`. A `Cache` field with a valid
    // `@ttl(<duration>)` and no cache-op body compiles with no diagnostics.
    let cs = codes(
        "ttlok",
        &agent_with("store c: Cache[String, Int] @ttl(5.minutes)", ""),
    );
    assert_eq!(
        cs,
        Vec::<String>::new(),
        "a Cache field with @ttl must compile clean: {cs:?}"
    );
}

// -- v0.87 storage Cache (ADR 0113) --

/// A `context shop` with a `Clock` capability and an agent whose single handler
/// (`given Clock` unless `given` is empty) runs `body` against a `Cache` field.
fn cache_agent(field: &str, given: &str, body: &str) -> String {
    format!(
        "context shop\n\n\
         capability Clock {{ fn now() -> Effect[Int] }}\n\
         provides Clock = C {{ fn now() -> Effect[Int] {{ 0 }} }}\n\n\
         agent A {{\n  key id: String\n  {field}\n  \
         on call f(k: String, p: Int) -> Effect[()]{given} {{ {body}\n    Effect.pure(()) }}\n}}\n"
    )
}

#[test]
fn valid_cache_agent_compiles_cleanly() {
    let cs = codes(
        "cacheok",
        &cache_agent(
            "store c: Cache[String, Int] @ttl(1.minutes)",
            " given Clock",
            "let _ <- c.put(k, p)",
        ),
    );
    assert_eq!(cs, Vec::<String>::new(), "{cs:?}");
}

#[test]
fn cache_without_ttl_is_rejected() {
    let cs = codes(
        "cachettl",
        &cache_agent(
            "store c: Cache[String, Int]",
            " given Clock",
            "let _ <- c.put(k, p)",
        ),
    );
    assert!(
        cs.contains(&"bynk.store.cache_ttl_required".to_string()),
        "{cs:?}"
    );
}

#[test]
fn cache_op_without_given_clock_is_rejected() {
    let cs = codes(
        "cacheclock",
        &cache_agent(
            "store c: Cache[String, Int] @ttl(1.minutes)",
            "",
            "let _ <- c.put(k, p)",
        ),
    );
    assert!(
        cs.contains(&"bynk.store.cache_needs_clock".to_string()),
        "{cs:?}"
    );
}

#[test]
fn cache_remove_needs_no_clock() {
    // `remove` is the one op that does not read the clock.
    let cs = codes(
        "cacherm",
        &cache_agent(
            "store c: Cache[String, Int] @ttl(1.minutes)",
            "",
            "let _ <- c.remove(k)",
        ),
    );
    assert!(
        !cs.contains(&"bynk.store.cache_needs_clock".to_string()),
        "remove must not require `given Clock`: {cs:?}"
    );
}

#[test]
fn cache_unknown_op_is_rejected() {
    let cs = codes(
        "cacheop",
        &cache_agent(
            "store c: Cache[String, Int] @ttl(1.minutes)",
            " given Clock",
            "let _ <- c.frobnicate(k)",
        ),
    );
    assert!(cs.contains(&"bynk.store.unknown_op".to_string()), "{cs:?}");
}

// -- v0.95 storage Log (ADR 0121) --

/// A `context shop` with a `Clock` capability and an agent whose handler `f`
/// (`given` as given) runs `body` against a `Log[Int]` field `lg`.
fn log_agent(field: &str, given: &str, ret: &str, body: &str) -> String {
    format!(
        "context shop\n\n\
         capability Clock {{ fn now() -> Effect[Instant] }}\n\
         provides Clock = C {{ fn now() -> Effect[Instant] {{ Instant.fromEpochMillis(0) }} }}\n\n\
         agent A {{\n  key id: String\n  {field}\n  \
         on call f(n: Int, t: Instant) -> {ret}{given} {{ {body} }}\n}}\n"
    )
}

#[test]
fn valid_log_append_compiles_cleanly() {
    let cs = log_agent(
        "store lg: Log[Int]",
        " given Clock",
        "Effect[()]",
        "let _ <- lg.append(n)\n    Effect.pure(())",
    );
    assert_eq!(
        codes("logok", &cs),
        Vec::<String>::new(),
        "{:?}",
        codes("logok2", &cs)
    );
}

#[test]
fn log_append_without_given_clock_is_rejected() {
    let cs = codes(
        "logclock",
        &log_agent(
            "store lg: Log[Int]",
            "",
            "Effect[()]",
            "let _ <- lg.append(n)\n    Effect.pure(())",
        ),
    );
    assert!(
        cs.contains(&"bynk.store.log_needs_clock".to_string()),
        "{cs:?}"
    );
}

#[test]
fn log_time_window_read_needs_no_clock() {
    // `since(Instant)` is a lazy read with an explicit bound — no `given Clock`.
    let cs = codes(
        "logread",
        &log_agent(
            "store lg: Log[Int]",
            "",
            "Effect[Int]",
            "lg.since(t).count()",
        ),
    );
    assert_eq!(cs, Vec::<String>::new(), "{cs:?}");
}

#[test]
fn log_retain_compiles_clean() {
    let cs = codes(
        "logretain",
        &log_agent(
            "store lg: Log[Int] @retain(30.days)",
            " given Clock",
            "Effect[()]",
            "let _ <- lg.append(n)\n    Effect.pure(())",
        ),
    );
    assert_eq!(cs, Vec::<String>::new(), "{cs:?}");
}

#[test]
fn log_query_vocabulary_composes() {
    // The general builders/terminals compose over a log query.
    let cs = codes(
        "logquery",
        &log_agent(
            "store lg: Log[Int]",
            "",
            "Effect[List[Int]]",
            "lg.recent(10).filter((x) => x > 0).collect()",
        ),
    );
    assert_eq!(cs, Vec::<String>::new(), "{cs:?}");
}

#[test]
fn log_unknown_op_is_rejected() {
    let cs = codes(
        "logop",
        &log_agent("store lg: Log[Int]", "", "Effect[Int]", "lg.frobnicate(n)"),
    );
    assert!(cs.contains(&"bynk.store.unknown_op".to_string()), "{cs:?}");
}

#[test]
fn log_cross_shape_join_with_map_composes() {
    // ADR 0121 D5: a `Map × Log` join — the windowed log query joined against a
    // store map, projected through the `into` combiner. Positional args (the v1
    // join surface).
    let src = "context shop\n\n\
        type E = { ref: String, amount: Int }\n\
        type R = { id: String, label: String }\n\
        type Out = { label: String, amount: Int }\n\n\
        agent A {\n  key id: String\n  \
        store events: Log[E]\n  \
        store recs: Map[String, R]\n  \
        on call f(t: Instant) -> Effect[List[Out]] {\n    \
        events.since(t).joinOn(recs, (e) => e.ref, (r) => r.id, (e, r) => Out { label: r.label, amount: e.amount }).collect()\n  }\n}\n";
    assert_eq!(
        codes("logjoin", src),
        Vec::<String>::new(),
        "{:?}",
        codes("lj2", src)
    );
}

#[test]
fn log_append_arg_type_is_checked() {
    let cs = codes(
        "logarg",
        &log_agent(
            "store lg: Log[Int]",
            " given Clock",
            "Effect[()]",
            "let _ <- lg.append(t)\n    Effect.pure(())",
        ),
    );
    assert!(
        cs.contains(&"bynk.types.argument_mismatch".to_string()),
        "{cs:?}"
    );
}
