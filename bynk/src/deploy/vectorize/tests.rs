//! #3479: the Vectorize deploy path, against a recording fake account
//! (`cloudflare::fake::Recording`, 1D.3a) that logs each call and answers the
//! `vectorize` ones. The shapes it
//! answers with are the ones `parse_*` reads; whether Cloudflare's wrangler
//! prints them is not something a test can pin.

use super::*;
use std::path::PathBuf;

fn decl(indexed: &[&str]) -> VectorizeDecl {
    VectorizeDecl {
        binding: "VECTORIZE_DOCS".into(),
        name: "shop-docs".into(),
        dimensions: 768,
        metric: "cosine".into(),
        indexed: indexed.iter().map(|s| s.to_string()).collect(),
        adopt: None,
    }
}

fn resources(indexed: &[&str]) -> Resources {
    Resources {
        vectorize_indexes: vec![decl(indexed)],
        ..Default::default()
    }
}

/// What the fake account answers the Vectorize calls with, and what it has seen: the shapes
/// `parse_*` reads.
#[derive(Default)]
struct Account {
    list: String,
    get: String,
    meta: String,
    /// What `list-metadata-index` answers once a `create-metadata-index` has run: on the `n`th
    /// poll, or from then on.
    after: BTreeMap<usize, String>,
    after_all: Option<String>,
    created: bool,
    polls: usize,
    /// `create-metadata-index` answers that the index is already there.
    exists: bool,
}

impl Account {
    fn answer(
        &mut self,
        op: &crate::cloudflare::Op<'_>,
    ) -> Result<String, crate::cloudflare::CloudflareError> {
        use crate::cloudflare::Op;
        let Op::Vectorize(call) = op else {
            return Err(crate::cloudflare::WranglerError::exit(
                op,
                crate::cloudflare::error::Exit::Code(2),
                "",
                "",
            )
            .into());
        };
        Ok(match call {
            VectorizeOp::List => self.list.clone(),
            VectorizeOp::Get { .. } => self.get.clone(),
            VectorizeOp::CreateMetadataIndex { .. } => {
                self.created = true;
                if self.exists {
                    // The reference documents no code for it: a refusal, whatever its code.
                    return Err(crate::cloudflare::RestError::answered(
                        "POST",
                        "/vectorize/v2/indexes/shop-docs/metadata_index/create",
                        409,
                        r#"[{"code":40004,"message":"metadata index already exists for this name"}]"#,
                    )
                    .into());
                }
                String::new()
            }
            VectorizeOp::ListMetadataIndex { .. } if self.created => {
                let n = self.polls;
                self.polls += 1;
                self.after
                    .get(&n)
                    .or(self.after_all.as_ref())
                    .unwrap_or(&self.meta)
                    .clone()
            }
            VectorizeOp::ListMetadataIndex { .. } => self.meta.clone(),
            VectorizeOp::Create { .. } | VectorizeOp::Delete { .. } => String::new(),
        })
    }
}

/// A recording fake account and a directory for the ledger the run writes.
struct Fake {
    dir: PathBuf,
    account: std::rc::Rc<std::cell::RefCell<Account>>,
    api: crate::cloudflare::fake::Recording,
}

impl Fake {
    fn new(tag: &str, list: &str, get: &str, meta: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bynk-vz-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let account = std::rc::Rc::new(std::cell::RefCell::new(Account {
            list: list.into(),
            get: get.into(),
            meta: meta.into(),
            ..Account::default()
        }));
        let answering = account.clone();
        let api =
            crate::cloudflare::fake::Recording::new(move |op| answering.borrow_mut().answer(op));
        Self { dir, account, api }
    }

    fn calls(&self) -> Vec<String> {
        self.api.calls()
    }

    /// What `list-metadata-index` answers once a `create-metadata-index` has
    /// run, as a `n`th poll (`Some(n)`) or from then on (`None`).
    fn after(&self, poll: Option<usize>, json: &str) {
        let mut account = self.account.borrow_mut();
        match poll {
            Some(n) => {
                account.after.insert(n, json.to_string());
            }
            None => account.after_all = Some(json.to_string()),
        }
    }

