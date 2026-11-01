//! The `parTraverse` family against the settle-all contract (concurrent
//! effects, slice 0b, #2662).
//!
//! Compiles a real project and runs it under `node` against capability doubles
//! whose promises the script settles by hand. For each of `parTraverse`,
//! `parTraverseAll` and `parTraverseTry`, on a list and on a store-`Map` query
//! receiver, it checks:
//!
//! - **settle-all:** a fast fault is raised only after a slow sibling settles;
//! - **input order:** with two faults the first in input order is raised, though
//!   it faulted last, and the later one rides along as `suppressed`;
//! - **synchronous throw:** a throw from `f` on element *k* leaves the elements
//!   before and after it issued, and the join waits for them;
//! - **`parTraverseTry` precedence:** a fault anywhere beats any `Err`; with no
//!   fault the first `Err` in input order is returned.
//!
//! Skips loudly without `node`; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure, as the other Node-driven suites do.

use std::fs;
use std::process::Command;

mod require;
mod tools;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const SOURCE: &str = r#"context demo

capability Notes {
  fn put(id: Int) -> Effect[()]
  fn tryPut(id: Int) -> Effect[Result[Int, String]]
}

service lst {
  on call(xs: List[Int]) -> Effect[()] given Notes {
    xs.parTraverse((n: Int) => Notes.put(n))
  }
}

service lstAll {
  on call(xs: List[Int]) -> Effect[List[Result[Int, String]]] given Notes {
    xs.parTraverseAll((n: Int) => Notes.tryPut(n))
  }
}

service lstTry {
  on call(xs: List[Int]) -> Effect[Result[List[Int], String]] given Notes {
    xs.parTraverseTry((n: Int) => Notes.tryPut(n))
  }
}

agent Room {
  key id: String
  store items: Map[String, Int]

  on call fan(x: Int) -> Effect[()] given Notes {
    items.parTraverse((n: Int) => Notes.put(n))
  }
  on call fanAll(x: Int) -> Effect[List[Result[Int, String]]] given Notes {
    items.parTraverseAll((n: Int) => Notes.tryPut(n))
  }
  on call fanTry(x: Int) -> Effect[Result[List[Int], String]] given Notes {
    items.parTraverseTry((n: Int) => Notes.tryPut(n))
  }
}
"#;

const SCRIPT: &str = r#"import assert from "node:assert/strict";
import { lst, lstAll, lstTry, Room } from "./demo.js";
import { Ok, Err } from "./__bynk_runtime/result.js";

