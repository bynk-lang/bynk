//! `[deploy.workers.<key>]` groups name only contexts the project declares
//! (#3562, slice G1). The check needs the project's units, so it runs at
//! `check` time, and the build and the editor read the table the same way:
//! this runs one project through both.

use bynk_driver::project::Roots;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::process::Command;

const MANIFEST: &str = "[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.ordres\", \"money\"]\n\n[deploy.workers.shop-payment]\ncontexts = [\"shop.orders\"]\n";
const MANIFEST_ONE_GROUP: &str =
    "[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.payment\"]\nname = \"acme-core\"\n";

const COMMONS: &str = "commons money {\n\tfn dbl(n: Int) -> Int { n + n }\n}\n";
const ORDERS: &str = "context shop.orders\n\nservice orders_probe {\n\ton call() -> Effect[Int] {\n\t\tEffect.pure(1)\n\t}\n}\n";
const PAYMENT: &str = "context shop.payment\n\nservice payment_probe {\n\ton call() -> Effect[Int] {\n\t\tEffect.pure(2)\n\t}\n}\n";

fn check(root: &std::path::Path) -> (bool, String) {
    run(root, &["check", "--format", "short"])
}

/// `bynk build`, which reads `[build] target`: the binding checks are a
/// Workers-topology check, and `check` builds the in-process bundle.
fn build(root: &std::path::Path) -> (bool, String) {
    run(root, &["build"])
}

fn run(root: &std::path::Path, args: &[&str]) -> (bool, String) {
    let checked = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(args)
        .arg(root)
        .output()
        .expect("run bynk");
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    (checked.status.success(), out)
}

fn write_project(root: &std::path::Path, manifest: &str) {
    let _ = fs::remove_dir_all(root);
    fs::create_dir_all(root.join("src/shop")).unwrap();
    fs::write(root.join("bynk.toml"), manifest).unwrap();
    fs::write(root.join("src/money.bynk"), COMMONS).unwrap();
    fs::write(root.join("src/shop/orders.bynk"), ORDERS).unwrap();
    fs::write(root.join("src/shop/payment.bynk"), PAYMENT).unwrap();
}

/// The group diagnostics' codes.
fn codes(found: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    found
        .into_iter()
        .filter(|code| code.starts_with("bynk.deploy."))
        .collect()
}

