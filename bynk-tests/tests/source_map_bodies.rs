//! Decode goldens for v0.70 — source maps for spliced bodies (handlers + tests).
//!
//! Slice 1 mapped free-function bodies and declarations; the bodies that lower
//! through a *spliced local buffer* (service/agent handlers, test cases) stayed at
//! declaration granularity. These tests confirm those bodies now map per-statement,
//! by compiling a real project, decoding the emitted `.ts.map`, and asserting the
//! named `.bynk` lines — including a multi-file test group, to exercise the
//! multi-source map.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

fn tmp(tag: &str) -> PathBuf {
    static C: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "bynk_smb_{tag}_{}_{}",
        std::process::id(),
        C.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Decode a (possibly multi-source) v3 mappings string into, per generated line,
/// `Some((source_id, source_line))` — both 0-based — for the one-segment-per-line
/// maps the emitter produces.
fn decode(mappings: &str) -> Vec<Option<(i64, i64)>> {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let dec = |s: &str| -> Vec<i64> {
        let mut out = Vec::new();
        let (mut shift, mut acc) = (0i64, 0i64);
        for &c in s.as_bytes() {
            let d = B64.iter().position(|&b| b == c).unwrap() as i64;
            acc += (d & 0b11111) << shift;
            if d & 0b100000 != 0 {
                shift += 5;
            } else {
                out.push(if acc & 1 == 1 { -(acc >> 1) } else { acc >> 1 });
                shift = 0;
                acc = 0;
            }
        }
        out
    };
    let (mut src, mut sl) = (0i64, 0i64);
    let mut lines = Vec::new();
    for seg in mappings.split(';') {
        if seg.is_empty() {
            lines.push(None);
            continue;
        }
        let f = dec(seg); // [genCol, srcIdxDelta, srcLineDelta, srcCol]
        src += f[1];
        sl += f[2];
        lines.push(Some((src, sl)));
    }
    lines
}

/// `(sources, mappings)` from a v3 JSON document (hand-extracted; no serde dep).
fn parse_map(json: &str) -> (Vec<String>, String) {
    let sources = {
        let k = "\"sources\":[";
        let start = json.find(k).unwrap() + k.len();
        let rest = &json[start..];
        let end = rest.find(']').unwrap();
        rest[..end]
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .collect()
    };
    let mappings = {
        let k = "\"mappings\":\"";
        let start = json.find(k).unwrap() + k.len();
        let rest = &json[start..];
        rest[..rest.find('"').unwrap()].to_string()
    };
    (sources, mappings)
}

fn gen_line_of(ts: &str, needle: &str) -> usize {
    ts.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no generated line contains {needle:?}\n{ts}"))
}

/// Compile a project (optionally Workers target) and return the `(typescript,
/// source_map)` of the one output file whose path ends with `suffix`.
fn compile_file(dir: &Path, workers: bool, suffix: &str) -> (String, String) {
    let opts = bynk_testkit::compile_options_split(
        dir.to_path_buf(),
        bynkc::try_read_project_paths(dir).expect("well-formed fixture manifest"),
    );
    let opts = if workers {
        opts.target(bynkc::BuildTarget::Workers)
    } else {
        opts
    };
    let out = bynkc::compile_project(&opts)
        .map_err(bynkc::ProjectFailure::flatten)
        .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
    let main_path = out
        .artefacts
        .docs
        .keys()
        .find(|p| p.to_string_lossy().replace('\\', "/").ends_with(suffix))
        .unwrap_or_else(|| panic!("no output file ending {suffix:?}"))
        .clone();
    let ts = out.artefacts.docs[&main_path].text();
    let map = out
        .artefacts
        .docs
        .get(&bynkc::sibling_path(&main_path, "map"))
        .unwrap_or_else(|| panic!("{suffix} carries no source map"))
        .text();
    (ts, map)
}

#[test]
fn service_handler_body_maps_per_statement() {
    let dir = tmp("svc");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"svc\"\n").unwrap();
    // Handler body on .bynk lines 7 (the effect-let) and 8 (the Ok tail).
    std::fs::write(
        dir.join("src").join("svc.bynk"),
        "context svc\n\nconsumes bynk { Logger }\n\nservice api from http {\n\ton GET(\"/\") () -> Effect[HttpResult[String]] by v: Visitor given Logger {\n\t\tlet _ <- Logger.info(\"hi\")\n\t\tOk(\"hello\")\n\t}\n}\n",
    )
    .unwrap();

    let (ts, map) = compile_file(&dir, true, "handlers.ts");
    let _ = std::fs::remove_dir_all(&dir);
    let (sources, mappings) = parse_map(&map);
    // v0.72: the map `source` is the file's absolute path (so an editor breakpoint
    // on the real `.bynk` binds), hence an `ends_with` rather than an exact match.
    assert!(
        sources.len() == 1 && sources[0].ends_with("svc.bynk"),
        "single source ending in svc.bynk, got {sources:?}"
    );
    let lines = decode(&mappings);
    let at = |g: usize| {
        lines[g]
            .unwrap_or_else(|| panic!("gen line {g} unmapped"))
            .1
    };

    // The lowered effect-let maps to .bynk line 7 (0-based 6), not the `service`
    // declaration line — the whole point of v0.70.
    assert_eq!(
        at(gen_line_of(&ts, "await deps.Logger.info")),
        6,
        "effect-let -> .bynk:7"
    );
    assert_eq!(
        at(gen_line_of(&ts, "return HttpResult.Ok")),
        7,
        "Ok tail -> .bynk:8"
    );
    // The `service`/handler signature still anchors to the declaration line (4, the
    // `service` keyword line, 0-based).
    assert_eq!(
        at(gen_line_of(&ts, "export const api")),
        4,
        "signature -> service decl"
    );
}

