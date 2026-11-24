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
const ORDERS: &str = "context shop.orders\n\nservice orders_probe {\n\ton call() -> Effect[Int] by Caller {\n\t\tEffect.pure(1)\n\t}\n}\n";
const PAYMENT: &str = "context shop.payment\n\nservice payment_probe {\n\ton call() -> Effect[Int] by Caller {\n\t\tEffect.pure(2)\n\t}\n}\n";

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
        "context {name}\n\nconsumes bynk.cloudflare {{ R2 }}\n\ncapability Archives = R2{options}\n\nservice {service} {{\n\ton call() -> Effect[Int] by Caller {{\n\t\tEffect.pure(1)\n\t}}\n}}\n"
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
        "context {name}\n\nagent Cart {{\n\tkey id: String\n\tstore n: Cell[Int] = 0\n\n\ton call bump() -> Effect[Int] {{ Effect.pure(1) }}\n}}\n\nservice {service} {{\n\ton call() -> Effect[Int] by Caller {{\n\t\tEffect.pure(1)\n\t}}\n}}\n"
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
        "context {name}\n\nservice {service} {{\n\ton call() -> Effect[Int] by Caller {{\n\t\tEffect.pure(1)\n\t}}\n}}\n"
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
            "context {name}\n\nservice probe {{\n\ton call() -> Effect[Int] by Caller {{\n\t\tEffect.pure(1)\n\t}}\n}}\n"
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
        "context {name}\n\nexports transparent {{ {event} }}\nconsumes bynk {{ Events }}\n\nevent {event} = {{\n\ttag: String,\n}}\n\nservice {service} {{\n\ton call() -> Effect[()] by Caller given Events {{\n\t\tlet _ <- Events.emit[{event}]({event} {{ tag: \"x\" }})\n\t\tEffect.pure(())\n\t}}\n}}\n"
    )
}

/// A group's members may all publish: they share one events fan-out (G3-1).
#[test]
fn a_group_may_have_several_publishing_members() {
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
    assert!(ok, "two publishers share the group's fan-out:\n{out}");
    let _ = fs::remove_dir_all(&root);
}

/// One context of `services`, a `shop.*` source for the trigger-conflict tests.
fn context_with(name: &str, body: &str) -> String {
    format!("context {name}\n\n{body}\n")
}

/// Write the two members of group `core` with `a` and `b` as their bodies, build
/// grouped and expect `code`. With `separate_ok`, then build ungrouped and expect
/// it clean, since separate Workers may each answer the same route. It is not
/// used for a queue: that is refused project-wide, grouped or not, so it is not a trigger
/// a group alone can clash on.
fn trigger_conflict(tag: &str, a: &str, b: &str, code: &str, separate_ok: bool) {
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
    if !separate_ok {
        let _ = fs::remove_dir_all(&root);
        return;
    }
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
    trigger_conflict(
        "route",
        ROUTE_A,
        ROUTE_B,
        "bynk.deploy.route_conflict",
        true,
    );
}

