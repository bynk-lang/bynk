//! Events track, slice 0 (spine #936, ADR 0284) — Workers-target wiring.
//!
//! `events_behaviour.rs` proves the emit/subscribe loop actually *runs* on
//! the Bundle/node target. Real delivery through a Cloudflare Durable Object
//! cannot be observed without `workerd`/`wrangler dev`, which this suite does
//! not have — so this file proves the two things that are checkable without
//! them: the emitted TypeScript type-checks under `tsc --strict` (nothing
//! here reads `wrangler.toml`, so a broken binding name would otherwise only
//! surface at deploy time), and the generated `wrangler.toml`/`events_fanout.
//! ts` actually declare the fan-out DO, its migration, and the
//! reverse-direction Service Binding a publisher needs to reach a subscriber
//! it does not itself `consumes` (the direction an ordinary `consumes` edge
//! never wires).

use bynkc::BuildTarget;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const SOURCE_ORDER: &str = r#"context commerce.order

exports transparent { PaymentConfirmed }
consumes bynk { Events }

event PaymentConfirmed = {
  orderId: String,
}

service markPaid {
  on call(orderId: String) -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId })
  }
}

agent Ledger {
  key id: String
  store total: Cell[Int] = 0

  invariant total_stays_small:
    total < 10

  on call bump(amount: Int) -> Effect[()] given Events {
    let _ <- total.update((n) => n + amount)
    do Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: "ledger-event" })
  }
}

service bumpLedger {
  on call(id: String, amount: Int) -> Effect[()] {
    let ledger = Ledger(id)
    let _ <- ledger.bump(amount)
  }
}
"#;

const SOURCE_NOTIFICATIONS: &str = r#"context commerce.notifications

consumes commerce.order
consumes bynk { Logger }

service OnPayment from Events(PaymentConfirmed) {
  on event(e: PaymentConfirmed) -> Effect[()] given Logger {
    Logger.info(e.orderId)
  }
}
"#;

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
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    };
    for p in prefix {
        cmd.arg(p);
    }
    for a in args {
        cmd.arg(a);
    }
    cmd.current_dir(cwd);
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => return (false, format!("failed to spawn {program}: {e}")),
    };
    let mut msg = String::from_utf8_lossy(&output.stdout).into_owned();
    msg.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), msg)
}

