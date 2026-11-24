use super::keys::combine_presence;
use super::messages::wrangler_floor_refusal;
use super::model::{
    analytics_dataset_problem, confirm_address_removals, confirm_deletions, recreate_refusal,
};
use super::run::secrets_file_passthrough_refusal;
use super::skew::CONTRACT_SKEW;
use super::*;
use crate::deploy::config::tests::project;
use crate::deploy::graph::tests::graph;
use crate::deploy::ledger::tests::{lock_with_deployed, with_kv, with_queue};
use crate::deploy::*;
use bynk_driver::npm_install::PackageManager;

/// Two contexts that list the same `SignedCookie` key binding: a login service that issues
/// the cookie and an API that it guards (#3386).
fn shared_session_key() -> BTreeMap<String, Resources> {
    BTreeMap::from([
        (
            "login".to_string(),
            Resources::default().session_keys(&[("User", "SESSION_KEY")]),
        ),
        (
            "api".to_string(),
            Resources::default().session_keys(&[("User", "SESSION_KEY")]),
        ),
    ])
}

fn decide_sessions(
    resources: &BTreeMap<String, Resources>,
    order: &[&str],
    held: &[(&str, bool)],
    source: &SecretSource,
) -> Result<BTreeMap<(String, String), ForeignKey>, String> {
    let order = names(order);
    let listed: BTreeMap<String, Option<BTreeSet<String>>> = held
        .iter()
        .map(|(w, has)| {
            (
                w.to_string(),
                Some(if *has {
                    BTreeSet::from(["SESSION_KEY".to_string()])
                } else {
                    BTreeSet::new()
                }),
            )
        })
        .collect();
    let lock = lock_with_deployed(&[order[0].as_str(), order[order.len() - 1].as_str()]);
    let opts = DeployOptions {
        environment: "default".to_string(),
        ..DeployOptions::default()
    };
    ManagedKeys {
        order: &order,
        resources,
        listed: &listed,
    }
    .decide(&lock, &opts, source)
}

#[test]
fn contexts_sharing_a_session_key_are_set_to_one_minted_value() {
    let keys = decide_sessions(
        &shared_session_key(),
        &["api", "login"],
        &[("api", false), ("login", false)],
        &SecretSource::of_values(&[]),
    )
    .expect("both are new: one key for both");
    let value = |w: &str| match &keys[&(w.to_string(), "SESSION_KEY".to_string())] {
        ForeignKey::Set {
            value,
            minted: true,
        } => value.clone(),
        other => panic!("expected a minted key, got {other:?}"),
    };
    assert_eq!(value("api"), value("login"));
}

#[test]
fn a_session_key_one_sibling_holds_and_another_lacks_is_refused() {
    let err = decide_sessions(
        &shared_session_key(),
        &["api", "login"],
        &[("api", true), ("login", false)],
        &SecretSource::of_values(&[]),
    )
    .expect_err("different keys would break every cookie");
    assert!(
        err.contains("SESSION_KEY") && err.contains("api, login"),
        "{err}"
    );
}

#[test]
fn a_run_reaching_only_one_sharing_context_does_not_mint() {
    let err = decide_sessions(
        &shared_session_key(),
        &["login"],
        &[("login", false)],
        &SecretSource::of_values(&[]),
    )
    .expect_err("the sibling's key would differ");
    assert!(err.contains("not all of them are in this run"), "{err}");
}

#[test]
fn a_supplied_session_key_is_set_on_every_sharing_context() {
    let key = "Ynluay10ZXN0LXNlc3Npb24ta2V5LTAxMjM0NTY3ODk";
    let keys = decide_sessions(
        &shared_session_key(),
        &["login"],
        &[("login", false)],
        &SecretSource::of_values(&[("SESSION_KEY", key)]),
    )
    .expect("a supplied value is the same everywhere");
    assert_eq!(
        keys[&("login".to_string(), "SESSION_KEY".to_string())],
        ForeignKey::Set {
            value: key.to_string(),
            minted: false
        }
    );
}

/// #3477: the plan names each container's image source and instance type,
/// says whose it is to reconcile, shows a changed adoption, and states no
/// price.
#[test]
fn the_plan_names_a_containers_image_instance_and_owner() {
    use crate::deploy::containers::tests_support::decl;
    let order = names(&["shop"]);
    let image = "registry.cloudflare.com/abc123/renderer@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let mut declared = project(vec![(
        "shop",
        Resources {
            containers: vec![
                decl("Renderer", Some("standard-1"), None),
                decl("Thumbs", None, Some(image)),
            ],
            ..Resources::default()
        },
    )]);
    let text = plan_report(
        &plan_of(&order, &declared, &DeployLock::default()),
        DeployFormat::Short,
    );
    assert!(
        text.contains("container __Container_Renderer (image: build from containers/render; instance standard-1; reconciled by Cloudflare)\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "container __Container_Thumbs (image: adopted {image}; instance lite; reconciled by Cloudflare)\n"
        )),
        "{text}"
    );
    // A recorded adoption that is dropped shows beside the new line.
    let mut recorded = DeployLock::default();
    recorded.record::<ContainerImage>("default", "shop", "Renderer", image.to_string());
    declared.get_mut("shop").unwrap().containers[0].adopt = None;
    let again = plan_report(&plan_of(&order, &declared, &recorded), DeployFormat::Short);
    assert!(
        again.contains(&format!(
            "image: build from containers/render, was adopted {image};"
        )),
        "{again}"
    );
    // The JSON plan carries the same facts, and no rate or allowance.
    let json = plan_report(&plan_of(&order, &declared, &recorded), DeployFormat::Json);
    assert!(json.contains("\"containers\""), "{json}");
    assert!(json.contains("\"reconciled_by\": \"Cloudflare\""), "{json}");
    assert!(
        !text.contains('$') && !text.to_lowercase().contains("billed"),
        "{text}"
    );
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// `derive_plan` with no secret input and no `--force` — the shape every
/// test that predates slice 3 wants, so those tests keep saying what they
/// are about rather than restating two arguments they do not exercise.
pub(crate) fn plan_of<'a>(
    order: &'a [String],
    resources: &'a BTreeMap<String, Resources>,
    lock: &DeployLock,
) -> Plan<'a> {
    derive_plan(
        order,
        resources,
        lock,
        &SecretSource::default(),
        false,
        "default",
    )
}

// ---- #3479: Vectorize indexes ----

#[test]
fn the_plan_names_each_index_its_shape_and_its_orphans() {
    let order = names(&["shop"]);
    let declared = project(vec![(
        "shop",
        Resources::default().vectorize_indexes(&[(
            "VECTORIZE_DOCS",
            "shop-docs",
            768,
            "cosine",
            &["category:string"],
        )]),
    )]);
    let mut lock = DeployLock::default();
    lock.record::<Vectorize>(
        "default",
        "shop",
        "VECTORIZE_OLD",
        VectorizeRecord::of("shop-old", false),
    );
    let fresh = plan_report(&plan_of(&order, &declared, &lock), DeployFormat::Short);
    assert!(
        fresh.contains("vectorize create shop-docs (VECTORIZE_DOCS, 768, cosine; metadata indexes category:string)\n"),
        "{fresh}"
    );
    assert!(
        fresh.contains("orphan vectorize shop VECTORIZE_OLD (shop-old; deleted by --prune)\n"),
        "{fresh}"
    );
    lock.record::<Vectorize>(
        "default",
        "shop",
        "VECTORIZE_DOCS",
        VectorizeRecord::of("shop-docs", false),
    );
    let again = plan_report(&plan_of(&order, &declared, &lock), DeployFormat::Short);
    assert!(
        again.contains("vectorize reuse shop-docs (VECTORIZE_DOCS, 768, cosine; metadata indexes category:string)\n"),
        "{again}"
    );
    // An adopted index is named as it is on the account, and planned as an
    // `adopt` until the ledger records it.
    let mut adopted = project(vec![(
        "shop",
        Resources::default().vectorize_indexes(&[(
            "VECTORIZE_DOCS",
            "shop-docs",
            768,
            "cosine",
            &["category:string"],
        )]),
    )]);
    adopted.get_mut("shop").unwrap().vectorize_indexes[0].adopt = Some("docs-prod".into());
    let adopt = plan_report(
        &plan_of(&order, &adopted, &DeployLock::default()),
        DeployFormat::Short,
    );
    assert!(
        adopt.contains("vectorize adopt docs-prod (VECTORIZE_DOCS, 768, cosine"),
        "{adopt}"
    );
    // Once recorded, an adopted index is still planned as `adopt`.
    let mut recorded = DeployLock::default();
    recorded.record::<Vectorize>(
        "default",
        "shop",
        "VECTORIZE_DOCS",
        VectorizeRecord::of("docs-prod", true),
    );
    let again = plan_report(&plan_of(&order, &adopted, &recorded), DeployFormat::Short);
    assert!(
        again.contains("vectorize adopt docs-prod (VECTORIZE_DOCS, 768, cosine"),
        "{again}"
    );
    // A staging deploy names the qualified index.
    let staging = derive_plan(
        &order,
        &declared,
        &DeployLock::default(),
        &SecretSource::default(),
        false,
        "staging",
    );
    assert_eq!(staging.contexts[0].vectorize[0].index, "shop-docs-staging");
}

// ---- #3494: the plan checked against the account ----

/// An account holding `names` of each kind (R2 within the jurisdiction
/// given), counting what it was asked for.
struct Account {
    names: Vec<(KindId, Option<&'static str>, &'static str)>,
    asked: std::cell::RefCell<Vec<super::super::on_account::Listing>>,
    /// The kinds whose listing fails.
    failing: Vec<KindId>,
}

impl super::super::on_account::Listings for Account {
    fn names(
        &self,
        kind: KindId,
        jurisdiction: Option<&str>,
    ) -> Result<BTreeSet<String>, crate::cloudflare::CloudflareError> {
        self.asked
            .borrow_mut()
            .push((kind, jurisdiction.map(str::to_string)));
        if self.failing.contains(&kind) {
            return Err("Cloudflare answered 403".into());
        }
        Ok(self
            .names
            .iter()
            .filter(|(k, j, _)| *k == kind && *j == jurisdiction)
            .map(|(_, _, n)| n.to_string())
            .collect())
    }
}

fn checked<'a>(plan: Plan<'a>, account: &Account) -> Plan<'a> {
    let on = super::super::on_account::OnAccount::read(&plan.account_listings(), account);
    plan.checked_against(&on)
}

fn every_kind() -> BTreeMap<String, Resources> {
    project(vec![(
        "shop",
        Resources::default()
            .needs_kv()
            .r2_buckets(&[
                ("R2_FILES", "shop-files", None),
                ("R2_ARCHIVE", "shop-archive", Some("eu")),
            ])
            .d1_databases(&[("D1_ORDERS", "shop-orders")])
            .consumes(&["intake"])
            .produces(&["outbound"])
            .vectorize_indexes(&[("VECTORIZE_DOCS", "shop-docs", 768, "cosine", &[])]),
    )])
}

/// #3494: a resource the ledger does not record but the account already
/// holds by name is `reuse`, as the deploy will use it, for every kind
/// whose name the plan derives; KV, whose second namespace of a title
/// Cloudflare refuses, is named in a note instead.
#[test]
fn a_resource_already_on_the_account_is_planned_as_reuse() {
    let order = names(&["shop"]);
    let declared = every_kind();
    let account = Account {
        names: vec![
            (KindId::Kv, None, "shop"),
            (KindId::R2, None, "shop-files"),
            // The same name in the default jurisdiction is another bucket.
            (KindId::R2, None, "shop-archive"),
            (KindId::D1, None, "shop-orders"),
            (KindId::Queue, None, "intake"),
            (KindId::Queue, None, "outbound"),
            (KindId::Vectorize, None, "shop-docs"),
        ],
        asked: Default::default(),
        failing: Vec::new(),
    };
    let plan = checked(plan_of(&order, &declared, &DeployLock::default()), &account);
    let text = plan_report(&plan, DeployFormat::Short);
    for line in [
        "r2 reuse shop-files (R2_FILES)\n",
        "r2 create shop-archive (R2_ARCHIVE) in jurisdiction eu\n",
        "d1 reuse shop-orders (D1_ORDERS)\n",
        "queue reuse intake\n",
        "queue reuse outbound\n",
        "vectorize reuse shop-docs (VECTORIZE_DOCS, 768, cosine)\n",
        "kv create shop\n",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    let notes = plan.account_notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("`shop`") && notes[0].contains("already on the account"),
        "{}",
        notes[0]
    );
    // A queue only sent to, not on the account, is now known to be created.
    let empty = Account {
        names: Vec::new(),
        asked: Default::default(),
        failing: Vec::new(),
    };
    let plan = checked(plan_of(&order, &declared, &DeployLock::default()), &empty);
    let text = plan_report(&plan, DeployFormat::Short);
    assert!(text.contains("queue create outbound\n"), "{text}");
    assert!(plan.account_notes().is_empty());
}

