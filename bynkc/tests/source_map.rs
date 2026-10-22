//! Source-map decode goldens (debugging track, slice 1; ADR 0103).
//!
//! Rather than commit opaque VLQ blobs, these tests compile a fixture, decode
//! the emitted `.ts.map`, and assert the *named* source→generated line pairs the
//! slice-0 spike fixed — the only golden form a reviewer can actually read. The
//! load-bearing claim is ADR 0103 D2 (nearest-enclosing statement): the `?`
//! `Err`-guard and the `match` `case` lines map back to their enclosing
//! statement, so a source-map-aware stepper coalesces the lowered expansion.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

/// Compile a single-commons fixture and return `(generated_ts, source_map_json)`
/// for its `reps.ts`. Each call uses its own temp dir — the tests run in parallel,
/// so a shared dir would race (one test's cleanup deletes another's input).
fn compile_reps(source: &str) -> (String, String) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("bynk_srcmap_{}_{unique}", std::process::id()));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("reps.bynk"), source).unwrap();

    let out = bynkc::compile_project(&bynk_testkit::compile_options_single(src.clone()))
        .map_err(bynkc::ProjectFailure::flatten)
        .unwrap_or_else(|e| panic!("compile failed: {e:?}"));

    let main_path = Path::new("reps.ts");
    let ts = out
        .artefacts
        .docs
        .get(main_path)
        .expect("reps.ts in output")
        .text();
    let map = out
        .artefacts
        .docs
        .get(&bynkc::sibling_path(main_path, "map"))
        .expect("reps.ts carries a source map")
        .text();
    let _ = std::fs::remove_dir_all(&dir);
    (ts, map)
}

/// #1352: like [`compile_reps`], but with the dev/test contract call-site
/// guard on (`CompileOptions::contracts(true)`, DECISION J — off by default,
/// `compile_options_single`'s own `contracts: false`) — the one real branch
/// `emit_free_fn`'s own source-map fix (#1352) left untested by every other
/// test in this file: `emit_contract_guarded_body` lowers its own body into
/// the SAME `body_text`/`merge` machinery, nested one IIFE deeper.
fn compile_reps_with_contracts(source: &str) -> (String, String) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "bynk_srcmap_contracts_{}_{unique}",
        std::process::id()
    ));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("reps.bynk"), source).unwrap();

    let out =
        bynkc::compile_project(&bynk_testkit::compile_options_single(src.clone()).contracts(true))
            .map_err(bynkc::ProjectFailure::flatten)
            .unwrap_or_else(|e| panic!("compile failed: {e:?}"));

    let main_path = Path::new("reps.ts");
    let ts = out
        .artefacts
        .docs
        .get(main_path)
        .expect("reps.ts in output")
        .text();
    let map = out
        .artefacts
        .docs
        .get(&bynkc::sibling_path(main_path, "map"))
        .expect("reps.ts carries a source map")
        .text();
    let _ = std::fs::remove_dir_all(&dir);
    (ts, map)
}

/// Decode the `mappings` string into `gen_line0 -> Some(src_line0)`, for the
/// one-segment-per-line maps the builder emits.
fn decode(mappings: &str) -> Vec<Option<i64>> {
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
    let mut src_line = 0i64;
    let mut lines = Vec::new();
    for seg in mappings.split(';') {
        if seg.is_empty() {
            lines.push(None);
            continue;
        }
        src_line += dec(seg)[2]; // [genCol, srcIdx, srcLineDelta, srcCol]
        lines.push(Some(src_line));
    }
    lines
}

fn extract_field<'a>(json: &'a str, key: &str) -> &'a str {
    let k = format!("\"{key}\":\"");
    let start = json.find(&k).expect("key present") + k.len();
    let rest = &json[start..];
    &rest[..rest.find('"').unwrap()]
}

/// The generated line (0-based) of the first line containing `needle`.
fn gen_line_of(ts: &str, needle: &str) -> usize {
    ts.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no generated line contains {needle:?}\n{ts}"))
}