/// A queue has one consumer Worker, so two contexts consuming one are refused by
/// the project-wide check (`bynk.queue.duplicate_consumer`), grouped or not, and
/// a group adds no check of its own: a project gets exactly one diagnostic.
#[test]
fn contexts_that_consume_the_same_queue_are_refused_once() {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-groups-queue-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    let svc = |n: &str| {
        format!(
            "service {n} from queue(\"jobs\") {{\n\ton message(j: Int) -> Effect[QueueResult] {{\n\t\tAck\n\t}}\n}}"
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
    let check = |out: &str| {
        assert_eq!(
            out.matches("bynk.queue.duplicate_consumer").count(),
            1,
            "{out}"
        );
        assert!(
            out.contains("contexts `shop.orders` and `shop.payment` both consume queue `jobs`"),
            "{out}"
        );
        assert!(!out.contains("queue_consumer_conflict"), "{out}");
    };
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    check(&out);
    // The same project, ungrouped: separate Workers still cannot share a queue.
    fs::write(
        root.join("bynk.toml"),
        format!("{UNSTABLE_R2}[build]\ntarget = \"workers\"\n"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    check(&out);
    let _ = fs::remove_dir_all(&root);
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
            "service {n} from cron {{\n\ton schedule(\"*/5 * * * *\") (at: Instant) -> Effect[Result[(), String]] {{\n\t\tEffect.pure(Ok(()))\n\t}}\n}}"
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

/// A context with one `from websocket` service and the agent it hands the
/// connection to (named per context, since two members cannot share an agent's
/// name).
fn websocket_context(name: &str, agent: &str) -> String {
    format!(
        r#"context {name}

type RoomId = opaque String
type UserId = opaque String
type ServerFrame = {{ text: String }}
type ClientFrame = {{ text: String }}

actor Participant {{ auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }}

service Gateway from websocket(in: ClientFrame, out: ServerFrame) {{
  on open (roomId: RoomId) -> Effect[()] by user: Participant {{
    let _ <- {agent}(roomId).join(user.identity, connection)
    ()
  }}

  on message (roomId: RoomId, frame: ClientFrame) -> Effect[()] by user: Participant {{
    let _ <- {agent}(roomId).post(user.identity, frame.text)
    ()
  }}

  on close (roomId: RoomId) -> Effect[()] by user: Participant {{
    let _ <- {agent}(roomId).leave(user.identity)
    ()
  }}
}}

agent {agent} {{
  key id: RoomId
  store members: Set[UserId]
  store conns: Map[UserId, Connection[ServerFrame]]

  on call join(u: UserId, conn: Connection[ServerFrame]) -> Effect[()] {{
    let _ <- members.add(u)
    let _ <- conns.put(u, conn)
    ()
  }}

  on call leave(u: UserId) -> Effect[()] {{
    let _ <- members.remove(u)
    let _ <- conns.remove(u)
    ()
  }}

  on call post(u: UserId, text: String) -> Effect[()] {{
    ()
  }}
}}
"#
    )
}

/// An upgrade request names no path or service, so a group cannot route one: it
/// has at most one member that accepts them.
#[test]
fn a_group_with_two_websocket_members_is_refused() {
    let root = std::env::temp_dir().join(format!("bynk-deploy-groups-ws-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    fs::write(
        root.join("src/shop/orders.bynk"),
        websocket_context("shop.orders", "OrderRoom"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        websocket_context("shop.payment", "PayRoom"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(
        out.contains("bynk.deploy.multiple_websocket_members"),
        "{out}"
    );
    assert!(out.contains("`shop.orders` and `shop.payment`"), "{out}");

    // One member that accepts upgrades, with one that does not, is fine.
    fs::write(
        root.join("src/shop/payment.bynk"),
        plain_context("shop.payment"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "one websocket member is fine:\n{out}");
    let _ = fs::remove_dir_all(&root);
}

fn service_context(name: &str, service: &str) -> String {
    format!(
        "context {name}\n\nservice {service} {{\n\ton call() -> Effect[Int] by Caller {{\n\t\tEffect.pure(1)\n\t}}\n}}\n"
    )
}

/// Three members, two of which declare a service of one name: one error, at the
/// later of the two, naming both.
#[test]
fn a_service_name_shared_by_two_of_three_members_is_one_error_at_the_later() {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-groups-three-{}", std::process::id()));
    let manifest = WORKERS_TARGET.replace(
        "contexts = [\"shop.orders\", \"shop.payment\"]",
        "contexts = [\"shop.orders\", \"shop.payment\", \"shop.billing\"]",
    );
    write_project(&root, &manifest);
    fs::write(
        root.join("src/shop/orders.bynk"),
        service_context("shop.orders", "orders_probe"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        service_context("shop.payment", "shared"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/billing.bynk"),
        service_context("shop.billing", "shared"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert_eq!(
        out.matches("bynk.deploy.service_name_conflict").count(),
        1,
        "one error for the one clash:\n{out}"
    );
    assert!(out.contains("`shop.payment` and `shop.billing`"), "{out}");
    assert!(out.contains("billing.bynk"), "at the later member:\n{out}");
    let _ = fs::remove_dir_all(&root);
}

/// A `bundle` build has every context in one program, so it says it ignores the
/// groups; a `workers` build uses them and says nothing.
#[test]
fn a_bundle_build_says_it_ignores_the_groups() {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-groups-bundle-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    let (ok, out) = run(&root, &["build", "--target", "bundle"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("bynk.deploy.workers_ignored_on_bundle"),
        "{out}"
    );

    let (ok, out) = build(&root);
    assert!(ok, "{out}");
    assert!(!out.contains("workers_ignored_on_bundle"), "{out}");

    // `check` is not a build, and says nothing either way.
    let (ok, out) = check(&root);
    assert!(ok, "{out}");
    assert!(!out.contains("workers_ignored_on_bundle"), "{out}");
    let _ = fs::remove_dir_all(&root);
}

fn asset_context(name: &str, assets: &[(&str, &str, &str)]) -> String {
    let declarations: String = assets
        .iter()
        .map(|(asset, file, at)| format!("asset {asset} = \"{file}\" at \"{at}\"\n"))
        .collect();
    format!(
        "context {name}\n\n{declarations}\nservice probe_{} {{\n\ton call() -> Effect[Int] by Caller {{\n\t\tEffect.pure(1)\n\t}}\n}}\n",
        name.replace('.', "_")
    )
}

/// A group has one assets directory: distinct fixed paths all ship, the same file
/// at one path is one file, and two files at one path, or a path that would be a
/// folder of another, are refused.
#[test]
fn members_may_share_an_assets_directory_but_not_a_fixed_path() {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-groups-assets-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(root.join("assets/a.txt"), "A").unwrap();
    fs::write(root.join("assets/b.txt"), "B").unwrap();
    let write = |orders: &[(&str, &str, &str)], payment: &[(&str, &str, &str)]| {
        fs::write(
            root.join("src/shop/orders.bynk"),
            asset_context("shop.orders", orders),
        )
        .unwrap();
        fs::write(
            root.join("src/shop/payment.bynk"),
            asset_context("shop.payment", payment),
        )
        .unwrap();
    };

    // Different paths: both ship, in the one directory.
    write(
        &[("terms", "assets/a.txt", "/terms.txt")],
        &[("privacy", "assets/b.txt", "/privacy.txt")],
    );
    let (ok, out) = build(&root);
    assert!(ok, "distinct paths merge:\n{out}");
    for file in ["terms.txt", "privacy.txt"] {
        assert!(
            root.join("out/workers/core/assets").join(file).is_file(),
            "{file} ships in the group's one assets directory"
        );
    }

    // The same file at the same path is one file.
    write(
        &[("terms", "assets/a.txt", "/legal.txt")],
        &[("terms", "assets/a.txt", "/legal.txt")],
    );
    let (ok, out) = build(&root);
    assert!(ok, "one file reached by both members is fine:\n{out}");

    // Two files at one path: one error, at the later member.
    write(
        &[("terms", "assets/a.txt", "/legal.txt")],
        &[("terms", "assets/b.txt", "/legal.txt")],
    );
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert_eq!(
        out.matches("bynk.deploy.asset_path_conflict").count(),
        1,
        "{out}"
    );
    assert!(out.contains("`shop.orders` and `shop.payment`"), "{out}");
    assert!(out.contains("payment.bynk"), "at the later member:\n{out}");

    // A path that would be a folder of the other's file.
    write(
        &[("terms", "assets/a.txt", "/legal")],
        &[("notes", "assets/b.txt", "/legal/notes.txt")],
    );
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(out.contains("bynk.deploy.asset_path_conflict"), "{out}");
    assert!(out.contains("one would be a folder of the other"), "{out}");

    // Ungrouped, each context's own Worker may use the same path.
    fs::write(
        root.join("bynk.toml"),
        format!("{UNSTABLE_R2}[build]\ntarget = \"workers\"\n"),
    )
    .unwrap();
    write(
        &[("terms", "assets/a.txt", "/legal.txt")],
        &[("terms", "assets/b.txt", "/legal.txt")],
    );
    let (ok, out) = build(&root);
    assert!(
        ok,
        "separate Workers each serve their own `/legal.txt`:\n{out}"
    );
    let _ = fs::remove_dir_all(&root);
}

fn route_context(name: &str, routes: &[&str]) -> String {
    let handlers: String = routes
        .iter()
        .map(|r| {
            let params = if r.contains(":page") { "page: String" } else { "" };
            format!(
                "  on GET(\"{r}\") ({params}) -> Effect[HttpResult[String]] by v: Visitor {{\n    Ok(\"x\")\n  }}\n"
            )
        })
        .collect();
    format!("context {name}\n\nservice api from http {{\n{handlers}}}\n")
}

/// A group's members share one assets directory, and the platform answers an
/// asset's path before the Worker runs: so one member's fixed asset shadows
/// another member's literal route at that path, as it does the same member's own.
#[test]
fn a_members_fixed_asset_shadows_another_members_literal_route() {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-groups-shadow-{}", std::process::id()));
    write_project(&root, WORKERS_TARGET);
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(root.join("assets/a.txt"), "A").unwrap();
    let orders = asset_context("shop.orders", &[("terms", "assets/a.txt", "/legal.txt")]);
    fs::write(root.join("src/shop/orders.bynk"), &orders).unwrap();

    // Another member's literal route at the asset's path: refused, at the route.
    fs::write(
        root.join("src/shop/payment.bynk"),
        route_context("shop.payment", &["/legal.txt"]),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert_eq!(out.matches("bynk.asset.shadows_route").count(), 1, "{out}");
    assert!(
        out.contains("payment.bynk"),
        "at the shadowed route:\n{out}"
    );
    assert!(
        out.contains("the asset `assets/a.txt` of `shop.orders` is served at `/legal.txt`"),
        "naming the asset's member:\n{out}"
    );

    // An asset both members reach through a shared commons shadows payment's
    // route once, not once per pass that sees it.
    fs::write(
        root.join("src/shop/shared.bynk"),
        "commons shop.shared\n\nasset terms = \"assets/a.txt\" at \"/legal.txt\"\n",
    )
    .unwrap();
    fs::write(
        root.join("src/shop/orders.bynk"),
        "context shop.orders\n\nuses shop.shared\n\nservice orders_probe {\n\ton call() -> Effect[Int] by Caller {\n\t\tEffect.pure(1)\n\t}\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        route_context("shop.payment", &["/legal.txt"]).replacen(
            "context shop.payment\n",
            "context shop.payment\n\nuses shop.shared\n",
            1,
        ),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert_eq!(
        out.matches("bynk.asset.shadows_route").count(),
        1,
        "one error for the one route:\n{out}"
    );
    fs::remove_file(root.join("src/shop/shared.bynk")).unwrap();
    fs::write(root.join("src/shop/orders.bynk"), &orders).unwrap();

    // A route at another path, or a parameterised one, is not an error here.
    fs::write(
        root.join("src/shop/payment.bynk"),
        route_context("shop.payment", &["/pay", "/:page"]),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "other paths are untouched:\n{out}");

    // Ungrouped, the asset is another Worker's, so the route is reachable.
    fs::write(
        root.join("src/shop/payment.bynk"),
        route_context("shop.payment", &["/legal.txt"]),
    )
    .unwrap();
    fs::write(
        root.join("bynk.toml"),
        format!("{UNSTABLE_R2}[build]\ntarget = \"workers\"\n"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "separate Workers each have their own paths:\n{out}");
    let _ = fs::remove_dir_all(&root);
}

fn calling_context(name: &str, callee: Option<&str>, service: &str) -> String {
    match callee {
        Some(callee) => format!(
            "context {name}\n\nconsumes {callee}\n\nservice {service} {{\n\ton call(n: Int) -> Effect[Int] by Caller {{\n\t\tlet r <- {callee}.to_{}(n)\n\t\tEffect.pure(r)\n\t}}\n}}\n",
            callee.rsplit('.').next().unwrap()
        ),
        None => format!(
            "context {name}\n\nservice {service} {{\n\ton call(n: Int) -> Effect[Int] by Caller {{\n\t\tEffect.pure(n)\n\t}}\n}}\n"
        ),
    }
}

/// A Worker is uploaded after the Workers it binds to, so two groups may depend on
/// one another one way, and a group's members may call each other (a Worker
/// binding to itself), but not both ways between two groups.
#[test]
fn groups_may_depend_one_way_but_not_in_a_cycle() {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-groups-cycle-{}", std::process::id()));
    let manifest = "[build]\ntarget = \"workers\"\n\n[deploy.workers.front]\ncontexts = [\"shop.a1\", \"shop.a2\"]\n\n[deploy.workers.back]\ncontexts = [\"shop.b1\", \"shop.b2\"]\n";
    write_project(&root, manifest);
    let write = |a1: Option<&str>, a2: Option<&str>, b1: Option<&str>, b2: Option<&str>| {
        for (file, name, callee) in [
            ("a1", "shop.a1", a1),
            ("a2", "shop.a2", a2),
            ("b1", "shop.b1", b1),
            ("b2", "shop.b2", b2),
        ] {
            fs::write(
                root.join(format!("src/shop/{file}.bynk")),
                calling_context(name, callee, &format!("to_{file}")),
            )
            .unwrap();
        }
    };

    // front -> back and front -> back again: one direction, fine.
    write(Some("shop.b1"), Some("shop.b2"), None, None);
    let (ok, out) = build(&root);
    assert!(ok, "one-way dependencies between groups are fine:\n{out}");

    // An edge inside a group (a1 calls a2) is a Worker binding to itself.
    write(Some("shop.a2"), None, None, None);
    let (ok, out) = build(&root);
    assert!(ok, "calls within a group are no edge:\n{out}");

    // front -> back (a1 calls b1) and back -> front (b2 calls a2): a cycle
    // between the Workers, though no cycle among the contexts.
    write(Some("shop.b1"), None, None, Some("shop.a2"));
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert_eq!(out.matches("bynk.deploy.worker_cycle").count(), 1, "{out}");
    assert!(
        out.contains("`back`, `front`") || out.contains("`front`, `back`"),
        "{out}"
    );
    assert!(
        out.contains("`shop.a1` binds to `shop.b1`")
            && out.contains("`shop.b2` binds to `shop.a2`"),
        "one context edge per hop:\n{out}"
    );

    // A cycle needs one group and any other Worker, not two groups: a1 -> c and
    // c -> a2, with a1 and a2 in `front` and `shop.c` a Worker of its own.
    fs::write(
        root.join("bynk.toml"),
        "[build]\ntarget = \"workers\"\n\n[deploy.workers.front]\ncontexts = [\"shop.a1\", \"shop.a2\"]\n",
    )
    .unwrap();
    write(Some("shop.b1"), None, Some("shop.a2"), None);
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert_eq!(out.matches("bynk.deploy.worker_cycle").count(), 1, "{out}");
    assert!(
        out.contains("`front` to `shop.b1` (`shop.a1` binds to `shop.b1`)"),
        "{out}"
    );
    assert!(
        out.contains("`shop.b1` to `front` (`shop.b1` binds to `shop.a2`)"),
        "{out}"
    );

    // Ungrouped, the same contexts have no cycle: each is its own Worker.
    fs::write(root.join("bynk.toml"), "[build]\ntarget = \"workers\"\n").unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "ungrouped, the contexts form no cycle:\n{out}");
    let _ = fs::remove_dir_all(&root);
}

/// A group is one Worker with one `bynk-contracts.json`, which `deploy` reads
/// beside it (#3562, G4-0): what its members provide, and what they expect of
/// contexts outside it. A call between members has no contract to skew against.
#[test]
fn a_group_has_one_contracts_manifest_for_its_members() {
    let root = std::env::temp_dir().join(format!(
        "bynk-deploy-groups-manifest-{}",
        std::process::id()
    ));
    write_project(
        &root,
        "[build]\ntarget = \"workers\"\n\n[deploy.workers.front]\ncontexts = [\"shop.a1\", \"shop.a2\"]\n",
    );
    // `a1` calls its member `a2` and the outside `b1`; `a2` provides one service.
    fs::write(
        root.join("src/shop/a1.bynk"),
        "context shop.a1\n\nconsumes shop.a2\nconsumes shop.b1\n\nservice to_a1 {\n\ton call(n: Int) -> Effect[Int] by Caller {\n\t\tlet x <- shop.a2.to_a2(n)\n\t\tlet y <- shop.b1.to_b1(x)\n\t\tEffect.pure(y)\n\t}\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/shop/a2.bynk"),
        calling_context("shop.a2", None, "to_a2"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/b1.bynk"),
        calling_context("shop.b1", None, "to_b1"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "{out}");
    let manifest = fs::read_to_string(root.join("out/workers/front/bynk-contracts.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    let keys =
        |v: &serde_json::Value| -> Vec<String> { v.as_object().unwrap().keys().cloned().collect() };
    assert_eq!(keys(&json["provides"]), ["to_a1", "to_a2"], "{manifest}");
    assert_eq!(
        keys(&json["expects"]),
        ["shop.b1"],
        "only the outside dependency is expected: {manifest}"
    );
    // The members' own manifests are not left beside it for `deploy` to miss.
    for member in ["shop-a1", "shop-a2"] {
        assert!(
            !root
                .join(format!("out/workers/front/{member}.bynk-contracts.json"))
                .exists(),
            "{member}"
        );
    }
    // Ungrouped, each Worker has its own.
    fs::write(root.join("bynk.toml"), "[build]\ntarget = \"workers\"\n").unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "{out}");
    let a1 = fs::read_to_string(root.join("out/workers/shop-a1/bynk-contracts.json")).unwrap();
    assert!(a1.contains("shop.a2") && a1.contains("shop.b1"), "{a1}");
    let _ = fs::remove_dir_all(&root);
}

/// `bynk deploy` over a group (#3562, G4-1): the group is one Worker, deployed in
/// dependency order with the others, selectable by `--worker`, and a member cannot
/// be deployed alone. Dry run, so nothing touches Cloudflare; skips when the
/// environment has no way to run `wrangler`.
#[test]
fn deploy_treats_a_group_as_one_worker() {
    let root = std::env::temp_dir().join(format!("bynk-deploy-groups-run-{}", std::process::id()));
    write_project(
        &root,
        "[build]\ntarget = \"workers\"\n\n[deploy.workers.front]\ncontexts = [\"shop.a1\", \"shop.a2\"]\n",
    );
    fs::remove_file(root.join("src/shop/orders.bynk")).unwrap();
    fs::remove_file(root.join("src/shop/payment.bynk")).unwrap();
    // `a1` calls its member `a2` (the group binding to itself) and the outside `pay`.
    fs::write(
        root.join("src/shop/a1.bynk"),
        "context shop.a1\n\nconsumes shop.a2\nconsumes shop.pay\n\nservice to_a1 {\n\ton call(n: Int) -> Effect[Int] by Caller {\n\t\tlet x <- shop.a2.to_a2(n)\n\t\tlet y <- shop.pay.to_pay(x)\n\t\tEffect.pure(y)\n\t}\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/shop/a2.bynk"),
        calling_context("shop.a2", None, "to_a2"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/pay.bynk"),
        calling_context("shop.pay", None, "to_pay"),
    )
    .unwrap();
    let deploy = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
            .args(["deploy", "--dry-run", "--no-install", "--yes"])
            .args(args)
            .arg(&root)
            .env("CLOUDFLARE_API_TOKEN", "dummy")
            .env("NO_COLOR", "1")
            .output()
            .expect("run bynk");
        (
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };
    let (ok, all) = deploy(&[]);
    if !ok && all.contains("environment not ready") {
        eprintln!("skipping: no wrangler for a deploy dry run:\n{all}");
        let _ = fs::remove_dir_all(&root);
        return;
    }
    assert!(ok, "{all}");
    // The group is one Worker, after the Worker it binds to; its members are not
    // Workers of their own.
    let pay = all.find("deploy shop-pay").expect(&all);
    let front = all.find("deploy front").expect(&all);
    assert!(pay < front, "a dependency deploys first:\n{all}");
    assert!(
        !all.contains("shop-a1") && !all.contains("shop-a2"),
        "a member is not a Worker:\n{all}"
    );
    // The runtime signs as the calling member and verifies the key named for it, so the
    // group's Worker holds a key per member (for the calls between them too) and the
    // Worker it calls holds one for each member that might call it.
    for key in [
        "BYNK_CALL_FROM_SHOP__A1",
        "BYNK_CALL_FROM_SHOP__A2",
        "BYNK_CALL_TO_SHOP__A2",
        "BYNK_CALL_TO_SHOP__PAY",
    ] {
        assert!(all.contains(key), "no key {key}:\n{all}");
    }
    assert!(
        !all.contains("BYNK_CALL_FROM_FRONT"),
        "a key by Worker:\n{all}"
    );

    // `--worker` selects a Worker by its directory. Like `--context`, it assumes
    // the Workers it binds to are live (D4): the group binds to `shop-pay`.
    let (ok, one) = deploy(&["--worker", "front"]);
    assert!(
        !ok && one.contains("`front` binds to `shop-pay`, which has never been deployed"),
        "{one}"
    );
    let (ok, pay) = deploy(&["--worker", "shop-pay"]);
    assert!(
        ok && pay.contains("deploy shop-pay") && !pay.contains("deploy front"),
        "{pay}"
    );
    let (ok, none) = deploy(&["--worker", "nope"]);
    assert!(!ok && none.contains("no Worker `nope` was built"), "{none}");

    // A member cannot deploy alone: the refusal names its Worker.
    let (ok, member) = deploy(&["--context", "shop.a1"]);
    assert!(
        !ok && member.contains("context `shop.a1` is in Worker `front`")
            && member.contains("--worker front"),
        "{member}"
    );
    // The dashed form `--context` also takes is refused the same way.
    let (ok, dashed) = deploy(&["--context", "shop-a1"]);
    assert!(
        !ok && dashed.contains("context `shop-a1` is in Worker `front`"),
        "{dashed}"
    );
    // A context on its own Worker still deploys by `--context`.
    let (ok, alone) = deploy(&["--context", "shop.pay"]);
    assert!(ok && alone.contains("deploy shop-pay"), "{alone}");
    let _ = fs::remove_dir_all(&root);
}

/// A cycle between Workers that involves a group is refused at check time,
/// whatever closes it, once and never beside `consumes_cycle`. One among contexts
/// in no group is not: it builds, and `deploy` refuses it before uploading.
#[test]
fn a_worker_cycle_through_a_group_is_refused_once() {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-worker-cycle-{}", std::process::id()));
    write_project(&root, "[build]\ntarget = \"workers\"\n");

    // `shop.payment` subscribes to `shop.orders`' event and also calls it: the
    // publisher's Worker binds to its subscriber for the fan-out, and the
    // subscriber's to the publisher for the call. No group is involved, so the
    // project builds (`bynk dev` and the tests run it), and `deploy` refuses it
    // before uploading, as Cloudflare rejects a binding to a Worker that does not
    // exist yet (code 10143).
    fs::write(
        root.join("src/shop/orders.bynk"),
        publisher_context("shop.orders", "Placed"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        "context shop.payment\n\nconsumes shop.orders as P\n\nservice OnPlaced from Events(Placed) {\n\ton event(e: Placed) -> Effect[()] {\n\t\tlet _ <- P.probe_shop_orders()\n\t\tEffect.pure(())\n\t}\n}\n",
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "an ungrouped cycle builds:\n{out}");
    assert!(!out.contains("worker_cycle"), "{out}");
    let dry_run = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["deploy", "--dry-run", "--no-install", "--yes"])
        .arg(&root)
        .env("CLOUDFLARE_API_TOKEN", "dummy")
        .env("NO_COLOR", "1")
        .output()
        .expect("run bynk");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&dry_run.stdout),
        String::from_utf8_lossy(&dry_run.stderr)
    );
    if !said.contains("environment not ready") {
        assert!(!dry_run.status.success(), "{said}");
        assert!(
            said.contains("form a cycle") && said.contains("shop-orders"),
            "deploy refuses it, naming the Workers:\n{said}"
        );
    }

    // The same cycle with the publisher in a group is the check's: the group's
    // Worker and `shop.payment`'s bind to each other.
    fs::write(
        root.join("bynk.toml"),
        "[build]\ntarget = \"workers\"\n\n[deploy.workers.front]\ncontexts = [\"shop.orders\"]\n",
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert_eq!(out.matches("bynk.deploy.worker_cycle").count(), 1, "{out}");
    assert!(
        out.contains("`front`") && out.contains("`shop.payment` binds to `shop.orders`"),
        "{out}"
    );
    fs::write(root.join("bynk.toml"), "[build]\ntarget = \"workers\"\n").unwrap();

    // Calling alone, one way, is fine.
    fs::write(
        root.join("src/shop/orders.bynk"),
        calling_context("shop.orders", None, "to_orders"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        calling_context("shop.payment", Some("shop.orders"), "to_payment"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(ok, "a one-way call builds:\n{out}");

    // A cycle through three Workers: front{a1, a2}, a1 -> b1 -> c1 -> a2.
    fs::remove_file(root.join("src/shop/orders.bynk")).unwrap();
    fs::remove_file(root.join("src/shop/payment.bynk")).unwrap();
    fs::write(
        root.join("bynk.toml"),
        "[build]\ntarget = \"workers\"\n\n[deploy.workers.front]\ncontexts = [\"shop.a1\", \"shop.a2\"]\n",
    )
    .unwrap();
    for (file, callee) in [
        ("a1", Some("shop.b1")),
        ("a2", None),
        ("b1", Some("shop.c1")),
        ("c1", Some("shop.a2")),
    ] {
        fs::write(
            root.join(format!("src/shop/{file}.bynk")),
            calling_context(&format!("shop.{file}"), callee, &format!("to_{file}")),
        )
        .unwrap();
    }
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert_eq!(out.matches("bynk.deploy.worker_cycle").count(), 1, "{out}");
    for hop in [
        "`shop.a1` binds to `shop.b1`",
        "`shop.b1` binds to `shop.c1`",
        "`shop.c1` binds to `shop.a2`",
    ] {
        assert!(out.contains(hop), "{hop}:\n{out}");
    }

    // A cycle among the contexts is `consumes_cycle`'s alone.
    fs::write(root.join("bynk.toml"), "[build]\ntarget = \"workers\"\n").unwrap();
    fs::write(
        root.join("src/shop/a2.bynk"),
        calling_context("shop.a2", Some("shop.a1"), "to_a2"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/c1.bynk"),
        calling_context("shop.c1", Some("shop.a2"), "to_c1"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(out.contains("bynk.context.consumes_cycle"), "{out}");
    assert_eq!(
        out.matches("bynk.deploy.worker_cycle").count(),
        0,
        "one cycle, one error:\n{out}"
    );

    // A disjoint cycle through a group beside it is reported at the next build, once
    // the `consumes` cycle is fixed: the bindings it reads are built only for a
    // project with no error, so the two never appear together or twice.
    fs::write(
        root.join("bynk.toml"),
        "[build]\ntarget = \"workers\"\n\n[deploy.workers.front]\ncontexts = [\"shop.orders\"]\n",
    )
    .unwrap();
    fs::write(
        root.join("src/shop/orders.bynk"),
        publisher_context("shop.orders", "Placed"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        "context shop.payment\n\nconsumes shop.orders as P\n\nservice OnPlaced from Events(Placed) {\n\ton event(e: Placed) -> Effect[()] {\n\t\tlet _ <- P.probe_shop_orders()\n\t\tEffect.pure(())\n\t}\n}\n",
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(out.contains("bynk.context.consumes_cycle"), "{out}");
    assert_eq!(out.matches("bynk.deploy.worker_cycle").count(), 0, "{out}");
    fs::write(
        root.join("src/shop/a2.bynk"),
        calling_context("shop.a2", None, "to_a2"),
    )
    .unwrap();
    let (ok, out) = build(&root);
    assert!(!ok, "{out}");
    assert!(!out.contains("bynk.context.consumes_cycle"), "{out}");
    assert_eq!(out.matches("bynk.deploy.worker_cycle").count(), 1, "{out}");
    assert!(
        out.contains("`front`") && out.contains("`shop.payment`"),
        "{out}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// #3562 (G5): a context that hosts an agent must not change scripts unannounced. `shop.orders`
/// was last deployed on its own Worker (the ledger says so); the manifest now puts it in a group,
/// where its agent would start empty. `deploy` refuses, `--dry-run` included, and
/// `--accept-fresh-agent shop.orders` goes ahead, saying where the old data stays.
#[test]
fn deploy_refuses_a_group_move_of_a_context_with_an_agent() {
    let root = std::env::temp_dir().join(format!("bynk-deploy-agent-move-{}", std::process::id()));
    write_project(
        &root,
        "[build]\ntarget = \"workers\"\n\n[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.payment\"]\n",
    );
    fs::write(
        root.join("src/shop/orders.bynk"),
        agent_context("shop.orders"),
    )
    .unwrap();
    fs::write(
        root.join("src/shop/payment.bynk"),
        calling_context("shop.payment", None, "to_pay"),
    )
    .unwrap();
    fs::write(
        root.join("bynk.deploy.lock"),
        "version = 1\n\n[environments.default.workers.shop-orders]\ndeployed = true\n",
    )
    .unwrap();
    let deploy = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
            .args(["deploy", "--dry-run", "--no-install", "--yes"])
            .args(args)
            .arg(&root)
            .env("CLOUDFLARE_API_TOKEN", "dummy")
            .env("NO_COLOR", "1")
            .output()
            .expect("run bynk");
        (
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };
    let (ok, refused) = deploy(&[]);
    if !ok && refused.contains("environment not ready") {
        eprintln!("skipping: no wrangler for a deploy dry run:\n{refused}");
        let _ = fs::remove_dir_all(&root);
        return;
    }
    assert!(!ok, "{refused}");
    assert!(
        refused.contains("bynk.deploy.agent_move_refused")
            && refused.contains("`shop.orders`")
            && refused.contains("`Cart`")
            && refused.contains("`shop-orders`")
            && refused.contains("--accept-fresh-agent shop.orders"),
        "{refused}"
    );
    let (ok, accepted) = deploy(&["--accept-fresh-agent", "shop.orders"]);
    assert!(ok, "{accepted}");
    assert!(
        accepted.contains("starts with fresh agent `Cart`")
            && accepted.contains("script `shop-orders`"),
        "{accepted}"
    );
    let (ok, typo) = deploy(&["--accept-fresh-agent", "shop.nope"]);
    assert!(
        !ok && typo.contains("`--accept-fresh-agent shop.nope` names no context"),
        "{typo}"
    );
    let _ = fs::remove_dir_all(&root);
}
