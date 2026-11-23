//! Back-half paydown 1D.2 (#4001): the characterisation fixtures of what the apply does, added
//! before any of the apply code moves, and never re-blessed by 1D.2's slices. A diff here is a
//! change in what `bynk deploy` asks of the account or writes to `bynk.deploy.lock`, and when,
//! which 1D.2 promises not to make (#4001's "Goldens", and ADR 0180).
//!
//! The every-kind project of `snapshot_tests.rs` is applied, Worker by Worker in the upload order,
//! against the every-kind ledger, in each of its two environments, through the recording account
//! below and a fake of Cloudflare's Hyperdrive API. Each Worker that pushes is recorded as `run`
//! records it, and once both have, `--prune` deletes the orphans. Three fixtures pin it:
//!
//! - `every_kind.apply.calls.txt`: every account call, in order, numbered (the push's deploy facts
//!   are stripped, as the provisioning tests strip them);
//! - `every_kind.apply.writes.txt`: each ledger write, by the call it followed, as the lines it
//!   changed from the ledger before it;
//! - `every_kind.apply.failures.txt`: for each call that changes the account, the run again with
//!   that call failing, and for each write, the run again with that write failing: the failure's
//!   message and the ledger left on disk.
//!
//! `BYNK_WRITE_APPLY_FIXTURES=1` writes the fixtures instead of comparing them. It exists to make
//! them, once; a slice of 1D.2 that needed it would be a stop-and-report, never a bless.

use super::*;
use crate::deploy::snapshot_tests::{LEDGER, every_kind, ledger, order};
use std::cell::RefCell;
use std::path::PathBuf;

const CALLS: &str = include_str!("fixtures/every_kind.apply.calls.txt");
const WRITES: &str = include_str!("fixtures/every_kind.apply.writes.txt");
const FAILURES: &str = include_str!("fixtures/every_kind.apply.failures.txt");
const CONFIGS: &str = include_str!("fixtures/every_kind.apply.configs.txt");

const ENVIRONMENTS: [&str; 2] = ["default", "staging"];

/// The account the apply runs against (1D.3b: the recording fake, where it was a `#!/bin/sh`
/// stand-in): it logs each call, counts it, fails the one `fail_at` names, and answers from a
/// small account kept in `state/` (the indexes, their metadata indexes and the queues), exactly
/// as the stand-in did, so the fixtures are unchanged.
fn account(root: PathBuf) -> crate::cloudflare::fake::Recording {
    use crate::cloudflare::Op;
    use crate::cloudflare::fake::{Recording, failed};
    Recording::new(move |op| {
        let n = calls_so_far(&root) + 1;
        std::fs::write(root.join("n"), format!("{n}\n")).unwrap();
        let mut log = std::fs::read_to_string(root.join("calls.log")).unwrap_or_default();
        log.push_str(&op.line());
        log.push('\n');
        std::fs::write(root.join("calls.log"), log).unwrap();
        if std::fs::read_to_string(root.join("fail_at")).is_ok_and(|at| at.trim().parse() == Ok(n))
        {
            return Err(failed(
                op,
                "",
                &format!("✘ [ERROR] the stand-in failed call {n}\n"),
            ));
        }
        let state = root.join("state");
        let at = |file: String| state.join(file);
        // The files `prefix*` in `state/`, in name order.
        let listed = |prefix: &str| {
            let mut names: Vec<String> = std::fs::read_dir(&state)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(prefix))
                .collect();
            names.sort();
            names
        };
        Ok(match *op {
            Op::KvCreate { title } => format!("kv-{title}"),
            Op::D1Create { name } => format!("0d1d0000-0000-4000-8000-0000000000{:02}", name.len()),
            Op::Vectorize(VectorizeOp::List) => {
                let names: Vec<String> = listed("index.")
                    .iter()
                    .map(|f| format!("{{\"name\":\"{}\"}}", &f["index.".len()..]))
                    .collect();
                format!("[{}]\n", names.join(","))
            }
            Op::Vectorize(VectorizeOp::Get { name }) => {
                std::fs::read_to_string(at(format!("index.{name}"))).unwrap_or_default()
            }
            Op::Vectorize(VectorizeOp::CreateMetadataIndex { name, property, ty }) => {
                std::fs::write(
                    at(format!("meta.{name}.{property}")),
                    format!("{{\"propertyName\":\"{property}\",\"indexType\":\"{ty}\"}}"),
                )
                .unwrap();
                String::new()
            }
            Op::Vectorize(VectorizeOp::Create {
                name,
                dimensions,
                metric,
            }) => {
                std::fs::write(
                    at(format!("index.{name}")),
                    format!(
                        "{{\"config\":{{\"dimensions\":{dimensions},\"metric\":\"{metric}\"}}}}"
                    ),
                )
                .unwrap();
                String::new()
            }
            Op::Vectorize(VectorizeOp::ListMetadataIndex { name }) => {
                let metas: Vec<String> = listed(&format!("meta.{name}."))
                    .iter()
                    .map(|f| std::fs::read_to_string(at(f.clone())).unwrap())
                    .collect();
                format!("[{}]\n", metas.join(","))
            }
            Op::Vectorize(VectorizeOp::Delete { name }) => {
                let _ = std::fs::remove_file(at(format!("index.{name}")));
                String::new()
            }
            Op::QueueInfo { name } => {
                if !at(format!("queue.{name}")).exists() {
                    return Err(failed(op, "", ""));
                }
                String::new()
            }
            Op::QueueCreate { name } => {
                std::fs::write(at(format!("queue.{name}")), "").unwrap();
                String::new()
            }
            Op::QueueDelete { name } => {
                let _ = std::fs::remove_file(at(format!("queue.{name}")));
                String::new()
            }
            _ => String::new(),
        })
    })
}