/// #3494 review: a listing that fails leaves its kind as the ledger says it:
/// a queue only sent to stays `create or reuse`, never a confident `create`.
#[test]
fn a_failed_listing_leaves_its_kind_as_the_ledger_says() {
    let order = names(&["shop"]);
    let declared = every_kind();
    let account = Account {
        names: Vec::new(),
        asked: Default::default(),
        failing: vec![KindId::Queue],
    };
    let plan = checked(plan_of(&order, &declared, &DeployLock::default()), &account);
    let text = plan_report(&plan, DeployFormat::Short);
    assert!(text.contains("queue create or reuse outbound\n"), "{text}");
    assert!(text.contains("queue create intake\n"), "{text}");
    assert_eq!(plan.account_notes().len(), 1, "{:?}", plan.account_notes());
    assert!(plan.account_notes()[0].contains("queues"));
}

/// #3494: a plan with nothing to create asks the account nothing.
#[test]
fn a_plan_the_ledger_covers_asks_the_account_nothing() {
    let order = names(&["shop"]);
    let declared = project(vec![(
        "shop",
        Resources::default().vectorize_indexes(&[(
            "VECTORIZE_DOCS",
            "shop-docs",
            768,
            "cosine",
            &[],
        )]),
    )]);
    let mut lock = DeployLock::default();
    lock.record::<Vectorize>(
        "default",
        "shop",
        "VECTORIZE_DOCS",
        VectorizeRecord::of("shop-docs", false),
    );
    let account = Account {
        names: Vec::new(),
        asked: Default::default(),
        failing: Vec::new(),
    };
    let plan = checked(plan_of(&order, &declared, &lock), &account);
    assert!(account.asked.borrow().is_empty());
    assert!(plan_report(&plan, DeployFormat::Short).contains("vectorize reuse shop-docs"));
}

// ---- P5 (#2323): R2 buckets ----

// ---- P3 (#2342): rate limiters ----

#[test]
fn the_plan_allocates_each_rate_limiter_then_reuses_its_id() {
    let declared = project(vec![(
        "shop",
        Resources::default().ratelimits(&["RATELIMIT_LOGIN"]),
    )]);
    let order = names(&["shop"]);
    let fresh = plan_of(&order, &declared, &DeployLock::default());
    let report = plan_report(&fresh, DeployFormat::Short);
    assert!(
        report.contains("ratelimit allocate RATELIMIT_LOGIN\n"),
        "{report}"
    );
    assert_eq!(
        RateLimit::unrecordable_in_ci(&ProjectCx {
            ledger: LedgerView::of(&DeployLock::default(), "default"),
            resources: &declared,
            order: &order,
        })
        .as_deref(),
        Some(
            "bynk: error: the rate limiter `RATELIMIT_LOGIN` of `shop` is unrecorded; deploy locally first and commit bynk.deploy.lock"
        ),
        "what CI refuses to allocate"
    );
    let mut lock = DeployLock::default();
    allocate_in(&mut lock, "default", "shop", "RATELIMIT_LOGIN");
    let again = plan_of(&order, &declared, &lock);
    let report = plan_report(&again, DeployFormat::Short);
    let id = crate::deploy::ledger::ratelimit_seed("default", "shop", "RATELIMIT_LOGIN");
    assert!(
        report.contains(&format!(
            "ratelimit reuse RATELIMIT_LOGIN (namespace {id})\n"
        )),
        "{report}"
    );
    assert_eq!(
        RateLimit::unrecordable_in_ci(&ProjectCx {
            ledger: LedgerView::of(&lock, "default"),
            resources: &declared,
            order: &order,
        }),
        None
    );
}

fn shop_with_buckets() -> BTreeMap<String, Resources> {
    project(vec![(
        "shop",
        Resources::default().r2_buckets(&[
            ("R2_ARCHIVES", "shop-archives", None),
            ("R2_INDEXES", "shop-indexes", Some("eu")),
        ]),
    )])
}

#[test]
fn the_plan_creates_each_bucket_then_reuses_it() {
    let declared = shop_with_buckets();
    let order = names(&["shop"]);
    let fresh = plan_of(&order, &declared, &DeployLock::default());
    let r2: Vec<(&str, &str, &str)> = fresh.contexts[0]
        .r2
        .iter()
        .map(|b| (b.binding, b.action, b.bucket.as_str()))
        .collect();
    assert_eq!(
        r2,
        [
            ("R2_ARCHIVES", "create", "shop-archives"),
            ("R2_INDEXES", "create", "shop-indexes")
        ]
    );
    let mut lock = DeployLock::default();
    lock.record::<R2>(
        "default",
        "shop",
        "R2_ARCHIVES",
        R2Bucket::of("shop-archives", None),
    );
    lock.record::<R2>(
        "default",
        "shop",
        "R2_INDEXES",
        R2Bucket::of("shop-indexes", Some("eu")),
    );
    let again = plan_of(&order, &declared, &lock);
    assert!(again.contexts[0].r2.iter().all(|b| b.action == "reuse"));
}

#[test]
fn a_named_environment_plans_its_own_qualified_buckets() {
    let declared = shop_with_buckets();
    let order = names(&["shop"]);
    let plan = derive_plan(
        &order,
        &declared,
        &DeployLock::default(),
        &SecretSource::default(),
        false,
        "staging",
    );
    assert_eq!(plan.contexts[0].r2[0].bucket, "shop-archives-staging");
    let report = plan_report(&plan, DeployFormat::Short);
    assert!(
        report.contains("r2 create shop-archives-staging (R2_ARCHIVES)\n"),
        "{report}"
    );
    assert!(
        report.contains("r2 create shop-indexes-staging (R2_INDEXES) in jurisdiction eu\n"),
        "{report}"
    );
}

/// #3877: `bynk deploy --env staging` plans the dead-letter queue under its qualified name, as
/// `create or reuse` (it is recorded only when Bynk makes it), beside the consumer's own queue.
#[test]
fn a_named_environment_plans_a_qualified_dead_letter_queue() {
    let declared = project(vec![(
        "shop",
        Resources::default()
            .consumes(&["outbound-email"])
            .produces(&["outbound-email-dlq"]),
    )]);
    let order = names(&["shop"]);
    let plan = derive_plan(
        &order,
        &declared,
        &DeployLock::default(),
        &SecretSource::default(),
        false,
        "staging",
    );
    let report = plan_report(&plan, DeployFormat::Short);
    assert!(
        report.contains("queue create outbound-email-staging\n"),
        "{report}"
    );
    assert!(
        report.contains("queue create or reuse outbound-email-dlq-staging\n"),
        "{report}"
    );
}

#[test]
fn a_changed_jurisdiction_is_a_recreate_that_says_the_old_bucket_stays() {
    let declared = shop_with_buckets();
    let mut lock = DeployLock::default();
    lock.record::<R2>(
        "default",
        "shop",
        "R2_ARCHIVES",
        R2Bucket::of("shop-archives", None),
    );
    lock.record::<R2>(
        "default",
        "shop",
        "R2_INDEXES",
        R2Bucket::of("shop-indexes", Some("fedramp")),
    );
    let order = names(&["shop"]);
    let plan = plan_of(&order, &declared, &lock);
    assert_eq!(plan.contexts[0].r2[0].action, "reuse");
    assert_eq!(plan.contexts[0].r2[1].action, "recreate");
    let report = plan_report(&plan, DeployFormat::Short);
    assert!(
        report.contains("r2 recreate shop-indexes (R2_INDEXES) in jurisdiction eu — the old bucket and its objects stay where they are"),
        "{report}"
    );
}

// ---- P4a (#2602): D1 databases ----

#[test]
fn the_plan_creates_each_database_then_reuses_it_and_reports_an_orphan() {
    let declared = project(vec![(
        "shop",
        Resources::default().d1_databases(&[("D1_ORDERS", "shop-orders")]),
    )]);
    let order = names(&["shop"]);
    let fresh = plan_of(&order, &declared, &DeployLock::default());
    let report = plan_report(&fresh, DeployFormat::Short);
    assert!(
        report.contains("d1 create shop-orders (D1_ORDERS)\n"),
        "{report}"
    );
    let mut lock = DeployLock::default();
    lock.record::<D1>(
        "default",
        "shop",
        "D1_ORDERS",
        D1Record::of("shop-orders", "id-1"),
    );
    lock.record::<D1>(
        "default",
        "shop",
        "D1_GONE",
        D1Record::of("shop-gone", "id-2"),
    );
    let again = plan_of(&order, &declared, &lock);
    assert_eq!(again.contexts[0].d1[0].action, "reuse");
    let report = plan_report(&again, DeployFormat::Short);
    assert!(
        report.contains("d1 reuse shop-orders (D1_ORDERS)\n")
            && report.contains("orphan d1 shop D1_GONE (shop-gone; not deleted)\n"),
        "{report}"
    );
}

#[test]
fn the_plan_says_which_databases_have_migrations_to_apply() {
    // P4b (#2608): offline, so the files, not which of them are pending.
    let mut resources = Resources::default()
        .d1_databases(&[("D1_ORDERS", "shop-orders"), ("D1_PLAIN", "shop-plain")]);
    resources.d1_databases[0].migrations = vec!["0001_create.sql".into(), "0002_more.sql".into()];
    let declared = project(vec![("shop", resources)]);
    let order = names(&["shop"]);
    let plan = plan_of(&order, &declared, &DeployLock::default());
    let report = plan_report(&plan, DeployFormat::Short);
    assert!(
        report.contains(
            "d1 create shop-orders (D1_ORDERS)\nd1 migrate shop-orders (2 files; pending ones applied before the push)\n"
        ) && !report.contains("d1 migrate shop-plain"),
        "{report}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert_eq!(
        json["contexts"][0]["d1"][0]["migrations"],
        serde_json::json!(["0001_create.sql", "0002_more.sql"])
    );
    assert!(json["contexts"][0]["d1"][1].get("migrations").is_none());
}

#[test]
fn two_instances_that_would_share_a_database_are_refused() {
    // In one build: `shop.cart`'s `Orders` and `shop`'s `CartOrders`.
    let colliding = project(vec![
        (
            "shop-cart",
            Resources::default().d1_databases(&[("D1_ORDERS", "shop-cart-orders")]),
        ),
        (
            "shop",
            Resources::default().d1_databases(&[("D1_CART_ORDERS", "shop-cart-orders")]),
        ),
    ]);
    let message = d1_database_problem(
        &colliding,
        LedgerView::of(&DeployLock::default(), "default"),
    )
    .unwrap();
    assert!(message.contains("`shop-cart-orders`"), "{message}");
    // Across environments: `OrdersStaging` by default is `Orders` under
    // `--env staging`, which the ledger holds.
    let suffixed = project(vec![(
        "shop",
        Resources::default().d1_databases(&[("D1_ORDERS_STAGING", "shop-orders-staging")]),
    )]);
    let mut lock = DeployLock::default();
    lock.record::<D1>(
        "staging",
        "shop",
        "D1_ORDERS",
        D1Record::of("shop-orders-staging", "id-1"),
    );
    let message = d1_database_problem(&suffixed, LedgerView::of(&lock, "default")).unwrap();
    assert!(message.contains("environment `staging`"), "{message}");
    // The database's own record is no clash.
    let own = project(vec![(
        "shop",
        Resources::default().d1_databases(&[("D1_ORDERS", "shop-orders")]),
    )]);
    assert_eq!(
        d1_database_problem(&own, LedgerView::of(&lock, "staging")),
        None
    );
}

#[test]
fn a_recreate_without_yes_is_refused_and_with_yes_is_not() {
    let declared = shop_with_buckets();
    let mut lock = DeployLock::default();
    lock.record::<R2>(
        "default",
        "shop",
        "R2_ARCHIVES",
        R2Bucket::of("shop-archives", None),
    );
    lock.record::<R2>(
        "default",
        "shop",
        "R2_INDEXES",
        R2Bucket::of("shop-indexes", Some("fedramp")),
    );
    let order = names(&["shop"]);
    let plan = plan_of(&order, &declared, &lock);
    assert!(recreate_refusal(&plan, false).is_some());
    assert!(recreate_refusal(&plan, true).is_none());
    // Nothing to recreate, nothing to refuse.
    let fresh = plan_of(&order, &declared, &DeployLock::default());
    assert!(recreate_refusal(&fresh, false).is_none());
}

#[test]
fn a_bucket_a_changed_jurisdiction_left_behind_is_reported_as_an_orphan() {
    let declared = shop_with_buckets();
    let mut lock = DeployLock::default();
    lock.record::<R2>(
        "default",
        "shop",
        "R2_INDEXES",
        R2Bucket::of("shop-indexes", Some("fedramp")),
    );
    lock.record::<R2>(
        "default",
        "shop",
        "R2_INDEXES",
        R2Bucket::of("shop-indexes", Some("eu")),
    );
    let order = names(&["shop"]);
    let report = plan_report(&plan_of(&order, &declared, &lock), DeployFormat::Short);
    assert!(
        report.contains(
            "orphan r2 shop R2_INDEXES (shop-indexes, jurisdiction fedramp; not deleted)\n"
        ),
        "{report}"
    );
}

#[test]
fn an_orphaned_bucket_is_shown_with_its_name_and_kept() {
    let declared = project(vec![("shop", Resources::default())]);
    let mut lock = DeployLock::default();
    lock.record::<R2>("default", "shop", "R2_OLD", R2Bucket::of("shop-old", None));
    let order = names(&["shop"]);
    let plan = plan_of(&order, &declared, &lock);
    let report = plan_report(&plan, DeployFormat::Short);
    assert!(
        report.contains("orphan r2 shop R2_OLD (shop-old; not deleted)\n"),
        "{report}"
    );
    assert!(!plan.orphans.has_prunable());
}