#[test]
fn unit_test_body_maps_to_its_bynk_source() {
    let dir = tmp("tb");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"tb\"\n").unwrap();
    std::fs::write(
        dir.join("src").join("calc.bynk"),
        "commons calc {\n  fn dbl(n: Int) -> Int {\n    n + n\n  }\n}\n",
    )
    .unwrap();
    // Test body: `let r = dbl(3)` on .bynk line 3, `expect r == 6` on line 4.
    std::fs::write(
        dir.join("tests").join("calc.bynk"),
        "suite calc {\n  case \"doubles\" {\n    let r = dbl(3)\n    expect r == 6\n  }\n}\n",
    )
    .unwrap();

    let (ts, map) = compile_file(&dir, false, "tests/calc.test.ts");
    let _ = std::fs::remove_dir_all(&dir);
    let (sources, mappings) = parse_map(&map);
    assert!(
        sources.len() == 1 && sources[0].ends_with("tests/calc.bynk"),
        "single source ending in tests/calc.bynk, got {sources:?}"
    );
    let lines = decode(&mappings);
    let at = |g: usize| {
        lines[g]
            .unwrap_or_else(|| panic!("gen line {g} unmapped"))
            .1
    };

    assert_eq!(
        at(gen_line_of(&ts, "const r = dbl(3)")),
        2,
        "let -> .bynk:3"
    );
    // `r === 6` is unique to the expect *statement* (the `__bynkExpectFailure`
    // helper is also defined earlier in the module).
    assert_eq!(at(gen_line_of(&ts, "r === 6")), 3, "expect -> .bynk:4");
}