/// One run's scratch directory: the project root and the account's state.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(environment: &str, fail_at: Option<usize>) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "bynk-1d2-apply-{environment}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("state")).expect("a scratch directory");
        std::fs::write(root.join("n"), "0\n").unwrap();
        if let Some(at) = fail_at {
            std::fs::write(root.join("fail_at"), format!("{at}\n")).unwrap();
        }
        // What the account holds before the run: the index the ledger records as made, without
        // its metadata index, and the queues it records.
        let qualified = |name: &str| env_qualify(environment, name);
        std::fs::write(
            root.join("state").join(format!(
                "index.{}",
                vectorize_index_name(environment, "shop-docs")
            )),
            r#"{"config":{"dimensions":768,"metric":"cosine"}}"#,
        )
        .unwrap();
        for queue in ["intake", "shared-feed", "old-jobs"] {
            std::fs::write(
                root.join("state")
                    .join(format!("queue.{}", qualified(queue))),
                "",
            )
            .unwrap();
        }
        std::fs::write(root.join(LOCK_FILE), LEDGER).unwrap();
        Scratch { root }
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    fn calls_so_far(&self) -> usize {
        calls_so_far(&self.root)
    }

    /// Count a call the fake API answers, as the stand-in counts its own: whether it is the one to
    /// fail.
    fn call(&self, line: &str) -> bool {
        let n = self.calls_so_far() + 1;
        std::fs::write(self.root.join("n"), format!("{n}\n")).unwrap();
        let mut log = std::fs::read_to_string(self.root.join("calls.log")).unwrap_or_default();
        log.push_str(line);
        log.push('\n');
        std::fs::write(self.root.join("calls.log"), log).unwrap();
        std::fs::read_to_string(self.root.join("fail_at"))
            .is_ok_and(|at| at.trim().parse() == Ok(n))
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|call| without_secrets_file(&strip_facts_var(call)))
            .collect()
    }
}

/// How many account calls the run in `root` has made.
fn calls_so_far(root: &Path) -> usize {
    std::fs::read_to_string(root.join("n"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Cloudflare's Hyperdrive API, logging into the stand-in's call log.
struct FakeHyperdrive<'s> {
    scratch: &'s Scratch,
}

impl FakeHyperdrive<'_> {
    fn answer<T>(&self, line: String, ok: T) -> Result<T, crate::cloudflare::CloudflareError> {
        if self.scratch.call(&line) {
            let n = self.scratch.calls_so_far();
            return Err(format!("Cloudflare answered 500 to call {n}").into());
        }
        Ok(ok)
    }
}

impl hyperdrive::HyperdriveApi for FakeHyperdrive<'_> {
    fn create(
        &self,
        name: &str,
        origin: &hyperdrive::Origin,
    ) -> Result<String, crate::cloudflare::CloudflareError> {
        self.answer(
            format!("hyperdrive create {name} {}", origin.host),
            format!("hd-{name}"),
        )
    }

    fn update(
        &self,
        id: &str,
        name: &str,
        origin: &hyperdrive::Origin,
    ) -> Result<(), crate::cloudflare::CloudflareError> {
        self.answer(format!("hyperdrive update {id} {name} {}", origin.host), ())
    }

    fn name_of(&self, id: &str) -> Result<Option<String>, crate::cloudflare::CloudflareError> {
        self.answer(
            format!("hyperdrive get {id}"),
            Some(format!("{id}-on-account")),
        )
    }

    fn delete(&self, id: &str) -> Result<(), crate::cloudflare::CloudflareError> {
        self.answer(format!("hyperdrive delete {id}"), ())
    }
}