#[test]
fn a_queue_name_that_overflows_once_qualified_is_refused_by_the_plan() {
    // #3877: a consumer's, producer's or dead-letter queue's name gains `-<environment>`.
    let edge = "a".repeat(55);
    let resources = project(vec![(
        "shop",
        Resources::default()
            .consumes(&["jobs"])
            .produces(&[edge.as_str()]),
    )]);
    assert!(queue_name_problem(&resources, "default").is_none());
    assert!(
        queue_name_problem(&resources, "staging").is_none(),
        "55 + `-staging` is exactly 63"
    );
    let longer = "a".repeat(56);
    let resources = project(vec![(
        "shop",
        Resources::default().produces(&[longer.as_str()]),
    )]);
    let message = queue_name_problem(&resources, "staging").expect("56 + `-staging` is 64");
    assert!(message.contains("environment `staging`"), "{message}");
    assert!(message.contains("64 characters"), "{message}");
    assert!(queue_name_problem(&resources, "default").is_none());
}

#[test]
fn a_derived_name_r2_would_refuse_is_an_error_naming_the_instance() {
    let lock = DeployLock::default();
    let long = project(vec![(
        "a-very-long-context-name-that-goes-on-and-on",
        Resources::default().r2_buckets(&[(
            "R2_SOME_RATHER_LONG_CAPABILITY_NAME",
            "a-very-long-context-name-that-goes-on-and-on-some-rather-long-capability-name",
            None,
        )]),
    )]);
    let message = r2_bucket_problem(&long, LedgerView::of(&lock, "default")).expect("too long");
    assert!(
        message.contains("`R2_SOME_RATHER_LONG_CAPABILITY_NAME`"),
        "{message}"
    );
    assert!(message.contains("3 to 63 characters"), "{message}");
    // The environment suffix counts toward the limit.
    let edge = project(vec![(
        "shop",
        Resources::default().r2_buckets(&[("R2_X", &"a".repeat(60), None)]),
    )]);
    assert!(r2_bucket_problem(&edge, LedgerView::of(&lock, "default")).is_none());
    assert!(r2_bucket_problem(&edge, LedgerView::of(&lock, "production")).is_some());
    let underscore = project(vec![(
        "shop_front",
        Resources::default().r2_buckets(&[("R2_X", "shop_front-x", None)]),
    )]);
    assert!(
        r2_bucket_problem(&underscore, LedgerView::of(&lock, "default"))
            .unwrap()
            .contains("lowercase letters, digits and hyphens")
    );
}

#[test]
fn two_instances_that_would_share_a_dataset_are_refused() {
    // P3b (#2582): `shop.cart`'s `Signups` and `shop`'s `CartSignups`.
    let colliding = project(vec![
        (
            "shop-cart",
            Resources::default().analytics(&[("ANALYTICS_SIGNUPS", "shop_cart_signups")]),
        ),
        (
            "shop",
            Resources::default().analytics(&[("ANALYTICS_CART_SIGNUPS", "shop_cart_signups")]),
        ),
    ]);
    let message = analytics_dataset_problem(&colliding, &DeployLock::default(), "default")
        .expect("a collision");
    assert!(message.contains("`shop_cart_signups`"), "{message}");
    // `SignupsStaging` by default meets `Signups` under `--env staging`,
    // once the ledger knows the project deploys to `staging`.
    let with_queue_in = |environment: &str| {
        let mut lock = DeployLock::default();
        QueueTable::created(
            lock.environments
                .entry(String::from(environment))
                .or_default(),
            "jobs",
        );
        lock
    };
    let suffixed = project(vec![(
        "shop",
        Resources::default().analytics(&[
            ("ANALYTICS_SIGNUPS", "shop_signups"),
            ("ANALYTICS_SIGNUPS_STAGING", "shop_signups_staging"),
        ]),
    )]);
    assert_eq!(
        analytics_dataset_problem(&suffixed, &DeployLock::default(), "default"),
        None
    );
    let message = analytics_dataset_problem(&suffixed, &with_queue_in("staging"), "default")
        .expect("a collision across environments");
    assert!(message.contains("`shop_signups_staging`"), "{message}");
    // Distinct names, in any environment, are fine.
    let distinct = project(vec![(
        "shop",
        Resources::default().analytics(&[("ANALYTICS_SIGNUPS", "shop_signups")]),
    )]);
    assert_eq!(
        analytics_dataset_problem(&distinct, &with_queue_in("staging"), "default"),
        None
    );
}

#[test]
fn two_instances_that_would_share_a_bucket_name_are_refused() {
    let colliding = project(vec![
        (
            "shop-cart",
            Resources::default().r2_buckets(&[("R2_A", "shop-cart-a", None)]),
        ),
        (
            "shop",
            Resources::default().r2_buckets(&[("R2_CART_A", "shop-cart-a", None)]),
        ),
    ]);
    let message = r2_bucket_problem(
        &colliding,
        LedgerView::of(&DeployLock::default(), "default"),
    )
    .expect("a collision");
    assert!(message.contains("`shop-cart-a`"), "{message}");
    // Another environment's bucket holding the name is the same clash.
    let mut lock = DeployLock::default();
    lock.record::<R2>(
        "production",
        "shop",
        "R2_OTHER",
        R2Bucket::of("shop-archives-staging", None),
    );
    let staging = project(vec![(
        "shop",
        Resources::default().r2_buckets(&[("R2_ARCHIVES", "shop-archives", None)]),
    )]);
    assert!(r2_bucket_problem(&staging, LedgerView::of(&lock, "staging")).is_some());
    assert!(r2_bucket_problem(&staging, LedgerView::of(&lock, "default")).is_none());
}

// ---- #3301: Secrets Store secrets are linked, never created ----

#[test]
fn a_stored_secret_is_a_linked_plan_line_with_no_action() {
    let resources = project(vec![(
        "shop-billing",
        Resources::default().stored_secrets(&[("SECRET_STRIPE_KEY", "payments", "stripe-live")]),
    )]);
    let order = names(&["shop-billing"]);
    let plan = plan_of(&order, &resources, &DeployLock::default());
    assert_eq!(
        plan_report(&plan, DeployFormat::Short),
        "stored secret SECRET_STRIPE_KEY stripe-live in store payments (linked; not created)\ndeploy shop-billing\n"
    );
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert_eq!(
        json["contexts"][0]["stored_secrets"],
        serde_json::json!([{
            "binding": "SECRET_STRIPE_KEY",
            "store": "payments",
            "secret": "stripe-live"
        }])
    );
    // A project without one has no such key, so the machine surface of
    // every existing project is unchanged.
    let plain = project(vec![("shop-billing", Resources::default())]);
    let plan = plan_of(&order, &plain, &DeployLock::default());
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert!(json["contexts"][0].get("stored_secrets").is_none());
}

// ---- #3513 A: mTLS certificates are linked, never uploaded ----

#[test]
fn an_mtls_certificate_is_a_linked_plan_line_with_no_action() {
    let resources = project(vec![(
        "shop-backend",
        Resources::default().mtls_certificates(&[("MTLS_BACKEND", "acme-client")]),
    )]);
    let order = names(&["shop-backend"]);
    let plan = plan_of(&order, &resources, &DeployLock::default());
    assert_eq!(
        plan_report(&plan, DeployFormat::Short),
        "will link mtls-certificate acme-client (MTLS_BACKEND; not created; not checked in a dry run)\ndeploy shop-backend\n"
    );
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert_eq!(
        json["contexts"][0]["mtls_certificates"],
        serde_json::json!([{
            "binding": "MTLS_BACKEND",
            "certificate": "acme-client"
        }])
    );
    // A project without one has no such key, so its machine surface is unchanged.
    let plain = project(vec![("shop-backend", Resources::default())]);
    let plan = plan_of(&order, &plain, &DeployLock::default());
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert!(json["contexts"][0].get("mtls_certificates").is_none());
}

// ---- #3480: AI Search instances are linked, never created ----

#[test]
fn an_ai_search_instance_is_a_linked_plan_line_with_no_action() {
    let resources = project(vec![(
        "shop-ask",
        Resources::default().ai_search(&[("AI_SEARCH_KB", "support-docs")]),
    )]);
    let order = names(&["shop-ask"]);
    let plan = plan_of(&order, &resources, &DeployLock::default());
    assert_eq!(
        plan_report(&plan, DeployFormat::Short),
        "will link ai-search support-docs (AI_SEARCH_KB; not created; not checked in a dry run)\ndeploy shop-ask\n"
    );
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert_eq!(
        json["contexts"][0]["ai_search"],
        serde_json::json!([{
            "binding": "AI_SEARCH_KB",
            "instance": "support-docs"
        }])
    );
    // A project without one has no such key, so the machine surface of
    // every existing project is unchanged.
    let plain = project(vec![("shop-ask", Resources::default())]);
    let plan = plan_of(&order, &plain, &DeployLock::default());
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert!(json["contexts"][0].get("ai_search").is_none());
}

// ---- #3328: Email Routing rules, reconciled by wrangler ----

#[test]
fn an_email_address_is_an_advisory_plan_line() {
    let resources = project(vec![(
        "shop-support",
        Resources::default().email_addresses(&["*@shop.example", "support@shop.example"]),
    )]);
    let order = names(&["shop-support"]);
    let plan = plan_of(&order, &resources, &DeployLock::default());
    assert_eq!(
        plan_report(&plan, DeployFormat::Short),
        "email address *@shop.example (Email Routing rule; advisory — wrangler reconciles it)\nemail address support@shop.example (Email Routing rule; advisory — wrangler reconciles it)\ndeploy shop-support\n"
    );
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert_eq!(
        json["contexts"][0]["email_addresses"],
        serde_json::json!(["*@shop.example", "support@shop.example"])
    );
    // A project with no email service has no such key.
    let plain = project(vec![("shop-support", Resources::default())]);
    let plan = plan_of(&order, &plain, &DeployLock::default());
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert!(json["contexts"][0].get("email_addresses").is_none());
}

/// #3328 (ADR 0805 F): the Email Routing gate. The first deploy records
/// without asking; an added address asks nothing; a removed one is refused
/// before anything is pushed unless `--prune` confirms it, and a push records
/// what it carried.
#[test]
fn removing_an_email_address_is_refused_unless_pruned() {
    let order = names(&["shop-support"]);
    let both = project(vec![(
        "shop-support",
        Resources::default().email_addresses(&["help@shop.example", "support@shop.example"]),
    )]);
    let one = project(vec![(
        "shop-support",
        Resources::default().email_addresses(&["support@shop.example"]),
    )]);
    let mut lock = DeployLock::default();

    // The first deploy: nothing recorded, so nothing is removed.
    let plan = plan_of(&order, &both, &lock);
    assert!(confirm_address_removals(&plan, false, false));
    record_pushed_tombstones(&mut lock, "default", "shop-support", &both["shop-support"]);

    // The same list again, and an added address, ask nothing.
    assert!(confirm_address_removals(
        &plan_of(&order, &both, &lock),
        false,
        false
    ));
    let added = project(vec![(
        "shop-support",
        Resources::default().email_addresses(&[
            "help@shop.example",
            "sales@shop.example",
            "support@shop.example",
        ]),
    )]);
    assert!(confirm_address_removals(
        &plan_of(&order, &added, &lock),
        false,
        false
    ));

    // A removal, unconfirmed: refused, and the plan names it.
    let removal = plan_of(&order, &one, &lock);
    assert!(!confirm_address_removals(&removal, false, false));
    assert!(
        !confirm_address_removals(&removal, false, true),
        "--yes alone is not --prune"
    );
    assert_eq!(
        plan_report(&removal, DeployFormat::Short),
        "email address support@shop.example (Email Routing rule; advisory — wrangler reconciles it)\nemail address help@shop.example removed (its Email Routing rule is deleted by the push; needs --prune)\ndeploy shop-support\n"
    );
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&removal, DeployFormat::Json)).unwrap();
    assert_eq!(
        json["contexts"][0]["removed_email_addresses"],
        serde_json::json!(["help@shop.example"])
    );

    // A removal, confirmed: allowed, and the push records the new list.
    assert!(confirm_address_removals(&removal, true, true));
    record_pushed_tombstones(&mut lock, "default", "shop-support", &one["shop-support"]);
    assert!(confirm_address_removals(
        &plan_of(&order, &one, &lock),
        false,
        false
    ));

    // Dropping every `from email` service removes the rest.
    let none = project(vec![("shop-support", Resources::default())]);
    assert!(!confirm_address_removals(
        &plan_of(&order, &none, &lock),
        false,
        false
    ));
}

// ---- interop I5 (#1972): Service Bindings to foreign Workers ----

#[test]
fn a_foreign_service_is_a_plan_line_with_no_action() {
    let resources = project(vec![(
        "shop-billing",
        Resources::default().foreign(&[("SERVICE_BILLING", "legacy-billing")]),
    )]);
    let order = names(&["shop-billing"]);
    let plan = plan_of(&order, &resources, &DeployLock::default());
    assert_eq!(
        plan_report(&plan, DeployFormat::Short),
        // I2 (#1971): a binding to a foreign Worker is no edge, so it
        // carries no edge key: the foreign Worker is not Bynk's and
        // never receives one.
        "service SERVICE_BILLING legacy-billing (foreign Worker; not provisioned)\ndeploy shop-billing\n"
    );
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert_eq!(
        json["contexts"][0]["foreign_services"],
        serde_json::json!([{ "binding": "SERVICE_BILLING", "worker": "legacy-billing" }])
    );
    assert_eq!(json["contexts"][0]["binds_to"], serde_json::json!([]));
}