/// Every generated line containing `needle` (0-based), in order.
fn gen_lines_of(ts: &str, needle: &str) -> Vec<usize> {
    ts.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

const FIXTURE: &str = "commons reps {
  type Reps = Int where InRange(1, 100)

  fn total(warmup: Int, working: Int) -> Result[Int, ValidationError] {
    let w = Reps.of(warmup)?
    let k = Reps.of(working)?
    Ok(w + k)
  }

  fn describe(warmup: Int, working: Int) -> String {
    let outcome = total(warmup, working)
    match outcome {
      Ok(n) => \"valid plan\"
      Err(e) => \"invalid plan\"
    }
  }
}
";

#[test]
fn map_is_valid_v3_with_embedded_source() {
    let (_ts, map) = compile_reps(FIXTURE);
    assert!(map.contains("\"version\":3"), "v3 header: {map}");
    // v0.72: the map `source` is the file's absolute path (forward-slashed), so an
    // editor breakpoint on the real `.bynk` binds — hence a suffix check, not an
    // exact relative match.
    assert!(
        map.contains("/reps.bynk\"]"),
        "sources ends in /reps.bynk: {map}"
    );
    // sourcesContent embeds the .bynk for dev/test fidelity (ADR 0103 D6). The
    // fixture line has no quotes, so it appears verbatim inside the JSON array.
    assert!(
        map.contains("\"sourcesContent\":["),
        "has sourcesContent array"
    );
    assert!(
        map.contains("let w = Reps.of(warmup)?"),
        "sourcesContent embeds the .bynk source"
    );
}

#[test]
fn question_propagation_anchors_to_its_let_statement() {
    // ADR 0103 D2: the `?` lowers to temp / Err-guard / unwrap — all three
    // generated lines must map back to the single `let` source line, so stepping
    // sees one source step, not a phantom stop on the guard. (Spike: 8→3.)
    let (ts, map) = compile_reps(FIXTURE);
    let lines = decode(extract_field(&map, "mappings"));
    let at = |g: usize| lines[g].unwrap_or_else(|| panic!("gen line {g} unmapped"));

    // Source (0-based): line 4 = `let w = Reps.of(warmup)?`, line 5 = `let k`.
    let guards = gen_lines_of(&ts, ".tag === \"Err\") return"); // both `?` guards
    assert_eq!(guards.len(), 2, "two `?` guards");
    assert_eq!(at(guards[0]), 4, "first `?` guard → `let w` source line");
    assert_eq!(at(guards[1]), 5, "second `?` guard → `let k` source line");

    // The unwrap binding shares the same source line as its guard.
    assert_eq!(at(gen_line_of(&ts, "const w = ")), 4);
    assert_eq!(at(gen_line_of(&ts, "const k = ")), 5);

    // The tail `Ok(w + k)` is source line 6. (Needle is specific: `return Ok(`
    // alone also matches the `Reps.of()` constructor's `return Ok(value …)`.)
    assert_eq!(at(gen_line_of(&ts, "return Ok(w + k)")), 6);
}

#[test]
fn match_arms_anchor_to_their_arm_source_line() {
    // ADR 0103 D2: each `match` arm's `case`/binding/`return` maps to that arm's
    // source line, so stepping a match walks arm-to-arm. (Spike: 13→6.)
    let (ts, map) = compile_reps(FIXTURE);
    let lines = decode(extract_field(&map, "mappings"));
    let at = |g: usize| lines[g].unwrap_or_else(|| panic!("gen line {g} unmapped"));

    // Source (0-based): 11 = `match outcome {`, 12 = `Ok(n) => …`, 13 = `Err(e) => …`.
    assert_eq!(
        at(gen_line_of(&ts, "switch (outcome.tag)")),
        11,
        "switch → match head"
    );
    assert_eq!(at(gen_line_of(&ts, "case \"Ok\":")), 12, "Ok case → Ok arm");
    assert_eq!(
        at(gen_line_of(&ts, "case \"Err\":")),
        13,
        "Err case → Err arm"
    );
}

