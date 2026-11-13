//! #3479: the Vectorize deploy path, against a fake `wrangler` that logs each
//! call and answers `vectorize` subcommands from files beside it. The shapes it
//! answers with are the ones `parse_*` reads; whether Cloudflare's wrangler
//! prints them is not something a test can pin.

#![cfg_attr(not(unix), allow(unused))]

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

/// A directory holding a fake `wrangler` and the JSON it answers with.
#[cfg(unix)]
struct Fake {
    dir: PathBuf,
}

#[cfg(unix)]
impl Fake {
    fn new(tag: &str, list: &str, get: &str, meta: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bynk-vz-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("list.json"), list).unwrap();
        std::fs::write(dir.join("get.json"), get).unwrap();
        std::fs::write(dir.join("meta.json"), meta).unwrap();
        let script = dir.join("wrangler");
        std::fs::write(
            &script,
            "#!/bin/sh\nd=$(dirname \"$0\")\necho \"$@\" >> \"$d/calls.log\"\n[ \"$1\" = vectorize ] || exit 2\ncase \"$2\" in\n  list) cat \"$d/list.json\";;\n  get) cat \"$d/get.json\";;\n  list-metadata-index) cat \"$d/meta.json\";;\n  *) exit 0;;\nesac\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir }
    }