    /// `create-metadata-index` answers that the index is already there.
    fn metadata_index_exists(&self) {
        self.account.borrow_mut().exists = true;
    }

    fn polls(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| c.contains("list-metadata-index"))
            .count()
    }

    /// Run with a pace a test can afford: 1 ms steps, a 300 ms timeout.
    fn run(
        &self,
        declared: &Resources,
        lock: &mut DeployLock,
        environment: &str,
    ) -> Result<(), DeployFailure> {
        self.run_paced(
            declared,
            lock,
            environment,
            Pacing {
                first: Duration::from_millis(1),
                longest: Duration::from_millis(4),
                timeout: Duration::from_millis(300),
            },
        )
    }

    fn run_paced(
        &self,
        declared: &Resources,
        lock: &mut DeployLock,
        environment: &str,
        pacing: Pacing,
    ) -> Result<(), DeployFailure> {
        let lock_path = self.dir.join("bynk.lock");
        provision_vectorize_indexes_paced(
            &self.api,
            &self.dir,
            "shop",
            declared,
            lock,
            &lock_path,
            environment,
            pacing,
        )
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const MATCHING: &str = r#"{"name":"shop-docs","config":{"dimensions":768,"metric":"cosine"}}"#;

#[test]
fn metadata_index_changes_are_additions_and_removals() {
    let d = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let (missing, removed) = metadata_index_changes(
        &d(&["a:string", "b:number"]),
        &d(&["b:number", "c:boolean"]),
    );
    assert_eq!((missing, removed), (d(&["a:string"]), d(&["c:boolean"])));
}

#[test]
fn a_missing_index_is_created_with_its_configuration_then_its_metadata_indexes() {
    let fake = Fake::new("create", "[]", MATCHING, "[]");
    fake.after(
        None,
        r#"[{"propertyName":"category","indexType":"String"},{"propertyName":"year","indexType":"Number"}]"#,
    );
    let mut lock = DeployLock::default();
    fake.run(
        &resources(&["category:string", "year:number"]),
        &mut lock,
        "default",
    )
    .unwrap();
    let calls = fake.calls();
    assert_eq!(
        calls,
        [
            "vectorize list --json",
            "vectorize create shop-docs --dimensions 768 --metric cosine",
            "vectorize list-metadata-index shop-docs --json",
            "vectorize create-metadata-index shop-docs --propertyName category --type string",
            "vectorize create-metadata-index shop-docs --propertyName year --type number",
            // The deploy waits for both to be listed before it returns.
            "vectorize list-metadata-index shop-docs --json",
        ]
    );
    let recorded = lock
        .recorded::<Vectorize>("default", "shop", "VECTORIZE_DOCS")
        .unwrap();
    assert_eq!(
        (recorded.name.as_str(), recorded.adopted),
        ("shop-docs", false)
    );
}

#[test]
fn a_matching_index_is_reused_and_never_created_again() {
    let fake = Fake::new(
        "reuse",
        r#"[{"name":"shop-docs-staging"}]"#,
        MATCHING,
        r#"[{"propertyName":"category","indexType":"string"}]"#,
    );
    let mut lock = DeployLock::default();
    fake.run(&resources(&["category:string"]), &mut lock, "staging")
        .unwrap();
    assert!(
        fake.calls().iter().all(|c| !c.contains("create ")),
        "{:?}",
        fake.calls()
    );
    assert_eq!(
        lock.recorded::<Vectorize>("staging", "shop", "VECTORIZE_DOCS")
            .unwrap()
            .name,
        "shop-docs-staging"
    );
}

#[test]
fn a_dimension_or_metric_mismatch_fails_and_changes_nothing() {
    for found in [
        r#"{"config":{"dimensions":1536,"metric":"cosine"}}"#,
        r#"{"config":{"dimensions":768,"metric":"euclidean"}}"#,
    ] {
        let fake = Fake::new("drift", r#"[{"name":"shop-docs"}]"#, found, "[]");
        let mut lock = DeployLock::default();
        let failure = fake
            .run(&resources(&[]), &mut lock, "default")
            .expect_err("drift fails the deploy");
        assert!(
            failure.message.contains("768 dimensions and `cosine`"),
            "{}",
            failure.message
        );
        assert!(
            failure
                .message
                .contains("recreating it would delete every vector")
        );
        // Nothing was created, changed, deleted or recorded.
        let calls = fake.calls();
        assert!(
            calls
                .iter()
                .all(|c| !c.contains("create") && !c.contains("delete")),
            "{calls:?}"
        );
        assert!(
            lock.recorded::<Vectorize>("default", "shop", "VECTORIZE_DOCS")
                .is_none()
        );
    }
}

#[test]
fn an_unreadable_configuration_fails_rather_than_passing() {
    let fake = Fake::new("opaque", r#"[{"name":"shop-docs"}]"#, "{}", "[]");
    let mut lock = DeployLock::default();
    let failure = fake.run(&resources(&[]), &mut lock, "default").unwrap_err();
    assert!(
        failure.message.contains("not checked against the source"),
        "{}",
        failure.message
    );
}

#[test]
fn a_later_metadata_index_is_created_and_a_removed_one_is_left() {
    let fake = Fake::new(
        "later",
        r#"[{"name":"shop-docs"}]"#,
        MATCHING,
        r#"[{"propertyName":"old","indexType":"string"}]"#,
    );
    fake.after(
        None,
        r#"[{"propertyName":"old","indexType":"string"},{"propertyName":"year","indexType":"Number"}]"#,
    );
    let mut lock = DeployLock::default();
    fake.run(&resources(&["year:number"]), &mut lock, "default")
        .unwrap();
    let calls = fake.calls();
    assert!(
        calls
            .iter()
            .any(|c| c.contains("--propertyName year --type number"))
    );
    assert!(calls.iter().all(|c| !c.contains("delete-metadata-index")));
}

#[test]
fn pruning_deletes_an_existing_index_and_is_quiet_about_one_already_gone() {
    let fake = Fake::new("prune", r#"[{"name":"shop-old"}]"#, MATCHING, "[]");
    delete_vectorize_index(&fake.api, "shop-old").unwrap();
    assert!(
        fake.calls()
            .contains(&"vectorize delete shop-old --force".to_string())
    );
    delete_vectorize_index(&fake.api, "shop-gone").unwrap();
    assert_eq!(
        fake.calls().iter().filter(|c| c.contains("delete")).count(),
        1
    );
}

#[test]
fn the_ledger_records_supersedes_orphans_and_forgets() {
    let mut lock = DeployLock::default();
    lock.record::<Vectorize>(
        "default",
        "shop",
        "VECTORIZE_DOCS",
        VectorizeRecord::of("shop-docs", false),
    );
    // Live and declared: no orphan.
    let live = BTreeMap::from([("shop".to_string(), resources(&[]))]);
    assert!(find_orphans(&lock, "default", &live).vectorize.is_empty());
    // A renamed instance leaves the old index recorded and reported.
    lock.record::<Vectorize>(
        "default",
        "shop",
        "VECTORIZE_DOCS",
        VectorizeRecord::of("shop-docs-v2", false),
    );
    let orphans = find_orphans(&lock, "default", &BTreeMap::new());
    assert_eq!(orphans.vectorize.len(), 2);
    assert!(orphans.has_prunable());
    // Forgetting the superseded one leaves the live record.
    VectorizeTable::forget_index(
        lock.environments.get_mut("default").unwrap(),
        "shop",
        "VECTORIZE_DOCS",
        "shop-docs",
    );
    assert_eq!(
        lock.recorded::<Vectorize>("default", "shop", "VECTORIZE_DOCS")
            .unwrap()
            .superseded,
        []
    );
    VectorizeTable::forget_index(
        lock.environments.get_mut("default").unwrap(),
        "shop",
        "VECTORIZE_DOCS",
        "shop-docs-v2",
    );
    assert!(
        lock.recorded::<Vectorize>("default", "shop", "VECTORIZE_DOCS")
            .is_none()
    );
    // An adopted index is reported and never prunable.
    lock.record::<Vectorize>(
        "default",
        "shop",
        "VECTORIZE_DOCS",
        VectorizeRecord::of("legacy", true),
    );
    let adopted = find_orphans(&lock, "default", &BTreeMap::new());
    assert!(adopted.vectorize[0].adopted);
    assert!(!adopted.has_prunable());
}

#[test]
fn a_name_the_environment_pushes_over_the_limit_is_an_error_naming_the_instance() {
    let mut long = decl(&[]);
    long.name = "a".repeat(60);
    let declared = Resources {
        vectorize_indexes: vec![long],
        ..Default::default()
    };
    let resources = BTreeMap::from([("shop".to_string(), declared)]);
    let lock = DeployLock::default();
    assert!(vectorize_index_problem(&resources, LedgerView::of(&lock, "default")).is_none());
    let message =
        vectorize_index_problem(&resources, LedgerView::of(&lock, "production")).expect("65 bytes");
    assert!(
        message.contains("VECTORIZE_DOCS") && message.contains("1 to 64 bytes"),
        "{message}"
    );
}

#[test]
fn moving_back_to_a_left_index_does_not_make_the_live_one_an_orphan() {
    let mut lock = DeployLock::default();
    lock.record::<Vectorize>(
        "default",
        "shop",
        "VECTORIZE_DOCS",
        VectorizeRecord::of("a", false),
    );
    lock.record::<Vectorize>(
        "default",
        "shop",
        "VECTORIZE_DOCS",
        VectorizeRecord::of("b", false),
    );
    lock.record::<Vectorize>(
        "default",
        "shop",
        "VECTORIZE_DOCS",
        VectorizeRecord::of("a", false),
    );
    let record = lock
        .recorded::<Vectorize>("default", "shop", "VECTORIZE_DOCS")
        .unwrap();
    assert_eq!(record.name, "a");
    assert_eq!(
        record
            .superseded
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
}

fn adopting(name: &str, indexed: &[&str]) -> Resources {
    let mut declared = resources(indexed);
    declared.vectorize_indexes[0].adopt = Some(name.to_string());
    declared
}

#[test]
fn an_adopted_index_that_matches_is_recorded_as_adopted_and_never_touched() {
    let fake = Fake::new(
        "adopt",
        r#"[{"name":"docs-prod"}]"#,
        MATCHING,
        r#"[{"propertyName":"category","indexType":"string"}]"#,
    );
    let mut lock = DeployLock::default();
    fake.run(
        &adopting("docs-prod", &["category:string"]),
        &mut lock,
        "production",
    )
    .unwrap();
    let recorded = lock
        .recorded::<Vectorize>("production", "shop", "VECTORIZE_DOCS")
        .unwrap();
    assert_eq!(
        (recorded.name.as_str(), recorded.adopted),
        ("docs-prod", true)
    );
    // Looked at, never created, changed or deleted.
    assert!(
        fake.calls()
            .iter()
            .all(|c| !c.contains("create") && !c.contains("delete")),
        "{:?}",
        fake.calls()
    );
}

#[test]
fn a_retyped_metadata_index_is_refused_before_anything_is_created() {
    let d = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let (missing, removed) =
        metadata_index_changes(&d(&["category:number"]), &d(&["category:string"]));
    let message = type_change_message("shop-docs", &missing, &removed).unwrap();
    assert!(
        message.contains("`category:string` is now `category:number`"),
        "{message}"
    );
    // A plain addition or removal is not a retype.
    let (m, r) = metadata_index_changes(&d(&["a:string"]), &d(&["b:string"]));
    assert_eq!(type_change_message("shop-docs", &m, &r), None);
}

#[test]
fn it_waits_for_every_new_metadata_index_to_be_listed_before_returning() {
    // `create-metadata-index` only enqueues: the listing is empty for two
    // polls, then holds one of the two, then both.
    let fake = Fake::new("wait", "[]", MATCHING, "[]");
    fake.after(Some(0), "[]");
    fake.after(Some(1), "[]");
    fake.after(
        Some(2),
        r#"[{"propertyName":"category","indexType":"String"}]"#,
    );
    fake.after(
        None,
        r#"[{"propertyName":"category","indexType":"String"},{"propertyName":"author.name","indexType":"String"}]"#,
    );
    let mut lock = DeployLock::default();
    fake.run(
        &resources(&["category:string", "author.name:string"]),
        &mut lock,
        "default",
    )
    .unwrap();
    // One poll before the creations, then four after: 0, 1, 2 (partial), 3.
    assert_eq!(fake.polls(), 5, "{:?}", fake.calls());
}

#[test]
fn an_index_that_never_appears_fails_the_deploy_and_says_to_run_it_again() {
    let fake = Fake::new("timeout", "[]", MATCHING, "[]");
    // The listing never catches up.
    fake.after(None, "[]");
    let mut lock = DeployLock::default();
    let err = fake
        .run_paced(
            &resources(&["category:string"]),
            &mut lock,
            "default",
            Pacing {
                first: Duration::from_millis(1),
                longest: Duration::from_millis(2),
                timeout: Duration::from_millis(40),
            },
        )
        .unwrap_err();
    let message = format!("{err:?}");
    assert!(message.contains("category:string"), "{message}");
    assert!(message.contains("not listed"), "{message}");
    assert!(message.contains("was not pushed"), "{message}");
    // The index itself was created and recorded, so a re-run resumes.
    assert!(
        lock.recorded::<Vectorize>("default", "shop", "VECTORIZE_DOCS")
            .is_some(),
        "the created index is recorded even when the wait times out"
    );
}

#[test]
fn a_deploy_that_creates_no_metadata_index_does_not_wait() {
    let fake = Fake::new(
        "nowait",
        r#"[{"name":"shop-docs"}]"#,
        MATCHING,
        r#"[{"propertyName":"category","indexType":"String"}]"#,
    );
    let mut lock = DeployLock::default();
    fake.run(&resources(&["category:string"]), &mut lock, "default")
        .unwrap();
    assert_eq!(fake.polls(), 1, "{:?}", fake.calls());
}

#[test]
fn a_metadata_index_listed_between_the_listing_and_the_create_is_not_a_failure() {
    // Cloudflare answers `already exists` for a property that has just become
    // listed; the deploy goes on to wait for it rather than failing.
    let fake = Fake::new("exists", "[]", MATCHING, "[]");
    fake.metadata_index_exists();
    fake.after(
        None,
        r#"[{"propertyName":"category","indexType":"String"}]"#,
    );
    let mut lock = DeployLock::default();
    fake.run(&resources(&["category:string"]), &mut lock, "default")
        .unwrap();
}

#[test]
fn an_adopted_index_that_is_missing_fails_and_is_not_created() {
    let fake = Fake::new("adopt-missing", "[]", MATCHING, "[]");
    let mut lock = DeployLock::default();
    let failure = fake
        .run(&adopting("docs-prod", &[]), &mut lock, "production")
        .unwrap_err();
    assert!(
        failure.message.contains("does not exist on the account"),
        "{}",
        failure.message
    );
    assert!(
        failure
            .message
            .contains("an adopted index is never created")
    );
    assert!(fake.calls().iter().all(|c| !c.contains("create")));
    assert!(
        lock.recorded::<Vectorize>("production", "shop", "VECTORIZE_DOCS")
            .is_none()
    );
}

#[test]
fn an_adopted_index_that_drifted_fails_and_a_missing_metadata_index_is_not_created() {
    let drifted = Fake::new(
        "adopt-drift",
        r#"[{"name":"docs-prod"}]"#,
        r#"{"config":{"dimensions":3,"metric":"cosine"}}"#,
        "[]",
    );
    let mut lock = DeployLock::default();
    let failure = drifted
        .run(&adopting("docs-prod", &[]), &mut lock, "production")
        .unwrap_err();
    assert!(
        failure.message.contains("has 3 dimensions"),
        "{}",
        failure.message
    );

    // A metadata index the adopted index lacks is said, not made.
    let lacking = Fake::new("adopt-meta", r#"[{"name":"docs-prod"}]"#, MATCHING, "[]");
    lacking
        .run(
            &adopting("docs-prod", &["category:string"]),
            &mut DeployLock::default(),
            "production",
        )
        .unwrap();
    assert!(lacking.calls().iter().all(|c| !c.contains("create")));
}

#[test]
fn bynk_toml_adopts_per_environment_by_capability() {
    let dir = std::env::temp_dir().join(format!("bynk-vz-adopt-toml-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[deploy.vectorize.Docs]\nindex = { production = \"docs-prod\" }\n",
    )
    .unwrap();
    let mut all = BTreeMap::from([("shop".to_string(), resources(&[]))]);
    apply_vectorize_adoptions(&dir, &mut all, "production").unwrap();
    assert_eq!(
        all["shop"].vectorize_indexes[0].adopt.as_deref(),
        Some("docs-prod")
    );
    assert_eq!(
        all["shop"].vectorize_indexes[0].physical_name("production"),
        "docs-prod"
    );
    // Another environment is created by the deploy, under the qualified name.
    apply_vectorize_adoptions(&dir, &mut all, "staging").unwrap();
    assert_eq!(all["shop"].vectorize_indexes[0].adopt, None);
    assert_eq!(
        all["shop"].vectorize_indexes[0].physical_name("staging"),
        "shop-docs-staging"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_adopted_name_the_platform_refuses_points_at_bynk_toml_not_the_capability() {
    let mut adopted = decl(&[]);
    adopted.adopt = Some("A_Bad_Name".repeat(8));
    let resources = BTreeMap::from([(
        "shop".to_string(),
        Resources {
            vectorize_indexes: vec![adopted],
            ..Default::default()
        },
    )]);
    let message = vectorize_index_problem(
        &resources,
        LedgerView::of(&DeployLock::default(), "production"),
    )
    .expect("a refused adopted name");
    assert!(
        message.contains("[deploy.vectorize]") && !message.contains("rename the capability"),
        "{message}"
    );
}

fn adopting_decl(binding: &str, name: &str, adopt: &str) -> VectorizeDecl {
    let mut d = decl(&[]);
    d.binding = binding.into();
    d.name = name.into();
    d.adopt = Some(adopt.into());
    d
}

#[test]
fn adopters_may_share_an_index_but_a_created_one_may_not_be_shared() {
    let shared = |a: VectorizeDecl, b: VectorizeDecl| {
        BTreeMap::from([
            (
                "shop".to_string(),
                Resources {
                    vectorize_indexes: vec![a],
                    ..Default::default()
                },
            ),
            (
                "blog".to_string(),
                Resources {
                    vectorize_indexes: vec![b],
                    ..Default::default()
                },
            ),
        ])
    };
    let lock = DeployLock::default();
    let both = shared(
        adopting_decl("VECTORIZE_DOCS", "shop-docs", "docs-shared"),
        adopting_decl("VECTORIZE_DOCS", "blog-docs", "docs-shared"),
    );
    assert!(vectorize_index_problem(&both, LedgerView::of(&lock, "production")).is_none());
    // Another environment already recording the shared name is no clash either.
    let mut recorded = DeployLock::default();
    recorded.record::<Vectorize>(
        "staging",
        "shop",
        "VECTORIZE_DOCS",
        VectorizeRecord::of("docs-shared", true),
    );
    assert!(vectorize_index_problem(&both, LedgerView::of(&recorded, "production")).is_none());
}

#[test]
fn an_entry_naming_no_capability_is_refused_with_the_nearest_binding() {
    let dir = std::env::temp_dir().join(format!("bynk-vz-typo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[deploy.vectorize.Doc]\nindex = { production = \"docs-prod\" }\n",
    )
    .unwrap();
    let mut all = BTreeMap::from([("shop".to_string(), resources(&[]))]);
    assert!(apply_vectorize_adoptions(&dir, &mut all, "production").is_err());
    assert_eq!(all["shop"].vectorize_indexes[0].adopt, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_unlisted_metadata_index_warning_allows_for_one_still_being_built() {
    // #3511: it must not claim the index is missing outright.
    let said = unlisted_metadata_index_warning("docs", "year:number", "year", "number");
    assert!(said.contains("does not list"), "{said}");
    assert!(said.contains("still be building"), "{said}");
    assert!(
        said.contains("create-metadata-index docs --propertyName year --type number"),
        "{said}"
    );
    assert!(!said.contains("has no metadata index"), "{said}");
}

#[test]
fn a_denied_token_names_the_permission_needed() {
    // #3495
    use crate::cloudflare::RestError;
    let denied: CloudflareError = RestError::answered(
        "GET",
        "/vectorize/v2/indexes",
        403,
        r#"[{"code":10000,"message":"Authentication error"}]"#,
    )
    .into();
    let hinted = with_permission_hint(&denied);
    assert!(hinted.starts_with(&denied.to_string()), "{hinted}");
    assert!(hinted.contains("Account > Vectorize > Edit"), "{hinted}");
    // Any other failure is passed through untouched.
    let other: CloudflareError =
        RestError::answered("GET", "/vectorize/v2/indexes/x", 404, r#"[{"code":3002}]"#).into();
    assert_eq!(with_permission_hint(&other), other.to_string());
}

/// 1D.4c (#4056): a refused metadata-index create that is never listed fails at the timeout,
/// and says what Cloudflare refused, rather than reading as a plain timeout.
#[test]
fn a_refused_metadata_index_that_never_appears_says_so_at_the_timeout() {
    let fake = Fake::new("refused-never", "[]", MATCHING, "[]");
    fake.metadata_index_exists();
    let mut lock = DeployLock::default();
    let failure = fake
        .run(&resources(&["category:string"]), &mut lock, "default")
        .unwrap_err();
    assert!(
        failure.message.contains("not listed after")
            && failure.message.contains("Cloudflare refused creating one")
            && failure.message.contains("40004"),
        "{}",
        failure.message
    );
}

/// 1D.4c (#4056 Decision E): every deploy of a declared index needs the credentials, recorded
/// or not, and so does a `--prune` of an orphan the project made; an adopted orphan is never
/// pruned, so it asks nothing.
#[test]
fn every_deploy_of_an_index_calls_the_account() {
    use crate::deploy::config::tests::project;
    let lock = DeployLock::default();
    let declared = project(vec![
        ("shop", resources(&[])),
        ("plain", Resources::default()),
    ]);
    let call = |order: &[&str], orphans: &[OrphanVectorize], prune: bool| {
        let order: Vec<String> = order.iter().map(|w| w.to_string()).collect();
        vectorize_calls_the_account(
            &ProjectCx {
                ledger: LedgerView::of(&lock, "default"),
                resources: &declared,
                order: &order,
            },
            orphans,
            prune,
        )
    };
    assert!(call(&["shop"], &[], false));
    assert!(!call(&["plain"], &[], false));
    let orphan = |adopted| OrphanVectorize {
        worker: "gone".into(),
        binding: "VECTORIZE_DOCS".into(),
        index: "gone-docs".into(),
        adopted,
    };
    assert!(call(&["plain"], &[orphan(false)], true));
    assert!(!call(&["plain"], &[orphan(false)], false));
    assert!(!call(&["plain"], &[orphan(true)], true));
}
