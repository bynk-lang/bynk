//! A `[deploy.workers.<key>]` group has one events fan-out for every member that
//! publishes (#3562, G3-1).
//!
//! The build is checked for its files: the members emit no fan-out of their own,
//! the group emits one `events_fanout.ts` and exports `__EventsFanout` once, and
//! the one `wrangler.toml` declares the class once. The run is under Node: two
//! members publish an event of the *same name*, each with its own subscriber, and
//! an event reaches only the subscriber of the member that published it, signed
//! as that member.
//!
//! The run needs `tsc` and `node`; each skips loudly without it, and
//! `BYNK_REQUIRE_TSC=1` (CI) turns the skip into a failure.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const MANIFEST: &str = "[build]\ntarget = \"workers\"\n\n[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.billing\"]\n";

fn publisher(context: &str, service: &str, route: &str) -> String {
    format!(
        "context {context}\n\nexports transparent {{ Placed }}\nconsumes bynk {{ Events }}\n\nevent Placed = {{\n\tn: Int,\n}}\n\nservice {service} from http {{\n\ton GET(\"/{route}/place/:n\") (n: String) -> Effect[HttpResult[String]] by v: Visitor given Events {{\n\t\tmatch Int.parse(n) {{\n\t\t\tSome(i) => {{\n\t\t\t\tlet _ <- Events.emit[Placed](Placed {{ n: i }})\n\t\t\t\tOk(\"placed\")\n\t\t\t}},\n\t\t\tNone => BadRequest(\"not an int\"),\n\t\t}}\n\t}}\n}}\n"
    )
}

fn subscriber(context: &str, publisher: &str) -> String {
    format!(
        "context {context}\n\nconsumes {publisher}\n\nservice OnPlaced from Events(Placed) {{\n\ton event(e: Placed) -> Effect[()] {{\n\t\tEffect.pure(())\n\t}}\n}}\n"
    )
}

fn stage(tag: &str, manifest: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("bynk-group-events-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/shop")).unwrap();
    fs::write(root.join("bynk.toml"), manifest).unwrap();
    for (file, source) in [
        ("orders", publisher("shop.orders", "orders_api", "orders")),
        (
            "billing",
            publisher("shop.billing", "billing_api", "billing"),
        ),
        ("audit", subscriber("shop.audit", "shop.orders")),
        ("ledger", subscriber("shop.ledger", "shop.billing")),
    ] {
        fs::write(root.join(format!("src/shop/{file}.bynk")), source).unwrap();
    }
    root
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn build(root: &Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["build", "."])
        .current_dir(root)
        .env("NO_COLOR", "1")
        .output()
        .expect("bynk runs");
    assert!(out.status.success(), "bynk build failed:\n{}", text(&out));
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join("out").join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn exists(root: &Path, rel: &str) -> bool {
    root.join("out").join(rel).is_file()
}

