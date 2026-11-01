//! Events track, slice 4 (#985) — behavioural (runtime) proof that
//! `via schema(N)` actually filters delivery by the envelope's
//! `schemaVersion`, not just that the annotation parses and type-checks.
//!
//! A `schemaVersion` is a compile-time constant per event type (baked into
//! the emitted mint site from `@schema(N)` or the schema registry) — one
//! compiled project can never emit the *same* event type at two different
//! versions in a single run. So this test compiles the *same* two
//! subscribers against the event twice: once with no `@schema` annotation
//! (version 1), once with `@schema(2)` (version 2), and asserts each
//! compile's single emission reaches only the subscriber whose `via
//! schema(...)` matches that build's version — proving real dispatch, not
//! just that both clauses parse.
//!
//! `OnV1` deliberately does **not** declare `env: EventEnvelope` — the only
//! way to prove the central plumbing fix (a synthetic envelope parameter
//! inserted when a bare `on event` handler's protocol carries a `via
//! schema(...)` clause) actually threads the value through, not just the
//! already-declared path `OnV2` exercises.
//!
//! Skips loudly without `tsc`+`node`; `BYNK_REQUIRE_TSC=1` turns the skip
//! into a failure (CI), matching every other toolchain-driving test in this
//! suite.

use bynkc::BuildTarget;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn base_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
}

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn discover_tsc() -> Option<(String, Vec<String>)> {
    if tool_exists("tsc") {
        return Some(("tsc".to_string(), vec![]));
    }
    if tool_exists("npx") {
        return Some((
            "npx".to_string(),
            vec![
                "--yes".to_string(),
                "-p".to_string(),
                format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED),
                "tsc".to_string(),
            ],
        ));
    }
    None
}

fn run(program: &str, prefix: &[String], args: &[&str], cwd: &Path) -> (bool, String) {
    let mut cmd = base_command(program);
    for p in prefix {
        cmd.arg(p);
    }
    for a in args {
        cmd.arg(a);
    }
    cmd.current_dir(cwd);
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => return (false, format!("could not launch {program}: {e}")),
    };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), combined)
}

const TSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "skipLibCheck": true,
    "outDir": "js",
    "rootDir": ".",
    "lib": ["ES2022", "DOM"]
  },
  "include": ["**/*.ts"]
}
"#;

// `OnV1` deliberately omits `env: EventEnvelope` (proves the synthetic
// envelope parameter); `OnV2` declares it (proves the already-declared path
// still works alongside the new guard).
const SOURCE_NOTIFICATIONS: &str = r#"context commerce.notifications

consumes commerce.order
consumes bynk { Logger }

service OnV1 from Events(PaymentConfirmed) via schema(1) {
  on event(e: PaymentConfirmed) -> Effect[()] given Logger {
    Logger.info("v1-fired")
  }
}

service OnV2 from Events(PaymentConfirmed) via schema(2) {
  on event(e: PaymentConfirmed, env: EventEnvelope) -> Effect[()] given Logger {
    Logger.info("v2-fired:\(env.schemaVersion)")
  }
}
"#;

fn source_order(schema_annotation: &str) -> String {
    format!(
        r#"context commerce.order

exports transparent {{ PaymentConfirmed }}
consumes bynk {{ Events }}

event PaymentConfirmed{schema_annotation} = {{
  orderId: String,
}}

service markPaid {{
  on call(orderId: String) -> Effect[()] given Events {{
    Events.emit[PaymentConfirmed](PaymentConfirmed {{ orderId: orderId }})
  }}
}}
"#
    )
}

// Events slice 4b (#990, through patterns slice 5's shared range production,
// #2699): a literal, a closed range, an open-above range and `_`. Each
// declares no `env`, so every one relies on the synthetic envelope parameter
// (`_` included, which has no guard but still takes the envelope).
const SOURCE_NOTIFICATIONS_RANGES: &str = r#"context commerce.notifications

consumes commerce.order
consumes bynk { Logger }

service OnExactlyV1 from Events(PaymentConfirmed) via schema(1) {
  on event(e: PaymentConfirmed) -> Effect[()] given Logger {
    Logger.info("exact-fired")
  }
}

service OnV2To4 from Events(PaymentConfirmed) via schema(2..4) {
  on event(e: PaymentConfirmed) -> Effect[()] given Logger {
    Logger.info("closed-fired")
  }
}

service OnV5Up from Events(PaymentConfirmed) via schema(5..) {
  on event(e: PaymentConfirmed) -> Effect[()] given Logger {
    Logger.info("open-fired")
  }
}

// Declares `env`, which also pulls `EventEnvelope` into this file's imports for
// the bare subscribers (the precedent `SOURCE_NOTIFICATIONS` sets).
service OnAny from Events(PaymentConfirmed) via schema(_) {
  on event(e: PaymentConfirmed, env: EventEnvelope) -> Effect[()] given Logger {
    Logger.info("any-fired")
  }
}

// `_` has no guard, but it still takes the synthetic envelope parameter.
service OnAnyBare from Events(PaymentConfirmed) via schema(_) {
  on event(e: PaymentConfirmed) -> Effect[()] given Logger {
    Logger.info("bare-wild-fired")
  }
}

