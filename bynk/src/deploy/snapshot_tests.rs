//! Back-half paydown 1D.1 (#3929): the byte-identity fixtures every `ResourceKind` slice is
//! held to. Added before any of the deploy code moves, and never re-blessed by a conversion:
//! a diff here is a change in what `bynk deploy` prints or writes, which 1D.1 promises not to
//! make (`design/tracks/back-half-paydown.md` §3, and #3929's "Goldens").
//!
//! - `every_kind.deploy.lock` records every kind with ledger state, in two environments, with
//!   `superseded` entries, adopted records, reused queues and orphans of each kind. Read and
//!   written again, it must come back byte for byte.
//! - `every_kind.plan.short.txt` and `every_kind.plan.json` are the plan of a project that
//!   declares every kind, derived against that ledger, in both formats, orphans included, with
//!   the notes checking it against an account adds.

use super::*;
use crate::deploy::config::tests::project;

pub(crate) const LEDGER: &str = include_str!("fixtures/every_kind.deploy.lock");
const PLAN_SHORT: &str = include_str!("fixtures/every_kind.plan.short.txt");
const PLAN_JSON: &str = include_str!("fixtures/every_kind.plan.json");

/// A scratch directory for one ledger, unique to this process and call: the tests run in
/// parallel, and each writes a ledger of its own.
fn scratch() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("bynk-1d1-snapshot-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The fixture ledger, as `bynk deploy` reads it.
pub(crate) fn ledger() -> DeployLock {
    let dir = scratch();
    let path = dir.join(LOCK_FILE);
    std::fs::write(&path, LEDGER).expect("the fixture is written");
    let lock = read_lock(&path).expect("the fixture ledger reads");
    let _ = std::fs::remove_dir_all(&dir);
    lock
}

/// A project declaring every kind: `shop` with each, `shop-cart` with a KV instance, a rate
/// limiter and a queue it shares. `gone` and the `*_OLD` bindings are recorded and not
/// declared, so each kind has an orphan.
pub(crate) fn every_kind() -> BTreeMap<String, Resources> {
    let mut shop = Resources::default()
        .needs_kv()
        .kv_instances(&["KV_CART", "KV_SESSIONS"])
        .r2_buckets(&[
            ("R2_ARCHIVE", "shop-archive", Some("eu")),
            ("R2_FILES", "shop-files", None),
            ("R2_NEW", "shop-new", None),
        ])
        .d1_databases(&[("D1_NEW", "shop-new"), ("D1_ORDERS", "shop-orders")])
        .vectorize_indexes(&[
            ("VECTORIZE_DOCS", "shop-docs", 768, "cosine", &["category"]),
            ("VECTORIZE_NEW", "shop-new", 384, "euclidean", &[]),
        ])
        .hyperdrive(&["HYPERDRIVE_ADOPTED", "HYPERDRIVE_MAIN", "HYPERDRIVE_NEW"])
        .ratelimits(&["RATELIMIT_LOGIN", "RATELIMIT_SIGNUP"])
        .consumes(&["intake"])
        .produces(&["outbound", "shared-feed"])
        .analytics(&[("ANALYTICS_EVENTS", "shop-events")])
        .stored_secrets(&[("SECRET_STRIPE_KEY", "default_secrets_store", "stripe_key")])
        .ai_search(&[("AI_SEARCH_KB", "kb")])
        .mtls_certificates(&[("MTLS_BACKEND", "backend-client")])
        .exports(&["Cart"])
        .foreign(&[("PAYMENTS", "payments-worker")])
        .email_addresses(&["orders@example.com"])
        .declares(&["API_TOKEN"])
        .binds(&["shop-cart"]);
    shop.d1_databases[1].migrations =
        vec!["0001_init.sql".to_string(), "0002_index.sql".to_string()];
    shop.containers = vec![
        ContainerDecl {
            capability: "Renderer".to_string(),
            class: "__BynkContainer_Renderer".to_string(),
            build: "containers/renderer".to_string(),
            instance_type: None,
            adopt: Some("registry.example/renderer@sha256:bbbb".to_string()),
        },
        ContainerDecl {
            capability: "Thumbnailer".to_string(),
            class: "__BynkContainer_Thumbnailer".to_string(),
            build: "containers/thumbnailer".to_string(),
            instance_type: Some("standard".to_string()),
            adopt: None,
        },
    ];
    shop.renamed_durable_objects = vec![("Basket".to_string(), "Cart".to_string())];
    shop.deleted_durable_objects = vec!["Legacy".to_string()];
    let cart = Resources::default()
        .kv_instances(&["KV_SESSIONS"])
        .ratelimits(&["RATELIMIT_CHECKOUT"])
        .consumes(&["shared-feed"]);
    project(vec![("shop", shop), ("shop-cart", cart)])
}

pub(crate) fn order() -> Vec<String> {
    vec!["shop-cart".to_string(), "shop".to_string()]
}

/// What `bynk.toml` and the environment say of each `Postgres` binding: one adopted by id.
fn hyperdrive_wants() -> hyperdrive::Wants {
    hyperdrive::Wants::from([(
        ("shop".to_string(), "HYPERDRIVE_ADOPTED".to_string()),
        hyperdrive::Want::Adopt {
            id: "hd-adopted".to_string(),
        },
    )])
}

/// An account holding a KV namespace the ledger does not record, an R2 bucket, a D1 database,
/// a Vectorize index and a queue of the names the plan would create.
struct Account;

impl on_account::Listings for Account {
    fn names(
        &self,
        kind: KindId,
        jurisdiction: Option<&str>,
    ) -> Result<BTreeSet<String>, crate::cloudflare::CloudflareError> {
        let names: &[&str] = match (kind, jurisdiction) {
            (KindId::Kv, None) => &["shop-cart"],
            (KindId::R2, None) => &["shop-new"],
            (KindId::D1, None) => &["shop-new"],
            (KindId::Vectorize, None) => &["shop-new"],
            (KindId::Queue, None) => &["outbound"],
            (KindId::Hyperdrive, None)
            | (KindId::RateLimit, None)
            | (KindId::StoredSecret, None)
            | (KindId::AiSearch, None)
            | (KindId::MtlsCertificate, None)
            | (KindId::ContainerImage, None) => &[],
            (_, Some(_)) => return Err("Cloudflare answered 403".into()),
        };
        Ok(names.iter().map(|n| n.to_string()).collect())
    }
}

/// The whole plan, both formats, then the account notes, as one text.
fn plan_text(format: DeployFormat) -> String {
    let lock = ledger();
    let resources = every_kind();
    let order = order();
    let mut out = String::new();
    for environment in ["default", "staging"] {
        let plan = derive_plan(
            &order,
            &resources,
            &lock,
            &SecretSource::default(),
            false,
            environment,
        )
        .with_lives(
            &order,
            &resources,
            &lock,
            Lives::for_plan(&hyperdrive_wants()),
        );
        let on = on_account::OnAccount::read(&plan.account_listings(), &Account);
        let plan = plan.checked_against(&on);
        out.push_str(&format!("== {environment} ==\n"));
        out.push_str(&plan_report(&plan, format));
        for note in plan.account_notes() {
            out.push_str(note);
            out.push('\n');
        }
    }
    out
}

#[test]
fn the_every_kind_ledger_round_trips_byte_for_byte() {
    let dir = scratch();
    let path = dir.join(LOCK_FILE);
    write_lock(&path, &ledger()).expect("the ledger is written");
    let written = std::fs::read_to_string(&path).expect("the written ledger reads");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        written, LEDGER,
        "the ledger fixture no longer round-trips: 1D.1 must not change `bynk.deploy.lock`"
    );
}