#[test]
fn declarations_anchor_to_their_declaration() {
    // Signature lines map to the declaration's span (so a breakpoint on `fn`
    // binds at the function header).
    let (ts, map) = compile_reps(FIXTURE);
    let lines = decode(extract_field(&map, "mappings"));
    let at = |g: usize| lines[g].unwrap_or_else(|| panic!("gen line {g} unmapped"));

    assert_eq!(
        at(gen_line_of(&ts, "export function total")),
        3,
        "total → `fn total`"
    );
    assert_eq!(
        at(gen_line_of(&ts, "export function describe")),
        9,
        "describe → `fn describe`"
    );
}

const IIFE_FIXTURE: &str = "commons reps {
  fn compute(cond: Bool) -> Int {
    let a = 1
    let r = if cond {
      let y = 2
      y + 1
    } else {
      0
    }
    a + r
  }
}
";

#[test]
fn value_position_if_iife_does_not_corrupt_earlier_checkpoints() {
    // #4 review: `lower_if`'s value-position IIFE builds its `then`/`else`
    // branches into a local buffer (`iife`), spliced into the real output
    // only once fully built. Before `without_source_map`, `record_span` was
    // still called for statements inside that local buffer (e.g. `let y = 2`
    // / `y + 1` / the `else` arm's `0`) with an offset relative to *that*
    // buffer's own (small) length — but `to_v3` resolves every checkpoint's
    // generated line against the *final*, fully-emitted file's line table.
    // A small IIFE-local offset (tens of bytes) resolves there to whatever
    // line actually sits at that byte position in the real file — here, the
    // header comment / import lines, nowhere near the `if`. Confirmed by
    // reverting the fix locally: the header lines (0-4) came back mapped to
    // source lines 5 and 7 (`y + 1` and the `else` arm's `0`) instead of
    // staying unmapped, silently overriding whatever checkpoint correctly
    // belonged to those early lines. With recording suppressed for the
    // duration of the local buffer, they correctly stay unmapped instead.
    let (ts, map) = compile_reps(IIFE_FIXTURE);
    let lines = decode(extract_field(&map, "mappings"));

    let header_end = gen_line_of(&ts, "export function compute");
    for (g, mapped) in lines.iter().enumerate().take(header_end) {
        assert_eq!(
            *mapped, None,
            "gen line {g} (before the function starts) must not be mapped to \
             a source line stolen from inside the IIFE:\n{ts}"
        );
    }

    let at = |g: usize| lines[g].unwrap_or_else(|| panic!("gen line {g} unmapped"));
    // Source (0-based): line 2 = `let a = 1`, line 9 = `a + r`.
    assert_eq!(
        at(gen_line_of(&ts, "const a = ")),
        2,
        "`let a` keeps its own line"
    );
    assert_eq!(
        at(gen_line_of(&ts, "return a + r")),
        9,
        "the tail `a + r` keeps its own line"
    );
}

/// #1359: like [`compile_reps`], but for a `context`-rooted fixture (a
/// `capability`/`provides` pair can't live in a bare `commons` at all —
/// `emit_capability`'s own #1358 test hit the identical constraint) —
/// `compile_options_single` still discovers it fine, the file just needs to
/// be named after the CONTEXT, not a `commons`.
fn compile_reps_context(source: &str) -> (String, String) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "bynk_srcmap_context_{}_{unique}",
        std::process::id()
    ));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("reps.bynk"), source).unwrap();

    let out = bynkc::compile_project(&bynk_testkit::compile_options_single(src.clone()))
        .map_err(bynkc::ProjectFailure::flatten)
        .unwrap_or_else(|e| panic!("compile failed: {e:?}"));

    let main_path = Path::new("reps.ts");
    let ts = out
        .artefacts
        .docs
        .get(main_path)
        .expect("reps.ts in output")
        .text();
    let map = out
        .artefacts
        .docs
        .get(&bynkc::sibling_path(main_path, "map"))
        .expect("reps.ts carries a source map")
        .text();
    let _ = std::fs::remove_dir_all(&dir);
    (ts, map)
}