#[test]
fn a_context_in_two_groups_fails_check() {
    let root = std::env::temp_dir().join(format!("bynk-deploy-groups-two-{}", std::process::id()));
    write_project(&root, MANIFEST);
    let (ok, out) = check(&root);
    assert!(!ok, "{out}");
    assert!(out.contains("bynk.deploy.context_in_two_groups"), "{out}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_group_of_declared_contexts_checks_clean() {
    let root = std::env::temp_dir().join(format!("bynk-deploy-groups-ok-{}", std::process::id()));
    write_project(&root, MANIFEST_ONE_GROUP);
    let (ok, out) = check(&root);
    assert!(ok, "{out}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_build_and_the_editor_refuse_the_same_members() {
    let manifest =
        "[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.ordres\", \"money\"]\n";
    let root = std::env::temp_dir().join(format!("bynk-deploy-groups-bad-{}", std::process::id()));
    write_project(&root, manifest);

    let (ok, out) = check(&root);
    assert!(!ok, "{out}");
    let build = codes(out.lines().filter_map(|line| {
        let (_, rest) = line.split_once('[')?;
        let (code, _) = rest.split_once("]: ")?;
        Some(code.to_string())
    }));

    let paths = bynk_driver::project::try_read_project_paths(&root).expect("the manifest reads");
    let overlay = HashMap::from([
        (root.join("bynk.toml"), manifest.to_string()),
        (root.join("src/money.bynk"), COMMONS.to_string()),
        (root.join("src/shop/orders.bynk"), ORDERS.to_string()),
        (root.join("src/shop/payment.bynk"), PAYMENT.to_string()),
    ]);
    let analysis = bynk_check::analysis::analyse_project(
        &Roots::Split {
            project_root: root.clone(),
            paths,
        },
        &overlay,
        &bynk_driver::project::SchemaLock::Off,
    );
    let editor = codes(analysis.errors.iter().map(|a| a.error.category.to_string()));
    let expected = BTreeSet::from([
        "bynk.deploy.not_a_context".to_string(),
        "bynk.deploy.unknown_context".to_string(),
    ]);
    assert_eq!(build, expected, "the build's report:\n{out}");
    assert_eq!(editor, expected, "the editor's analysis");
    let _ = fs::remove_dir_all(&root);
}

const UNSTABLE_R2: &str = "[unstable]\nallow = [\"bynk.cloudflare.R2\"]\n\n";

const WORKERS_TARGET: &str = "[unstable]\nallow = [\"bynk.cloudflare.R2\"]\n\n[build]\ntarget = \"workers\"\n\n[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.payment\"]\n";

fn archive_context(name: &str, options: &str) -> String {
    let service = format!("probe_{}", name.replace('.', "_"));
    format!(
        "context {name}\n\nconsumes bynk.cloudflare {{ R2 }}\n\ncapability Archives = R2{options}\n\nservice {service} {{\n\ton call() -> Effect[Int] {{\n\t\tEffect.pure(1)\n\t}}\n}}\n"
    )
}

#[test]
fn members_with_different_bindings_under_one_name_collide() {
    let root = std::env::temp_dir().join(format!("bynk-deploy-groups-bind-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    fs::write(
        root.join("src/shop/orders.bynk"),
        archive_context("shop.orders", ""),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        archive_context("shop.payment", " @jurisdiction(\"eu\")"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(out.contains("bynk.deploy.binding_collision"), "{out}");

    // The same declaration in both is one shared binding.
    fs::write(
        root.join("src/shop/payment.bynk"),
        archive_context("shop.payment", ""),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "identical declarations share one binding:\n{out}");

    // And two contexts that are not grouped each keep their own Worker.
    fs::write(
        root.join("bynk.toml"),
        format!("{UNSTABLE_R2}[build]\ntarget = \"workers\"\n"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        archive_context("shop.payment", " @jurisdiction(\"eu\")"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "separate Workers may differ:\n{out}");
    let _ = fs::remove_dir_all(&root);
}

fn agent_context(name: &str) -> String {
    let service = format!("probe_{}", name.replace('.', "_"));
    format!(
        "context {name}\n\nagent Cart {{\n\tkey id: String\n\tstore n: Cell[Int] = 0\n\n\ton call bump() -> Effect[Int] {{ Effect.pure(1) }}\n}}\n\nservice {service} {{\n\ton call() -> Effect[Int] {{\n\t\tEffect.pure(1)\n\t}}\n}}\n"
    )
}

#[test]
fn members_that_each_declare_an_agent_of_one_name_collide() {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-groups-agent-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    fs::write(
        root.join("src/shop/orders.bynk"),
        agent_context("shop.orders"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        agent_context("shop.payment"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(out.contains("bynk.deploy.binding_collision"), "{out}");
    assert!(out.contains("rename one of the agents"), "{out}");

    // Ungrouped, each context's Worker has its own `Cart` class.
    fs::write(
        root.join("bynk.toml"),
        format!("{UNSTABLE_R2}[build]\ntarget = \"workers\"\n"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "separate Workers may each declare `Cart`:\n{out}");
    let _ = fs::remove_dir_all(&root);
}

fn plain_context(name: &str) -> String {
    let service = format!("probe_{}", name.replace('.', "_"));
    format!(
        "context {name}\n\nservice {service} {{\n\ton call() -> Effect[Int] {{\n\t\tEffect.pure(1)\n\t}}\n}}\n"
    )
}

/// A group is one deployment unit, so a platform-native member locks the whole
/// Worker, and the diagnostic names the group, not a context.
#[test]
fn the_platform_lock_names_the_group() {
    let root = std::env::temp_dir().join(format!("bynk-deploy-groups-lock-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    fs::write(
        root.join("src/shop/orders.bynk"),
        archive_context("shop.orders", ""),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        plain_context("shop.payment"),
    )
    .unwrap();

    let (ok, out) = run(&root, &["build", "--platform", "node"]);
    assert!(!ok, "{out}");
    assert!(out.contains("bynk.target.vendor_required"), "{out}");
    assert!(
        out.contains("Worker group `core` uses the platform-native capabilities"),
        "{out}"
    );

    // Ungrouped, the same context is named as before.
    fs::write(
        root.join("bynk.toml"),
        format!("{UNSTABLE_R2}[build]\ntarget = \"workers\"\n"),
    )
    .unwrap();
    let (ok, out) = run(&root, &["build", "--platform", "node"]);
    assert!(!ok, "{out}");
    assert!(
        out.contains("context `shop.orders` uses the platform-native capabilities"),
        "{out}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// A Worker answers its members' services by name, so two members of one group
/// cannot each declare a service of one name.
#[test]
fn members_that_each_declare_a_service_of_one_name_collide() {
    let root = std::env::temp_dir().join(format!("bynk-deploy-groups-svc-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    let same = |name: &str| {
        format!(
            "context {name}\n\nservice probe {{\n\ton call() -> Effect[Int] {{\n\t\tEffect.pure(1)\n\t}}\n}}\n"
        )
    };
    fs::write(root.join("src/shop/orders.bynk"), same("shop.orders")).unwrap();
    fs::write(root.join("src/shop/payment.bynk"), same("shop.payment")).unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(out.contains("bynk.deploy.service_name_conflict"), "{out}");

    // Ungrouped, each context's Worker has its own `probe`.
    fs::write(
        root.join("bynk.toml"),
        format!("{UNSTABLE_R2}[build]\ntarget = \"workers\"\n"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "separate Workers may each declare `probe`:\n{out}");
    let _ = fs::remove_dir_all(&root);
}

fn publisher_context(name: &str, event: &str) -> String {
    let service = format!("probe_{}", name.replace('.', "_"));
    format!(
        "context {name}\n\nexports transparent {{ {event} }}\nconsumes bynk {{ Events }}\n\nevent {event} = {{\n\ttag: String,\n}}\n\nservice {service} {{\n\ton call() -> Effect[()] given Events {{\n\t\tlet _ <- Events.emit[{event}]({event} {{ tag: \"x\" }})\n\t\tEffect.pure(())\n\t}}\n}}\n"
    )
}

/// Temporary: each publisher has its own events fan-out class, so a group may
/// have one publishing member only.
#[test]
fn a_group_with_two_publishing_members_is_refused_for_now() {
    let root = std::env::temp_dir().join(format!("bynk-deploy-groups-pub-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    fs::write(
        root.join("src/shop/orders.bynk"),
        publisher_context("shop.orders", "Placed"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        publisher_context("shop.payment", "Paid"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(out.contains("bynk.deploy.multiple_publishers"), "{out}");
    assert!(out.contains("temporary"), "{out}");

    // One publisher, with a member that publishes nothing, is fine.
    fs::write(
        root.join("src/shop/payment.bynk"),
        plain_context("shop.payment"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "one publishing member is allowed:\n{out}");
    let _ = fs::remove_dir_all(&root);
}

/// One context of `services`, a `shop.*` source for the trigger-conflict tests.
fn context_with(name: &str, body: &str) -> String {
    format!("context {name}\n\n{body}\n")
}

/// Write the two members of group `core` with `a` and `b` as their bodies, build
/// grouped, and (when `code` is given) expect that error; then build ungrouped
/// and expect it clean, since separate Workers may each answer the same thing.
fn trigger_conflict(tag: &str, a: &str, b: &str, code: &str) {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-groups-{tag}-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    fs::write(
        root.join("src/shop/orders.bynk"),
        context_with("shop.orders", a),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        context_with("shop.payment", b),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(out.contains(code), "{out}");
    assert!(
        out.contains("contexts `shop.orders` and `shop.payment` share Worker `core`"),
        "{out}"
    );
    // The same project, ungrouped: each Worker answers its own.
    fs::write(
        root.join("bynk.toml"),
        format!("{UNSTABLE_R2}[build]\ntarget = \"workers\"\n"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "separate Workers may each answer it:\n{out}");
    let _ = fs::remove_dir_all(&root);
}

const ROUTE_A: &str = "service a from http by Visitor {\n\ton GET(\"/ping\") () -> Effect[HttpResult[Int]] {\n\t\tOk(1)\n\t}\n}";
const ROUTE_B: &str = "service b from http by Visitor {\n\ton GET(\"/ping\") () -> Effect[HttpResult[Int]] {\n\t\tOk(2)\n\t}\n}";

/// A Worker answers a route with one handler.
#[test]
fn members_that_declare_the_same_route_collide() {
    trigger_conflict("route", ROUTE_A, ROUTE_B, "bynk.deploy.route_conflict");
}

/// A Worker has one consumer for a queue.
#[test]
fn members_that_consume_the_same_queue_collide() {
    let svc = |n: &str| {
        format!(
            "service {n} from queue(\"jobs\") {{\n\ton message(j: Int) -> Effect[QueueResult] {{\n\t\tAck\n\t}}\n}}"
        )
    };
    trigger_conflict(
        "queue",
        &svc("a"),
        &svc("b"),
        "bynk.deploy.queue_consumer_conflict",
    );
}

/// An address is claimed once a project, so two members cannot claim one: the
/// project-wide check already refuses it, grouped or not, and a group adds no
/// check of its own.
#[test]
fn members_that_claim_the_same_address_are_refused() {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-groups-email-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    let svc = |n: &str, addr: &str| {
        format!(
            "consumes bynk.cloudflare {{ InboundEmail, EmailVerdict }}\n\nservice {n} from email(\"{addr}\") {{\n\ton message(mail: InboundEmail) -> Effect[EmailVerdict] by s: Sender {{\n\t\tAccept\n\t}}\n}}"
        )
    };
    fs::write(
        root.join("src/shop/orders.bynk"),
        context_with("shop.orders", &svc("a", "support@shop.example")),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        context_with("shop.payment", &svc("b", "Support@shop.example")),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(out.contains("bynk.email.duplicate_address"), "{out}");
    let _ = fs::remove_dir_all(&root);
}

/// A cron expression is not claimed: two members may share it, and the group's
/// Worker runs both handlers.
#[test]
fn members_may_share_a_cron_expression() {
    let root = std::env::temp_dir().join(format!("bynk-deploy-groups-cron-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    let svc = |n: &str| {
        format!(
            "service {n} from cron {{\n\ton schedule(\"*/5 * * * *\") (at: Int) -> Effect[Result[(), String]] {{\n\t\tEffect.pure(Ok(()))\n\t}}\n}}"
        )
    };
    fs::write(
        root.join("src/shop/orders.bynk"),
        context_with("shop.orders", &svc("a")),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        context_with("shop.payment", &svc("b")),
    )
    .unwrap();
    let (ok, out) = check(&root);
    assert!(ok, "two members may share a cron expression:\n{out}");
    let _ = fs::remove_dir_all(&root);
}