/// #3386: a `SignedCookie` actor's signing key is a managed secret in the plan: `deploy` mints
/// it, so it is listed (as `managed`) without the operator naming it.
#[test]
fn a_session_key_is_a_managed_secret_in_the_plan() {
    let resources = project(vec![(
        "shop-api",
        Resources::default().session_keys(&[("User", "SESSION_KEY")]),
    )]);
    let order = names(&["shop-api"]);
    let plan = plan_of(&order, &resources, &DeployLock::default());
    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    let secrets = json["contexts"][0]["secrets"].clone();
    assert_eq!(
        secrets,
        serde_json::json!([{ "name": "SESSION_KEY", "origin": "managed", "action": "set" }])
    );
}

/// Track §11 Q8: warn, never refuse. Each Worker is asked once however
/// many contexts bind it; a present one says nothing, a missing one says
/// Cloudflare will reject the upload, and an unreadable answer is a
/// warning of its own rather than a guess either way.
#[test]
fn foreign_worker_warnings_ask_once_and_never_refuse() {
    let resources = project(vec![
        (
            "a",
            Resources::default().foreign(&[("SERVICE_BILLING", "legacy-billing")]),
        ),
        (
            "b",
            Resources::default().foreign(&[
                ("SERVICE_BILLING", "legacy-billing"),
                ("SERVICE_LEDGER", "ledger"),
                ("SERVICE_TAX", "tax"),
            ]),
        ),
    ]);
    let order = names(&["a", "b"]);
    let mut asked = Vec::new();
    let warnings = foreign_worker_warnings(&order, &resources, |name| {
        asked.push(name.to_string());
        match name {
            "legacy-billing" => WorkerPresence::Missing,
            "ledger" => WorkerPresence::Present,
            _ => WorkerPresence::Unknown("Authentication error [code: 10000]".into()),
        }
    });
    assert_eq!(asked, ["ledger", "legacy-billing", "tax"]);
    assert_eq!(warnings.len(), 2);
    assert!(
        warnings[0].contains("`a`, `b` binds to Worker `legacy-billing`")
            && warnings[0].contains("rejects a Service Binding to a missing Worker at upload"),
        "{}",
        warnings[0]
    );
    assert!(
        warnings[1].contains("could not check that Worker `tax`")
            && warnings[1].contains("[code: 10000]"),
        "{}",
        warnings[1]
    );
}

/// Only Cloudflare's "Worker not found" codes read as missing.
#[test]
fn worker_presence_reads_only_not_found_as_missing() {
    use crate::cloudflare::fake::{Recording, failed};
    let answering = |stderr: &'static str| {
        Recording::new(move |op| {
            if stderr.is_empty() {
                Ok(String::new())
            } else {
                Err(failed(op, "", stderr))
            }
        })
    };
    assert_eq!(
        worker_presence(&answering(""), "billing"),
        WorkerPresence::Present
    );
    assert_eq!(
        worker_presence(
            &answering(
                "A request to the Cloudflare API (/accounts/x/workers/scripts/legacy-billing/deployments) failed.\n  This Worker does not exist on your account. [code: 10007]"
            ),
            "billing"
        ),
        WorkerPresence::Missing
    );
    assert_eq!(
        worker_presence(&answering("… [code: 10090]"), "billing"),
        WorkerPresence::Missing
    );
    assert!(matches!(
        worker_presence(&answering("fetch failed"), "billing"),
        WorkerPresence::Unknown(_)
    ));
    let missing = Recording::new(|op| Err(crate::cloudflare::WranglerError::missing(op)));
    assert_eq!(
        worker_presence(&missing, "billing"),
        WorkerPresence::Unknown("wrangler not found".into())
    );
}

// ---- v0.177 (#643): the deploy-time contract-skew gate ----

fn lock_with(worker: &str, contracts: &[(&str, &str)]) -> DeployLock {
    let mut lock = DeployLock::default();
    lock.record_deployed(
        "default",
        worker,
        Some(
            contracts
                .iter()
                .map(|(s, h)| (s.to_string(), h.to_string()))
                .collect(),
        ),
    );
    lock
}

fn expects(dep: &str, svc: &str, hash: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    BTreeMap::from([(
        dep.to_string(),
        BTreeMap::from([(svc.to_string(), hash.to_string())]),
    )])
}

#[test]
fn conflicting_env_passthrough_finds_bare_and_equals_forms() {
    assert_eq!(
        conflicting_env_passthrough(&names(&["--env", "production"])),
        Some("--env")
    );
    assert_eq!(
        conflicting_env_passthrough(&names(&["--env=production"])),
        Some("--env=production")
    );
    assert_eq!(
        conflicting_env_passthrough(&names(&["--environment", "production"])),
        Some("--environment")
    );
    assert_eq!(
        conflicting_env_passthrough(&names(&["--minify"])),
        None,
        "an unrelated flag is not a conflict"
    );
    assert_eq!(conflicting_env_passthrough(&[]), None);
}

#[test]
fn plan_creates_or_reuses_kv_from_the_ledger() {
    let order = names(&["api"]);
    let declared = project(vec![("api", Resources::default().needs_kv())]);
    let fresh = DeployLock::default();
    assert_eq!(
        plan_of(&order, &declared, &fresh).contexts[0].kv[0].action,
        "create"
    );
    assert_eq!(
        plan_of(&order, &declared, &with_kv(DeployLock::default(), "api")).contexts[0].kv[0].action,
        "reuse"
    );
    assert!(
        plan_of(
            &order,
            &project(vec![("api", Resources::default())]),
            &fresh
        )
        .contexts[0]
            .kv
            .is_empty(),
        "a context declaring no KV gets no KV line"
    );
}

// ---- #600 slice 1: queues and Durable Objects ----------------------

#[test]
fn plan_creates_or_reuses_a_queue_by_its_name() {
    // Queues reconcile on the name `from queue("n")` gave them — there is
    // no id — so the ledger's whole answer is "have we made this before?"
    let order = names(&["jobs"]);
    let declared = project(vec![("jobs", Resources::default().consumes(&["intake"]))]);
    let line = |lock: &DeployLock| plan_of(&order, &declared, lock).contexts[0].queues[0].action;
    assert_eq!(line(&DeployLock::default()), "create");
    assert_eq!(line(&with_queue(DeployLock::default(), "intake")), "reuse");
    // The name is keyed environment-wide, not per worker: a different
    // context consuming `intake` means the same queue.
    assert!(
        with_queue(DeployLock::default(), "intake")
            .recorded::<Queue>("default", "", "intake")
            .is_some()
    );
    assert!(
        with_queue(DeployLock::default(), "intake")
            .recorded::<Queue>("default", "", "other")
            .is_none()
    );
}

#[test]
fn an_unrecorded_queue_only_sent_to_may_be_created_or_reused() {
    // P2 (#2578): an adopted producer-only queue is never recorded, so
    // the plan cannot claim it will be created.
    let order = names(&["shop"]);
    let declared = project(vec![("shop", Resources::default().produces(&["events"]))]);
    let line = |lock: &DeployLock| plan_of(&order, &declared, lock).contexts[0].queues[0].action;
    assert_eq!(line(&DeployLock::default()), "create or reuse");
    assert_eq!(line(&with_queue(DeployLock::default(), "events")), "reuse");
    // Consumed as well as sent to, it is recorded either way: `create`.
    let both = project(vec![(
        "shop",
        Resources::default()
            .consumes(&["events"])
            .produces(&["events"]),
    )]);
    assert_eq!(
        plan_of(&order, &both, &DeployLock::default()).contexts[0].queues[0].action,
        "create"
    );
}

#[test]
fn a_dataset_is_listed_under_its_qualified_name_with_no_action() {
    // P3b (#2582): nothing to create or record, so the line only names it.
    let order = names(&["shop"]);
    let declared = project(vec![(
        "shop",
        Resources::default().analytics(&[("ANALYTICS_SIGNUPS", "shop_signups")]),
    )]);
    let plan = derive_plan(
        &order,
        &declared,
        &DeployLock::default(),
        &SecretSource::default(),
        false,
        "staging",
    );
    assert_eq!(plan.contexts[0].datasets[0].dataset, "shop_signups_staging");
    assert!(
        plan_report(&plan, DeployFormat::Short)
            .contains("dataset shop_signups_staging (ANALYTICS_SIGNUPS; created on first write)\n")
    );
}

#[test]
fn a_context_with_no_queues_gets_no_queue_lines() {
    assert!(
        plan_of(
            &names(&["api"]),
            &project(vec![("api", Resources::default())]),
            &DeployLock::default(),
        )
        .contexts[0]
            .queues
            .is_empty()
    );
}

#[test]
fn a_passthrough_secrets_file_is_refused_only_when_the_run_has_a_first_deploy() {
    // #2055: a first deploy passes its own `--secrets-file`.
    let order = names(&["api", "web"]);
    let mut lock = DeployLock::default();
    lock.record_deployed("staging", "api", None);
    let args = |a: &[&str]| names(a);
    assert_eq!(
        secrets_file_passthrough_refusal(
            &args(&["--secrets-file", "s.env"]),
            &order,
            &lock,
            "staging"
        )
        .as_deref(),
        Some(
            "bynk: error: `--secrets-file` after `--` conflicts with the secrets file `bynk deploy` uploads with a new Worker's first version (`web-staging`).\n  \
             Pass it to bynk instead, before `--`: `bynk deploy --secrets-file <file>`.\n"
        )
    );
    assert!(
        secrets_file_passthrough_refusal(
            &args(&["--secrets-file=s.env"]),
            &order,
            &lock,
            "staging"
        )
        .is_some_and(|r| r.contains("`--secrets-file=s.env`")),
        "the `=`-joined spelling is the same flag"
    );
    lock.record_deployed("staging", "web", None);
    assert_eq!(
        secrets_file_passthrough_refusal(
            &args(&["--secrets-file", "s.env"]),
            &order,
            &lock,
            "staging"
        ),
        None,
        "a redeploy passes no secrets file of its own"
    );
    assert_eq!(
        secrets_file_passthrough_refusal(&args(&["--minify"]), &order, &lock, "default"),
        None
    );
}

#[test]
fn a_wrangler_below_the_floor_refuses_only_a_push_that_declares_a_durable_object() {
    // #1796: `exports` is read from WRANGLER_MIN on, so below it a context
    // with an agent can't deploy; one without still can.
    let min = probe::Version::parse(bynk_emit::WRANGLER_MIN).unwrap();
    let wrangler = |version: probe::Version| probe::Probe {
        tool: "wrangler".to_string(),
        version: Some(version),
        provenance: probe::Provenance::Path("/usr/bin/wrangler".into()),
    };
    // The newest version strictly below the floor, whatever the floor is.
    let old = if min.patch > 0 {
        probe::Version {
            patch: min.patch - 1,
            ..min
        }
    } else if min.minor > 0 {
        probe::Version {
            minor: min.minor - 1,
            patch: 999,
            ..min
        }
    } else {
        probe::Version {
            major: min.major - 1,
            minor: 999,
            patch: 999,
        }
    };
    let resources = project(vec![
        ("api", Resources::default()),
        ("jobs", Resources::default().exports(&["JobLedger"])),
    ]);
    let both = names(&["api", "jobs"]);

    let refusal = wrangler_floor_refusal(&wrangler(old), PackageManager::Npm, &both, &resources)
        .expect("an agent-bearing push below the floor is refused");
    for part in [
        format!("wrangler {old}"),
        bynk_emit::WRANGLER_MIN.to_string(),
        "`jobs`".to_string(),
        "npm install -g wrangler@4".to_string(),
    ] {
        assert!(refusal.contains(&part), "missing {part:?} in: {refusal}");
    }
    // The upgrade command follows the detected package manager.
    let pnpm = wrangler_floor_refusal(&wrangler(old), PackageManager::Pnpm, &both, &resources)
        .expect("refused");
    assert!(pnpm.contains("`pnpm add -g wrangler@4`") && !pnpm.contains("npm install"));

    // No class in what's pushed (`--context api`), the floor itself, or an
    // unversioned npx wrangler: nothing to refuse.
    assert_eq!(
        wrangler_floor_refusal(
            &wrangler(old),
            PackageManager::Npm,
            &names(&["api"]),
            &resources
        ),
        None
    );
    assert_eq!(
        wrangler_floor_refusal(&wrangler(min), PackageManager::Npm, &both, &resources),
        None
    );
    let npx = probe::Probe {
        tool: "wrangler".to_string(),
        version: None,
        provenance: probe::Provenance::Runner(PackageManager::Npm),
    };
    assert_eq!(
        wrangler_floor_refusal(&npx, PackageManager::Npm, &both, &resources),
        None
    );
}