const PROVIDER_FIXTURE: &str = "context reps {
  capability Calc {
    fn double(n: Int) -> Int
    fn triple(n: Int) -> Effect[Int]
  }

  provides Calc = SimpleCalc {
    fn double(n: Int) -> Int {
      let doubled = n * 2
      doubled
    }

    fn triple(n: Int) -> Effect[Int] {
      let tripled = n * 3
      tripled
    }
  }
}
";

#[test]
fn provider_op_body_keeps_its_own_statement_lines() {
    // #1359: emit_provider's own per-op method bodies now go through the
    // same local-sub-builder-then-merge pattern #1352/#1353 established for
    // emit_free_fn/emit_contract_guarded_body -- applied once per method
    // instead of once per function, since the whole class's own wrapper
    // stays hand-written but each method is a real, individually-printed
    // TsClassMethod fragment. Regressing to a direct-into-out (or a
    // wrongly-based local buffer) lowering would collapse `double`'s own
    // body statements toward whatever offset the buffer's own length
    // happened to land on -- the same failure mode
    // `value_position_if_iife_does_not_corrupt_earlier_checkpoints`/
    // `contract_guarded_body_keeps_its_own_statement_lines` already pin for
    // the sibling cases.
    //
    // Review of #1360, finding 2: a SECOND op (`triple`, effectful, so
    // `is_async: true` gets a project-form assertion too) is the real edge
    // -- a single-op fixture can't catch a `base` that was hoisted or went
    // stale across loop iterations (each method's own `base` must be
    // recomputed from `out.len()` fresh, not reused from the first one),
    // since with only one method the class header already precedes the
    // body in `out` regardless.
    let (ts, map) = compile_reps_context(PROVIDER_FIXTURE);
    let lines = decode(extract_field(&map, "mappings"));
    let at = |g: usize| lines[g].unwrap_or_else(|| panic!("gen line {g} unmapped\n{ts}"));

    // Source (0-based): line 0 = `context reps {`, line 6 = `provides Calc = SimpleCalc {`,
    // line 8 = `let doubled = n * 2`, line 9 = `doubled`,
    // line 12 = `fn triple(n: Int) -> Effect[Int] {`,
    // line 13 = `let tripled = n * 3`, line 14 = `tripled`.
    assert_eq!(
        at(gen_line_of(&ts, "class SimpleCalc")),
        6,
        "the class header -> `provides Calc = SimpleCalc` source line"
    );
    assert_eq!(
        at(gen_line_of(&ts, "const doubled = ")),
        8,
        "`let doubled` keeps its own line, not collapsed toward the class header"
    );
    assert_eq!(
        at(gen_line_of(&ts, "return doubled;")),
        9,
        "the tail `doubled` keeps its own line"
    );
    // #2280: each op's header anchors to its own `fn` line (7 and 12), not to
    // the class header or the previous op's last body line.
    assert_eq!(
        at(gen_line_of(&ts, "  double(n: number): number {")),
        7,
        "the first op's header -> its own `fn double` line"
    );
    assert_eq!(
        at(gen_line_of(&ts, "async triple(")),
        12,
        "the second op's header -> its own `fn triple` line, not the first op's tail"
    );
    assert_eq!(
        at(gen_line_of(&ts, "const tripled = ")),
        13,
        "the second method's own `let tripled` keeps its own line, not the first method's `base`"
    );
    assert_eq!(
        at(gen_line_of(&ts, "return tripled;")),
        14,
        "the second method's own tail keeps its own line"
    );
    // #2280: the class's closing brace and the provider registration anchor to
    // the `provides` line, not the last op's final body line.
    assert_eq!(
        at(gen_line_of(&ts, "export const __SimpleCalcProvider")),
        6,
        "the registration -> the `provides` line"
    );
}