service OnUpTo2 from Events(PaymentConfirmed) via schema(..2) {
  on event(e: PaymentConfirmed) -> Effect[()] given Logger {
    Logger.info("below-fired")
  }
}
"#;

const DRIVER_TS: &str = r#"import { composeApp } from "./compose.js";

const app: any = composeApp();

await app.order.markPaid("o1");

console.log("ALL OK");
"#;

/// Compile + run one variant (`schema_annotation` is `""` for version 1, or
/// `" @schema(2)"` for version 2), returning the captured stdout/stderr.
fn compile_and_run(
    runner: &(String, Vec<String>),
    notifications: &str,
    schema_annotation: &str,
    tag: &str,
) -> String {
    let tmp = std::env::temp_dir().join(format!(
        "bynk-events-schema-dispatch-behaviour-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/commerce");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("order.bynk"), source_order(schema_annotation)).unwrap();
    fs::write(proj.join("notifications.bynk"), notifications).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Bundle),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the schema-dispatch project ({tag}):\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };

    let run_dir = tmp.join("run");
    for (path, doc) in &out.artefacts.docs {
        let target = run_dir.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, doc.text()).unwrap();
    }
    bynk_testkit::write_runtime_modules(&run_dir).unwrap();
    fs::write(run_dir.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(run_dir.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(run_dir.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = runner;
    let (ok, msg) = run(program, prefix, &["-p", "tsconfig.json"], &run_dir);
    assert!(
        ok,
        "tsc failed on the events-schema-dispatch-behaviour bundle ({tag}):\n{msg}"
    );

    let (ok, msg) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && msg.contains("ALL OK"),
        "events-schema-dispatch-behaviour driver did not pass ({tag}):\n{msg}"
    );

    let _ = fs::remove_dir_all(&tmp);
    msg
}

#[test]
fn bundle_via_schema_dispatches_only_to_the_matching_version() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! EVENTS SCHEMA-DISPATCH BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! EVENTS SCHEMA-DISPATCH BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile A: no `@schema` annotation — version 1. Only `OnV1` (the
    // handler with NO declared `env`) should fire.
    let msg_v1 = compile_and_run(&runner, SOURCE_NOTIFICATIONS, "", "v1");
    assert!(
        msg_v1.contains("v1-fired"),
        "a version-1 emission must reach the `via schema(1)` subscriber, \
         which declares no `env` at all — this is the synthetic-envelope- \
         parameter plumbing fix:\n{msg_v1}"
    );
    assert!(
        !msg_v1.contains("v2-fired"),
        "a version-1 emission must NOT reach the `via schema(2)` subscriber:\n{msg_v1}"
    );

    // Compile B: `@schema(2)` — version 2. Only `OnV2` should fire, with
    // its own declared `env.schemaVersion` reading back 2.
    let msg_v2 = compile_and_run(&runner, SOURCE_NOTIFICATIONS, " @schema(2)", "v2");
    assert!(
        msg_v2.contains("v2-fired:2"),
        "a version-2 emission must reach the `via schema(2)` subscriber, \
         with env.schemaVersion reading back 2:\n{msg_v2}"
    );
    assert!(
        !msg_v2.contains("v1-fired"),
        "a version-2 emission must NOT reach the `via schema(1)` subscriber:\n{msg_v2}"
    );
}

/// #990: `via schema(2..4)`, `(5..)`, `(..2)` and `(_)` dispatch on the
/// envelope's version with inclusive bounds, alongside a literal. One compile
/// per version tested, at both edges of each range.
#[test]
fn bundle_via_schema_ranges_and_wildcard_dispatch_by_version() {
    let Some(runner) = discover_tsc() else {
        eprintln!(
            "\n!!! EVENTS SCHEMA-DISPATCH BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but no tsc runner was found");
        }
        return;
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! EVENTS SCHEMA-DISPATCH BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }
    let all = [
        "exact-fired",
        "closed-fired",
        "open-fired",
        "any-fired",
        "bare-wild-fired",
        "below-fired",
    ];
    // (version, the subscribers that must fire).
    let cases: [(u32, &[&str]); 5] = [
        (
            1,
            &["exact-fired", "any-fired", "bare-wild-fired", "below-fired"],
        ),
        (
            2,
            &[
                "closed-fired",
                "any-fired",
                "bare-wild-fired",
                "below-fired",
            ],
        ),
        (4, &["closed-fired", "any-fired", "bare-wild-fired"]),
        (5, &["open-fired", "any-fired", "bare-wild-fired"]),
        (7, &["open-fired", "any-fired", "bare-wild-fired"]),
    ];
    for (version, fires) in cases {
        let annotation = if version == 1 {
            String::new()
        } else {
            format!(" @schema({version})")
        };
        let msg = compile_and_run(
            &runner,
            SOURCE_NOTIFICATIONS_RANGES,
            &annotation,
            &format!("ranges-v{version}"),
        );
        for name in all {
            assert_eq!(
                msg.contains(name),
                fires.contains(&name),
                "version {version}: `{name}` should {}fire:\n{msg}",
                if fires.contains(&name) { "" } else { "not " }
            );
        }
    }
}