/// State-migrations S3 (#2248): a rename and a deletion each get a plan
/// line saying what happens to stored data; a deletion is named as
/// `<context>.<Class>`, the form `--confirm-delete` takes.
#[test]
fn renames_and_deletions_are_planned_by_what_they_do_to_data() {
    let order = names(&["shop-orders"]);
    let declared = Resources {
        renamed_durable_objects: vec![("Cart".to_string(), "Basket".to_string())],
        deleted_durable_objects: vec!["Wishlist".to_string()],
        ..Resources::default().exports(&["Basket"])
    };
    let resources = project(vec![("shop-orders", declared)]);
    let plan = plan_of(&order, &resources, &DeployLock::default());
    let context = &plan.contexts[0];
    assert_eq!(context.deleted_durable_objects.len(), 1);
    assert_eq!(
        context.deleted_durable_objects[0].class,
        "shop.orders.Wishlist"
    );
    assert!(!context.deleted_durable_objects[0].already_applied);
    let text = plan_report(&plan, DeployFormat::Short);
    assert!(
        text.contains("rename durable object Cart → Basket (moves its stored data)"),
        "{text}"
    );
    assert!(
        text.contains("delete durable object shop.orders.Wishlist (all stored data; irreversible)"),
        "{text}"
    );
    let json = plan_report(&plan, DeployFormat::Json);
    assert!(json.contains("\"durable_objects_renamed\""), "{json}");
    assert!(json.contains("\"shop.orders.Wishlist\""), "{json}");
}

/// #2279 review: what a push records is what the next plan reads, for
/// renames and deletions alike, and a later push without them clears both.
#[test]
fn a_push_records_its_tombstones_for_the_next_plan() {
    let order = names(&["shop"]);
    let declared = Resources {
        renamed_durable_objects: vec![("Box".to_string(), "Crate".to_string())],
        deleted_durable_objects: vec!["Wishlist".to_string()],
        ..Resources::default().exports(&["Crate"])
    };
    let mut lock = DeployLock::default();
    record_pushed_tombstones(&mut lock, "default", "shop", &declared);
    let resources = project(vec![("shop", declared)]);
    let plan = plan_of(&order, &resources, &lock);
    assert!(plan.contexts[0].renamed_durable_objects[0].already_applied);
    assert!(plan.contexts[0].deleted_durable_objects[0].already_applied);
    record_pushed_tombstones(&mut lock, "default", "shop", &Resources::default());
    assert!(!lock.has_renamed_class("default", "shop", "Box", "Crate"));
    assert!(!lock.has_deleted_class("default", "shop", "Wishlist"));
}

/// #2276 Decision D: a rename an earlier push carried reads as applied;
/// a different target, or another environment, still reads as moving data.
#[test]
fn a_rename_already_pushed_reads_as_applied() {
    let order = names(&["shop"]);
    let declared = Resources {
        renamed_durable_objects: vec![("Box".to_string(), "Crate".to_string())],
        ..Resources::default().exports(&["Crate"])
    };
    let resources = project(vec![("shop", declared)]);
    let mut lock = DeployLock::default();
    lock.record_renamed_classes(
        "default",
        "shop",
        [("Box".to_string(), "Crate".to_string())].into(),
    );
    let plan = plan_of(&order, &resources, &lock);
    assert!(plan.contexts[0].renamed_durable_objects[0].already_applied);
    let text = plan_report(&plan, DeployFormat::Short);
    assert!(
        text.contains("rename durable object Box → Crate (already applied by an earlier deploy; the tombstone stays until Cloudflare reports it stale)"),
        "{text}"
    );
    let json = plan_report(&plan, DeployFormat::Json);
    assert!(json.contains("\"already_applied\": true"), "{json}");
    lock.record_renamed_classes(
        "default",
        "shop",
        [("Box".to_string(), "Bin".to_string())].into(),
    );
    let plan = plan_of(&order, &resources, &lock);
    assert!(!plan.contexts[0].renamed_durable_objects[0].already_applied);
    lock.record_renamed_classes("default", "shop", BTreeMap::new());
    assert!(!lock.has_renamed_class("default", "shop", "Box", "Bin"));
    assert!(
        !toml::to_string(&lock).unwrap().contains("renamed_classes"),
        "an empty record leaves nothing in the ledger"
    );
}

/// State-migrations S3 (#2248, Decision E): a deletion needs a named
/// confirmation; `--yes` never stands in, and a confirmation for a class
/// the plan does not delete is refused. (Tests run without a terminal, so
/// the interactive prompt is not reached.)
#[test]
fn a_deletion_needs_its_own_named_confirmation() {
    let order = names(&["shop"]);
    let declared = Resources {
        deleted_durable_objects: vec!["Wishlist".to_string()],
        ..Resources::default()
    };
    let resources = project(vec![("shop", declared)]);
    let lock = DeployLock::default();
    let plan = plan_of(&order, &resources, &lock);
    assert!(
        !confirm_deletions(&plan, &[], &lock),
        "unconfirmed: refused"
    );
    assert!(confirm_deletions(&plan, &names(&["shop.Wishlist"]), &lock));
    assert!(
        !confirm_deletions(&plan, &names(&["shop.Wishlist", "shop.Other"]), &lock),
        "a confirmation for a class not deleted is refused"
    );
    let none = project(vec![("shop", Resources::default())]);
    let quiet = plan_of(&order, &none, &lock);
    assert!(
        confirm_deletions(&quiet, &[], &lock),
        "nothing to delete, nothing to confirm"
    );
    assert!(!confirm_deletions(
        &quiet,
        &names(&["shop.Wishlist"]),
        &lock
    ));
}

/// #2253 review: the `removed agent` declaration stays until Cloudflare
/// reports its tombstone stale, so the deploy after the deletion carries
/// the tombstone again. The ledger records the first push's deletion, so
/// the second plan shows it as applied and asks nothing; a CI flag still
/// naming it — even after the declaration is gone — is not refused.
#[test]
fn a_deletion_already_pushed_is_not_confirmed_again() {
    let order = names(&["shop"]);
    let declared = Resources {
        deleted_durable_objects: vec!["Wishlist".to_string()],
        ..Resources::default()
    };
    let resources = project(vec![("shop", declared)]);
    let mut lock = DeployLock::default();
    lock.record_deleted_classes("default", "shop", ["Wishlist".to_string()].into());
    let plan = plan_of(&order, &resources, &lock);
    assert!(plan.contexts[0].deleted_durable_objects[0].already_applied);
    let text = plan_report(&plan, DeployFormat::Short);
    assert!(
        text.contains("already deleted by an earlier deploy"),
        "{text}"
    );
    assert!(
        confirm_deletions(&plan, &[], &lock),
        "an applied deletion asks nothing"
    );
    assert!(confirm_deletions(&plan, &names(&["shop.Wishlist"]), &lock));
    // The declaration removed: the plan deletes nothing, and the leftover
    // flag names a deletion the ledger records, so it is not refused.
    let gone = project(vec![("shop", Resources::default())]);
    let after = plan_of(&order, &gone, &lock);
    assert!(confirm_deletions(&after, &names(&["shop.Wishlist"]), &lock));
    assert!(!confirm_deletions(&after, &names(&["shop.Other"]), &lock));
}

#[test]
fn the_durable_object_lines_are_advisory_in_every_ledger_state() {
    // #1796, ADR 0194 D1's principle: Cloudflare reconciles the declared
    // `exports` against the Worker's namespaces, so the plan says what the
    // push will *declare* and never what already exists. A ledger that has
    // deployed this context before must not change the lines — there is no
    // state here for the ledger to have an opinion about.
    let order = names(&["jobs"]);
    let declared = project(vec![(
        "jobs",
        Resources::default().exports(&["JobLedger", "__EventsFanout"]),
    )]);
    for lock in [DeployLock::default(), lock_with_deployed(&["jobs"])] {
        let plan = plan_of(&order, &declared, &lock);
        let objects = &plan.contexts[0].durable_objects;
        assert_eq!(
            objects.iter().map(|o| o.class).collect::<Vec<_>>(),
            ["JobLedger", "__EventsFanout"],
            "every declared class gets a line, the fan-out class included"
        );
        for object in objects {
            assert_eq!(object.storage, "sqlite");
            assert_eq!(
                object.reconciled_by, "Cloudflare",
                "the plan names an owner other than bynk — that is the advisory"
            );
        }
    }
    // No agent, no Durable Object line.
    assert!(
        plan_of(
            &names(&["api"]),
            &project(vec![("api", Resources::default())]),
            &DeployLock::default(),
        )
        .contexts[0]
            .durable_objects
            .is_empty()
    );
}

// ---- #601 slice 2: `--context` dependency liveness (D4) ------------

#[test]
fn context_flag_names_a_dependency_that_was_never_deployed() {
    let g = graph(&[("orders", &["payment"]), ("payment", &[])]);
    assert_eq!(
        absent_dependencies("orders", &g, &DeployLock::default(), "default"),
        names(&["payment"]),
        "deploying orders alone would fail at upload — say which target is missing"
    );
    // Once payment is in the ledger, orders alone is fine.
    assert!(
        absent_dependencies("orders", &g, &lock_with_deployed(&["payment"]), "default").is_empty()
    );
    // A worker with no bindings never has an absent dependency.
    assert!(absent_dependencies("payment", &g, &DeployLock::default(), "default").is_empty());
}

#[test]
fn the_plan_distinguishes_a_first_deploy_from_a_redeploy() {
    let order = names(&["api"]);
    let declared = project(vec![("api", Resources::default())]);
    assert_eq!(
        plan_of(&order, &declared, &DeployLock::default()).contexts[0].action,
        "deploy"
    );
    assert_eq!(
        plan_of(&order, &declared, &lock_with_deployed(&["api"])).contexts[0].action,
        "redeploy",
        "a re-run re-pushes rather than skipping, so the plan must not say `deploy`"
    );
}

#[test]
fn the_plan_carries_the_order_and_each_context_s_bindings() {
    let order = names(&["payment", "orders"]);
    let declared = project(vec![
        ("orders", Resources::default().binds(&["payment"])),
        ("payment", Resources::default()),
    ]);
    let plan = plan_of(&order, &declared, &DeployLock::default());
    assert_eq!(plan.order, vec!["payment", "orders"]);
    assert_eq!(plan.contexts[1].worker, "orders");
    assert_eq!(plan.contexts[1].binds_to, vec!["payment"]);
    assert!(plan.contexts[0].binds_to.is_empty());
}

/// #2028: under `--env staging` the plan names what the push creates —
/// each Worker, binding target and queue `-staging` — while the ledger,
/// keyed by logical name within the environment, still decides each
/// action.
#[test]
fn under_a_named_environment_the_plan_names_what_the_push_creates() {
    let order = names(&["payment", "orders"]);
    let declared = project(vec![
        (
            "orders",
            Resources::default().binds(&["payment"]).consumes(&["jobs"]),
        ),
        ("payment", Resources::default()),
    ]);
    let mut lock = DeployLock::default();
    lock.record_deployed("staging", "payment", Some(Default::default()));
    lock.record_deployed("staging", "gone", Some(Default::default()));
    QueueTable::created(
        lock.environments
            .entry(String::from("staging"))
            .or_default(),
        "jobs",
    );
    QueueTable::created(
        lock.environments
            .entry(String::from("staging"))
            .or_default(),
        "old-jobs",
    );
    let plan = derive_plan(
        &order,
        &declared,
        &lock,
        &SecretSource::default(),
        false,
        "staging",
    );
    assert_eq!(plan.order, vec!["payment-staging", "orders-staging"]);
    assert_eq!(plan.contexts[0].worker, "payment-staging");
    assert_eq!(plan.contexts[0].action, "redeploy", "found by logical name");
    assert_eq!(plan.contexts[1].worker, "orders-staging");
    assert_eq!(plan.contexts[1].action, "deploy");
    assert_eq!(plan.contexts[1].binds_to, vec!["payment-staging"]);
    assert_eq!(plan.contexts[1].queues[0].queue, "jobs-staging");
    assert_eq!(plan.contexts[1].queues[0].action, "reuse");
    // Shown as the account names them; pruned by ledger key.
    assert_eq!(plan.shown_orphans.workers, vec!["gone-staging"]);
    assert_eq!(plan.shown_orphans.queues, vec!["old-jobs-staging"]);
    assert_eq!(plan.orphans.workers, vec!["gone"]);
    assert_eq!(plan.orphans.queues, vec!["old-jobs"]);

    let text = plan_report(&plan, DeployFormat::Short);
    assert!(text.contains("orphan worker gone-staging\n"), "{text}");
    assert!(text.contains("orphan queue old-jobs-staging\n"), "{text}");
    assert!(text.contains("queue reuse jobs-staging\n"), "{text}");
    assert!(text.contains("redeploy payment-staging\n"), "{text}");
    assert!(text.contains("deploy orders-staging\n"), "{text}");
    assert!(
        text.contains("order payment-staging → orders-staging\n"),
        "{text}"
    );

    let json: serde_json::Value =
        serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
    assert_eq!(
        json["order"],
        serde_json::json!(["payment-staging", "orders-staging"])
    );
    assert_eq!(json["contexts"][1]["worker"], "orders-staging");
    assert_eq!(
        json["contexts"][1]["binds_to"],
        serde_json::json!(["payment-staging"])
    );
    assert_eq!(json["contexts"][1]["queues"][0]["queue"], "jobs-staging");
    assert_eq!(
        json["orphans"]["workers"],
        serde_json::json!(["gone-staging"])
    );
    assert_eq!(
        json["orphans"]["queues"],
        serde_json::json!(["old-jobs-staging"])
    );
}