/// The members publish through one fan-out, which the group owns.
#[test]
fn a_group_with_two_publishers_has_one_fanout_the_group_owns() {
    let root = stage("files", MANIFEST);
    build(&root);

    // The Worker's: one fan-out, in the group's directory, and none per member.
    assert!(exists(&root, "workers/core/events_fanout.ts"));
    for member in ["shop-orders", "shop-billing"] {
        assert!(
            !exists(&root, &format!("workers/core/{member}.events_fanout.ts")),
            "{member} has no fan-out of its own"
        );
        let entry = read(&root, &format!("workers/core/{member}.index.ts"));
        assert!(!entry.contains("__EventsFanout"), "{entry}");
    }

    // Routed by publisher and type, so an event of one name in two members is two
    // routes, and signed as the publisher the event names.
    let fanout = read(&root, "workers/core/events_fanout.ts");
    assert!(
        fanout.contains(
            "\"shop.billing/Placed\": [{ binding: \"SHOP_LEDGER\", context: \"shop.ledger\", service: \"OnPlaced\" }]"
        ),
        "{fanout}"
    );
    assert!(
        fanout.contains(
            "\"shop.orders/Placed\": [{ binding: \"SHOP_AUDIT\", context: \"shop.audit\", service: \"OnPlaced\" }]"
        ),
        "{fanout}"
    );
    assert!(
        fanout.contains("ev.publisher + \"/\" + ev.type"),
        "{fanout}"
    );
    assert!(
        fanout.contains("this.env, ev.publisher, sub.context"),
        "{fanout}"
    );

    // Each member names itself on the events it hands over.
    let compose = read(&root, "workers/core/shop-billing.compose.ts");
    assert!(
        compose.contains("({ ...e, publisher: \"shop.billing\" } as"),
        "{compose}"
    );

    // The group's entry exports the class once; the one `wrangler.toml` declares
    // it and its binding once.
    let index = read(&root, "workers/core/index.ts");
    assert_eq!(
        index
            .matches("export { __EventsFanout } from \"./events_fanout.js\";")
            .count(),
        1,
        "{index}"
    );
    let toml = read(&root, "workers/core/wrangler.toml");
    assert_eq!(
        toml.matches("class_name = \"__EventsFanout\"").count(),
        1,
        "{toml}"
    );
    assert_eq!(
        toml.matches("[exports.__EventsFanout]").count(),
        1,
        "{toml}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// Without a group a publisher keeps its own fan-out and sends its events as
/// they are.
#[test]
fn without_a_group_each_publisher_keeps_its_own_fanout() {
    let root = stage("ungrouped", "[build]\ntarget = \"workers\"\n");
    build(&root);
    for member in ["shop-orders", "shop-billing"] {
        let fanout = read(&root, &format!("workers/{member}/events_fanout.ts"));
        assert!(!fanout.contains("ev.publisher"), "{fanout}");
        assert!(!fanout.contains("  publisher: string;"), "{fanout}");
        let compose = read(&root, &format!("workers/{member}/compose.ts"));
        assert!(!compose.contains("publisher:"), "{compose}");
    }
    let _ = fs::remove_dir_all(&root);
}

const SCRIPT: &str = r#"import worker, { __EventsFanout } from "./out-js/workers/core/index.js";
const delivered = [];
const KEY = "edge-key";
const binding = (name) => ({
  fetch: async (request) => {
    delivered.push({ to: name, path: new URL(request.url).pathname, from: request.headers.get("x-bynk-caller") });
    return new Response(null, { status: 204 });
  },
});
const env = {
  SHOP_AUDIT: binding("audit"),
  SHOP_LEDGER: binding("ledger"),
  BYNK_CALL_TO_SHOP__AUDIT: KEY,
  BYNK_CALL_TO_SHOP__LEDGER: KEY,
};
for (const c of ["SHOP__ORDERS", "SHOP__BILLING", "SHOP__AUDIT", "SHOP__LEDGER"]) {
  env["BYNK_CALL_TO_" + c] = KEY;
  env["BYNK_CALL_FROM_" + c] = KEY;
}
const fanout = new __EventsFanout({}, env);
env.___EVENTS_FANOUT = {
  idFromName: (n) => n,
  get: () => ({ fetch: (url, init) => fanout.fetch(new Request("http://fanout/_bynk/fanout", init)) }),
};
const pending = [];
const ctx = { waitUntil(p) { pending.push(p); } };
const results = [];
for (const path of ["/orders/place/5", "/billing/place/7", "/orders/place/9"]) {
  delivered.length = 0;
  const r = await worker.fetch(new Request("http://x" + path), env, ctx);
  await Promise.allSettled(pending.splice(0));
  results.push({ path, status: r.status, body: await r.text(), delivered: [...delivered] });
}
// The fan-out signs as the publisher an event names, so it must refuse one it
// was not built for: logged, not delivered, never signed.
const logs = [];
console.error = (...args) => logs.push(args.map((a) => typeof a === "string" ? a : JSON.stringify(a)).join(" "));
const envelope = { eventId: "e", publisherId: "p", emittedAt: 0, schemaVersion: 1 };
for (const [label, publisher] of [["unknown", "shop.audit"], ["missing", undefined], ["proto", "__proto__"], ["other-type", "shop.orders"]]) {
  delivered.length = 0;
  logs.length = 0;
  const type = label === "other-type" ? "Paid" : "Placed";
  const events = [{ ...(publisher === undefined ? {} : { publisher }), type, payload: { n: 1 }, envelope }];
  const r = await fanout.fetch(new Request("http://fanout/_bynk/fanout", { method: "POST", body: JSON.stringify({ events }) }));
  results.push({ path: "refuse-" + label, status: r.status, delivered: [...delivered], logs: [...logs] });
}
console.log(JSON.stringify(results));
"#;

fn run_group(root: &Path) -> Option<Vec<serde_json::Value>> {
    let Some(mut tsc) = tools::tsc_command() else {
        tools::skip(
            "WORKER GROUP EVENTS (tsc)",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "no TypeScript toolchain (`tsc` or `npx`) is on PATH",
        );
        return None;
    };
    if !tools::tool_exists("node") {
        tools::skip(
            "WORKER GROUP EVENTS (node)",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "`node` is not on PATH",
        );
        return None;
    }
    build(root);
    let emitted = tsc
        .arg("-p")
        .arg(root.join("out/tsconfig.json"))
        .current_dir(root)
        .output()
        .expect("run tsc");
    assert!(emitted.status.success(), "tsc failed:\n{}", text(&emitted));
    fs::write(root.join("out-js/package.json"), "{\"type\":\"module\"}").unwrap();
    fs::write(root.join("run.mjs"), SCRIPT).unwrap();
    let ran = Command::new("node")
        .arg("run.mjs")
        .current_dir(root)
        .output()
        .expect("run node");
    assert!(ran.status.success(), "node failed:\n{}", text(&ran));
    let line = String::from_utf8_lossy(&ran.stdout).trim().to_string();
    Some(serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}")))
}

/// Two members publish an event of one name; each reaches only its own
/// subscriber, signed as the member that published it.
#[test]
fn an_event_reaches_the_subscriber_of_the_member_that_published_it() {
    let root = stage("run", MANIFEST);
    let Some(results) = run_group(&root) else {
        let _ = fs::remove_dir_all(&root);
        return;
    };
    let delivery = |to: &str, from: &str| serde_json::json!([{ "to": to, "path": "/_bynk/event/OnPlaced", "from": from }]);
    for (path, to, from) in [
        ("/orders/place/5", "audit", "shop.orders"),
        ("/billing/place/7", "ledger", "shop.billing"),
        ("/orders/place/9", "audit", "shop.orders"),
    ] {
        let result = results.iter().find(|r| r["path"] == path).unwrap();
        assert_eq!(result["status"], 200, "{result}");
        assert_eq!(result["delivered"], delivery(to, from), "{path}: {result}");
    }
    let _ = fs::remove_dir_all(&root);
}

/// An event naming a publisher the group's fan-out was not built for is logged and
/// dropped, never signed; a known publisher's event of a type it does not publish
/// has no route and is dropped quietly.
#[test]
fn the_fanout_signs_only_for_its_own_publishers() {
    let root = stage("refuse", MANIFEST);
    let Some(results) = run_group(&root) else {
        let _ = fs::remove_dir_all(&root);
        return;
    };
    for label in ["unknown", "missing", "proto"] {
        let path = format!("refuse-{label}");
        let result = results.iter().find(|r| r["path"] == path.as_str()).unwrap();
        assert_eq!(result["status"], 204, "{result}");
        assert_eq!(
            result["delivered"],
            serde_json::json!([]),
            "{path}: {result}"
        );
        let logs = result["logs"].to_string();
        assert!(
            logs.contains("EventsFanout refused an event from an unknown publisher"),
            "{path}: {result}"
        );
    }
    let result = results
        .iter()
        .find(|r| r["path"] == "refuse-other-type")
        .unwrap();
    assert_eq!(result["delivered"], serde_json::json!([]), "{result}");
    assert_eq!(result["logs"], serde_json::json!([]), "{result}");
    let _ = fs::remove_dir_all(&root);
}