#[test]
fn multi_file_test_group_has_multiple_sources() {
    // One `test calc` target split across two `.bynk` files → the test module map
    // must carry both as sources, and each case map to its own file.
    let dir = tmp("multi");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("tests").join("calc")).unwrap();
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"multi\"\n").unwrap();
    std::fs::write(
        dir.join("src").join("calc.bynk"),
        "commons calc {\n  fn dbl(n: Int) -> Int {\n    n + n\n  }\n}\n",
    )
    .unwrap();
    // A directory test group: two files sharing the `test calc` header.
    std::fs::write(
        dir.join("tests").join("calc").join("a.bynk"),
        "suite calc {\n  case \"a\" {\n    expect dbl(1) == 2\n  }\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("tests").join("calc").join("b.bynk"),
        "suite calc {\n  case \"b\" {\n    expect dbl(2) == 4\n  }\n}\n",
    )
    .unwrap();

    let (_ts, map) = compile_file(&dir, false, "tests/calc.test.ts");
    let _ = std::fs::remove_dir_all(&dir);
    let (sources, mappings) = parse_map(&map);
    assert!(
        sources.len() == 2
            && sources.iter().any(|s| s.ends_with("a.bynk"))
            && sources.iter().any(|s| s.ends_with("b.bynk")),
        "module map should carry both test files as sources, got {sources:?}"
    );
    // Both source ids appear in the mappings (each case maps to its own file).
    let used: std::collections::HashSet<i64> = decode(&mappings)
        .into_iter()
        .flatten()
        .map(|(s, _)| s)
        .collect();
    assert!(
        used.contains(&0) && used.contains(&1),
        "both sources referenced, got {used:?}"
    );
}

#[test]
fn system_suite_case_maps_to_its_own_bynk_source() {
    // Review of #1497 (#1484's own conversion), finding 1: `emit_integration_
    // module`'s own inline case wrapper now stages each case's body map in a
    // local `case_smb` (merged under a placeholder source id) before the
    // printer's own `nested_map` merge re-tags it to the real per-case
    // `src_id` at print time — a genuine shape change from the pre-#1484
    // direct `module_smb.merge(&body_smb, &body_src, &out, body_base,
    // src_id)` call, not just a position change like every other site this
    // track has converted so far. Nothing in the suite pinned that the
    // re-tag still lands each case under its own `.bynk` file until this
    // test — the multi-source guarantee `multi_file_test_group_has_multiple_
    // sources` (above) already pins for a *unit* suite, mirrored here for a
    // `system` one (two participant contexts, two test files each
    // contributing one case to the same `suite ... as system` group).
    let dir = tmp("sys");
    std::fs::create_dir_all(dir.join("src").join("shop")).unwrap();
    std::fs::create_dir_all(dir.join("tests").join("shop_orders")).unwrap();
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"sys\"\n").unwrap();
    std::fs::write(
        dir.join("src").join("shop").join("orders.bynk"),
        "context shop.orders\n\nconsumes shop.payment as Pay\n\nexports transparent { OrderError }\n\ntype OrderError = enum { Rejected }\n\nservice place {\n  on call(cents: Int) -> Effect[Result[Int, OrderError]] {\n    let a <- Pay.authorise(cents)\n    match a {\n      Ok(n)  => Ok(n)\n      Err(_) => Err(Rejected)\n    }\n  }\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src").join("shop").join("payment.bynk"),
        "context shop.payment\n\nexports transparent { PayError }\n\ntype PayError = enum { Declined }\n\ncapability Bank {\n  fn charge(cents: Int) -> Effect[Result[Int, PayError]]\n}\n\nprovides Bank = StubBank {\n  fn charge(cents: Int) -> Effect[Result[Int, PayError]] {\n    if cents > 10000 { Err(Declined) } else { Ok(cents) }\n  }\n}\n\nservice authorise {\n  on call(cents: Int) -> Effect[Result[Int, PayError]] given Bank {\n    let r <- Bank.charge(cents)\n    r\n  }\n}\n",
    )
    .unwrap();
    // Each case's own `let r <- ...` is .bynk line 3 (0-based 2), `expect` is
    // line 4 (0-based 3) — identical line numbers in both files, so a
    // source mix-up (right line, wrong file) would slip past a same-file
    // regression this shape alone couldn't catch.
    std::fs::write(
        dir.join("tests").join("shop_orders").join("a.bynk"),
        "suite shop.orders as system {\n  case \"small order authorises across the wire\" {\n    let r <- shop.orders.place(100)\n    expect r is Ok(_)\n  }\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("tests").join("shop_orders").join("b.bynk"),
        "suite shop.orders as system {\n  case \"large order is rejected end to end\" {\n    let r <- shop.orders.place(50000)\n    expect r is Err(_)\n  }\n}\n",
    )
    .unwrap();

    let (ts, map) = compile_file(&dir, false, "tests/integration_shop_orders.test.ts");
    let _ = std::fs::remove_dir_all(&dir);
    let (sources, mappings) = parse_map(&map);
    assert!(
        sources.len() == 2
            && sources.iter().any(|s| s.ends_with("shop_orders/a.bynk"))
            && sources.iter().any(|s| s.ends_with("shop_orders/b.bynk")),
        "module map should carry both system test files as sources, got {sources:?}"
    );
    let a_id = sources
        .iter()
        .position(|s| s.ends_with("shop_orders/a.bynk"))
        .unwrap() as i64;
    let b_id = sources
        .iter()
        .position(|s| s.ends_with("shop_orders/b.bynk"))
        .unwrap() as i64;

    let lines = decode(&mappings);
    let at = |g: usize| lines[g].unwrap_or_else(|| panic!("gen line {g} unmapped in:\n{mappings}"));

    // Case a's own body — the call-site `let`, distinguished from case b's by
    // its literal argument, and the `expect`'s own lowered `Ok` tag check.
    assert_eq!(
        // #1657: an `Int` argument is serialised through the safe-integer
        // guard, so the literal is the IIFE's argument.
        at(gen_line_of(&ts, "})(100)")),
        (a_id, 2),
        "case a's own call -> a.bynk:3, source a"
    );
    assert_eq!(
        at(gen_line_of(&ts, "r.tag === \"Ok\"")),
        (a_id, 3),
        "case a's own expect -> a.bynk:4, source a"
    );
    // Case b's own body, same two lines in its own file — must resolve to
    // source b, not a (the real risk this test exists to catch).
    assert_eq!(
        at(gen_line_of(&ts, "})(50000)")),
        (b_id, 2),
        "case b's own call -> b.bynk:3, source b"
    );
    assert_eq!(
        at(gen_line_of(&ts, "r.tag === \"Err\"")),
        (b_id, 3),
        "case b's own expect -> b.bynk:4, source b"
    );
}