/// What each `Postgres` binding wants: one adopted by id, two made from a connection string.
fn wants() -> hyperdrive::Wants {
    let connection = |variable: &str| {
        let raw = "postgres://svc:pw@db.example:5432/shop";
        hyperdrive::Want::Connection {
            variable: variable.to_string(),
            origin: hyperdrive::parse_connection_string(raw).expect("a connection string"),
            raw: hyperdrive::Secret::of(raw),
        }
    };
    hyperdrive::Wants::from([
        (
            ("shop".to_string(), "HYPERDRIVE_ADOPTED".to_string()),
            hyperdrive::Want::Adopt {
                id: "hd-adopted".to_string(),
            },
        ),
        (
            ("shop".to_string(), "HYPERDRIVE_MAIN".to_string()),
            connection("MAIN_URL"),
        ),
        (
            ("shop".to_string(), "HYPERDRIVE_NEW".to_string()),
            connection("NEW_URL"),
        ),
    ])
}

/// The ids `link_for_run` would have found before the run confirmed.
fn linked() -> BTreeMap<(String, String), String> {
    BTreeMap::from([
        (
            ("shop".to_string(), "SECRET_STRIPE_KEY".to_string()),
            "store-1".to_string(),
        ),
        (
            ("shop".to_string(), "MTLS_BACKEND".to_string()),
            "cert-2".to_string(),
        ),
    ])
}