const SERVICE_FIXTURE: &str = "context reps {
  consumes bynk { Events }

  event Pinged = {
    n: Int,
  }

  service pinger {
    on call(n: Int) -> Effect[()] given Events {
      let doubled = n * 2
      Events.emit[Pinged](Pinged { n: doubled })
    }
  }

  service tripler {
    on call(n: Int) -> Effect[Int] {
      let tripled = n * 3
      tripled
    }
  }
}
";

#[test]
fn service_handler_body_keeps_its_own_statement_lines_inside_the_events_iife() {
    // #1361: emit_service's own per-handler bodies were already using the
    // local-sub-builder-then-merge pattern correctly (unlike #1352/#1353's
    // own bugs) -- but converting the handler's own header/params to a real
    // TsObjectEntry::Method meant the body could no longer be spliced
    // directly; it's now captured as one opaque Raw blob NESTED inside the
    // events-emit IIFE wrapper (`const __result = await (async () => {
    // <body> })();`), needing a two-level offset (fragment-level, then
    // blob-internal) instead of the single-level one #1352/#1359/#1360
    // established. `pinger` specifically exercises `body_emits_directly`
    // (a real `Events.emit` call), the one real shape where the body's own
    // offset within the printed method is NOT simply "right after the
    // opening brace" -- it sits after the `const __events`/`const __result
    // = await (async () => {` prologue lines too. `tripler` (review of
    // #1362, finding 3) exercises the OTHER, more common branch --
    // `body_emits_directly == false`, `body_out_offset_in_raw == 0` -- which
    // had no direct source-map coverage: a regression there is silent (the
    // `if let` chain just stops matching and the mapping is dropped) rather
    // than loud, so it needs its own assertion, not just the emitting one.
    let (ts, map) = compile_reps_context(SERVICE_FIXTURE);
    let lines = decode(extract_field(&map, "mappings"));
    let at = |g: usize| lines[g].unwrap_or_else(|| panic!("gen line {g} unmapped\n{ts}"));

    // Source (0-based): line 0 = `context reps {`, line 8 = `on call(...)`,
    // line 9 = `let doubled = n * 2`, line 10 = `Events.emit[...]`.
    assert_eq!(
        at(gen_line_of(&ts, "const doubled = ")),
        9,
        "`let doubled` keeps its own line, not collapsed toward the events-IIFE prologue"
    );
    assert_eq!(
        at(gen_line_of(&ts, "__events.push(")),
        10,
        "the emit call's own lowered push keeps its own line"
    );
    assert_eq!(
        at(gen_line_of(&ts, "const tripled = ")),
        16,
        "`tripler`'s own plain (non-emitting) handler body keeps its own line, not collapsed toward the method header"
    );
}

const CONTRACT_FIXTURE: &str = "commons reps {
  fn scale(n: Int) -> Int
  requires positive: n > 0
  ensures small: result < 1000
  {
    let doubled = n * 2
    doubled
  }
}
";

#[test]
fn contract_guarded_body_keeps_its_own_statement_lines() {
    // #1352: emit_contract_guarded_body's own body -- the branch emit_free_fn's
    // source-map fix must also cover -- lowers through the SAME body_smb/merge
    // machinery as the ordinary (unguarded) path, one IIFE deeper. Regressing
    // to the pre-#1352 direct-into-out lowering collapses every statement
    // inside `scale`'s own body toward whatever offset the isolated buffer's
    // own (small) length happened to land on in the real file -- exactly the
    // failure mode `value_position_if_iife_does_not_corrupt_earlier_checkpoints`
    // above already pins for the unguarded IIFE case, here for the guarded one.
    let (ts, map) = compile_reps_with_contracts(CONTRACT_FIXTURE);
    let lines = decode(extract_field(&map, "mappings"));
    let at = |g: usize| lines[g].unwrap_or_else(|| panic!("gen line {g} unmapped\n{ts}"));

    // Source (0-based): line 0 = `commons reps {`, line 1 = `fn scale(...)`,
    // line 5 = `let doubled = n * 2`, line 6 = `doubled`.
    assert_eq!(
        at(gen_line_of(&ts, "contract violated: precondition")),
        1,
        "precondition guard -> `fn scale` header line"
    );
    assert_eq!(
        at(gen_line_of(&ts, "const doubled = ")),
        5,
        "`let doubled` keeps its own line, not collapsed toward the module header"
    );
    assert_eq!(
        at(gen_line_of(&ts, "return doubled;")),
        6,
        "the tail `doubled` keeps its own line"
    );
}

