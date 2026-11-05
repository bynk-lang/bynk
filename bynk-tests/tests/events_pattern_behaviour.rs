//! Events track, slice 1 (spine #936) — behavioural (runtime) proof that a
//! `from Events(E { field: value, .. })` subscription pattern actually
//! filters delivery, on the Bundle/node target.
//!
//! Static fixtures prove the pattern *type-checks* and the emitted guard is
//! syntactically valid TypeScript; neither proves the guard actually decides
//! anything at runtime. This file composes a real two-context project with
//! two sibling subscribers of the same event — one whose pattern matches an
//! emission (must observably run) and one whose pattern does not (must
//! observably NOT run) — and asserts both in the same run. Asserting only
//! the negative would pass trivially if delivery broke entirely; the
//! positive is the control that proves the negative means something.
//!
//! Two emissions, opposite region, so neither subscriber is proved by
//! accident of which one happens to fire first in delivery order — each
//! subscriber must match exactly one of the two emissions, never both,
//! never neither.
//!
//! Deliver-and-filter (ADR 0286, unchanged by slice 1): the fan-out
//! mechanism still delivers every emission of `Tick` to every subscriber;
//! the guard this test proves lives inside each subscriber's own generated
//! handler, not in routing. No narrowing: `e.region` stays typed `Region` in
//! both handler bodies (Events slice 1's own scope decision).
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

// The publisher: one event with a discriminator field, and two trigger
// services (a plain service's `on call` takes no method name, so two
// distinct emissions need two services, not one with two methods).
const SOURCE_ORDER: &str = r#"context commerce.order

exports transparent { Tick, Region }
consumes bynk { Events }

type Region =
  | Domestic
  | International

event Tick = {
  region: Region,
  tag: String,
}

service triggerDomestic {
  on call(tag: String) -> Effect[()] given Events {
    Events.emit[Tick](Tick { region: Region.Domestic, tag: tag })
  }
}

service triggerInternational {
  on call(tag: String) -> Effect[()] given Events {
    Events.emit[Tick](Tick { region: Region.International, tag: tag })
  }
}
"#;

// Two sibling subscribers of the same event, opposite patterns. Neither
// handler body reads a narrowed type — `e.region` is plain `Region` in both
// (Events slice 1's scope decision: filtering only, no static narrowing).
const SOURCE_NOTIFICATIONS: &str = r#"context commerce.notifications

consumes commerce.order
consumes bynk { Logger }

service OnDomestic from Events(Tick { region: Region.Domestic, .. }) {
  on event(e: Tick) -> Effect[()] given Logger {
    Logger.info("domestic:\(e.tag)")
  }
}

service OnInternational from Events(Tick { region: Region.International, .. }) {
  on event(e: Tick) -> Effect[()] given Logger {
    Logger.info("international:\(e.tag)")
  }
}
"#;

const DRIVER_TS: &str = r#"import { composeApp } from "./compose.js";

const app: any = composeApp();

// Two emissions, opposite region, so each subscriber is proved by a real
// mismatch on the other's emission — not merely by never having anything to
// reject.
await app.order.triggerDomestic("d1");
await app.order.triggerInternational("i1");

console.log("ALL OK");
"#;

#[test]
fn bundle_events_subscription_pattern_filters_delivery() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! EVENTS PATTERN BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! EVENTS PATTERN BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!(
        "bynk-events-pattern-behaviour-{}",
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
            "compile the events-pattern project to a bundle:\n{}",
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
        "tsc failed on the events-pattern-behaviour bundle:\n{msg}"
    );

    let (ok, msg) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && msg.contains("ALL OK"),
        "events-pattern-behaviour driver did not pass:\n{msg}"
    );

    // The positive control: each subscriber's matching emission is delivered
    // exactly once.
    assert_eq!(
        msg.matches("domestic:d1").count(),
        1,
        "OnDomestic's pattern matches the Domestic emission and must run \
         exactly once:\n{msg}"
    );
    assert_eq!(
        msg.matches("international:i1").count(),
        1,
        "OnInternational's pattern matches the International emission and \
         must run exactly once:\n{msg}"
    );
    // The negative this test exists to prove: each subscriber's pattern
    // rejects the OTHER emission. Checked directly, not inferred from the
    // positive counts above — a broken guard that matches everything would
    // still pass the two assertions above if it only ever saw one emission
    // each, so the cross-checks are the actual assertion.
    assert_eq!(
        msg.matches("domestic:i1").count(),
        0,
        "OnDomestic's pattern must reject the International emission:\n{msg}"
    );
    assert_eq!(
        msg.matches("international:d1").count(),
        0,
        "OnInternational's pattern must reject the Domestic emission:\n{msg}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
