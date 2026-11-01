//! A `par { … }` block against the semantics it promises (concurrent effects,
//! slice 0, #2640).
//!
//! The Bynk harness resolves a stub at once, so a suite cannot tell "issued
//! before the other completed" from "issued after". This test compiles a real
//! handler and runs it under `node` against capability doubles whose promises
//! the script settles by hand, which can:
//!
//! - **overlap:** every effect of the block is issued, in source order, before
//!   any is settled, and the names are bound once all have settled, whichever
//!   order that was;
//! - **settle-all, source order:** when two effects fault, the one raised is
//!   the first in source order even though it faulted last, and only after the
//!   slow, non-faulting sibling has settled;
//! - **re-binding:** a binding whose own name shadows an outer `let` still reads
//!   the outer one in its expression.
//!
//! Skips loudly without `node`; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure, as the other Node-driven suites do.

mod require;
mod tools;

use std::fs;
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const SOURCE: &str = r#"context demo

capability Users {
  fn get(id: Int) -> Effect[String]
}

capability Orders {
  fn forUser(id: Int) -> Effect[Int]
}

capability Audit {
  fn touch(id: Int) -> Effect[()]
}

service dash {
  on call(id: Int) -> Effect[String] given Users, Orders, Audit {
    par {
      let user <- Users.get(id)
      let orders <- Orders.forUser(id)
      do Audit.touch(id)
    }
    Effect.pure("\(user) \(orders)")
  }
}

service rebind {
  on call(id: Int) -> Effect[Int] given Orders {
    let orders <- Orders.forUser(id)
    par {
      let orders <- Orders.forUser(orders + 1)
      let extra <- Orders.forUser(7)
    }
    Effect.pure(orders * 100 + extra)
  }
}
"#;

const SCRIPT: &str = r#"import assert from "node:assert/strict";
import { dash, rebind } from "./demo.js";

function deferred() {
  let resolve, reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
const tick = () => new Promise((r) => setTimeout(r, 5));

function doubles() {
  const issued = [];
  const pending = {};
  const call = (name) => (...args) => {
    issued.push(`${name}(${args.join(",")})`);
    const d = deferred();
    (pending[name] ??= []).push(d);
    return d.promise;
  };
  return {
    issued,
    pending,
    deps: {
      Users: { get: call("Users.get") },
      Orders: { forUser: call("Orders.forUser") },
      Audit: { touch: call("Audit.touch") },
    },
  };
}

// 1. Every effect is issued, in source order, before any settles; the names bind
//    once all have, whatever order they settled in.
{
  const { issued, pending, deps } = doubles();
  let done = false;
  const result = dash.call(7, deps).then((v) => { done = true; return v; });
  await tick();
  assert.deepEqual(issued, ["Users.get(7)", "Orders.forUser(7)", "Audit.touch(7)"], "issued in source order, all before any settles");
  assert.equal(done, false);
  pending["Orders.forUser"][0].resolve(3);
  await tick();
  pending["Audit.touch"][0].resolve(undefined);
  await tick();
  assert.equal(done, false, "the block waits for the first binding too");
  pending["Users.get"][0].resolve("ada");
  assert.equal(await result, "ada 3");
}

// 2. Two faults: the first in SOURCE order is raised, though it faulted last,
//    and only after the slow non-faulting sibling has settled.
{
  const { pending, deps } = doubles();
  let outcome;
  const result = dash.call(1, deps).then(
    (v) => { outcome = { ok: v }; },
    (e) => { outcome = { err: e }; },
  );
  await tick();
  const usersFault = new Error("users down");
  const ordersFault = new Error("orders down");
  pending["Orders.forUser"][0].reject(ordersFault);
  await tick();
  assert.equal(outcome, undefined, "a fast fault waits for its slow siblings");
  pending["Audit.touch"][0].resolve(undefined);
  await tick();
  assert.equal(outcome, undefined, "still waiting for Users.get");
  pending["Users.get"][0].reject(usersFault);
  await result;
  assert.equal(outcome.err, usersFault, "the first fault in source order is raised");
  assert.deepEqual(outcome.err.suppressed, [ordersFault], "the later fault rides along");
}

// 3. A non-faulting sibling that is slower than the fault is waited for.
{
  const { pending, deps } = doubles();
  let outcome;
  const result = dash.call(1, deps).then(() => { outcome = "ok"; }, () => { outcome = "fault"; });
  await tick();
  pending["Users.get"][0].reject(new Error("boom"));
  pending["Audit.touch"][0].resolve(undefined);
  await tick();
  assert.equal(outcome, undefined, "Orders.forUser has not settled yet");
  pending["Orders.forUser"][0].resolve(2);
  await result;
  assert.equal(outcome, "fault");
}

// 4. A binding sees the OUTER value of a name its own `let` re-binds.
{
  const { issued, pending, deps } = doubles();
  const result = rebind.call(5, deps);
  await tick();
  pending["Orders.forUser"][0].resolve(10);
  await tick();
  assert.deepEqual(issued.slice(1), ["Orders.forUser(11)", "Orders.forUser(7)"], "the re-binding reads the outer `orders`");
  pending["Orders.forUser"][1].resolve(1);
  pending["Orders.forUser"][2].resolve(2);
  assert.equal(await result, 1 * 100 + 2);
}

console.log("par behaviour ok");
"#;

#[test]
fn par_overlaps_effects_and_settles_all_in_source_order() {
    if !tools::tool_exists("node")
        && tools::skip(
            "PAR BEHAVIOUR",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "`node` is not on PATH",
        )
    {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-par-behaviour-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("demo.bynk"), SOURCE).unwrap();

    let out = match bynkc::compile_project(&bynk_testkit::compile_options_single(&proj)) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the par project:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynkc::strip_project_to_js(out).expect("the par project strips to JS");
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
        run.status.success() && text.contains("par behaviour ok"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