#[test]
fn workers_events_fanout_do_and_wrangler_wiring() {
    let tmp =
        std::env::temp_dir().join(format!("bynk-events-workers-wiring-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/commerce");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("order.bynk"), SOURCE_ORDER).unwrap();
    fs::write(proj.join("notifications.bynk"), SOURCE_NOTIFICATIONS).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the events project to workers:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };

    let find = |suffix: &str| -> String {
        out.artefacts
            .docs
            .iter()
            .find(|(p, _)| p.to_string_lossy().ends_with(suffix))
            .unwrap_or_else(|| panic!("no compiled file ends with {suffix:?}"))
            .1
            .text()
    };

    // The publisher's wrangler.toml: the fan-out DO binding + export, and
    // a Service Binding reaching the subscriber it does not itself `consumes`
    // (the reverse of the ordinary `consumes` edge — nothing else wires it).
    let order_wrangler = find("workers/commerce-order/wrangler.toml");
    assert!(
        order_wrangler.contains("name = \"___EVENTS_FANOUT\"")
            && order_wrangler.contains("class_name = \"__EventsFanout\""),
        "commerce-order's wrangler.toml must declare the fan-out DO binding:\n{order_wrangler}"
    );
    // #1796: declared in `exports` beside the context's real agents, each
    // SQLite-backed, with no `[[migrations]]` at all — a tagged migration
    // would never register the fan-out class on a Worker that first deployed
    // before it emitted.
    for class in ["Ledger", "__EventsFanout"] {
        assert!(
            order_wrangler.contains(&format!(
                "[exports.{class}]\ntype = \"durable-object\"\nstorage = \"sqlite\"\n"
            )),
            "{class} must be exported as a SQLite-backed Durable Object:\n{order_wrangler}"
        );
    }
    assert!(
        !order_wrangler.contains("[[migrations]]"),
        "`exports` and `migrations` are mutually exclusive:\n{order_wrangler}"
    );
    assert!(
        order_wrangler.contains("binding = \"COMMERCE_NOTIFICATIONS\"")
            && order_wrangler.contains("service = \"commerce-notifications\""),
        "commerce-order must gain a reverse-direction Service Binding to its \
         subscriber (it does not itself `consumes` commerce.notifications):\n{order_wrangler}"
    );

    // The fan-out DO's own routing table: compile-time-known, not carried on
    // the wire (ADR 0284's DECISION — see `emit_events_fanout_do`).
    let fanout_ts = find("workers/commerce-order/events_fanout.ts");
    assert!(
        fanout_ts.contains("\"PaymentConfirmed\": [{ binding: \"COMMERCE_NOTIFICATIONS\", context: \"commerce.notifications\", service: \"OnPayment\" }]"),
        "the fan-out DO's routing table must name the real subscriber:\n{fanout_ts}"
    );
    assert!(
        fanout_ts.contains("export class __EventsFanout"),
        "the fan-out DO class must be exported (wrangler resolves `class_name` \
         against the Worker's `main` exports):\n{fanout_ts}"
    );

    let order_index = find("workers/commerce-order/index.ts");
    assert!(
        order_index.contains("export { __EventsFanout } from \"./events_fanout.js\";"),
        "index.ts must re-export the fan-out DO class so wrangler's \
         `class_name` resolves against it:\n{order_index}"
    );

    // The subscriber's own wiring: the `/_bynk/event/` route the fan-out DO's
    // Service Binding call lands on, and no reciprocal fan-out DO of its own
    // (commerce.notifications never emits).
    let notif_index = find("workers/commerce-notifications/index.ts");
    assert!(
        notif_index.contains("/_bynk/event/") && notif_index.contains("case \"OnPayment\":"),
        "commerce-notifications's entry must route to its `on event` handler:\n{notif_index}"
    );

    // #973: the payload and envelope are validated at this route before
    // dispatch — a malformed one must never reach the handler unchecked.
    assert!(
        notif_index.contains("__deserialiseEventEnvelope(envelope, \"$.envelope\")")
            && notif_index
                .contains("handlers.__deserialise_PaymentConfirmed(payload, \"$.payload\")")
            && notif_index.contains("status: 400"),
        "the event route must validate both the envelope and the payload, \
         rejecting a malformed one with 400 before ever dispatching:\n{notif_index}"
    );

    // #973's root cause: a pure subscriber calls no method on the publisher,
    // so `called_consumed_services`'s narrowing (ADR 0200 Decision E) never
    // saw its event payload type — the subscriber's own module had no codec
    // for it at all. This is the assertion that would have caught that gap.
    let notif_handlers = find("workers/commerce-notifications/handlers.ts");
    assert!(
        notif_handlers.contains("export function __deserialise_PaymentConfirmed"),
        "a subscriber must get its own codec for the event payload type it \
         receives, even though it calls no method on the publisher:\n{notif_handlers}"
    );

    let notif_wrangler = find("workers/commerce-notifications/wrangler.toml");
    assert!(
        !notif_wrangler.contains("EventsFanout"),
        "a context with no emitting handler must get no fan-out DO of its own:\n{notif_wrangler}"
    );

    // Real `tsc --strict` over the whole emitted tree — the actual emitter
    // gate (goldens only prove self-consistency); skips loudly without a
    // toolchain, `BYNK_REQUIRE_TSC=1` turns the skip into a failure (CI).
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! EVENTS WORKERS WIRING TSC CHECK SKIPPED !!!\nno tsc runner.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            let _ = fs::remove_dir_all(&tmp);
            return;
        }
    };

    let run_dir = tmp.join("run");
    for (path, doc) in &out.artefacts.docs {
        let target = run_dir.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, doc.text()).unwrap();
    }
    bynk_testkit::write_runtime_modules(&run_dir).unwrap();

    let (program, prefix) = &runner;
    let (ok, msg) = run(
        program,
        prefix,
        &["--strict", "--noEmit", "-p", "tsconfig.json"],
        &run_dir,
    );
    assert!(
        ok,
        "tsc --strict failed on the workers-target output:\n{msg}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

// Events track, slice 1 (spine #936): the same fixture as above, but the
// subscriber's header carries a structural pattern. Deliver-and-filter (ADR
// 0286, unchanged by slice 1) means the fan-out DO's routing table and the
// wrangler.toml wiring are unaffected by a pattern — the guard lives inside
// the subscriber's own handler method, not in routing — so this asserts that
// negative directly rather than assuming it, alongside confirming the guard
// itself made it into the emitted TypeScript and still passes `tsc --strict`.
const SOURCE_NOTIFICATIONS_PATTERNED: &str = r#"context commerce.notifications

consumes commerce.order
consumes bynk { Logger }

service OnPayment from Events(PaymentConfirmed { orderId: "ledger-event", .. }) {
  on event(e: PaymentConfirmed) -> Effect[()] given Logger {
    Logger.info(e.orderId)
  }
}
"#;

#[test]
fn workers_events_pattern_leaves_fanout_routing_unchanged() {
    let tmp = std::env::temp_dir().join(format!(
        "bynk-events-workers-wiring-pattern-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/commerce");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("order.bynk"), SOURCE_ORDER).unwrap();
    fs::write(
        proj.join("notifications.bynk"),
        SOURCE_NOTIFICATIONS_PATTERNED,
    )
    .unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the patterned events project to workers:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };

    let find = |suffix: &str| -> String {
        out.artefacts
            .docs
            .iter()
            .find(|(p, _)| p.to_string_lossy().ends_with(suffix))
            .unwrap_or_else(|| panic!("no compiled file ends with {suffix:?}"))
            .1
            .text()
    };

    // Byte-identical to the pattern-less fixture's own assertions: the
    // routing table and wrangler.toml wiring do not know a pattern exists.
    let order_wrangler = find("workers/commerce-order/wrangler.toml");
    assert!(
        order_wrangler.contains("name = \"___EVENTS_FANOUT\"")
            && order_wrangler.contains("class_name = \"__EventsFanout\"")
            && order_wrangler.contains("binding = \"COMMERCE_NOTIFICATIONS\""),
        "a subscription pattern must not change the fan-out DO/Service Binding wiring:\n{order_wrangler}"
    );
    let fanout_ts = find("workers/commerce-order/events_fanout.ts");
    assert!(
        fanout_ts.contains("\"PaymentConfirmed\": [{ binding: \"COMMERCE_NOTIFICATIONS\", context: \"commerce.notifications\", service: \"OnPayment\" }]"),
        "a subscription pattern must not change the fan-out DO's routing table \
         (deliver-and-filter, ADR 0286 — the guard lives in the subscriber's own \
         handler, not in routing):\n{fanout_ts}"
    );

    // The guard itself must have made it into the subscriber's emitted
    // handler on the Workers target too (not just Bundle, per
    // `events_pattern_behaviour.rs`) — the generated handler method body
    // lands in `handlers.ts`, not the routing `index.ts`.
    let notif_handlers = find("workers/commerce-notifications/handlers.ts");
    assert!(
        notif_handlers.contains(r#"e.orderId === "ledger-event""#),
        "the pattern's guard must appear in the emitted handler:\n{notif_handlers}"
    );

    // Real `tsc --strict` over the whole emitted tree — the guard string is
    // hand-built (`event_pattern_guard`), so this is what actually catches a
    // syntactically-broken guard.
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! EVENTS WORKERS WIRING (PATTERNED) TSC CHECK SKIPPED !!!\nno tsc runner.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            let _ = fs::remove_dir_all(&tmp);
            return;
        }
    };

    let run_dir = tmp.join("run");
    for (path, doc) in &out.artefacts.docs {
        let target = run_dir.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, doc.text()).unwrap();
    }
    bynk_testkit::write_runtime_modules(&run_dir).unwrap();

    let (program, prefix) = &runner;
    let (ok, msg) = run(
        program,
        prefix,
        &["--strict", "--noEmit", "-p", "tsconfig.json"],
        &run_dir,
    );
    assert!(
        ok,
        "tsc --strict failed on the patterned workers-target output:\n{msg}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

// Events track, slice 2 (spine #936): the same fixture again, but the
// subscriber declares the optional `env: EventEnvelope` parameter.
// `EventEnvelope` is a plain first-party record (`bynk-std/lib/core/
// bynk.bynk`, `exports transparent`), imported directly from the `bynk`
// adapter's own module like any other first-party type — declaring it as a
// handler parameter needs no generated codec of its own (confirmed: it is
// not owned by a *consumed context*, so `collect_codec_closure`'s
// closure-of-a-consumed-context's-exports machinery never runs for it).
//
// #973: the envelope actually reaching this handler is now validated at the
// route, via the hand-written `deserialiseEventEnvelope` runtime helper
// (`runtime/src/boundary.ts`) — not a generated codec, since `EventEnvelope`
// is adapter-owned. Before this fix, the wire hop rode a bare `unknown`/`as
// any` cast the whole way with no runtime check at all.
const SOURCE_NOTIFICATIONS_ENVELOPE: &str = r#"context commerce.notifications

consumes commerce.order
consumes bynk { Logger }

service OnPayment from Events(PaymentConfirmed) {
  on event(e: PaymentConfirmed, env: EventEnvelope) -> Effect[()] given Logger {
    Logger.info("\(e.orderId):\(env.eventId):\(env.publisherId)")
  }
}
"#;

#[test]
fn workers_events_envelope_param_type_checks_and_leaves_fanout_unchanged() {
    let tmp = std::env::temp_dir().join(format!(
        "bynk-events-workers-wiring-envelope-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/commerce");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("order.bynk"), SOURCE_ORDER).unwrap();
    fs::write(
        proj.join("notifications.bynk"),
        SOURCE_NOTIFICATIONS_ENVELOPE,
    )
    .unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the envelope-declaring events project to workers:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };

    let find = |suffix: &str| -> String {
        out.artefacts
            .docs
            .iter()
            .find(|(p, _)| p.to_string_lossy().ends_with(suffix))
            .unwrap_or_else(|| panic!("no compiled file ends with {suffix:?}"))
            .1
            .text()
    };

    // Byte-identical to the pattern-less/patterned fixtures' own assertions:
    // declaring `env` changes nothing about routing or wiring.
    let order_wrangler = find("workers/commerce-order/wrangler.toml");
    assert!(
        order_wrangler.contains("name = \"___EVENTS_FANOUT\"")
            && order_wrangler.contains("class_name = \"__EventsFanout\"")
            && order_wrangler.contains("binding = \"COMMERCE_NOTIFICATIONS\""),
        "declaring `env: EventEnvelope` must not change the fan-out DO/Service \
         Binding wiring:\n{order_wrangler}"
    );
    let fanout_ts = find("workers/commerce-order/events_fanout.ts");
    assert!(
        fanout_ts.contains("\"PaymentConfirmed\": [{ binding: \"COMMERCE_NOTIFICATIONS\", context: \"commerce.notifications\", service: \"OnPayment\" }]"),
        "declaring `env: EventEnvelope` must not change the fan-out DO's \
         routing table:\n{fanout_ts}"
    );

    // The subscriber's own real handler method (`handlers.ts`) declares
    // `env: EventEnvelope` directly — no cast needed there, since it's the
    // typed declaration, not a wrapper.
    let notif_handlers = find("workers/commerce-notifications/handlers.ts");
    assert!(
        notif_handlers.contains("async event(e: PaymentConfirmed, env: EventEnvelope,"),
        "the subscriber's generated handler method must declare the typed \
         `env: EventEnvelope` parameter:\n{notif_handlers}"
    );

    // The entry route (`index.ts`) destructures `{ payload, envelope }`
    // uniformly, validates both (#973), and forwards the *validated* values
    // positionally, regardless of which subscriber it dispatches to.
    let notif_index = find("workers/commerce-notifications/index.ts");
    assert!(
        notif_index.contains("const { payload, envelope }")
            && notif_index.contains("__deserialiseEventEnvelope(envelope, \"$.envelope\")")
            && notif_index
                .contains("handlers.__deserialise_PaymentConfirmed(payload, \"$.payload\")")
            && notif_index.contains("surface.OnPayment(__r_payload.value, __r_envelope.value)"),
        "the entry route must validate both payload and envelope and forward \
         the validated values uniformly:\n{notif_index}"
    );

    // The compose-surface wrapper (`emit_event_wrapper`, in `compose.ts`) is
    // the one place that forwards `envelope` into the real handler with a
    // cast, since the wire value only arrives as `unknown`.
    let notif_compose = find("workers/commerce-notifications/compose.ts");
    assert!(
        notif_compose.contains("envelope as any"),
        "a subscriber that declared `env: EventEnvelope` must have it \
         forwarded into its generated handler method:\n{notif_compose}"
    );

    // Real `tsc --strict` — the actual proof that `EventEnvelope` reaching a
    // handler signature (and so `collect_boundary_types`' generated codec
    // for it) type-checks, not just that the strings above are present.
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! EVENTS WORKERS WIRING (ENVELOPE) TSC CHECK SKIPPED !!!\nno tsc runner.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            let _ = fs::remove_dir_all(&tmp);
            return;
        }
    };

    let run_dir = tmp.join("run");
    for (path, doc) in &out.artefacts.docs {
        let target = run_dir.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, doc.text()).unwrap();
    }
    bynk_testkit::write_runtime_modules(&run_dir).unwrap();

    let (program, prefix) = &runner;
    let (ok, msg) = run(
        program,
        prefix,
        &["--strict", "--noEmit", "-p", "tsconfig.json"],
        &run_dir,
    );
    assert!(
        ok,
        "tsc --strict failed on the envelope-declaring workers-target output:\n{msg}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

// #973: a nested-record payload field, to exercise `collect_codec_closure`'s
// transitive walk through the *new* event-roots path (the fix's Change A) —
// every other fixture in this file has a flat, single-field payload, which
// would pass even if the closure only ever collected the root type and never
// followed a field into another record.
const SOURCE_ORDER_NESTED: &str = r#"context commerce.order

exports transparent { PaymentConfirmed, Meta }
consumes bynk { Events }

type Meta = {
  region: String,
}

event PaymentConfirmed = {
  orderId: String,
  meta: Meta,
}

service markPaid {
  on call(orderId: String, region: String) -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId, meta: Meta { region: region } })
  }
}
"#;