/// The account's live KV namespace ids, as `run` reads them once: every recorded id but
/// `shop`'s `KV_SESSIONS` in `staging`, which has gone, so the apply makes it again although
/// the plan says `reuse`.
fn live_kv() -> BTreeSet<String> {
    [
        "kv-gone-default",
        "kv-shop-default",
        "kv-gone-cache-default",
        "kv-shop-old-default",
        "kv-shop-sessions-default",
        "kv-shop-staging",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// The generated configuration of `worker`, as the build leaves it for the apply to complete.
fn generated_config(worker: &str, declared: &Resources) -> String {
    use bynk_emit::emitter::wrangler as w;
    let mut config = format!("name = \"{worker}\"\nmain = \"index.ts\"\n");
    for binding in kv_bindings(declared) {
        config.push_str(&format!(
            "\n[[kv_namespaces]]\nbinding = \"{binding}\"\nid = \"{}\" # set at deploy time\n",
            w::kv_id_placeholder(binding)
        ));
    }
    for bucket in &declared.r2_buckets {
        config.push_str(&format!(
            "\n[[r2_buckets]]\nbinding = \"{}\"\nbucket_name = \"{}\"\n",
            bucket.binding, bucket.name
        ));
    }
    for db in &declared.d1_databases {
        config.push_str(&format!(
            "\n[[d1_databases]]\nbinding = \"{}\"\ndatabase_name = \"{}\"\ndatabase_id = \"{}\" # set at deploy time\n",
            db.binding,
            db.name,
            w::d1_database_id_placeholder(&db.binding)
        ));
        if !db.migrations.is_empty() {
            config.push_str(&format!("migrations_dir = \"migrations/{}\"\n", db.name));
        }
    }
    for h in &declared.hyperdrive {
        config.push_str(&format!(
            "\n[[hyperdrive]]\nbinding = \"{}\"\nid = \"{}\"\n",
            h.binding,
            w::hyperdrive_id_placeholder(&h.binding)
        ));
    }
    for binding in &declared.ratelimits {
        config.push_str(&format!(
            "\n[[ratelimits]]\nname = \"{binding}\"\nnamespace_id = \"{}\" # set at deploy time\nsimple = {{ limit = 20, period = 60 }}\n",
            w::ratelimit_namespace_placeholder(binding)
        ));
    }
    for s in &declared.stored_secrets {
        config.push_str(&format!(
            "\n[[secrets_store_secrets]]\nbinding = \"{}\"\nstore_id = \"{}\" # the store's name; its id is set at deploy time\nsecret_name = \"{}\"\n",
            s.binding, s.store, s.secret
        ));
    }
    for c in &declared.mtls_certificates {
        config.push_str(&format!(
            "\n[[mtls_certificates]]\nbinding = \"{}\"\ncertificate_id = \"{}\"\n",
            c.binding, c.certificate
        ));
    }
    for c in &declared.containers {
        config.push_str(&format!(
            "\n[[containers]]\nclass_name = \"{}\"\nscheduling_policy = \"durable_object\"\n\n[containers.images.{}]\ndockerfile = \"../../../{}/Dockerfile\"\nbuild_context = \"../../../{}\"\n",
            c.class, c.capability, c.build, c.build
        ));
    }
    config
}

/// #3073: every push carries the deploy facts; the fixture keeps the rest of the call.
fn strip_facts_var(call: &str) -> String {
    if !call.starts_with("deploy") {
        return call.to_string();
    }
    let marker = format!(" --var {}:", facts::FACTS_VAR);
    let Some((head, rest)) = call.split_once(&marker) else {
        return call.to_string();
    };
    match rest.split_once(' ') {
        Some((_, tail)) => format!("{head} {tail}"),
        None => head.to_string(),
    }
}

/// A first push's secrets file is a fresh temporary directory each run; the fixture keeps the flag.
fn without_secrets_file(text: &str) -> String {
    let flag = "--secrets-file ";
    let Some(at) = text.find(flag) else {
        return text.to_string();
    };
    let rest = &text[at + flag.len()..];
    let tail = rest.find(' ').map_or("", |end| &rest[end..]);
    format!("{}{flag}<secrets-file>{tail}", &text[..at])
}

/// One ledger write: how many account calls had been made, and the ledger written.
struct Write {
    after_call: usize,
    text: String,
}

/// What one run of the apply did.
struct Applied {
    calls: Vec<String>,
    writes: Vec<Write>,
    /// The message the run stopped with, if it failed.
    failure: Option<String>,
    /// The ledger on disk when it ended.
    on_disk: String,
    /// Each Worker's `wrangler.toml` when it ended, by Worker (1C.2's pin, Decision C of #4039).
    configs: BTreeMap<String, String>,
}

/// Apply the every-kind project in `environment`, as `run` does after its confirmation: each
/// Worker in the upload order, recorded once it has pushed, then `--prune`. `fail_call` fails that
/// account call; `fail_write` fails that ledger write (both counted from 1).
fn apply(environment: &str, fail_call: Option<usize>, fail_write: Option<usize>) -> Applied {
    let scratch = Scratch::new(environment, fail_call);
    let mut resources = every_kind();
    // The plan's fixture names a metadata index by its property alone, which the plan never
    // reads; the apply makes it, so it is given its type.
    for index in resources
        .values_mut()
        .flat_map(|declared| declared.vectorize_indexes.iter_mut())
    {
        for entry in &mut index.indexed {
            if !entry.contains(':') {
                entry.push_str(":string");
            }
        }
    }
    let order = order();
    let mut lock = ledger();
    let lock_path = scratch.lock_path();
    let workers_dir = scratch.root.join("workers");
    for (worker, declared) in &resources {
        std::fs::create_dir_all(workers_dir.join(worker)).unwrap();
        std::fs::write(
            workers_dir.join(worker).join("wrangler.toml"),
            generated_config(worker, declared),
        )
        .unwrap();
    }
    let wants = wants();
    let api = FakeHyperdrive { scratch: &scratch };
    let (live_kv, linked) = (live_kv(), linked());
    let queues = RefCell::new(BTreeSet::new());
    let lives = Lives {
        kv: Some(&live_kv),
        hyperdrive: hyperdrive::HyperdriveRun {
            wants: &wants,
            api: Some(&api),
        },
        queues: Some(&queues),
        linked: Some(&linked),
    };
    let plan = derive_plan(
        &order,
        &resources,
        &lock,
        &SecretSource::default(),
        false,
        environment,
    )
    .with_lives(&order, &resources, &lock, Lives::for_plan(&wants));
    let api = account(scratch.root.clone());
    let source = SecretSource::of_values(&[("API_TOKEN", "t0ken")]);
    let (edge_keys, foreign_keys) = (BTreeMap::new(), BTreeMap::new());
    let foreign_secrets = BTreeSet::new();
    let listed: BTreeMap<String, Option<BTreeSet<String>>> = order
        .iter()
        .map(|worker| (worker.clone(), Some(BTreeSet::new())))
        .collect();
    let mut resolved = BTreeMap::new();
    let groups = GroupNames::of(&scratch.root);

    let writes = std::rc::Rc::new(RefCell::new(Vec::new()));
    let watch: write_watch::Watch = {
        let (writes, root) = (std::rc::Rc::clone(&writes), scratch.root.clone());
        Box::new(move |_path, lock| {
            let mut writes = writes.borrow_mut();
            if fail_write == Some(writes.len() + 1) {
                return Err("the disk is full".to_string());
            }
            writes.push(Write {
                after_call: calls_so_far(&root),
                text: toml::to_string_pretty(lock).expect("the ledger serialises"),
            });
            Ok(())
        })
    };
    let failure = write_watch::watching(watch, || {
        let mut writer = LedgerWriter::new(&mut lock, &lock_path);
        // 1D.2b (#4001): each context's plan applied, as `run` applies it.
        for (worker, context) in order.iter().zip(&plan.contexts) {
            let pushed = apply_worker(
                &mut RunCx {
                    tools: Tools {
                        api: &api,
                        project_root: &scratch.root,
                    },
                    workers_dir: &workers_dir,
                    project: &resources,
                    writer: &mut writer,
                    secrets: &mut Secrets {
                        source: &source,
                        force: false,
                        edge_keys: &edge_keys,
                        foreign_keys: &foreign_keys,
                        foreign_secrets: &foreign_secrets,
                        listed: &listed,
                        resolved: &mut resolved,
                        with_upload: true,
                    },
                    wrangler_args: &[],
                    environment,
                    lives,
                },
                context,
            );
            match pushed {
                // As `run` records a Worker that pushed (it has no agent moves to record).
                Ok(Pushed::Ok) => {
                    let pushed =
                        PushedWorker::of(worker, &workers_dir, &groups, &resources[worker]);
                    if let Err(e) = record_pushed(&mut writer, environment, &pushed, |_| {}) {
                        return Some(unrecorded_deploy_failure(
                            environment,
                            worker,
                            &lock_path.display().to_string(),
                            &e,
                        ));
                    }
                }
                Ok(Pushed::Interrupted) => return Some("interrupted".to_string()),
                Err(f) => return Some(format!("exit {}: {}", f.code, f.message)),
            }
        }
        prune_orphans(
            Tools {
                api: &api,
                project_root: &scratch.root,
            },
            &mut writer,
            environment,
            &plan.orphans,
            lives,
        )
        .err()
        .map(|f| format!("exit {}: {}", f.code, f.message))
    });
    let root = scratch.root.display().to_string();
    Applied {
        calls: scratch.calls(),
        writes: writes.take(),
        failure: failure.map(|f| without_secrets_file(&f.replace(&root, "<root>"))),
        on_disk: std::fs::read_to_string(&lock_path).expect("the ledger is on disk"),
        configs: resources
            .keys()
            .map(|worker| {
                let text = std::fs::read_to_string(workers_dir.join(worker).join("wrangler.toml"))
                    .expect("the Worker's configuration is on disk");
                (worker.clone(), text.replace(&root, "<root>"))
            })
            .collect(),
    }
}

/// The lines `after` changes from `before`, `-` and `+` with their line numbers: a longest
/// common subsequence of lines, so the change reads as the ledger reads.
fn changed_lines(before: &str, after: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, String::new());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if j < b.len() && (i == a.len() || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push_str(&format!("+{:>3} {}\n", j + 1, b[j]));
            j += 1;
        } else {
            out.push_str(&format!("-{:>3} {}\n", i + 1, a[i]));
            i += 1;
        }
    }
    if out.is_empty() {
        out.push_str("  (unchanged)\n");
    }
    out
}

