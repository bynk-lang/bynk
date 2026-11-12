//! #3319: the Hyperdrive deploy, against a fake of Cloudflare's API.

use super::*;
use crate::deploy::config::tests::project;
use crate::deploy::plan::tests::plan_of;
use bynk_project::hyperdrive::HyperdriveDeploy;
use std::cell::RefCell;

/// A connection string with parts that are each recognisable if they leak.
const STRING: &str =
    "postgres://svc_user:Hunter2-s3cret%21@db-host.internal.example:6543/shopdb?sslmode=require";
const PASSWORD: &str = "Hunter2-s3cret!";
const FAKE_HOST: &str = "db-host.internal.example";

/// Every part of [`STRING`] that must never be printed or stored.
fn secret_parts() -> [&'static str; 6] {
    [STRING, PASSWORD, "Hunter2", "s3cret", FAKE_HOST, "svc_user"]
}

fn assert_clean(what: &str, text: &str) {
    for part in secret_parts() {
        assert!(!text.contains(part), "{what} leaks `{part}`:\n{text}");
    }
}

/// A fake Cloudflare: records each call without its password, answers ids from
/// a counter, and can be told to fail echoing what it was sent.
#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<String>>,
    existing: RefCell<BTreeMap<String, String>>,
    echo_secret_on_failure: bool,
}

impl HyperdriveApi for Fake {
    fn create(&self, name: &str, origin: &Origin) -> Result<String, String> {
        let n = self.calls.borrow().len();
        self.calls.borrow_mut().push(format!(
            "create {name} {}:{}/{} as {}",
            origin.host, origin.port, origin.database, origin.user
        ));
        if self.echo_secret_on_failure {
            return Err(format!(
                "POST /hyperdrive/configs: Cloudflare answered 400 bad origin {}@{}",
                origin.password.expose(),
                origin.host
            ));
        }
        let id = format!("cfg-{n}");
        self.existing
            .borrow_mut()
            .insert(id.clone(), name.to_string());
        Ok(id)
    }

    fn update(&self, id: &str, name: &str, origin: &Origin) -> Result<(), String> {
        self.calls
            .borrow_mut()
            .push(format!("update {id} {name} {}", origin.host));
        Ok(())
    }

    fn name_of(&self, id: &str) -> Result<Option<String>, String> {
        self.calls.borrow_mut().push(format!("get {id}"));
        Ok(self.existing.borrow().get(id).cloned())
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("delete {id}"));
        Ok(())
    }
}

/// One capability's settings: its name, `connection_env`, and `config_id`s.
type Entry<'a> = (&'a str, Option<&'a str>, &'a [(&'a str, &'a str)]);

