//! `e.within(d)` against the semantics it promises (concurrent effects, slice
//! 3, #2683).
//!
//! The Bynk harness runs `within` on virtual time, which cannot say what the
//! production helper does with a real clock. This test compiles real handlers
//! and runs them under `node` against capability doubles whose promises the
//! script settles by hand, with a `waitUntil` that keeps what it is given:
//!
//! - **a slow effect times out**, yielding `Err(TimedOut { after })`;
//! - **the abandoned effect still completes**, observed after the handler has
//!   returned: it is handed to `waitUntil`, never cancelled;
//! - **a fast effect clears its timer**, and is not handed to `waitUntil`;
//! - **`par`:** a `within` in a `par` binding bounds its own effect alone;
//! - **a lambda written in the handler** closes over its `deps`.
//!
//! A second test runs the harness's own virtual-time runtime
//! (`emitter/test_runtime/virtual_time.ts`) the same way, to pin what a Bynk
//! case cannot reach: a real provider call is not timed out by auto-advance,
//! `advance Clock by` fires the timers that fall due, and a tie goes to the
//! effect.
//!
//! Skips loudly without `node`; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure, as the other Node-driven suites do.

mod require;
mod tools;

use std::fs;
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const SOURCE: &str = r#"context demo

consumes bynk

capability Pricing {
  fn quote(id: Int) -> Effect[Int]
}

capability Audit {
  fn touch(id: Int) -> Effect[()]
}

service bounded {
  on call(id: Int) -> Effect[String] given Pricing {
    let r <- Pricing.quote(id).within(50.milliseconds)
    match r {
      Ok(q) => Effect.pure("quoted \(q)")
      Err(TimedOut { after, .. }) => Effect.pure("late \(after.toMillis())")
    }
  }
}

service both {
  on call(id: Int) -> Effect[String] given Pricing, Audit {
    par {
      let a <- Pricing.quote(id).within(50.milliseconds)
      let b <- Pricing.quote(id + 1).within(50.milliseconds)
      do Audit.touch(id)
    }
    Effect.pure("\(a.isOk()) \(b.isOk())")
  }
}

service many {
  on call(ids: List[Int]) -> Effect[Int] given Pricing {
    let rs <- ids.parTraverseAll((id) => Pricing.quote(id).within(50.milliseconds))
    Effect.pure(rs.length())
  }
}
"#;

const SCRIPT: &str = r#"import assert from "node:assert/strict";
import { bounded, both, many } from "./demo.js";