/// Whether `call` changes the account (so the fixture fails it): a read is not failed.
fn changes_the_account(call: &str) -> bool {
    [
        "kv namespace create ",
        "kv namespace delete ",
        "r2 bucket create ",
        "d1 create ",
        "d1 migrations apply ",
        "vectorize create ",
        "vectorize create-metadata-index ",
        "vectorize delete ",
        "queues create ",
        "queues delete ",
        "secret ",
        "deploy",
        "hyperdrive create ",
        "hyperdrive update ",
        "hyperdrive delete ",
    ]
    .iter()
    .any(|prefix| call.starts_with(prefix))
}

/// The four fixtures' texts.
fn fixtures() -> (String, String, String, String) {
    let (mut calls, mut writes, mut failures) = (String::new(), String::new(), String::new());
    let mut configs = String::new();
    for environment in ENVIRONMENTS {
        let run = apply(environment, None, None);
        assert_eq!(run.failure, None, "the every-kind apply succeeds");
        calls.push_str(&format!("== {environment} ==\n"));
        for (n, call) in run.calls.iter().enumerate() {
            calls.push_str(&format!("{:>3} {call}\n", n + 1));
        }
        writes.push_str(&format!("== {environment} ==\n"));
        let mut before = LEDGER.to_string();
        for (n, write) in run.writes.iter().enumerate() {
            let after = match write.after_call {
                0 => "before any call".to_string(),
                k => format!("after call {k}: {}", run.calls[k - 1]),
            };
            writes.push_str(&format!("-- write {} ({after})\n", n + 1));
            writes.push_str(&changed_lines(&before, &write.text));
            before.clone_from(&write.text);
        }
        assert_eq!(run.on_disk, before, "the last write is what is on disk");
        for (worker, text) in &run.configs {
            configs.push_str(&format!("== {environment}: {worker} ==\n{text}\n"));
        }
        failures.push_str(&format!("== {environment} ==\n"));
        for (n, call) in run.calls.iter().enumerate() {
            if !changes_the_account(call) {
                continue;
            }
            let failed = apply(environment, Some(n + 1), None);
            failures.push_str(&format!("-- call {} fails: {call}\n", n + 1));
            failures.push_str(&format!(
                "stopped: {}\n",
                failed
                    .failure
                    .as_deref()
                    .unwrap_or("(nothing: it succeeded)")
            ));
            failures.push_str(&changed_lines(LEDGER, &failed.on_disk));
        }
        for n in 1..=run.writes.len() {
            let failed = apply(environment, None, Some(n));
            failures.push_str(&format!("-- write {n} fails\n"));
            failures.push_str(&format!(
                "stopped: {}\n",
                failed
                    .failure
                    .as_deref()
                    .unwrap_or("(nothing: it succeeded)")
            ));
            failures.push_str(&changed_lines(LEDGER, &failed.on_disk));
        }
    }
    (calls, writes, failures, configs)
}