/// #2028: the plan and the push get every name from the same functions.
/// The `[env.staging]` block the push hands Wrangler is synthesised from a
/// generated config, and its `name`, Service Binding targets and queue
/// names are exactly the plan's `worker`, `binds_to` and `queues`.
#[test]
fn the_plan_and_the_push_share_one_name_function() {
    const ORDERS: &str = r#"name = "shop-orders"
main = "index.ts"

[[services]]
binding = "PAYMENT"
service = "shop-payment"

[[queues.consumers]]
queue = "jobs"
"#;
    let order = names(&["shop-payment", "shop-orders"]);
    let declared = project(vec![
        (
            "shop-orders",
            Resources::default()
                .binds(&["shop-payment"])
                .consumes(&["jobs"]),
        ),
        ("shop-payment", Resources::default()),
    ]);
    for environment in ["staging", "production", "default"] {
        let plan = derive_plan(
            &order,
            &declared,
            &DeployLock::default(),
            &SecretSource::default(),
            false,
            environment,
        );
        let orders = &plan.contexts[1];
        let pushed: toml::Table =
            environment_block(ORDERS, environment, &BTreeMap::new(), &BTreeSet::new())
                .unwrap()
                .parse()
                .unwrap();
        let block = &pushed["env"][environment];
        assert_eq!(block["name"].as_str(), Some(orders.worker.as_str()));
        assert_eq!(
            block["services"][0]["service"].as_str(),
            Some(orders.binds_to[0].as_str())
        );
        assert_eq!(
            block["queues"]["consumers"][0]["queue"].as_str(),
            Some(orders.queues[0].queue.as_str())
        );
        assert_eq!(plan.order[1], orders.worker);
        assert_eq!(
            orders.worker,
            deployed_worker_name(environment, "shop-orders")
        );
    }
}

#[test]
fn dry_run_and_ci_gates_do_not_reach_mutation() {
    assert!(
        DeployOptions {
            dry_run: true,
            ..Default::default()
        }
        .dry_run
    );
}

/// Review of #1974: an instance title another Worker's namespace already
/// takes is refused, naming both; distinct titles pass.
#[test]
fn a_kv_title_two_namespaces_would_share_is_refused() {
    let colliding = project(vec![
        (
            "shop",
            Resources::default().kv_instances(&["KV_CART_SESSIONS"]),
        ),
        (
            "shop-cart",
            Resources::default().kv_instances(&["KV_SESSIONS"]),
        ),
    ]);
    let message = kv_title_collision(
        &colliding,
        LedgerView::of(&DeployLock::default(), "default"),
    )
    .expect("a collision");
    assert!(
        message.contains("`shop-cart-sessions`")
            && message.contains("`KV_CART_SESSIONS` for `shop`")
            && message.contains("`KV_SESSIONS` for `shop-cart`"),
        "{message}"
    );
    // An instance title equal to another Worker's bare namespace title.
    let bare = project(vec![
        ("shop", Resources::default().kv_instances(&["KV_CART"])),
        ("shop-cart", Resources::default().needs_kv()),
    ]);
    assert!(kv_title_collision(&bare, LedgerView::of(&DeployLock::default(), "default")).is_some());
    let distinct = project(vec![(
        "shop-cart",
        Resources::default()
            .needs_kv()
            .kv_instances(&["KV_PRICE_CACHE", "KV_SESSIONS"]),
    )]);
    assert_eq!(
        kv_title_collision(&distinct, LedgerView::of(&DeployLock::default(), "staging")),
        None
    );
}

/// #2048: Worker `shop` with the bare `Kv` and `capability Staging`. Its
/// bare namespace under `--env staging` and its instance in the default
/// environment are both titled `shop-staging`.
fn shop_with_staging_instance() -> BTreeMap<String, Resources> {
    project(vec![(
        "shop",
        Resources::default()
            .needs_kv()
            .kv_instances(&["KV_STAGING"]),
    )])
}

/// #2048: a first staging deploy whose bare title the default
/// environment's instance already holds is refused, naming both sides
/// and both environments.
#[test]
fn a_title_another_environment_holds_is_refused() {
    let resources = shop_with_staging_instance();
    let mut lock = DeployLock::default();
    lock.record::<Kv>("default", "shop", "KV", KvNamespace::of("bare-id"));
    lock.record::<Kv>(
        "default",
        "shop",
        "KV_STAGING",
        KvNamespace::of("instance-id"),
    );
    let message =
        kv_title_collision(&resources, LedgerView::of(&lock, "staging")).expect("a collision");
    assert_eq!(
        message,
        "KV namespace for `shop` in `--env staging` and KV namespace `KV_STAGING` for `shop` \
         in the default environment would both be titled `shop-staging` on the account, \
         which holds one namespace per title; rename a capability or context so they differ"
    );
}

/// #2048: the reverse order. A default deploy that would create a title
/// a known staging environment would create is refused too, before
/// either fails at `kv namespace create`.
#[test]
fn a_title_another_environment_would_create_is_refused() {
    let resources = shop_with_staging_instance();
    let mut lock = DeployLock::default();
    // Staging is known to the ledger, but has not provisioned KV yet.
    lock.record_deployed("staging", "other", Some(Default::default()));
    let message =
        kv_title_collision(&resources, LedgerView::of(&lock, "default")).expect("a collision");
    assert!(
        message.contains("KV namespace `KV_STAGING` for `shop` in the default environment")
            && message.contains("KV namespace for `shop` in `--env staging`")
            && message.contains("`shop-staging`"),
        "{message}"
    );
}

/// #2048: a recorded namespace in another environment holds its title
/// even when the source no longer declares it (an orphan not yet
/// pruned).
#[test]
fn an_orphan_in_another_environment_still_holds_its_title() {
    let resources = project(vec![(
        "shop",
        Resources::default().kv_instances(&["KV_STAGING"]),
    )]);
    let mut lock = DeployLock::default();
    lock.record::<Kv>("staging", "shop", "KV", KvNamespace::of("orphaned-id"));
    let message =
        kv_title_collision(&resources, LedgerView::of(&lock, "default")).expect("a collision");
    assert!(
        message.contains("KV namespace for `shop` in `--env staging`"),
        "{message}"
    );
}

/// #2048: only namespaces this deploy creates are checked. One already
/// recorded here exists, so it cannot fail; the other environment's deploy
/// is where the clash is refused.
#[test]
fn a_namespace_already_recorded_here_is_not_refused() {
    let resources = shop_with_staging_instance();
    let mut lock = DeployLock::default();
    lock.record::<Kv>("default", "shop", "KV", KvNamespace::of("bare-id"));
    lock.record::<Kv>(
        "default",
        "shop",
        "KV_STAGING",
        KvNamespace::of("instance-id"),
    );
    lock.record_deployed("staging", "other", Some(Default::default()));
    assert_eq!(
        kv_title_collision(&resources, LedgerView::of(&lock, "default")),
        None
    );
    // ...and the staging deploy is the one refused.
    assert!(kv_title_collision(&resources, LedgerView::of(&lock, "staging")).is_some());
}

/// #2048: an environment the ledger does not know is not compared, and
/// distinct titles across known environments pass.
#[test]
fn unknown_environments_and_distinct_titles_pass() {
    let resources = shop_with_staging_instance();
    assert_eq!(
        kv_title_collision(
            &resources,
            LedgerView::of(&DeployLock::default(), "staging")
        ),
        None
    );
    let plain = project(vec![(
        "shop-cart",
        Resources::default()
            .needs_kv()
            .kv_instances(&["KV_SESSIONS"]),
    )]);
    let mut lock = DeployLock::default();
    lock.record::<Kv>("default", "shop-cart", "KV", KvNamespace::of("bare-id"));
    lock.record::<Kv>(
        "default",
        "shop-cart",
        "KV_SESSIONS",
        KvNamespace::of("sessions-id"),
    );
    lock.record::<Kv>("production", "shop-cart", "KV", KvNamespace::of("prod-id"));
    assert_eq!(
        kv_title_collision(&plain, LedgerView::of(&lock, "staging")),
        None
    );
}

/// S4b (#1967): CI refuses any unrecorded namespace, including an
/// instance whose Worker's bare namespace is recorded.
#[test]
fn ci_refuses_the_first_unrecorded_namespace_bare_or_instance() {
    use crate::deploy::ledger::tests::with_kv_instance;
    let refusal = |noun: &str| {
        Some(format!(
            "bynk: error: {noun} is unrecorded; provision locally first and commit {LOCK_FILE}"
        ))
    };
    let order = names(&["shop-cart"]);
    let declared = project(vec![(
        "shop-cart",
        Resources::default()
            .needs_kv()
            .kv_instances(&["KV_PRICE_CACHE", "KV_SESSIONS"]),
    )]);
    assert_eq!(
        Kv::unrecordable_in_ci(&ProjectCx {
            ledger: LedgerView::of(&DeployLock::default(), "default"),
            resources: &declared,
            order: &order
        }),
        refusal("KV namespace for `shop-cart`")
    );
    let bare = with_kv(DeployLock::default(), "shop-cart");
    assert_eq!(
        Kv::unrecordable_in_ci(&ProjectCx {
            ledger: LedgerView::of(&bare, "default"),
            resources: &declared,
            order: &order
        }),
        refusal("KV namespace `KV_PRICE_CACHE` for `shop-cart`")
    );
    let all = with_kv_instance(
        with_kv_instance(bare, "shop-cart", "KV_PRICE_CACHE"),
        "shop-cart",
        "KV_SESSIONS",
    );
    assert_eq!(
        Kv::unrecordable_in_ci(&ProjectCx {
            ledger: LedgerView::of(&all, "default"),
            resources: &declared,
            order: &order
        }),
        None
    );
    assert_eq!(
        Kv::unrecordable_in_ci(&ProjectCx {
            ledger: LedgerView::of(&all, "staging"),
            resources: &declared,
            order: &order
        }),
        refusal("KV namespace for `shop-cart`"),
        "recorded under another environment is unrecorded here"
    );
}

/// #3562: a group's Worker holds one key name per member, and an end holding only
/// some of them is keyed afresh (absent), never kept.
#[test]
fn an_end_holding_only_some_of_its_keys_is_keyed_afresh() {
    use Presence::{Absent, Present, Unknown};
    assert_eq!(combine_presence([Present, Present]), Present);
    assert_eq!(combine_presence([Present, Absent]), Absent);
    assert_eq!(combine_presence([Absent, Present]), Absent);
    assert_eq!(combine_presence([Present, Unknown]), Unknown);
    assert_eq!(combine_presence([Unknown, Absent]), Absent);
    assert_eq!(combine_presence([]), Present);
}

#[test]
fn an_interrupted_rotation_says_how_to_repair_it() {
    let edge = |caller: &str, callee: &str| Edge {
        caller: caller.to_string(),
        callee: callee.to_string(),
        caller_contexts: Vec::new(),
        callee_contexts: Vec::new(),
    };
    let set = |rotating: bool| EdgeKey::Set {
        value: "minted-value".into(),
        rotating,
    };
    let keys: BTreeMap<Edge, EdgeKey> = [
        (edge("commerce-orders", "commerce-payment"), set(true)),
        (edge("commerce-orders", "commerce-ledger"), set(false)),
        (edge("commerce-ledger", "commerce-payment"), EdgeKey::Keep),
    ]
    .into_iter()
    .collect();
    // `commerce-payment` finished; `commerce-orders` did not.
    let hint = interrupted_rotation_hint("default", &keys, &names(&["commerce-payment"]))
        .expect("a rotation was interrupted");
    assert!(
        hint.contains("`commerce-orders` → `commerce-payment`"),
        "{hint}"
    );
    assert!(
        !hint.contains("commerce-ledger"),
        "an edge that had no key, or kept its key, cannot be split: {hint}"
    );
    assert!(hint.contains("bynk deploy --force"), "{hint}");
    assert!(!hint.contains("minted-value"), "never the value: {hint}");
    // Both ends finished, so nothing was split.
    assert_eq!(
        interrupted_rotation_hint(
            "default",
            &keys,
            &names(&["commerce-payment", "commerce-orders"])
        ),
        None
    );
}

#[test]
fn non_interactive_deploy_requires_yes() {
    assert!(!requires_interactive_confirmation(false, false));
    assert!(requires_interactive_confirmation(false, true));
    assert!(!requires_interactive_confirmation(true, false));
}

#[test]
fn matching_contracts_are_not_a_skew() {
    let lock = lock_with("app-b", &[("whoami", "317bdd3de84d2176")]);
    let found = contract_skews(
        &expects("app.b", "whoami", "317bdd3de84d2176"),
        &lock,
        |c| c.replace('.', "-"),
        "default",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_changed_contract_is_a_skew() {
    // The scenario the increment exists for: B was redeployed with a new
    // contract, and A still stamps the old one.
    let lock = lock_with("app-b", &[("whoami", "ffffffffffffffff")]);
    let found = contract_skews(
        &expects("app.b", "whoami", "317bdd3de84d2176"),
        &lock,
        |c| c.replace('.', "-"),
        "default",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].dependency, "app.b");
    assert_eq!(found[0].service, "whoami");
    assert_eq!(found[0].expected, "317bdd3de84d2176");
    assert_eq!(found[0].live, "ffffffffffffffff");
}

#[test]
fn a_service_the_live_callee_no_longer_provides_is_a_skew() {
    let lock = lock_with("app-b", &[("somethingElse", "317bdd3de84d2176")]);
    let found = contract_skews(
        &expects("app.b", "whoami", "317bdd3de84d2176"),
        &lock,
        |c| c.replace('.', "-"),
        "default",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].live, "<absent>");
}

