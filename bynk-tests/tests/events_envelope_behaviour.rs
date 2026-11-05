//! Events track, slice 2 (spine #936) — behavioural (runtime) proof that the
//! `EventEnvelope` handed to an `on event(e: E, env: EventEnvelope)` handler
//! is minted once per emission and shared by every subscriber, not minted
//! independently per subscriber or per delivery.
//!
//! Two sibling subscribers of the same event, both declaring the optional
//! `env` parameter, and two emissions. The discriminating shape (no
//! single-subscriber test can produce it): within one emission the two
//! subscribers must observe the *identical* `eventId` (a mint-at-delivery or
//! mint-inside-the-fan-out-loop bug would give each subscriber its own id);
//! across the two emissions the ids must *differ* (a constant or
//! once-only-minted id would pass an equality-only test trivially).
//!
//! Deliver-and-filter (ADR 0286, unchanged): both subscribers are
//! pattern-less and receive both emissions. The envelope is minted at the
//! single `__events.push` call site (`lower.rs`), before fan-out duplicates
//! that one buffered entry to both subscribers — this test is the proof that
//! invariant actually holds at runtime, not just by reading the lowering
//! code.
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

// The publisher: one event, one trigger service, called twice with distinct
// tags so the two emissions are distinguishable in the driver output.
const SOURCE_ORDER: &str = r#"context commerce.order

exports transparent { Tick }
consumes bynk { Events }

event Tick = {
  tag: String,
}

service trigger {
  on call(tag: String) -> Effect[()] given Events {
    Events.emit[Tick](Tick { tag: tag })
  }
}
"#;

// Two sibling subscribers of the same event, both declaring the optional
// `env: EventEnvelope` parameter. Neither has a subscription pattern — both
// receive both emissions.
const SOURCE_NOTIFICATIONS: &str = r#"context commerce.notifications

consumes commerce.order
consumes bynk { Logger }

service SubscriberA from Events(Tick) {
  on event(e: Tick, env: EventEnvelope) -> Effect[()] given Logger {
    Logger.info("a:\(env.eventId):\(e.tag)")
  }
}

service SubscriberB from Events(Tick) {
  on event(e: Tick, env: EventEnvelope) -> Effect[()] given Logger {
    Logger.info("b:\(env.eventId):\(e.tag)")
  }
}
"#;

const DRIVER_TS: &str = r#"import { composeApp } from "./compose.js";

const app: any = composeApp();

// Two emissions, distinct tags, so each logged eventId is attributable to
// exactly one emission regardless of delivery order.
await app.order.trigger("t1");
await app.order.trigger("t2");

console.log("ALL OK");
"#;

/// Extracts the `eventId` logged as `"<subscriber>:<eventId>:<tag>"` for a
/// given subscriber tag ("a"/"b") and emission tag ("t1"/"t2"). Matching on
/// both the subscriber prefix and the emission-tag suffix (rather than
/// relying on line order) makes the assertion independent of delivery
/// ordering between the two subscribers.
fn extract_event_id(msg: &str, subscriber: &str, emission_tag: &str) -> String {
    let prefix = format!("{subscriber}:");
    let suffix = format!(":{emission_tag}");
    for line in msg.lines() {
        if let Some(rest) = line.strip_prefix(&prefix)
            && let Some(id) = rest.strip_suffix(&suffix)
        {
            return id.to_string();
        }
    }
    panic!("no line matching `{prefix}...{suffix}` found in driver output:\n{msg}");
}

#[test]
fn bundle_events_envelope_is_minted_once_per_emission() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! EVENTS ENVELOPE BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! EVENTS ENVELOPE BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!(
        "bynk-events-envelope-behaviour-{}",
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
            "compile the events-envelope project to a bundle:\n{}",
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
        "tsc failed on the events-envelope-behaviour bundle:\n{msg}"
    );

    let (ok, msg) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && msg.contains("ALL OK"),
        "events-envelope-behaviour driver did not pass:\n{msg}"
    );

    let a_t1 = extract_event_id(&msg, "a", "t1");
    let b_t1 = extract_event_id(&msg, "b", "t1");
    let a_t2 = extract_event_id(&msg, "a", "t2");
    let b_t2 = extract_event_id(&msg, "b", "t2");

    assert!(!a_t1.is_empty(), "eventId must not be empty:\n{msg}");

    // The positive this test exists to prove: one emission mints exactly one
    // eventId, shared by every subscriber it fans out to.
    assert_eq!(
        a_t1, b_t1,
        "both subscribers of the t1 emission must observe the identical eventId \
         (minted once at __events.push, not per delivery):\n{msg}"
    );
    assert_eq!(
        a_t2, b_t2,
        "both subscribers of the t2 emission must observe the identical eventId:\n{msg}"
    );
    // The control: a constant or once-only-minted id would pass the two
    // equality assertions above trivially. Two distinct emissions must mint
    // two distinct ids.
    assert_ne!(
        a_t1, a_t2,
        "two distinct emissions must mint two distinct eventIds, not a \
         constant or once-only value:\n{msg}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