function deferred() {
  let resolve, reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// A Worker's execution context: it keeps what it is handed alive.
function execContext() {
  const kept = [];
  return { kept, waitUntil(p) { kept.push(p.then(() => "done", () => "failed")); } };
}

function doubles(exec) {
  const pending = [];
  return {
    pending,
    deps: {
      __exec: exec,
      Pricing: { quote: (id) => { const d = deferred(); pending.push({ id, ...d }); return d.promise; } },
      Audit: { touch: async () => undefined },
    },
  };
}

// 1. A slow effect times out; the abandoned effect still completes after the
//    handler has returned, and its side effect happens.
{
  const exec = execContext();
  const { pending, deps } = doubles(exec);
  let completed = false;
  const result = bounded.call(1, deps);
  await sleep(5);
  pending[0].promise.then(() => { completed = true; });
  assert.equal(await result, "late 50");
  assert.equal(completed, false, "the handler returned while the effect was still running");
  assert.equal(exec.kept.length, 1, "the abandoned effect was handed to waitUntil");
  pending[0].resolve(42);
  assert.deepEqual(await Promise.all(exec.kept), ["done"]);
  assert.equal(completed, true, "the abandoned effect ran to completion");
}

// 2. A fast effect yields Ok, is not handed to waitUntil, and clears its timer.
{
  const exec = execContext();
  const set = [], cleared = [];
  exec.timers = {
    set(fn, ms) { const h = { fn, ms }; set.push(h); return h; },
    clear(h) { cleared.push(h); },
  };
  const { pending, deps } = doubles(exec);
  const result = bounded.call(2, deps);
  await sleep(5);
  pending[0].resolve(7);
  assert.equal(await result, "quoted 7");
  assert.equal(exec.kept.length, 0);
  assert.equal(set.length, 1);
  assert.equal(set[0].ms, 50);
  assert.deepEqual(cleared, set, "the timer was cleared when the effect won");
}

// 3. A fast effect leaves no host timer pending (the 50 ms deadline would
//    otherwise hold the event loop open).
{
  const exec = execContext();
  const { pending, deps } = doubles(exec);
  const result = bounded.call(3, deps);
  await sleep(5);
  pending[0].resolve(1);
  await result;
  const timers = process.getActiveResourcesInfo().filter((n) => n === "Timeout").length;
  assert.equal(timers, 0, "no timer is left behind");
}

// 4. In a `par` block each binding is bounded alone: the slow one times out,
//    the fast one does not, and the block still waits for both.
{
  const exec = execContext();
  const { pending, deps } = doubles(exec);
  const result = both.call(10, deps);
  await sleep(5);
  assert.equal(pending.length, 2, "both effects were issued before either settled");
  pending[1].resolve(11);
  assert.equal(await result, "false true");
  assert.equal(exec.kept.length, 1);
  pending[0].resolve(10);
  await Promise.all(exec.kept);
}

// 5. A lambda written in the handler bounds each of its calls alone.
{
  const exec = execContext();
  const { pending, deps } = doubles(exec);
  const result = many.call([1, 2, 3], deps);
  await sleep(5);
  pending[0].resolve(1);
  assert.equal(await result, 3);
  assert.equal(exec.kept.length, 2, "the two slow effects were abandoned");
  pending[1].resolve(2);
  pending[2].resolve(3);
  await Promise.all(exec.kept);
}

console.log("within behaviour ok");
"#;

const VIRTUAL_SCRIPT: &str = r#"
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
function check(cond: boolean, what: string): void {
  if (!cond) throw new Error(`check failed: ${what}`);
}
const execLike = () => {
  const pending: Promise<unknown>[] = [];
  return { pending, timers: __bynkTimers, waitUntil(p: Promise<unknown>) { pending.push(p); } };
};

// `within` registers its deadline on the harness clock, not on the host's.
{
  __bynkResetVirtualTime();
  const exec = execLike();
  const never = __bynkNever();
  __bynkInflight++;
  let outcome: unknown;
  const settled = __bynkWithin(never, 2000, exec).then((r) => { outcome = r; });
  await sleep(5);
  check(outcome === undefined, "nothing fires on real time");
  __bynkAdvanceClock(1999);
  await sleep(5);
  check(outcome === undefined, "not yet due");
  __bynkAdvanceClock(1);
  await settled;
  check((outcome as { tag: string }).tag === "Err", "advance Clock fired the due deadline");
  check(__bynkClockNow === 2000, "the clock reads the deadline");
}

// A real provider call is not timed out by auto-advance: only virtual time
// moves, and only when every call in flight is waiting on it.
{
  __bynkResetVirtualTime();
  const exec = execLike();
  const deps: any = {
    Slow: { go: () => sleep(40).then(() => 5) },
  };
  __bynkTrackDeps(deps);
  const call = __bynkWithin(deps.Slow.go(), 2000, exec);
  const r: any = await __bynkSettleVirtual({ __exec: exec }, call);
  check(r.tag === "Ok" && r.value === 5, "the real call finished, un-timed-out");
  check(__bynkClockNow === 0, "the virtual clock did not move");
}

// A stub with latency completes when the clock reaches it; a tie goes to the
// effect, and the effect's own timer is first in the queue at that instant.
{
  __bynkResetVirtualTime();
  const exec = execLike();
  const deps: any = { Pricing: { quote: async () => { await __bynkSleep(2000); return 7; } } };
  __bynkTrackDeps(deps);
  const call = __bynkWithin(deps.Pricing.quote(), 2000, exec);
  const r: any = await __bynkSettleVirtual({ __exec: exec }, call);
  check(r.tag === "Ok" && r.value === 7, "the effect won the tie");
}

// A slower stub times out and its abandoned effect completes at the end of the case.
{
  __bynkResetVirtualTime();
  const exec = execLike();
  let ran = false;
  const deps: any = { Pricing: { quote: async () => { await __bynkSleep(3000); ran = true; return 7; } } };
  __bynkTrackDeps(deps);
  const call = __bynkWithin(deps.Pricing.quote(), 2000, exec);
  const r: any = await __bynkSettleVirtual({ __exec: exec }, call);
  check(r.tag === "Err" && r.error.after === 2000, "timed out");
  check(ran, "the abandoned effect was settled at the end of the case");
  check(__bynkClockNow === 3000, "the clock reached the abandoned effect");
}

// `never` is abandoned, not waited for.
{
  __bynkResetVirtualTime();
  const exec = execLike();
  const deps: any = { Pricing: { quote: () => __bynkNever() } };
  __bynkTrackDeps(deps);
  const call = __bynkWithin(deps.Pricing.quote(), 1000, exec);
  const r: any = await __bynkSettleVirtual({ __exec: exec }, call);
  check(r.tag === "Err", "timed out");
}

// A provider operation that waits on two stubs at once is still waiting on
// virtual time alone, so the clock advances (two sleepers, one call).
{
  __bynkResetVirtualTime();
  const exec = execLike();
  const deps: any = {
    Prov: { op: async () => { await Promise.all([__bynkSleep(1000), __bynkSleep(2000)]); return 3; } },
  };
  __bynkTrackDeps(deps);
  const call = __bynkWithin(deps.Prov.op(), 5000, exec);
  const r: any = await __bynkSettleVirtual({ __exec: exec }, call);
  check(r.tag === "Ok" && r.value === 3, "concurrent stub waits inside one call are driven");
  check(__bynkClockNow === 2000, "the clock reached the later wait");
}

// A call that can never complete, with no `within` to end it, is an error and
// not a hang.
{
  __bynkResetVirtualTime();
  const deps: any = { Pricing: { quote: () => __bynkNever() } };
  __bynkTrackDeps(deps);
  let message = "";
  try {
    await __bynkSettleVirtual({}, deps.Pricing.quote());
  } catch (e) {
    message = String(e);
  }
  check(message.includes("can never complete"), "a stuck call is reported");
}

// `advance Clock by` fires every timer due by then, earliest first; at the
// same instant an effect's completion goes before a deadline.
{
  __bynkResetVirtualTime();
  const fired: string[] = [];
  __bynkTimers.set(() => fired.push("deadline@5"), 5);
  const sleeping = __bynkSleep(5);
  __bynkTimers.set(() => fired.push("deadline@2"), 2);
  __bynkTimers.set(() => fired.push("late@9"), 9);
  const order: string[] = [];
  for (let t = __bynkTakeTimer(5); t !== undefined; t = __bynkTakeTimer(5)) {
    order.push(`${t.effect ? "effect" : "deadline"}@${t.at}`);
  }
  check(order.join(",") === "deadline@2,effect@5,deadline@5", `taken in order: ${order.join(",")}`);
  void sleeping;
  __bynkResetVirtualTime();
  let woke = false;
  __bynkSleep(5).then(() => { woke = true; });
  __bynkTimers.set(() => fired.push("fired@3"), 3);
  __bynkAdvanceClock(5);
  await sleep(5);
  check(woke && fired.includes("fired@3"), "advance fired every due timer");
  check(__bynkClockNow === 5, "clock at the target");
  try {
    __bynkAdvanceClock(-1);
    check(false, "a negative advance faults");
  } catch (e) {
    check(String(e).includes("non-negative"), "negative advance message");
  }
}

console.log("virtual time ok");
"#;

fn skip_without_node() -> bool {
    !tools::tool_exists("node")
        && tools::skip(
            "WITHIN BEHAVIOUR",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "`node` is not on PATH",
        )
}

#[test]
fn within_times_out_hands_the_abandoned_effect_on_and_clears_its_timer() {
    if skip_without_node() {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-within-behaviour-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("demo.bynk"), SOURCE).unwrap();

    let out = match bynkc::compile_project(&bynk_testkit::compile_options_single(&proj)) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the within project:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynkc::strip_project_to_js(out).expect("the within project strips to JS");
    let js = tmp.join("out");
    bynkc::write_output(&out, &js).unwrap();
    fs::write(js.join("check.mjs"), SCRIPT).unwrap();

    let run = Command::new("node")
        .arg("check.mjs")
        .current_dir(&js)
        .output()
        .expect("run node");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        run.status.success() && text.contains("within behaviour ok"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn the_virtual_time_runtime_advances_only_virtual_time() {
    if skip_without_node() {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-virtual-time-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let runtime = include_str!("../../bynk-emit/src/emitter/test_runtime/virtual_time.ts");
    // The real helper, reading the harness timers from `exec.timers`.
    fs::write(
        tmp.join("result.ts"),
        include_str!("../../bynk-emit/runtime/src/result.ts"),
    )
    .unwrap();
    fs::write(
        tmp.join("settle.ts"),
        include_str!("../../bynk-emit/runtime/src/settle.ts"),
    )
    .unwrap();
    fs::write(
        tmp.join("vt.mts"),
        format!("import {{ __bynkWithin }} from \"./settle.ts\";\n{runtime}\n{VIRTUAL_SCRIPT}"),
    )
    .unwrap();
    let run = Command::new("node")
        .arg("--experimental-strip-types")
        .arg("vt.mts")
        .current_dir(&tmp)
        .output()
        .expect("run node");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        run.status.success() && text.contains("virtual time ok"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