/// Like [`compile_reps_context`], but compiled for the Workers target (only
/// mode `emit_ws_do_method`'s call site is reachable from at all — bundle
/// mode never hosts a `from websocket` `on open`'s connection in a DO), and
/// returning the workers-mode `handlers.ts` doc instead of a bundle's own
/// `<name>.ts`.
fn compile_chat_workers(source: &str) -> (String, String) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "bynk_srcmap_ws_context_{}_{unique}",
        std::process::id()
    ));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("chat.bynk"), source).unwrap();

    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(src.clone()).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .unwrap_or_else(|e| panic!("compile failed: {e:?}"));

    let main_path = out
        .artefacts
        .docs
        .keys()
        .find(|p| {
            p.to_string_lossy()
                .replace('\\', "/")
                .ends_with("handlers.ts")
        })
        .unwrap_or_else(|| panic!("no handlers.ts in output"))
        .clone();
    let ts = out.artefacts.docs[&main_path].text();
    let map = out
        .artefacts
        .docs
        .get(&bynkc::sibling_path(&main_path, "map"))
        .expect("handlers.ts carries a source map")
        .text();
    let _ = std::fs::remove_dir_all(&dir);
    (ts, map)
}

const WS_DO_FIXTURE: &str = "context chat

type RoomId = opaque String
type UserId = opaque String
type ServerFrame = { text: String }
type ClientFrame = { text: String }

actor Participant { auth = Bearer(secret = \"AUTH_SECRET\"), identity = UserId }

service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- connection.send(ServerFrame { text: \"welcome\" })
    let _ <- Room(roomId).join(user.identity, connection)
    ()
  }

  on message (roomId: RoomId, frame: ClientFrame) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).post(user.identity, frame.text)
    ()
  }
}

agent Room {
  key id: RoomId
  store members: Set[UserId]
  store conns: Map[UserId, Connection[ServerFrame]]

  on call join(u: UserId, conn: Connection[ServerFrame]) -> Effect[()] {
    let _ <- members.add(u)
    let _ <- conns.put(u, conn)
    ()
  }

  on call post(sender: UserId, text: String) -> Effect[()] {
    let _ <- conns.parTraverse((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))
    ()
  }
}
";

#[test]
fn ws_do_method_body_keeps_its_own_statement_lines() {
    // #1380/#1381 review finding 1: `emit_ws_do_method` (Arc C slice 25) is
    // the first `print_class_method` merge site in `emit.rs` to convert
    // without a dedicated source-map test — every existing check
    // (`238_websocket_inbound_workers`'s own zero-diff fixture, `tsc_verify`,
    // `cargo xtask ci`) only observes the emitted TEXT, which this slice
    // deliberately left byte-identical, so a regression in the merge
    // arithmetic itself (the `if let` guard simply not firing) would be
    // silent: zero diff, tsc green, CI green, every breakpoint inside a
    // hosted `on message` body mapping nowhere. `open`'s own body (two
    // statements) exercises the DO's `__wsOpen_*` method; `message` (one
    // statement, review of #1362/#1376's own "exercise a second real
    // fixture shape, not the same branch twice" precedent) exercises
    // `__wsMessage_*` — both go through the identical single-level shape
    // `emit_provider`'s own #1359 ops already established, now shared via
    // `emit_class_method_and_merge_source_map` (review finding 2's own
    // extraction).
    let (ts, map) = compile_chat_workers(WS_DO_FIXTURE);
    let lines = decode(extract_field(&map, "mappings"));
    let at = |g: usize| lines[g].unwrap_or_else(|| panic!("gen line {g} unmapped\n{ts}"));

    // Source (0-based): line 11 = `connection.send(...)`, line 12 =
    // `Room(roomId).join(...)`, line 17 = `Room(roomId).post(...)`.
    assert_eq!(
        at(gen_line_of(&ts, "\"welcome\"")),
        11,
        "the hosted `on open`'s own first statement keeps its own line"
    );
    assert_eq!(
        at(gen_line_of(&ts, "this.join(")),
        12,
        "the hosted `on open`'s own second statement keeps its own line"
    );
    assert_eq!(
        at(gen_line_of(&ts, "this.post(")),
        17,
        "the hosted `on message`'s own statement keeps its own line"
    );
}