#[test]
fn service_handler_with_subscriber_and_schema_prologue_maps_per_statement() {
    // #1363: `from Events(Pattern { .. })` prepends a subscriber-filter guard
    // line, and `via schema(N)` prepends a schema-gate guard line, both
    // spliced into `body_out` *after* the body's own statements were already
    // lowered and their checkpoints recorded against the pre-insert text.
    // Both prologues at once (two inserted lines) is the case that used to
    // resolve every real statement two generated lines too early.
    let dir = tmp("evt");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"evt\"\n").unwrap();
    // Handler body on .bynk lines 11 (the effect-let) and 12 (the trailing call).
    std::fs::write(
        dir.join("src").join("notif.bynk"),
        "context notif\n\nconsumes bynk { Logger }\n\nevent Tick = {\n\tregion: String,\n}\n\nservice OnTick from Events(Tick { region: \"known\", .. }) via schema(1) {\n\ton event(e: Tick) -> Effect[()] given Logger {\n\t\tlet _ <- Logger.info(\"hi\")\n\t\tLogger.info(\"bye\")\n\t}\n}\n",
    )
    .unwrap();

    let (ts, map) = compile_file(&dir, false, "notif.ts");
    let _ = std::fs::remove_dir_all(&dir);
    let (sources, mappings) = parse_map(&map);
    assert!(
        sources.len() == 1 && sources[0].ends_with("notif.bynk"),
        "single source ending in notif.bynk, got {sources:?}"
    );
    let lines = decode(&mappings);
    let at = |g: usize| {
        lines[g]
            .unwrap_or_else(|| panic!("gen line {g} unmapped"))
            .1
    };

    assert_eq!(
        at(gen_line_of(&ts, "Logger.info(\"hi\")")),
        10,
        "effect-let -> .bynk:11"
    );
    assert_eq!(
        at(gen_line_of(&ts, "Logger.info(\"bye\")")),
        11,
        "trailing call -> .bynk:12"
    );
}