fn settings(entries: &[Entry<'_>]) -> HyperdriveDeployConfig {
    entries
        .iter()
        .map(|(capability, env, ids)| {
            (
                capability.to_string(),
                HyperdriveDeploy {
                    connection_env: env.map(str::to_string),
                    config_id: ids
                        .iter()
                        .map(|(e, i)| (e.to_string(), i.to_string()))
                        .collect(),
                },
            )
        })
        .collect()
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn shop() -> BTreeMap<String, Resources> {
    project(vec![(
        "shop",
        Resources::default().hyperdrive(&["HYPERDRIVE_MAIN"]),
    )])
}

fn lookup(value: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
    move |_| value.map(str::to_string)
}

const CONFIG: &str = "name = \"shop\"\n\n[[hyperdrive]]\nbinding = \"HYPERDRIVE_MAIN\"\nid = \"<HYPERDRIVE_MAIN_ID>\" # set at deploy time\n";

fn scratch(label: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "bynk-hyperdrive-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let config = root.join("wrangler.toml");
    std::fs::write(&config, CONFIG).unwrap();
    (root.clone(), root.join("bynk.deploy.lock"), config)
}

fn wants_of(value: &'static str, environment: &str) -> Wants {
    resolve_wants(
        &names(&["shop"]),
        &shop(),
        &settings(&[("Main", Some("MAIN_DATABASE_URL"), &[])]),
        environment,
        &lookup(Some(value)),
    )
    .unwrap()
}

fn run_with<'a>(wants: &'a Wants, api: &'a Fake) -> HyperdriveRun<'a> {
    HyperdriveRun {
        wants,
        api: Some(api),
    }
}

// ---- reading the string ----

#[test]
fn a_connection_string_is_read_into_the_parts_cloudflare_takes() {
    let origin = parse_connection_string(STRING).unwrap();
    assert_eq!(
        (origin.scheme.as_str(), origin.host.as_str(), origin.port),
        ("postgres", FAKE_HOST, 6543)
    );
    assert_eq!(origin.database, "shopdb");
    assert_eq!(origin.user, "svc_user");
    assert_eq!(origin.password.expose(), PASSWORD, "percent-decoded");
}

#[test]
fn the_port_defaults_and_ipv6_hosts_and_raw_at_signs_in_a_password_read() {
    let plain = parse_connection_string("postgresql://u:p@host/db").unwrap();
    assert_eq!((plain.scheme.as_str(), plain.port), ("postgres", 5432));
    let v6 = parse_connection_string("postgres://u:p@[::1]:5433/db").unwrap();
    assert_eq!((v6.host.as_str(), v6.port), ("::1", 5433));
    let at = parse_connection_string("postgres://u:p@ss@host/db").unwrap();
    assert_eq!((at.password.expose(), at.host.as_str()), ("p@ss", "host"));
}

#[test]
fn a_raw_hash_or_question_mark_in_a_password_is_part_of_the_password() {
    let hash = parse_connection_string("postgres://app:pa#ss@db.example.com/app").unwrap();
    assert_eq!(
        (
            hash.user.as_str(),
            hash.password.expose(),
            hash.host.as_str()
        ),
        ("app", "pa#ss", "db.example.com")
    );
    let question =
        parse_connection_string("postgres://app:p?ss@db.example.com:5433/app?x=1#f").unwrap();
    assert_eq!(
        (
            question.password.expose(),
            question.port,
            question.database.as_str()
        ),
        ("p?ss", 5433, "app")
    );
}

#[test]
fn a_malformed_string_names_the_part_that_is_wrong() {
    for (raw, problem) in [
        ("mysql://u:p@h/d", ParseProblem::Scheme),
        ("not a url", ParseProblem::Scheme),
        ("postgres://h/d", ParseProblem::Credentials),
        ("postgres://u@h/d", ParseProblem::Password),
        ("postgres://u:@h/d", ParseProblem::Password),
        ("postgres://u:p@/d", ParseProblem::Host),
        ("postgres://u:p@h:99999/d", ParseProblem::Port),
        ("postgres://u:p@h:0/d", ParseProblem::Port),
        ("postgres://u:p@h", ParseProblem::Database),
        ("postgres://u:p@h/", ParseProblem::Database),
    ] {
        assert_eq!(parse_connection_string(raw).err(), Some(problem), "{raw}");
    }
}

// ---- DECISIONS A and B: where a config comes from ----

#[test]
fn a_missing_variable_fails_naming_the_variable_and_the_capability() {
    let problems = resolve_wants(
        &names(&["shop"]),
        &shop(),
        &settings(&[("Main", Some("MAIN_DATABASE_URL"), &[])]),
        "default",
        &lookup(None),
    )
    .unwrap_err();
    assert_eq!(problems.len(), 1);
    let message = &problems[0];
    assert!(
        message.contains("`MAIN_DATABASE_URL` is not set"),
        "{message}"
    );
    assert!(message.contains("`HYPERDRIVE_MAIN` of `shop`"), "{message}");
    assert!(
        message.contains("`config_id`"),
        "names the other way out: {message}"
    );
}

#[test]
fn an_empty_variable_is_a_missing_one() {
    let problems = resolve_wants(
        &names(&["shop"]),
        &shop(),
        &settings(&[("Main", Some("MAIN_DATABASE_URL"), &[])]),
        "default",
        &lookup(Some("  ")),
    )
    .unwrap_err();
    assert!(problems[0].contains("is not set"), "{problems:?}");
}

#[test]
fn a_capability_with_no_settings_is_refused_pointing_at_the_table() {
    let problems = resolve_wants(
        &names(&["shop"]),
        &shop(),
        &settings(&[("Other", Some("X"), &[])]),
        "default",
        &lookup(Some(STRING)),
    )
    .unwrap_err();
    assert!(
        problems[0].contains("[deploy.hyperdrive.<Capability>]"),
        "{problems:?}"
    );
}

#[test]
fn an_unreadable_string_is_refused_without_being_echoed() {
    let problems = resolve_wants(
        &names(&["shop"]),
        &shop(),
        &settings(&[("Main", Some("MAIN_DATABASE_URL"), &[])]),
        "default",
        &lookup(Some(
            "mysql://svc_user:Hunter2-s3cret@db-host.internal.example/d",
        )),
    )
    .unwrap_err();
    assert!(problems[0].contains("`MAIN_DATABASE_URL`"), "{problems:?}");
    assert!(problems[0].contains("postgres://"), "{problems:?}");
    assert_clean("the unreadable-string refusal", &problems.join("\n"));
}

#[test]
fn a_per_environment_config_id_adopts_and_needs_no_string() {
    // The variable is not set, and does not matter for the adopted environment.
    let settings = settings(&[(
        "Main",
        Some("MAIN_DATABASE_URL"),
        &[("production", "abc123")],
    )]);
    let wants = resolve_wants(
        &names(&["shop"]),
        &shop(),
        &settings,
        "production",
        &lookup(None),
    )
    .unwrap();
    assert_eq!(
        wants[&("shop".to_string(), "HYPERDRIVE_MAIN".to_string())],
        Want::Adopt {
            id: "abc123".into()
        }
    );
    // Another environment of the same capability still wants its variable.
    assert!(
        resolve_wants(
            &names(&["shop"]),
            &shop(),
            &settings,
            "staging",
            &lookup(None)
        )
        .is_err()
    );
}

#[test]
fn every_problem_is_found_in_one_run() {
    let both = project(vec![(
        "shop",
        Resources::default().hyperdrive(&["HYPERDRIVE_MAIN", "HYPERDRIVE_REPORTS"]),
    )]);
    let problems = resolve_wants(
        &names(&["shop"]),
        &both,
        &settings(&[
            ("Main", Some("MAIN_DATABASE_URL"), &[]),
            ("Reports", Some("REPORTS_DATABASE_URL"), &[]),
        ]),
        "default",
        &lookup(None),
    )
    .unwrap_err();
    assert_eq!(problems.len(), 2, "{problems:?}");
}

// ---- the plan: create, sync, adopt (DECISION C) ----

#[test]
fn the_plan_creates_then_syncs_from_the_variable_and_never_prints_it() {
    let (_root, lock_path, config) = scratch("plan");
    let resources = shop();
    let order = names(&["shop"]);
    let api = Fake::default();
    let mut lock = DeployLock::default();

    let wants = wants_of(STRING, "default");
    let fresh =
        plan_of(&order, &resources, &lock).with_hyperdrive(&order, &resources, &lock, &wants);
    let report = plan_report(&fresh, DeployFormat::Short);
    assert!(
        report
            .contains("hyperdrive create shop-main (HYPERDRIVE_MAIN; from `MAIN_DATABASE_URL`)\n"),
        "{report}"
    );

    provision(
        &run_with(&wants, &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();

    let same =
        plan_of(&order, &resources, &lock).with_hyperdrive(&order, &resources, &lock, &wants);
    let report = plan_report(&same, DeployFormat::Short);
    assert!(
        report.contains("hyperdrive sync shop-main (HYPERDRIVE_MAIN; from `MAIN_DATABASE_URL`)\n"),
        "{report}"
    );

    let rotated = wants_of(
        "postgres://svc_user:Rotated-Pw-99@db-host.internal.example:6543/shopdb",
        "default",
    );
    let changed =
        plan_of(&order, &resources, &lock).with_hyperdrive(&order, &resources, &lock, &rotated);
    let report = plan_report(&changed, DeployFormat::Short);
    assert!(
        report.contains("hyperdrive sync shop-main (HYPERDRIVE_MAIN; from `MAIN_DATABASE_URL`)\n"),
        "{report}"
    );
    // The plan names the variable and never the value, in either format.
    assert_clean("the plan", &report);
    assert!(!report.contains("Rotated-Pw-99"), "{report}");
    let json = plan_report(&changed, DeployFormat::Json);
    assert_clean("the plan JSON", &json);
    assert!(
        !json.contains("Rotated-Pw-99") && !json.contains("db-host"),
        "{json}"
    );
    assert!(
        json.contains("\"action\": \"sync\"") || json.contains("\"action\":\"sync\""),
        "{json}"
    );
}

#[test]
fn the_plan_adopts_a_named_config_then_reuses_it() {
    let (_root, lock_path, config) = scratch("adopt");
    let resources = shop();
    let order = names(&["shop"]);
    let api = Fake::default();
    api.existing
        .borrow_mut()
        .insert("abc123".into(), "shop-prod".into());
    let mut lock = DeployLock::default();
    let wants = resolve_wants(
        &order,
        &resources,
        &settings(&[("Main", None, &[("default", "abc123")])]),
        "default",
        &lookup(None),
    )
    .unwrap();
    let plan =
        plan_of(&order, &resources, &lock).with_hyperdrive(&order, &resources, &lock, &wants);
    assert!(
        plan_report(&plan, DeployFormat::Short)
            .contains("hyperdrive adopt abc123 (HYPERDRIVE_MAIN)\n")
    );

    provision(
        &run_with(&wants, &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();
    assert_eq!(
        *api.calls.borrow(),
        ["get abc123"],
        "adopting only looks the config up"
    );
    let recorded = recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_MAIN", "default").unwrap();
    assert!(recorded.adopted);
    assert_eq!(
        (recorded.id.as_str(), recorded.name.as_str()),
        ("abc123", "shop-prod")
    );
    assert!(
        std::fs::read_to_string(&config)
            .unwrap()
            .contains("id = \"abc123\"")
    );

    let again =
        plan_of(&order, &resources, &lock).with_hyperdrive(&order, &resources, &lock, &wants);
    assert!(plan_report(&again, DeployFormat::Short).contains("hyperdrive reuse abc123"));
}

#[test]
fn adopting_a_config_the_account_does_not_have_fails() {
    let (_root, lock_path, config) = scratch("adopt-gone");
    let resources = shop();
    let wants = resolve_wants(
        &names(&["shop"]),
        &resources,
        &settings(&[("Main", None, &[("default", "nope")])]),
        "default",
        &lookup(None),
    )
    .unwrap();
    let mut lock = DeployLock::default();
    let e = provision(
        &run_with(&wants, &Fake::default()),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap_err();
    assert!(
        e.message.contains("`nope`") && e.message.contains("does not exist"),
        "{}",
        e.message
    );
    assert!(recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_MAIN", "default").is_none());
}

// ---- provisioning ----

#[test]
fn a_create_sends_the_parts_records_the_id_and_binds_it() {
    let (_root, lock_path, config) = scratch("create");
    let resources = shop();
    let api = Fake::default();
    let mut lock = DeployLock::default();
    let wants = wants_of(STRING, "default");
    provision(
        &run_with(&wants, &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();
    assert_eq!(
        *api.calls.borrow(),
        [format!(
            "create shop-main {FAKE_HOST}:6543/shopdb as svc_user"
        )]
    );
    let recorded = recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_MAIN", "default").unwrap();
    assert_eq!(
        (
            recorded.id.as_str(),
            recorded.name.as_str(),
            recorded.adopted
        ),
        ("cfg-0", "shop-main", false)
    );
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("id = \"cfg-0\""), "{text}");
    assert!(!text.contains("<HYPERDRIVE_MAIN_ID>"), "{text}");

    // A second run syncs the same config; it never makes a second one.
    let before = api.calls.borrow().len();
    provision(
        &run_with(&wants, &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();
    assert_eq!(api.calls.borrow().len(), before + 1);
    assert_eq!(
        api.calls.borrow().last().unwrap().split(' ').next(),
        Some("update")
    );
}

#[test]
fn every_deploy_syncs_the_same_config_and_keeps_its_id() {
    let (_root, lock_path, config) = scratch("update");
    let resources = shop();
    let api = Fake::default();
    let mut lock = DeployLock::default();
    provision(
        &run_with(&wants_of(STRING, "default"), &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();
    // The same string again: synced all the same, since nothing derived from
    // the string is kept to tell that it did not change.
    let rotated = wants_of(
        "postgres://svc_user:Rotated-Pw-99@new-host.example/shopdb",
        "default",
    );
    provision(
        &run_with(&rotated, &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();
    assert_eq!(
        api.calls.borrow().last().unwrap(),
        "update cfg-0 shop-main new-host.example"
    );
    let after = recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_MAIN", "default").unwrap();
    assert_eq!(after.id, "cfg-0", "an update keeps the id");
    assert!(after.superseded.is_empty(), "a sync leaves no orphan");
    // The ledger holds the id and the name, and nothing of the string.
    assert_clean("the ledger", &std::fs::read_to_string(&lock_path).unwrap());
}

#[test]
fn a_named_environment_makes_its_own_config_and_copies_the_stanza() {
    let (root, lock_path, config) = scratch("env");
    let resources = shop();
    let api = Fake::default();
    let mut lock = DeployLock::default();
    provision(
        &run_with(&wants_of(STRING, "default"), &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();
    provision(
        &run_with(&wants_of(STRING, "staging"), &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "staging",
    )
    .unwrap();
    assert!(
        api.calls
            .borrow()
            .iter()
            .any(|c| c.starts_with("create shop-main-staging ")),
        "{:?}",
        api.calls.borrow()
    );
    let staging = recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_MAIN", "staging").unwrap();
    assert_ne!(
        staging.id,
        recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_MAIN", "default")
            .unwrap()
            .id
    );

    // The `[env.staging]` block carries the stanza, with staging's id.
    let block = crate::deploy::config::synthesise_environment_block(
        &std::fs::read_to_string(&config).unwrap(),
        "staging",
        &BTreeMap::new(),
        &BTreeSet::new(),
    );
    let doc: toml::Table = block.unwrap_or_else(|e| panic!("{e}")).parse().unwrap();
    let entries = doc["env"]["staging"]["hyperdrive"].as_array().unwrap();
    assert_eq!(entries[0]["binding"].as_str(), Some("HYPERDRIVE_MAIN"));
    assert_eq!(entries[0]["id"].as_str(), Some(staging.id.as_str()));
    drop(root);
}

// ---- renames and pruning (standard-library D3) ----

#[test]
fn a_renamed_capability_plans_a_create_and_an_orphan_that_prune_deletes() {
    let (_root, lock_path, config) = scratch("rename");
    let api = Fake::default();
    let mut lock = DeployLock::default();
    let old = shop();
    provision(
        &run_with(&wants_of(STRING, "default"), &api),
        "shop",
        &old["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();

    // `capability Main = Postgres` becomes `capability Primary = Postgres`.
    let renamed = project(vec![(
        "shop",
        Resources::default().hyperdrive(&["HYPERDRIVE_PRIMARY"]),
    )]);
    let order = names(&["shop"]);
    let wants = resolve_wants(
        &order,
        &renamed,
        &settings(&[("Primary", Some("MAIN_DATABASE_URL"), &[])]),
        "default",
        &lookup(Some(STRING)),
    )
    .unwrap();
    let plan = plan_of(&order, &renamed, &lock).with_hyperdrive(&order, &renamed, &lock, &wants);
    let report = plan_report(&plan, DeployFormat::Short);
    assert!(
        report.contains(
            "hyperdrive create shop-primary (HYPERDRIVE_PRIMARY; from `MAIN_DATABASE_URL`)\n"
        ),
        "{report}"
    );
    assert!(
        report.contains("orphan hyperdrive shop HYPERDRIVE_MAIN (shop-main; deleted by --prune)\n"),
        "{report}"
    );
    let orphans = find_orphans(&lock, "default", &renamed);
    assert!(orphans.has_prunable(), "behind --prune");

    // Without --prune nothing is deleted: the orphan is only reported.
    assert!(!api.calls.borrow().iter().any(|c| c.starts_with("delete")));
    prune(&api, &mut lock, &lock_path, "default", &orphans.hyperdrive).unwrap();
    assert_eq!(api.calls.borrow().last().unwrap(), "delete cfg-0");
    assert!(recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_MAIN", "default").is_none());
    assert!(
        find_orphans(&lock, "default", &renamed)
            .hyperdrive
            .is_empty()
    );
}

#[test]
fn an_adopted_orphan_is_reported_and_never_deleted() {
    let (_root, lock_path, _config) = scratch("adopted-orphan");
    let api = Fake::default();
    let mut lock = DeployLock::default();
    lock.record_hyperdrive(
        "default",
        "shop",
        "HYPERDRIVE_GONE",
        HyperdriveRecord {
            name: "theirs".into(),
            id: "abc123".into(),
            adopted: true,
            superseded: Vec::new(),
        },
    );
    let orphans = find_orphans(
        &lock,
        "default",
        &project(vec![("shop", Resources::default())]),
    );
    assert_eq!(orphans.hyperdrive.len(), 1);
    assert!(!orphans.has_prunable(), "an adopted config is not prunable");
    prune(&api, &mut lock, &lock_path, "default", &orphans.hyperdrive).unwrap();
    assert!(api.calls.borrow().is_empty(), "no delete was sent");
    assert!(recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_GONE", "default").is_some());
}

#[test]
fn switching_a_binding_from_created_to_adopted_orphans_the_old_config() {
    let (_root, lock_path, config) = scratch("switch");
    let resources = shop();
    let api = Fake::default();
    api.existing
        .borrow_mut()
        .insert("abc123".into(), "theirs".into());
    let mut lock = DeployLock::default();
    provision(
        &run_with(&wants_of(STRING, "default"), &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();
    let adopt = resolve_wants(
        &names(&["shop"]),
        &resources,
        &settings(&[("Main", None, &[("default", "abc123")])]),
        "default",
        &lookup(None),
    )
    .unwrap();
    provision(
        &run_with(&adopt, &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();
    let orphans = find_orphans(&lock, "default", &resources);
    assert_eq!(orphans.hyperdrive.len(), 1);
    assert_eq!(orphans.hyperdrive[0].id, "cfg-0");
    assert!(!orphans.hyperdrive[0].adopted);
}

#[test]
fn ci_will_not_make_a_config_it_cannot_commit() {
    let resources = shop();
    let order = names(&["shop"]);
    let wants = wants_of(STRING, "default");
    let lock = DeployLock::default();
    assert_eq!(
        first_unrecorded(&order, &resources, &lock, "default", &wants),
        Some(("shop", "HYPERDRIVE_MAIN"))
    );
    let adopted = resolve_wants(
        &order,
        &resources,
        &settings(&[("Main", None, &[("default", "abc")])]),
        "default",
        &lookup(None),
    )
    .unwrap();
    assert_eq!(
        first_unrecorded(&order, &resources, &lock, "default", &adopted),
        None
    );
}

// ---- R-16: the connection string reaches nothing it must not ----

#[test]
fn the_string_is_in_no_ledger_no_generated_file_and_no_plan() {
    let (root, lock_path, config) = scratch("r16");
    let resources = shop();
    let order = names(&["shop"]);
    let api = Fake::default();
    let mut lock = DeployLock::default();
    let wants = wants_of(STRING, "default");
    provision(
        &run_with(&wants, &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();

    // The ledger, as written to disk.
    assert_clean("the ledger", &std::fs::read_to_string(&lock_path).unwrap());
    // The generated configuration.
    assert_clean("wrangler.toml", &std::fs::read_to_string(&config).unwrap());
    // Every file the run wrote.
    for entry in std::fs::read_dir(&root).unwrap().flatten() {
        assert_clean(
            &entry.path().display().to_string(),
            &std::fs::read_to_string(entry.path()).unwrap(),
        );
    }
    // The plan, both formats.
    let plan =
        plan_of(&order, &resources, &lock).with_hyperdrive(&order, &resources, &lock, &wants);
    assert_clean("the plan", &plan_report(&plan, DeployFormat::Short));
    assert_clean("the plan JSON", &plan_report(&plan, DeployFormat::Json));
    // The debug forms of everything that holds it (a stray `{:?}` or an
    // `assert_eq!` failure would print them).
    assert_clean("Debug of the wants", &format!("{wants:?}"));
    assert_clean(
        "Debug of an origin",
        &format!("{:?}", parse_connection_string(STRING).unwrap()),
    );
    // A second environment's ledger and a named environment's block.
    provision(
        &run_with(&wants_of(STRING, "staging"), &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "staging",
    )
    .unwrap();
    assert_clean(
        "the ledger with staging",
        &std::fs::read_to_string(&lock_path).unwrap(),
    );
    assert_clean(
        "wrangler.toml with staging",
        &std::fs::read_to_string(&config).unwrap(),
    );
}

#[test]
fn what_cloudflare_answers_is_scrubbed_before_it_is_shown() {
    let (_root, lock_path, config) = scratch("scrub");
    let resources = shop();
    let api = Fake {
        echo_secret_on_failure: true,
        ..Fake::default()
    };
    let mut lock = DeployLock::default();
    let e = provision(
        &run_with(&wants_of(STRING, "default"), &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap_err();
    assert!(
        e.message
            .contains("could not create the Hyperdrive config `shop-main`"),
        "{}",
        e.message
    );
    assert!(e.message.contains("<redacted>"), "{}", e.message);
    assert!(!e.message.contains(PASSWORD), "{}", e.message);
    assert!(
        recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_MAIN", "default").is_none(),
        "a failed create records nothing"
    );
}

#[test]
fn every_message_of_the_flow_is_free_of_the_string() {
    // The refusals the resolver can print, each given the string.
    let mut messages = Vec::new();
    for value in [
        Some(STRING),
        None,
        Some("mysql://svc_user:Hunter2-s3cret@db-host.internal.example/d"),
    ] {
        for settings in [
            settings(&[("Main", Some("MAIN_DATABASE_URL"), &[])]),
            settings(&[("Other", Some("X"), &[])]),
            settings(&[("Main", None, &[("staging", "id")])]),
        ] {
            if let Err(problems) = resolve_wants(
                &names(&["shop"]),
                &shop(),
                &settings,
                "default",
                &lookup(value),
            ) {
                messages.extend(problems);
            }
        }
    }
    assert!(!messages.is_empty());
    assert_clean("a resolver message", &messages.join("\n"));
}

#[test]
fn scrub_removes_the_string_and_the_password() {
    let said = format!("bad origin {STRING} (password {PASSWORD})");
    let clean = scrub(&said, &[STRING, PASSWORD]);
    assert_eq!(clean, "bad origin <redacted> (password <redacted>)");
    assert_eq!(scrub("fine", &[STRING]), "fine");
}

#[test]
fn moving_between_adopted_configs_leaves_no_orphan() {
    let (_root, lock_path, config) = scratch("adopt-adopt");
    let resources = shop();
    let api = Fake::default();
    api.existing
        .borrow_mut()
        .insert("abc123".into(), "a".into());
    api.existing
        .borrow_mut()
        .insert("def456".into(), "b".into());
    let mut lock = DeployLock::default();
    for id in ["abc123", "def456"] {
        let wants = resolve_wants(
            &names(&["shop"]),
            &resources,
            &settings(&[("Main", None, &[("default", id)])]),
            "default",
            &lookup(None),
        )
        .unwrap();
        provision(
            &run_with(&wants, &api),
            "shop",
            &resources["shop"],
            &mut lock,
            &lock_path,
            &config,
            "default",
        )
        .unwrap();
    }
    assert_eq!(
        recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_MAIN", "default")
            .unwrap()
            .id,
        "def456"
    );
    assert!(
        find_orphans(&lock, "default", &resources)
            .hyperdrive
            .is_empty()
    );

    // And from an adopted config to one the deploy makes: the adopted one was
    // never the project's, so it is not an orphan either.
    provision(
        &run_with(&wants_of(STRING, "default"), &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();
    assert!(
        find_orphans(&lock, "default", &resources)
            .hyperdrive
            .is_empty()
    );
    assert!(
        !recorded_hyperdrive(&lock, "shop", "HYPERDRIVE_MAIN", "default")
            .unwrap()
            .adopted
    );
}

// ---- #3322: a `MySql` instance's config ----

const MYSQL_STRING: &str =
    "mysql://svc_user:Hunter2-s3cret%21@db-host.internal.example:3307/shopdb?ssl-mode=REQUIRED";

fn mysql_shop(tls: Option<&str>) -> BTreeMap<String, Resources> {
    let mut resources = shop();
    let decl = &mut resources.get_mut("shop").unwrap().hyperdrive[0];
    decl.kind = HyperdriveKind::MySql;
    decl.tls = tls.map(str::to_string);
    resources
}

fn mysql_wants(raw: &'static str, tls: Option<&str>) -> Result<Wants, Vec<String>> {
    resolve_wants(
        &names(&["shop"]),
        &mysql_shop(tls),
        &settings(&[("Main", Some("MAIN_DATABASE_URL"), &[])]),
        "default",
        &lookup(Some(raw)),
    )
}

#[test]
fn a_mysql_string_reads_with_its_own_scheme_and_port() {
    let origin = parse_connection_string_for(HyperdriveKind::MySql, MYSQL_STRING).unwrap();
    assert_eq!(
        (origin.scheme.as_str(), origin.host.as_str(), origin.port),
        ("mysql", FAKE_HOST, 3307)
    );
    assert_eq!(
        (origin.database.as_str(), origin.user.as_str()),
        ("shopdb", "svc_user")
    );
    assert_eq!(origin.password.expose(), PASSWORD);
    let default_port =
        parse_connection_string_for(HyperdriveKind::MySql, "mysql://u:p@h/d").unwrap();
    assert_eq!(default_port.port, 3306);
    // A string for the other database is refused, naming the right scheme.
    assert_eq!(
        parse_connection_string_for(HyperdriveKind::MySql, STRING).err(),
        Some(ParseProblem::Scheme)
    );
    let problems = mysql_wants(STRING, None).unwrap_err();
    assert!(
        problems[0].contains("MySQL connection URL (`mysql://"),
        "{}",
        problems[0]
    );
    assert!(
        problems[0].contains("must start with `mysql://`"),
        "{}",
        problems[0]
    );
    assert!(
        problems[0].contains("`MySql` capability"),
        "{}",
        problems[0]
    );
    assert_clean("a refused string's message", &problems.join("\n"));
}

#[test]
fn a_mysql_config_is_made_with_the_required_tls_mode_and_a_postgres_one_with_none() {
    let wants = mysql_wants(MYSQL_STRING, Some("required")).unwrap();
    let Want::Connection { origin, .. } =
        &wants[&("shop".to_string(), "HYPERDRIVE_MAIN".to_string())]
    else {
        panic!("a connection");
    };
    let body = origin_json("shop-main", origin);
    assert_eq!(body["origin"]["scheme"], "mysql");
    assert_eq!(body["mtls"]["sslmode"], "REQUIRED");
    // The default is the same as `required`.
    assert!(mysql_wants(MYSQL_STRING, None).is_ok());
    let pg = parse_connection_string(STRING).unwrap();
    assert!(origin_json("shop-main", &pg).get("mtls").is_none());
}

#[test]
fn a_verify_mode_is_refused_for_a_made_config_and_adopted_ones_are_not() {
    for (mode, sslmode) in [
        ("verify-ca", "VERIFY_CA"),
        ("verify-identity", "VERIFY_IDENTITY"),
    ] {
        let problems = mysql_wants(MYSQL_STRING, Some(mode)).unwrap_err();
        assert!(
            problems[0].contains(&format!("@tls(\"{mode}\")")),
            "{}",
            problems[0]
        );
        assert!(
            problems[0].contains(sslmode) && problems[0].contains("config_id"),
            "{}",
            problems[0]
        );
        assert_clean("the verify message", &problems.join("\n"));
        // The guidance comes first: a user with no connection string (as a
        // verify mode wants) and no variable set is told to adopt, not that
        // the variable is missing.
        let unset = resolve_wants(
            &names(&["shop"]),
            &mysql_shop(Some(mode)),
            &settings(&[("Main", Some("MAIN_DATABASE_URL"), &[])]),
            "default",
            &lookup(None),
        )
        .unwrap_err();
        assert!(
            unset[0].contains("`config_id`") && unset[0].contains("certificate"),
            "{}",
            unset[0]
        );
        assert!(!unset[0].contains("is not set"), "{}", unset[0]);
        // An adopted config has its TLS settings already: nothing to refuse.
        let adopted = resolve_wants(
            &names(&["shop"]),
            &mysql_shop(Some(mode)),
            &settings(&[("Main", None, &[("default", "cfg-1")])]),
            "default",
            &lookup(None),
        );
        assert!(adopted.is_ok(), "{mode}");
    }
}

#[test]
fn a_mysql_string_is_in_no_ledger_no_generated_file_and_no_plan() {
    let (root, lock_path, config) = scratch("mysql-r16");
    let resources = mysql_shop(Some("required"));
    let order = names(&["shop"]);
    let api = Fake::default();
    let mut lock = DeployLock::default();
    let wants = mysql_wants(MYSQL_STRING, Some("required")).unwrap();
    provision(
        &run_with(&wants, &api),
        "shop",
        &resources["shop"],
        &mut lock,
        &lock_path,
        &config,
        "default",
    )
    .unwrap();
    assert_clean("the ledger", &std::fs::read_to_string(&lock_path).unwrap());
    assert_clean("wrangler.toml", &std::fs::read_to_string(&config).unwrap());
    for entry in std::fs::read_dir(&root).unwrap().flatten() {
        assert_clean(
            &entry.path().display().to_string(),
            &std::fs::read_to_string(entry.path()).unwrap(),
        );
    }
    let plan =
        plan_of(&order, &resources, &lock).with_hyperdrive(&order, &resources, &lock, &wants);
    assert_clean("the plan", &plan_report(&plan, DeployFormat::Short));
    assert_clean("the plan JSON", &plan_report(&plan, DeployFormat::Json));
    assert_clean("Debug of the wants", &format!("{wants:?}"));
    let Want::Connection { origin, .. } =
        &wants[&("shop".to_string(), "HYPERDRIVE_MAIN".to_string())]
    else {
        panic!("a connection");
    };
    assert_clean("Debug of an origin", &format!("{origin:?}"));
}