const AGENT_FIXTURE: &str = "context reps {
  consumes bynk { Events }

  event Pinged = {
    n: Int,
  }

  agent Counter {
    key id: String

    store count: Cell[Int]

    on call bump(n: Int) -> Effect[Int] given Events {
      let doubled = n * 2
      let next = count + doubled
      count := next
      let _ <- Events.emit[Pinged](Pinged { n: doubled })
      next
    }

    on call ping(n: Int) -> Effect[Int] given Events {
      let tripled = n * 3
      let _ <- Events.emit[Pinged](Pinged { n: tripled })
      tripled
    }
  }
}
";

#[test]
fn agent_handler_body_keeps_its_own_statement_lines_inside_the_commit_and_events_wrappers() {
    // #1375: emit_agent's own per-handler bodies now use the identical
    // "whole prologue+body+epilogue as one opaque Raw, two-level offset"
    // shape #1361 (emit_service) already established -- but an agent
    // handler has a THIRD wrapper dimension `emit_service` never needed:
    // `writes_state` (the implicit-commit closure, `const __state = { ...
    // }; ... await this.commitState(__state);`), which can combine with
    // `body_emits_directly` (the events-IIFE) at the same time. `bump` here
    // exercises exactly that combination -- both wrappers present at once,
    // the deepest real nesting `body_out_offset_in_raw` can reach in this
    // conversion -- not just one or the other in isolation the way the
    // service-side test (#1361/#1362) only needed to. `ping` (review of
    // #1376) covers the OTHER reachable prologue shape `is_store_agent ==
    // true` reaches: `body_emits_directly == true` with `writes_state ==
    // false` -- `body_out_offset_in_raw` is assigned independently in that
    // branch too, right after its own (shorter) prologue, so a wrong
    // capture point there would silently shift every mapping for a
    // non-writing, emitting handler with nothing in the zero-diff fixture
    // check (which compares emitted TEXT only) or in `bump`'s own assertions
    // above (a different branch entirely) to catch it.
    let (ts, map) = compile_reps_context(AGENT_FIXTURE);
    let lines = decode(extract_field(&map, "mappings"));
    let at = |g: usize| lines[g].unwrap_or_else(|| panic!("gen line {g} unmapped\n{ts}"));

    assert_eq!(
        at(gen_line_of(&ts, "const doubled = ")),
        13,
        "`let doubled` keeps its own line, not collapsed toward the commit/events prologue"
    );
    assert_eq!(
        at(gen_line_of(&ts, "__state.count = ")),
        15,
        "the `:=` write's own lowered assignment keeps its own line"
    );
    assert_eq!(
        at(gen_line_of(&ts, "__events.push(")),
        16,
        "the emit call's own lowered push keeps its own line"
    );
    assert_eq!(
        at(gen_line_of(&ts, "const tripled = ")),
        21,
        "`ping`'s own non-writing, emitting handler keeps `let tripled` on its own line"
    );

    // #2270: a handler's header and prologue anchor to its own `on call` line,
    // not to the previous handler's last body line.
    for (needle, line) in [("async bump(", 12), ("async ping(", 20)] {
        let header = gen_line_of(&ts, needle);
        assert_eq!(
            at(header),
            line,
            "`{needle}` header maps to its own `on call` line"
        );
        assert_eq!(
            at(header + 1),
            line,
            "`{needle}` prologue maps to its own `on call` line"
        );
    }
}
