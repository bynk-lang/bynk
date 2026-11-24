//! A consumer builds a context's `transparent` types itself under the Workers
//! target (#4213). The guards it carries are a copy of the owner's, emitted into
//! the consumer's module, so the checks `tsc` accepts have to run here: this
//! compiles the shapes/app pair for Workers, strips it to JavaScript and runs the
//! consumer's `of` under `node` with in-range and out-of-range values, a
//! `Decimal` predicate, a `Matches` pattern and a renamed (`as`) type, whose
//! violation must name the type as its owner declares it.
//!
//! Skips loudly without `node`; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure, as the other Node-driven suites do.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;

use std::fs;
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const SHAPES: &str = r#"context shapes

exports transparent { Port, Shape, Price, Code }

type Port = Int where InRange(1, 65535)

type Shape = | Dot | Circle(radius: Int)

type Price = Decimal where Positive

type Code = String where Matches("[A-Z]{3}")
"#;

const APP: &str = r#"context app

consumes shapes { Port, Shape, Price, Code as Tag }

fn port(n: Int) -> Result[Port, ValidationError] {
  Port.of(n)
}

fn circle(r: Int) -> Shape {
  Shape.Circle(r)
}

fn dot() -> Shape {
  Dot
}

fn price(d: Decimal) -> Result[Price, ValidationError] {
  Price.of(d)
}

fn tag(s: String) -> Result[Tag, ValidationError] {
  Tag.of(s)
}
"#;

const SCRIPT: &str = r#"import assert from "node:assert/strict";
import * as app from "./workers/app/handlers.js";

const ok = (r) => r.tag === "Ok";
assert.ok(ok(app.port(8080)));
assert.equal(app.port(0).tag, "Err");
assert.equal(app.port(0).error.field, "Port");
assert.equal(app.port(70000).tag, "Err");
assert.deepEqual(app.circle(2), { tag: "Circle", radius: 2 });
assert.deepEqual(app.dot(), { tag: "Dot" });
assert.ok(ok(app.tag("ABC")));
assert.equal(app.tag("abc").tag, "Err");
// The renamed type reports the name its owner declares, as on every target.
assert.equal(app.tag("abc").error.field, "Code");
console.log("consumed construction ok");
"#;

#[test]
fn a_consumer_builds_a_consumed_contexts_transparent_types_on_workers() {
    if !tools::tool_exists("node")
        && tools::skip(
            "CONSUMED CONSTRUCTION",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "`node` is not on PATH",
        )
    {
        return;
    }
    let tmp =
        std::env::temp_dir().join(format!("bynk-consumed-construction-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("shapes.bynk"), SHAPES).unwrap();
    fs::write(proj.join("app.bynk"), APP).unwrap();

    let out = match bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(&proj)
            .target(bynk_driver::project::BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the shapes project:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynk_driver::strip::strip_project_to_js(out).expect("the project strips to JS");
    let js = tmp.join("out");
    bynk_driver::write_output(&out, &js).unwrap();
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
        run.status.success() && text.contains("consumed construction ok"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