#[test]
fn the_every_kind_plan_is_byte_identical_in_the_short_format() {
    assert_eq!(
        plan_text(DeployFormat::Short),
        PLAN_SHORT,
        "the short plan changed: 1D.1 must not change `bynk deploy`'s output"
    );
}

#[test]
fn the_every_kind_plan_is_byte_identical_in_the_json_format() {
    assert_eq!(
        plan_text(DeployFormat::Json),
        PLAN_JSON,
        "the JSON plan changed: 1D.1 must not change `bynk deploy --format json`"
    );
}

/// 1D.2b (#4001 Decision A): one decision for plan and apply. Each step of the every-kind plan
/// decides what its line printed before the account check, and the account check, which only
/// rewrites lines, never reaches a step: `shop`'s `R2_NEW`, which the account holds and the
/// ledger lacks, prints `reuse` and is still made and recorded by the apply.
#[test]
fn each_step_decides_what_its_line_printed_and_the_account_check_reaches_no_step() {
    let lock = ledger();
    let resources = every_kind();
    let order = order();
    for environment in ["default", "staging"] {
        let plan = derive_plan(
            &order,
            &resources,
            &lock,
            &SecretSource::default(),
            false,
            environment,
        )
        .with_lives(
            &order,
            &resources,
            &lock,
            Lives::for_plan(&hyperdrive_wants()),
        );
        for context in &plan.contexts {
            let steps = context.steps();
            let actions = |lines: Vec<&str>, steps: Vec<&str>| assert_eq!(lines, steps);
            actions(
                context.kv.iter().map(|l| l.action).collect(),
                steps.kv.iter().map(KvStep::action).collect(),
            );
            actions(
                context.r2.iter().map(|l| l.action).collect(),
                steps.r2.iter().map(|s| s.decision.action()).collect(),
            );
            actions(
                context.d1.iter().map(|l| l.action).collect(),
                steps.d1.iter().map(D1Step::action).collect(),
            );
            actions(
                context.hyperdrive.iter().map(|l| l.action).collect(),
                steps.hyperdrive.iter().map(|s| s.action).collect(),
            );
            actions(
                context.vectorize.iter().map(|l| l.action).collect(),
                steps
                    .vectorize
                    .iter()
                    .filter_map(|s| match s {
                        VectorizeStep::Index(index) => Some(index.action()),
                        VectorizeStep::MetadataIndexes(_) => None,
                    })
                    .collect(),
            );
        }
        if environment != "default" {
            continue;
        }
        // The account holds `shop-new`, the default environment's name for `R2_NEW`.
        let on = on_account::OnAccount::read(&plan.account_listings(), &Account);
        let plan = plan.checked_against(&on);
        let shop = &plan.contexts[1];
        let new = shop
            .r2
            .iter()
            .position(|l| l.binding == "R2_NEW")
            .expect("shop binds R2_NEW");
        assert_eq!(shop.r2[new].action, "reuse", "the account holds the bucket");
        assert_eq!(
            shop.steps().r2[new].decision,
            R2Decision::Create,
            "the ledger lacks it, so the apply makes it and records it"
        );
    }
}