    fn provenance(&self) -> Provenance {
        Provenance::Path(self.dir.join("wrangler"))
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn run(
        &self,
        declared: &Resources,
        lock: &mut DeployLock,
        environment: &str,
    ) -> Result<(), DeployFailure> {
        let lock_path = self.dir.join("bynk.lock");
        provision_vectorize_indexes(
            &self.provenance(),
            &self.dir,
            "shop",
            declared,
            lock,
            &lock_path,
            environment,
        )
    }
}

const MATCHING: &str = r#"{"name":"shop-docs","config":{"dimensions":768,"metric":"cosine"}}"#;

#[test]
fn the_account_output_is_read_defensively() {
    let top = parse_index_config(r#"{"dimensions":3,"metric":"DOT_PRODUCT"}"#).unwrap();
    assert_eq!((top.dimensions, top.metric.as_str()), (3, "dot-product"));
    assert_eq!(parse_index_config(MATCHING).unwrap().dimensions, 768);
    assert_eq!(parse_index_config(r#"{"name":"x"}"#), None);
    assert_eq!(parse_index_config("not json"), None);
    assert_eq!(
        parse_index_names(r#"[{"name":"a"},{"name":"b"}]"#).unwrap(),
        ["a", "b"]
    );
    assert_eq!(parse_index_names(r#"{"name":"a"}"#), None);
    assert_eq!(
        parse_metadata_indexes(r#"[{"propertyName":"category","indexType":"String"}]"#).unwrap(),
        ["category:string"]
    );
    assert_eq!(
        parse_metadata_indexes(
            r#"{"metadataIndexes":[{"property_name":"y","index_type":"number"}]}"#
        )
        .unwrap(),
        ["y:number"]
    );
    assert_eq!(parse_metadata_indexes(r#"[{"x":1}]"#), None);
}

#[test]
fn metadata_index_changes_are_additions_and_removals() {
    let d = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let (missing, removed) = metadata_index_changes(
        &d(&["a:string", "b:number"]),
        &d(&["b:number", "c:boolean"]),
    );
    assert_eq!((missing, removed), (d(&["a:string"]), d(&["c:boolean"])));
}

#[cfg(unix)]
#[test]
fn a_missing_index_is_created_with_its_configuration_then_its_metadata_indexes() {
    let fake = Fake::new("create", "[]", MATCHING, "[]");
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
        ]
    );
    let recorded = recorded_vectorize(&lock, "shop", "VECTORIZE_DOCS", "default").unwrap();
    assert_eq!(
        (recorded.name.as_str(), recorded.adopted),
        ("shop-docs", false)
    );
}

#[cfg(unix)]
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
        recorded_vectorize(&lock, "shop", "VECTORIZE_DOCS", "staging")
            .unwrap()
            .name,
        "shop-docs-staging"
    );
}

#[cfg(unix)]
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
        assert!(recorded_vectorize(&lock, "shop", "VECTORIZE_DOCS", "default").is_none());
    }
}

#[cfg(unix)]
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

#[cfg(unix)]
#[test]
fn a_later_metadata_index_is_created_and_a_removed_one_is_left() {
    let fake = Fake::new(
        "later",
        r#"[{"name":"shop-docs"}]"#,
        MATCHING,
        r#"[{"propertyName":"old","indexType":"string"}]"#,
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

#[cfg(unix)]
#[test]
fn pruning_deletes_an_existing_index_and_is_quiet_about_one_already_gone() {
    let fake = Fake::new("prune", r#"[{"name":"shop-old"}]"#, MATCHING, "[]");
    delete_vectorize_index(&fake.provenance(), "shop-old", &fake.dir).unwrap();
    assert!(
        fake.calls()
            .contains(&"vectorize delete shop-old --force".to_string())
    );
    delete_vectorize_index(&fake.provenance(), "shop-gone", &fake.dir).unwrap();
    assert_eq!(
        fake.calls().iter().filter(|c| c.contains("delete")).count(),
        1
    );
}

#[test]
fn the_ledger_records_supersedes_orphans_and_forgets() {
    let mut lock = DeployLock::default();
    lock.record_vectorize("default", "shop", "VECTORIZE_DOCS", "shop-docs", false);
    // Live and declared: no orphan.
    let live = BTreeMap::from([("shop".to_string(), resources(&[]))]);
    assert!(find_orphans(&lock, "default", &live).vectorize.is_empty());
    // A renamed instance leaves the old index recorded and reported.
    lock.record_vectorize("default", "shop", "VECTORIZE_DOCS", "shop-docs-v2", false);
    let orphans = find_orphans(&lock, "default", &BTreeMap::new());
    assert_eq!(orphans.vectorize.len(), 2);
    assert!(orphans.has_prunable());
    // Forgetting the superseded one leaves the live record.
    lock.forget_vectorize("default", "shop", "VECTORIZE_DOCS", "shop-docs");
    assert_eq!(
        recorded_vectorize(&lock, "shop", "VECTORIZE_DOCS", "default")
            .unwrap()
            .superseded,
        []
    );
    lock.forget_vectorize("default", "shop", "VECTORIZE_DOCS", "shop-docs-v2");
    assert!(recorded_vectorize(&lock, "shop", "VECTORIZE_DOCS", "default").is_none());
    // An adopted index is reported and never prunable.
    lock.record_vectorize("default", "shop", "VECTORIZE_DOCS", "legacy", true);
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
    assert!(vectorize_index_problem(&resources, &lock, "default").is_none());
    let message = vectorize_index_problem(&resources, &lock, "production").expect("65 bytes");
    assert!(
        message.contains("VECTORIZE_DOCS") && message.contains("1 to 64 bytes"),
        "{message}"
    );
}

#[test]
fn moving_back_to_a_left_index_does_not_make_the_live_one_an_orphan() {
    let mut lock = DeployLock::default();
    lock.record_vectorize("default", "shop", "VECTORIZE_DOCS", "a", false);
    lock.record_vectorize("default", "shop", "VECTORIZE_DOCS", "b", false);
    lock.record_vectorize("default", "shop", "VECTORIZE_DOCS", "a", false);
    let record = recorded_vectorize(&lock, "shop", "VECTORIZE_DOCS", "default").unwrap();
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

#[cfg(unix)]
fn adopting(name: &str, indexed: &[&str]) -> Resources {
    let mut declared = resources(indexed);
    declared.vectorize_indexes[0].adopt = Some(name.to_string());
    declared
}

#[test]
#[cfg(unix)]
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
    let recorded = recorded_vectorize(&lock, "shop", "VECTORIZE_DOCS", "production").unwrap();
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
#[cfg(unix)]
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
    assert!(recorded_vectorize(&lock, "shop", "VECTORIZE_DOCS", "production").is_none());
}

#[test]
#[cfg(unix)]
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