function deferred() {
  let resolve, reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
const tick = () => new Promise((r) => setTimeout(r, 5));

function doubles() {
  const issued = [];
  const pending = {};
  const make = (name) => (n) => {
    issued.push(`${name}(${n})`);
    const d = deferred();
    pending[n] = d;
    return d.promise;
  };
  return {
    issued,
    pending,
    deps: { Notes: { put: make("put"), tryPut: make("tryPut") } },
  };
}

function room() {
  const stored = { items: { a: 1, b: 2, c: 3 }, __v: 1 };
  return new Room({ storage: { get: async () => stored, put: async () => {} } });
}

// Each subject runs the operation over the elements 1, 2, 3, in that order.
const subjects = [
  { name: "list parTraverse", method: "put", run: (deps) => lst.call([1, 2, 3], deps) },
  { name: "list parTraverseAll", method: "tryPut", run: (deps) => lstAll.call([1, 2, 3], deps) },
  { name: "list parTraverseTry", method: "tryPut", run: (deps) => lstTry.call([1, 2, 3], deps) },
  { name: "query parTraverse", method: "put", run: (deps) => room().fan(0, deps) },
  { name: "query parTraverseAll", method: "tryPut", run: (deps) => room().fanAll(0, deps) },
  { name: "query parTraverseTry", method: "tryPut", run: (deps) => room().fanTry(0, deps) },
];

const ok = (s, n) => (s.method === "put" ? undefined : Ok(n));
const watch = (promise) => {
  const box = {};
  box.done = promise.then((v) => { box.value = v; box.settled = true; }, (e) => { box.error = e; box.settled = true; });
  return box;
};

for (const s of subjects) {
  // 1. A fast fault waits for the slow siblings.
  {
    const { issued, pending, deps } = doubles();
    const box = watch(s.run(deps));
    await tick();
    assert.deepEqual(issued, [`${s.method}(1)`, `${s.method}(2)`, `${s.method}(3)`], `${s.name}: issued in input order`);
    const fault = new Error("down");
    pending[1].reject(fault);
    await tick();
    assert.equal(box.settled, undefined, `${s.name}: waits for the siblings`);
    pending[2].resolve(ok(s, 2));
    await tick();
    assert.equal(box.settled, undefined, `${s.name}: still waiting for the last sibling`);
    pending[3].resolve(ok(s, 3));
    await box.done;
    assert.equal(box.error, fault, `${s.name}: the fault is raised once all settle`);
  }

  // 2. Two faults: the first in input order is raised, though it faulted last.
  {
    const { pending, deps } = doubles();
    const box = watch(s.run(deps));
    await tick();
    const late = new Error("element 3");
    const early = new Error("element 1");
    pending[3].reject(late);
    pending[2].resolve(ok(s, 2));
    await tick();
    assert.equal(box.settled, undefined, `${s.name}: element 1 is still pending`);
    pending[1].reject(early);
    await box.done;
    assert.equal(box.error, early, `${s.name}: first fault in input order`);
    assert.deepEqual(box.error.suppressed, [late], `${s.name}: the later fault rides along`);
  }

  // 3. A synchronous throw from `f` on element 2: 1 and 3 are still issued and awaited.
  {
    const { issued, pending, deps } = doubles();
    const sync = new Error("sync throw");
    const inner = deps.Notes[s.method];
    deps.Notes[s.method] = (n) => {
      if (n === 2) { issued.push(`${s.method}(2 throws)`); throw sync; }
      return inner(n);
    };
    const box = watch(s.run(deps));
    await tick();
    assert.deepEqual(issued, [`${s.method}(1)`, `${s.method}(2 throws)`, `${s.method}(3)`], `${s.name}: every element is issued`);
    assert.equal(box.settled, undefined, `${s.name}: the join waits for the elements before and after`);
    pending[1].resolve(ok(s, 1));
    await tick();
    assert.equal(box.settled, undefined, `${s.name}: element 3 has not settled`);
    pending[3].resolve(ok(s, 3));
    await box.done;
    assert.equal(box.error, sync, `${s.name}: the throw is raised`);
  }
}

// parTraverseAll gathers every Result in input order.
for (const run of [(d) => lstAll.call([1, 2, 3], d), (d) => room().fanAll(0, d)]) {
  const { pending, deps } = doubles();
  const box = watch(run(deps));
  await tick();
  pending[3].resolve(Ok(3));
  pending[1].resolve(Err("one"));
  pending[2].resolve(Ok(2));
  await box.done;
  assert.deepEqual(box.value, [Err("one"), Ok(2), Ok(3)]);
}

// parTraverseTry: a fault anywhere wins over an Err; with none, the first Err in
// input order is returned.
for (const run of [(d) => lstTry.call([1, 2, 3], d), (d) => room().fanTry(0, d)]) {
  {
    const { pending, deps } = doubles();
    const box = watch(run(deps));
    await tick();
    const fault = new Error("fault at 3");
    pending[1].resolve(Err("one"));
    pending[2].resolve(Ok(2));
    pending[3].reject(fault);
    await box.done;
    assert.equal(box.error, fault, "a fault beats an Err");
  }
  {
    const { pending, deps } = doubles();
    const box = watch(run(deps));
    await tick();
    pending[3].resolve(Err("three"));
    pending[2].resolve(Err("two"));
    pending[1].resolve(Ok(1));
    await box.done;
    assert.deepEqual(box.value, Err("two"), "the first Err in input order");
  }
}

console.log("par traverse behaviour ok");
"#;

#[test]
fn par_traverse_family_settles_all_and_raises_in_input_order() {
    if !tools::tool_exists("node")
        && tools::skip(
            "PAR TRAVERSE BEHAVIOUR",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "`node` is not on PATH",
        )
    {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-par-traverse-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("demo.bynk"), SOURCE).unwrap();

    let out = match bynkc::compile_project(&bynk_testkit::compile_options_single(&proj)) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the project:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynkc::strip_project_to_js(out).expect("the project strips to JS");
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
        run.status.success() && text.contains("par traverse behaviour ok"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