#[test]
fn a_ledger_with_no_contract_record_yields_no_finding() {
    // Silence is not a match. A dependency deployed by a pre-v0.177 driver
    // has no contract record, and the gate must report only what it *knows*
    // is skewed — never what it merely cannot rule out. The runtime check is
    // the backstop for exactly this case, so a false accusation here would
    // block a legitimate deploy for no gain.
    let mut lock = DeployLock::default();
    lock.record_deployed("default", "app-b", None);
    let found = contract_skews(
        &expects("app.b", "whoami", "317bdd3de84d2176"),
        &lock,
        |c| c.replace('.', "-"),
        "default",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_callee_that_now_provides_nothing_is_a_total_skew() {
    // The counterpart to the rule above, and why the sentinel is `Option`
    // rather than an empty map: a callee that removed *all* its `on call`
    // services emits no manifest, so a bare-map field would record `{}` —
    // indistinguishable from "old ledger" — and the gate would wave through
    // the most complete skew there is. `Some({})` says "known to provide
    // nothing", which is a finding.
    let lock = lock_with("app-b", &[]);
    let found = contract_skews(
        &expects("app.b", "whoami", "317bdd3de84d2176"),
        &lock,
        |c| c.replace('.', "-"),
        "default",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].live, "<absent>");
}

#[test]
fn a_never_deployed_dependency_yields_no_finding_here() {
    // That is D4's existing job (`absent_dependencies`), and it runs first.
    // Reporting it twice, in two vocabularies, would only confuse.
    let found = contract_skews(
        &expects("app.b", "whoami", "317bdd3de84d2176"),
        &DeployLock::default(),
        |c| c.replace('.', "-"),
        "default",
    );
    assert!(found.is_empty(), "{found:?}");
}

// ---- #2077: the callee half of the contract-skew gate ----

/// Chapter 14 of the shop, live: payment provides `authorise` at
/// `febd15c2757b72e1`, and orders was compiled against it.
fn live_shop(orders_expects: Option<&str>) -> DeployLock {
    let mut lock = lock_with("shop-payment", &[("authorise", "febd15c2757b72e1")]);
    lock.record_expects("default", "shop-payment", BTreeMap::new());
    lock.record_deployed("default", "shop-orders", Some(BTreeMap::new()));
    if let Some(hash) = orders_expects {
        lock.record_expects(
            "default",
            "shop-orders",
            expects("shop.payment", "authorise", hash),
        );
    }
    lock
}

fn provides(svc: &str, hash: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(svc.to_string(), hash.to_string())])
}

fn worker_of(c: &str) -> String {
    c.replace('.', "-")
}

#[test]
fn a_callee_whose_contract_changed_under_a_live_caller_is_refused() {
    // The issue's reproduction: `authorise`'s parameter renamed, so the
    // new build provides `da0d725f3a004040` while live orders stamps the
    // old hash.
    let lock = live_shop(Some("febd15c2757b72e1"));
    let found = callee_contract_skews(
        "shop-payment",
        &provides("authorise", "da0d725f3a004040"),
        &lock,
        worker_of,
        "default",
    );
    assert_eq!(
        found,
        [CalleeSkew {
            caller: "shop-orders".into(),
            dependency: "shop.payment".into(),
            service: "authorise".into(),
            expected: "febd15c2757b72e1".into(),
            provided: "da0d725f3a004040".into(),
            compat: None,
        }]
    );
    let refusal = callee_contract_skew_refusal("default", "shop-payment", &found);
    assert!(refusal.contains("`shop-orders` calls shop.payment.authorise"));
    assert!(refusal.contains(CONTRACT_SKEW));
}

/// #2077 review: a caller this build still has is advised to deploy the
/// whole project, which moves it with the callee.
#[test]
fn a_built_caller_is_refused_with_the_whole_project_remedy() {
    let lock = live_shop(Some("febd15c2757b72e1"));
    let found = callee_contract_skews(
        "shop-payment",
        &provides("authorise", "da0d725f3a004040"),
        &lock,
        worker_of,
        "default",
    );
    let bindings = graph(&[("shop-orders", &["shop-payment"]), ("shop-payment", &[])]);
    let (built, orphaned) = split_orphaned_callers(found, &bindings);
    assert_eq!(built.len(), 1);
    assert!(orphaned.is_empty(), "{orphaned:?}");
    let refusal = callee_contract_skew_refusal("default", "shop-payment", &built);
    assert!(refusal.contains("Deploy the whole project (`bynk deploy`)"));
}

/// #2077 review: a caller of a context removed from the project is still
/// live in the ledger, and still refused — but a whole-project deploy
/// would neither move nor check it, so its refusal says to retire it (or
/// restore its context), never just to deploy the whole project.
#[test]
fn an_orphaned_caller_is_refused_with_the_orphan_remedy() {
    let mut lock = live_shop(Some("febd15c2757b72e1"));
    lock.record_deployed("staging", "shop-payment", Some(BTreeMap::new()));
    lock.record_deployed("staging", "shop-legacy", Some(BTreeMap::new()));
    lock.record_expects(
        "staging",
        "shop-legacy",
        expects("shop.payment", "authorise", "febd15c2757b72e1"),
    );
    let found = callee_contract_skews(
        "shop-payment",
        &provides("authorise", "da0d725f3a004040"),
        &lock,
        worker_of,
        "staging",
    );
    // This build has orders and payment; `shop-legacy` is gone.
    let bindings = graph(&[("shop-orders", &["shop-payment"]), ("shop-payment", &[])]);
    let (built, orphaned) = split_orphaned_callers(found, &bindings);
    assert!(built.is_empty(), "{built:?}");
    assert_eq!(orphaned.len(), 1);
    assert_eq!(orphaned[0].caller, "shop-legacy");
    let refusal = orphaned_caller_skew_refusal(
        "staging",
        "shop-payment",
        &orphaned,
        ("npx", &|c: &str| c.to_string()),
    );
    assert!(refusal.contains(CONTRACT_SKEW), "{refusal}");
    assert!(
        refusal.contains("`shop-legacy-staging` (orphaned) calls shop.payment.authorise"),
        "{refusal}"
    );
    assert!(
        refusal.contains("`npx wrangler delete --name shop-legacy-staging`")
            && refusal.contains("`[environments.staging.workers.shop-legacy]`")
            && refusal.contains("`bynk deploy --env staging`"),
        "{refusal}"
    );
}

#[test]
fn the_orphan_refusal_names_the_detected_managers_runner() {
    use bynk_driver::npm_install::PackageManager::{Bun, Npm, Pnpm, Yarn};
    for (manager, runner) in [
        (Npm, "npx"),
        (Pnpm, "pnpm dlx"),
        (Bun, "bunx"),
        (Yarn, "npx"),
    ] {
        let orphaned = vec![CalleeSkew {
            caller: "shop-legacy".to_string(),
            dependency: "shop.payment".to_string(),
            service: "authorise".to_string(),
            expected: "febd15c2757b72e1".to_string(),
            provided: "da0d725f3a004040".to_string(),
            compat: None,
        }];
        let refusal = orphaned_caller_skew_refusal(
            "staging",
            "shop-payment",
            &orphaned,
            (secrets::wrangler_runner(manager), &|c: &str| c.to_string()),
        );
        assert!(
            refusal.contains(&format!(
                "`{runner} wrangler delete --name shop-legacy-staging`"
            )),
            "{refusal}"
        );
    }
}