#[test]
fn the_every_kind_apply_makes_the_same_calls_writes_and_failures() {
    // Each push polls the process's stop signal, which the stop tests set: run apart from them.
    let _lock = crate::stop::TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (calls, writes, failures, configs) = fixtures();
    if std::env::var_os("BYNK_WRITE_APPLY_FIXTURES").is_some() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/deploy/fixtures");
        std::fs::write(dir.join("every_kind.apply.calls.txt"), &calls).unwrap();
        std::fs::write(dir.join("every_kind.apply.writes.txt"), &writes).unwrap();
        std::fs::write(dir.join("every_kind.apply.failures.txt"), &failures).unwrap();
        std::fs::write(dir.join("every_kind.apply.configs.txt"), &configs).unwrap();
        return;
    }
    assert_eq!(
        calls, CALLS,
        "the apply's account calls changed: 1D.2 must not change what `bynk deploy` asks of the account, or in what order"
    );
    assert_eq!(
        writes, WRITES,
        "the apply's ledger writes changed: 1D.2 must not change what `bynk deploy` records, or when (ADR 0180)"
    );
    assert_eq!(
        configs, CONFIGS,
        "the Workers' final configurations changed: 1C.2 must not change what `bynk deploy` hands Wrangler"
    );
    assert_eq!(
        failures, FAILURES,
        "what a failed apply leaves changed: 1D.2 must not change what `bynk deploy` records before it stops"
    );
}