#[test]
fn workers_events_nested_payload_field_gets_a_transitive_codec() {
    let tmp = std::env::temp_dir().join(format!(
        "bynk-events-workers-wiring-nested-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/commerce");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("order.bynk"), SOURCE_ORDER_NESTED).unwrap();
    fs::write(proj.join("notifications.bynk"), SOURCE_NOTIFICATIONS).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the nested-payload events project to workers:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };

    let find = |suffix: &str| -> String {
        out.artefacts
            .docs
            .iter()
            .find(|(p, _)| p.to_string_lossy().ends_with(suffix))
            .unwrap_or_else(|| panic!("no compiled file ends with {suffix:?}"))
            .1
            .text()
    };

    // The subscriber's own module must gain codecs for BOTH the root event
    // payload type and the nested record it transitively reaches — a closure
    // that only collected the root would leave `deserialise_Meta` missing,
    // and the compile-time `handlers.__deserialise_PaymentConfirmed` call the
    // route emits would then reference a function whose own body calls an
    // undefined `deserialise_Meta`, caught by the `tsc --strict` gate below.
    let notif_handlers = find("workers/commerce-notifications/handlers.ts");
    assert!(
        notif_handlers.contains("export function __deserialise_PaymentConfirmed")
            && notif_handlers.contains("export function __deserialise_Meta"),
        "a subscriber must get codecs for the event payload's full transitive \
         closure, not just its root type:\n{notif_handlers}"
    );

    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! EVENTS WORKERS WIRING (NESTED PAYLOAD) TSC CHECK SKIPPED !!!\nno tsc runner.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            let _ = fs::remove_dir_all(&tmp);
            return;
        }
    };

    let run_dir = tmp.join("run");
    for (path, doc) in &out.artefacts.docs {
        let target = run_dir.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, doc.text()).unwrap();
    }
    bynk_testkit::write_runtime_modules(&run_dir).unwrap();

    let (program, prefix) = &runner;
    let (ok, msg) = run(
        program,
        prefix,
        &["--strict", "--noEmit", "-p", "tsconfig.json"],
        &run_dir,
    );
    assert!(
        ok,
        "tsc --strict failed on the nested-payload workers-target output:\n{msg}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