#[test]
fn a_callee_that_still_provides_the_live_hash_passes() {
    let lock = live_shop(Some("febd15c2757b72e1"));
    let found = callee_contract_skews(
        "shop-payment",
        &provides("authorise", "febd15c2757b72e1"),
        &lock,
        worker_of,
        "default",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_service_a_live_caller_still_calls_but_the_callee_removed_is_refused() {
    let lock = live_shop(Some("febd15c2757b72e1"));
    let found = callee_contract_skews(
        "shop-payment",
        &BTreeMap::new(),
        &lock,
        worker_of,
        "default",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].provided, "<absent>");
}

#[test]
fn a_pushed_worker_is_never_held_to_its_own_stale_record() {
    // The gate is a `--context` gate, as D4 is: `run` reaches it only for
    // a single selected context, so a whole-project run (which redeploys
    // the callers beside the callee) is never refused by it — the
    // binary-driven `deploy_env` test pins that. Here: a pushed caller's
    // own recorded `expects` is being replaced, so it is no finding even
    // when the pushed Worker's contract changed.
    let mut lock = live_shop(Some("febd15c2757b72e1"));
    lock.record_expects(
        "default",
        "shop-orders",
        expects("shop.orders", "place", "0000000000000000"),
    );
    let found = callee_contract_skews("shop-orders", &BTreeMap::new(), &lock, worker_of, "default");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_caller_recorded_before_expects_is_not_refused_but_is_named_for_a_warning() {
    // An old lock: orders is deployed, but the ledger never recorded what
    // it expects. Silence is not a match, so no refusal; orders binds to
    // payment in this build, so it is named for the warning.
    let lock = live_shop(None);
    let found = callee_contract_skews(
        "shop-payment",
        &provides("authorise", "da0d725f3a004040"),
        &lock,
        worker_of,
        "default",
    );
    assert!(found.is_empty(), "{found:?}");
    let bindings = graph(&[("shop-orders", &["shop-payment"]), ("shop-payment", &[])]);
    assert_eq!(
        unrecorded_callers("shop-payment", &bindings, &lock, "default"),
        names(&["shop-orders"])
    );
    // A Worker that does not bind to payment is no caller of it.
    let unrelated = graph(&[("shop-orders", &[]), ("shop-payment", &[])]);
    assert!(unrecorded_callers("shop-payment", &unrelated, &lock, "default").is_empty());
    // And once orders' expectations are recorded, there is nothing to warn of.
    let recorded = live_shop(Some("febd15c2757b72e1"));
    assert!(unrecorded_callers("shop-payment", &bindings, &recorded, "default").is_empty());
}

#[test]
fn a_caller_that_expects_nothing_of_the_callee_is_not_a_finding() {
    let mut lock = live_shop(None);
    lock.record_expects("default", "shop-orders", BTreeMap::new());
    let found = callee_contract_skews(
        "shop-payment",
        &BTreeMap::new(),
        &lock,
        worker_of,
        "default",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_caller_live_in_another_environment_is_not_this_environments_caller() {
    let lock = live_shop(Some("febd15c2757b72e1"));
    let found = callee_contract_skews(
        "shop-payment",
        &provides("authorise", "da0d725f3a004040"),
        &lock,
        worker_of,
        "staging",
    );
    assert!(found.is_empty(), "{found:?}");
}
// ---- #2485: classifying a skew -------------------------------------------

mod classified {
    use super::*;
    use bynk_check::contract_compat::{Field, Shape, Verdict};

    /// `charge(order: Order) -> { ok: Bool }`, with `extra` as additional
    /// fields of the response.
    fn charge(extra: &[(&str, Shape)]) -> ServiceShape {
        let mut fields = vec![Field {
            name: "ok".into(),
            ty: Shape::Base("Bool".into()),
            preds: Vec::new(),
            defaulted: false,
        }];
        fields.extend(extra.iter().map(|(n, t)| Field {
            name: n.to_string(),
            ty: t.clone(),
            preds: Vec::new(),
            defaulted: false,
        }));
        fields.sort_by_key(|f| f.name.clone());
        ServiceShape {
            name: "charge".into(),
            params: vec![("order".into(), Shape::Base("Int".into()))],
            ret: Shape::Record {
                name: "Receipt".into(),
                fields,
            },
        }
    }

    fn lock_providing(shape: &ServiceShape) -> DeployLock {
        let mut lock = DeployLock::default();
        lock.record_deployed(
            "default",
            "shop-payment",
            Some(BTreeMap::from([("charge".into(), shape.hash())])),
        );
        lock.record_shapes(
            BTreeMap::from([("charge".into(), shape.to_text())]),
            BTreeMap::new(),
        );
        lock
    }

    fn worker_of(dep: &str) -> String {
        dep.replace('.', "-")
    }

    #[test]
    fn a_caller_skew_with_both_shapes_is_classified() {
        let live = charge(&[]);
        let compiled = charge(&[("at", Shape::Base("Int".into()))]);
        let lock = lock_providing(&live);
        let expects = BTreeMap::from([(
            "shop.payment".to_string(),
            BTreeMap::from([("charge".to_string(), compiled.hash())]),
        )]);
        let mut skews = contract_skews(&expects, &lock, worker_of, "default");
        assert_eq!(skews.len(), 1);
        let shapes = BTreeMap::from([(
            "shop.payment".to_string(),
            BTreeMap::from([("charge".to_string(), compiled.to_text())]),
        )]);
        classify_contract_skews(&mut skews, &shapes, &lock, worker_of, "default");
        // The caller was built expecting an extra response field the live
        // callee does not send: it cannot read that answer.
        let c = skews[0].compat.as_ref().expect("classified");
        assert_eq!(c.verdict, Verdict::Breaking, "{c:#?}");
        let refusal = contract_skew_refusal("default", "shop-orders", &skews);
        assert!(refusal.contains("breaking response.at"), "{refusal}");
    }

    #[test]
    fn a_callee_skew_that_only_adds_a_response_field_is_compatible() {
        let old = charge(&[]);
        let new = charge(&[("at", Shape::Base("Int".into()))]);
        let mut lock = DeployLock::default();
        lock.record_deployed("default", "shop-orders", Some(BTreeMap::new()));
        lock.record_expects(
            "default",
            "shop-orders",
            BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), old.hash())]),
            )]),
        );
        lock.record_shapes(
            BTreeMap::new(),
            BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), old.to_text())]),
            )]),
        );
        let provides = BTreeMap::from([("charge".to_string(), new.hash())]);
        let mut skews =
            callee_contract_skews("shop-payment", &provides, &lock, worker_of, "default");
        assert_eq!(skews.len(), 1);
        classify_callee_skews(
            &mut skews,
            &BTreeMap::from([("charge".to_string(), new.to_text())]),
            &lock,
            "default",
        );
        let c = skews[0].compat.as_ref().expect("classified");
        assert_eq!(c.verdict, Verdict::Compatible, "{c:#?}");
        let refusal = callee_contract_skew_refusal("default", "shop-payment", &skews);
        assert!(refusal.contains("compatible response.at"), "{refusal}");
        assert!(
            refusal.contains("A compatible change can be accepted"),
            "{refusal}"
        );
    }

    // ---- #2554: accepting a compatible callee-ahead skew ----------------

    /// A live `shop-orders` compiled against `old`, and the callee's new
    /// build providing `new`: the one callee-ahead skew, classified.
    fn callee_skew(old: &ServiceShape, new: &ServiceShape) -> (DeployLock, Vec<CalleeSkew>) {
        let mut lock = DeployLock::default();
        lock.record_deployed("default", "shop-orders", Some(BTreeMap::new()));
        lock.record_expects(
            "default",
            "shop-orders",
            BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), old.hash())]),
            )]),
        );
        lock.record_shapes(
            BTreeMap::new(),
            BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), old.to_text())]),
            )]),
        );
        let provides = BTreeMap::from([("charge".to_string(), new.hash())]);
        let mut skews =
            callee_contract_skews("shop-payment", &provides, &lock, worker_of, "default");
        classify_callee_skews(
            &mut skews,
            &BTreeMap::from([("charge".to_string(), new.to_text())]),
            &lock,
            "default",
        );
        (lock, skews)
    }

    #[test]
    fn a_compatible_skew_is_accepted_only_once_the_build_carries_its_hash() {
        let old = charge(&[]);
        let new = charge(&[("at", Shape::Base("Int".into()))]);
        let (lock, skews) = callee_skew(&old, &new);

        // Before the lock is recorded the build accepts nothing: the skew
        // is refused, and the shape to record is the caller's.
        let none = BTreeMap::new();
        let wanted = shapes_to_accept(&skews, &none, &lock, "default");
        assert_eq!(
            wanted,
            [(
                "shop.payment".to_string(),
                "charge".to_string(),
                old.hash(),
                old.to_text()
            )]
        );
        let (accepted, refused) = partition_accepted(skews.clone(), &none);
        assert!((accepted.len(), refused.len()) == (0, 1));

        // Once the rebuilt callee lists the hash, it is answered, and
        // nothing more needs recording.
        let carries = BTreeMap::from([("charge".to_string(), vec![old.hash()])]);
        assert!(shapes_to_accept(&skews, &carries, &lock, "default").is_empty());
        let (accepted, refused) = partition_accepted(skews, &carries);
        assert!((accepted.len(), refused.len()) == (1, 0));
    }

    #[test]
    fn a_breaking_skew_is_never_accepted_whatever_the_build_carries() {
        let old = charge(&[("at", Shape::Base("Int".into()))]);
        let new = charge(&[]);
        let (lock, skews) = callee_skew(&old, &new);
        assert_eq!(skews[0].compat.as_ref().unwrap().verdict, Verdict::Breaking);
        // Even if a hand-edited lock listed the hash, the verdict stands.
        let carries = BTreeMap::from([("charge".to_string(), vec![old.hash()])]);
        assert!(shapes_to_accept(&skews, &carries, &lock, "default").is_empty());
        let (accepted, refused) = partition_accepted(skews, &carries);
        assert!((accepted.len(), refused.len()) == (0, 1));
    }

    #[test]
    fn an_unclassified_skew_is_never_accepted() {
        let old = charge(&[]);
        let new = charge(&[("at", Shape::Base("Int".into()))]);
        let (lock, mut skews) = callee_skew(&old, &new);
        skews[0].compat = None;
        let carries = BTreeMap::from([("charge".to_string(), vec![old.hash()])]);
        assert!(shapes_to_accept(&skews, &carries, &lock, "default").is_empty());
        assert!(partition_accepted(skews, &carries).0.is_empty());
    }

    #[test]
    fn the_notice_says_what_was_recorded_and_what_a_dry_run_did_not() {
        let old = charge(&[]);
        let new = charge(&[("at", Shape::Base("Int".into()))]);
        let (_, skews) = callee_skew(&old, &new);
        let real = accepted_skew_notice("default", "shop-payment", &skews, true, false);
        assert!(
            real.contains("will also answer 1 live caller contract that is compatible"),
            "{real}"
        );
        assert!(
            real.contains("`shop-orders` calls shop.payment.charge"),
            "{real}"
        );
        assert!(real.contains("Recorded in bynk.contracts.lock"), "{real}");
        let dry = accepted_skew_notice("default", "shop-payment", &skews, false, true);
        assert!(dry.contains("Nothing was written"), "{dry}");
        assert!(!dry.contains("Recorded in"), "{dry}");
    }

    #[test]
    fn a_caller_ahead_of_its_callee_is_told_to_deploy_the_callee_first() {
        // The new caller expects the old contract; the live callee already
        // sends more: compatible for the caller, yet the callee still
        // refuses a hash it does not know.
        let caller = charge(&[]);
        let live = charge(&[("at", Shape::Base("Int".into()))]);
        let lock = lock_providing(&live);
        let expects = BTreeMap::from([(
            "shop.payment".to_string(),
            BTreeMap::from([("charge".to_string(), caller.hash())]),
        )]);
        let mut skews = contract_skews(&expects, &lock, worker_of, "default");
        classify_contract_skews(
            &mut skews,
            &BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), caller.to_text())]),
            )]),
            &lock,
            worker_of,
            "default",
        );
        assert_eq!(
            skews[0].compat.as_ref().unwrap().verdict,
            Verdict::Compatible
        );
        let refusal = contract_skew_refusal("default", "shop-orders", &skews);
        assert!(
            refusal.contains("A compatible change is still refused here"),
            "{refusal}"
        );
        assert!(
            refusal.contains("Deploy `shop.payment` ahead of this caller"),
            "{refusal}"
        );
    }

    #[test]
    fn the_lock_entries_nothing_live_needs_are_reported() {
        let old = charge(&[]);
        let newer = charge(&[("at", Shape::Base("Int".into()))]);
        let unused_one = charge(&[("by", Shape::Base("Int".into()))]);
        let (lock, _) = callee_skew(&old, &newer);
        let mut contracts = bynk_project::contracts_lock::ContractsLock::new();
        contracts.accept("shop.payment", "charge", &old.to_text());
        contracts.accept("shop.payment", "charge", &unused_one.to_text());
        let unused = unused_accepted(
            &contracts,
            ("shop-payment", &|c: &str| c.replace('.', "-")),
            &lock,
            "default",
        );
        assert_eq!(
            unused,
            [(
                "shop.payment".to_string(),
                "charge".to_string(),
                unused_one.hash()
            )],
            "the contract shop-orders was compiled against is still needed"
        );
        assert!(
            unused_accepted(
                &contracts,
                ("shop-orders", &|c: &str| c.replace('.', "-")),
                &lock,
                "default"
            )
            .is_empty()
        );
        assert!(unused_accepted_notice(&unused).contains("shop.payment.charge"));
    }

    #[test]
    fn a_mixed_run_accepts_the_compatible_callers_and_refuses_the_rest() {
        // Two live callers of one callee: `shop-orders` on a contract the
        // new build stays compatible with, `shop-audit` on one it breaks.
        let compatible_old = charge(&[]);
        let breaking_old = charge(&[("by", Shape::Base("Int".into()))]);
        let new = charge(&[("at", Shape::Base("Int".into()))]);
        let mut lock = DeployLock::default();
        for (caller, old) in [
            ("shop-orders", &compatible_old),
            ("shop-audit", &breaking_old),
        ] {
            lock.record_deployed("default", caller, Some(BTreeMap::new()));
            lock.record_expects(
                "default",
                caller,
                BTreeMap::from([(
                    "shop.payment".to_string(),
                    BTreeMap::from([("charge".to_string(), old.hash())]),
                )]),
            );
            lock.record_shapes(
                BTreeMap::new(),
                BTreeMap::from([(
                    "shop.payment".to_string(),
                    BTreeMap::from([("charge".to_string(), old.to_text())]),
                )]),
            );
        }
        let provides = BTreeMap::from([("charge".to_string(), new.hash())]);
        let mut skews =
            callee_contract_skews("shop-payment", &provides, &lock, worker_of, "default");
        classify_callee_skews(
            &mut skews,
            &BTreeMap::from([("charge".to_string(), new.to_text())]),
            &lock,
            "default",
        );
        assert_eq!(skews.len(), 2);

        // Only the compatible caller's contract is worth recording.
        let none = BTreeMap::new();
        let wanted = shapes_to_accept(&skews, &none, &lock, "default");
        assert_eq!(
            wanted.iter().map(|w| w.2.as_str()).collect::<Vec<_>>(),
            [compatible_old.hash().as_str()]
        );

        // With the build carrying exactly that hash, one is answered and the
        // breaking one is still refused.
        let carries = BTreeMap::from([("charge".to_string(), vec![compatible_old.hash()])]);
        let (accepted, refused) = partition_accepted(skews, &carries);
        assert_eq!(
            accepted
                .iter()
                .map(|s| s.caller.as_str())
                .collect::<Vec<_>>(),
            ["shop-orders"]
        );
        assert_eq!(
            refused
                .iter()
                .map(|s| s.caller.as_str())
                .collect::<Vec<_>>(),
            ["shop-audit"]
        );
    }

    #[test]
    fn a_skew_whose_shapes_are_equal_is_not_accepted() {
        // Different hashes but the same structure cannot happen for a real
        // build; if a ledger ever says so, the gate refuses rather than
        // guess. `Unchanged` is not `Compatible`.
        let shape = charge(&[]);
        let (lock, mut skews) = callee_skew(&shape, &charge(&[("at", Shape::Base("Int".into()))]));
        skews[0].compat = Some(check_call(&shape, &shape));
        assert_eq!(
            skews[0].compat.as_ref().unwrap().verdict,
            Verdict::Unchanged
        );
        let carries = BTreeMap::from([("charge".to_string(), vec![shape.hash()])]);
        assert!(shapes_to_accept(&skews, &carries, &lock, "default").is_empty());
        assert!(partition_accepted(skews, &carries).0.is_empty());
    }

    #[test]
    fn a_missing_or_drifted_shape_leaves_the_skew_unclassified() {
        let live = charge(&[]);
        let compiled = charge(&[("at", Shape::Base("Int".into()))]);
        let lock = lock_providing(&live);
        let expects = BTreeMap::from([(
            "shop.payment".to_string(),
            BTreeMap::from([("charge".to_string(), compiled.hash())]),
        )]);
        // No shapes from the build (an older compiler).
        let mut skews = contract_skews(&expects, &lock, worker_of, "default");
        classify_contract_skews(&mut skews, &BTreeMap::new(), &lock, worker_of, "default");
        assert_eq!(skews[0].compat, None);
        // A shape that does not hash to the hash it sits beside.
        let mut skews = contract_skews(&expects, &lock, worker_of, "default");
        let wrong = BTreeMap::from([(
            "shop.payment".to_string(),
            BTreeMap::from([("charge".to_string(), live.to_text())]),
        )]);
        classify_contract_skews(&mut skews, &wrong, &lock, worker_of, "default");
        assert_eq!(skews[0].compat, None);
        // A ledger holding only hashes.
        let mut hashes_only = DeployLock::default();
        hashes_only.record_deployed(
            "default",
            "shop-payment",
            Some(BTreeMap::from([("charge".into(), live.hash())])),
        );
        let mut skews = contract_skews(&expects, &hashes_only, worker_of, "default");
        let shapes = BTreeMap::from([(
            "shop.payment".to_string(),
            BTreeMap::from([("charge".to_string(), compiled.to_text())]),
        )]);
        classify_contract_skews(&mut skews, &shapes, &hashes_only, worker_of, "default");
        assert_eq!(skews[0].compat, None);
        let refusal = contract_skew_refusal("default", "shop-orders", &skews);
        assert!(!refusal.contains("breaking") && !refusal.contains("compatible"));
    }
}
