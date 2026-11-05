//! Events track, slice 3b (#978) — behavioural (runtime) proof that
//! `env.schemaVersion` reflects an event's declared `@schema(N)` annotation,
//! or `1` when the annotation is absent — not a constant, and not silently
//! wrong for one of the two body kinds `Events.emit` can be lowered inside
//! (a plain service handler vs. an agent handler; the plumbing threading
//! `event_schema_versions` through `ModuleCtx` is assigned at both).
//!
//! `events_boundary_workerd.rs`'s tests POST a hand-authored envelope
//! directly to the receiving route, bypassing emission entirely — they
//! cannot observe the mint site this slice changes. This test instead goes
//! through the real `Events.emit` lowering (`bundle` target, in-process
//! dispatch), the only path that actually exercises it.
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

// `PaymentConfirmed` (`@schema(3)`) is emitted from a plain service handler;
// `OrderCancelled` (no annotation, so version `1`) is emitted from an agent
// handler — the two distinct body kinds the `event_schema_versions` map is
// threaded through, covered in one project rather than two separate tests.
const SOURCE_ORDER: &str = r#"context commerce.order

exports transparent { PaymentConfirmed, OrderCancelled }
consumes bynk { Events }

event PaymentConfirmed @schema(3) = {
  orderId: String,
}

event OrderCancelled = {
  orderId: String,
}

agent OrderAgent {
  key id: String
  store cancelled: Cell[Bool] = false

  on call cancel(orderId: String) -> Effect[()] given Events {
    cancelled := true
    Events.emit[OrderCancelled](OrderCancelled { orderId: orderId })
  }
}

service markPaid {
  on call(orderId: String) -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId })
  }
}

service cancelOrder {
  on call(orderId: String) -> Effect[()] given Events {
    let _ <- OrderAgent(orderId).cancel(orderId)
    Effect.pure(())
  }
}
"#;

const SOURCE_NOTIFICATIONS: &str = r#"context commerce.notifications

consumes commerce.order
consumes bynk { Logger }

service OnPaymentConfirmed from Events(PaymentConfirmed) {
  on event(e: PaymentConfirmed, env: EventEnvelope) -> Effect[()] given Logger {
    Logger.info("payment:\(env.schemaVersion)")
  }
}

service OnOrderCancelled from Events(OrderCancelled) {
  on event(e: OrderCancelled, env: EventEnvelope) -> Effect[()] given Logger {
    Logger.info("cancelled:\(env.schemaVersion)")
  }
}
"#;

const DRIVER_TS: &str = r#"import { composeApp } from "./compose.js";

const app: any = composeApp();

await app.order.markPaid("o1");
await app.order.cancelOrder("o2");

console.log("ALL OK");
"#;

/// Extracts the integer logged as `"<prefix>:<n>"` — the same
/// prefix/line-scan idiom `events_envelope_behaviour.rs` uses, independent
/// of delivery ordering between the two subscribers.
fn extract_schema_version(msg: &str, prefix: &str) -> i64 {
    let needle = format!("{prefix}:");
    for line in msg.lines() {
        if let Some(rest) = line.strip_prefix(&needle) {
            return rest
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("`{line}` did not end in an integer"));
        }
    }
    panic!("no line starting with `{needle}` found in driver output:\n{msg}");
}

#[test]
fn bundle_events_schema_version_reflects_the_declared_annotation_or_defaults_to_one() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! EVENTS SCHEMA-VERSION BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! EVENTS SCHEMA-VERSION BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!(
        "bynk-events-schema-version-behaviour-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/commerce");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("order.bynk"), SOURCE_ORDER).unwrap();
    fs::write(proj.join("notifications.bynk"), SOURCE_NOTIFICATIONS).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Bundle),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the schema-version project to a bundle:\n{}",
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

    let (program, prefix) = &runner;
    let (ok, msg) = run(program, prefix, &["-p", "tsconfig.json"], &run_dir);
    assert!(
        ok,
        "tsc failed on the events-schema-version-behaviour bundle:\n{msg}"
    );

    let (ok, msg) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && msg.contains("ALL OK"),
        "events-schema-version-behaviour driver did not pass:\n{msg}"
    );

    let payment_version = extract_schema_version(&msg, "payment");
    let cancelled_version = extract_schema_version(&msg, "cancelled");

    assert_eq!(
        payment_version, 3,
        "an event declaring `@schema(3)` must carry schemaVersion 3 on the wire, not a shared/global constant:\n{msg}"
    );
    assert_eq!(
        cancelled_version, 1,
        "an event declaring no `@schema` annotation must still default to schemaVersion 1 — emitted from an agent handler, the other body kind the plumbing threads through:\n{msg}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
