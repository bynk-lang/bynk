use super::*;
use crate::cloudflare::CloudflareError;

/// How one context's push ended, short of failing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Pushed {
    /// `wrangler deploy` exited 0.
    Ok,
    /// `wrangler deploy` died of a shared Ctrl-C. **Not** a failure: the
    /// terminal delivered the SIGINT to us too, which is exactly why
    /// [`exit_status_byte`] maps it to 0. The run stops without reporting an
    /// error, and `bynk` exits cleanly — as slice 0 did, since it passed the
    /// same status straight through `ExitCode::from`.
    Interrupted,
}

/// Why one context's deploy failed, and what `bynk` should exit with.
///
/// The code rides with the message because `wrangler deploy`'s own exit code is
/// the signal a CI job reads, and slice 0 propagated it
/// (`ExitCode::from(exit_status_byte(&status))`). Flattening every failure to 1
/// would lose it — the multi-context loop reports the *first* failure's code,
/// since the run stops there. A driver-side failure (KV, materialisation, a
/// missing or unspawnable wrangler) has no child code to carry and is a plain 1.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DeployFailure {
    pub(crate) message: String,
    pub(crate) code: u8,
}

impl DeployFailure {
    pub(crate) fn driver(message: String) -> Self {
        Self { message, code: 1 }
    }
}

/// Map a finished `wrangler deploy` to an outcome. Pure, so the propagation
/// rule is tested without spawning wrangler. A failure names the Worker the
/// push was creating, its [`deployed_worker_name`] (#2047).
pub(crate) fn wrangler_outcome(
    environment: &str,
    worker: &str,
    exit: Exit,
) -> Result<Pushed, DeployFailure> {
    if exit.success() {
        return Ok(Pushed::Ok);
    }
    // `Exit::byte` is the driver's one rule for "what should a child's status make us exit
    // with?" (the same as `exit_status_byte`), including the deliberate SIGINT → 0 (a shared
    // Ctrl-C is a clean stop) and the `128 + signal` convention for the signals that are real
    // deaths. Reuse it rather than re-deciding here.
    match exit.byte() {
        0 => Ok(Pushed::Interrupted),
        code => Err(DeployFailure {
            message: format!(
                "wrangler deploy failed for `{}` (exit {code})",
                deployed_worker_name(environment, worker)
            ),
            code,
        }),
    }
}

/// #2055: the first wrangler whose `deploy` takes `--secrets-file`
/// (workers-sdk #10896, released in wrangler 4.74.0). Below
/// [`bynk_emit::WRANGLER_MIN`], so only an installed wrangler a project with
/// no agent still deploys on can be older.
pub(crate) const SECRETS_FILE_MIN: &str = "4.74.0";

/// #2055: can this wrangler upload secrets with a new Worker? An unknown
/// version (npx-provisioned, whose version is not known without running it)
/// counts as able, by the rule [`doctor::wrangler_below_min`] uses: npx resolves
/// `wrangler@4`, the newest 4.x.
pub(crate) fn secrets_file_supported(probe: &probe::Probe) -> bool {
    let min = probe::Version::parse(SECRETS_FILE_MIN).expect("SECRETS_FILE_MIN is a version");
    probe
        .version
        .is_none_or(|v| (v.major, v.minor, v.patch) >= (min.major, min.minor, min.patch))
}

/// #2055: a first deploy's secrets, as the JSON object
/// `wrangler deploy --secrets-file` reads (`{"NAME": "value"}`; wrangler tries
/// JSON before dotenv, and JSON escaping is exact for any value).
///
/// Written to a fresh directory under the system temp dir — never the project,
/// so it cannot be committed or land beside the ledger — the directory 0700 and
/// the file 0600, created new so nothing already there is reused. Its contents
/// are never logged; only its path reaches wrangler's argv.
///
/// What removes it, exactly: dropping it removes the directory, so it is gone
/// once the push returns, whether wrangler succeeded, failed or could not be
/// started, and on a panic. SIGINT and SIGTERM to `bynk` during the upload
/// are deferred until wrangler has exited ([`CloudflareApi::push`]), so
/// they end the run only after the drop. SIGKILL (or a power cut) cannot be
/// caught and leaves the file, still 0600 in its 0700 directory, until the
/// next `bynk deploy` by this user runs [`sweep_stale_upload_dirs`]. The
/// directory name carries the pid (`bynk-secrets-<pid>-<nanos>-<n>`) so the
/// sweep can tell a dead run's file from a live one's.
pub(crate) struct SecretsFile {
    dir: std::path::PathBuf,
    path: std::path::PathBuf,
}

impl SecretsFile {
    pub(crate) fn write(prepared: &[(String, String)]) -> io::Result<Self> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "bynk-secrets-{}-{unique}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // Not recursive: the directory must be new, or someone else's.
        builder.create(&dir)?;
        let file = Self {
            path: dir.join("secrets.json"),
            dir,
        };
        let object: BTreeMap<&str, &str> = prepared
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let body = serde_json::to_vec(&object).map_err(io::Error::other)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&file.path)?.write_all(&body)?;
        Ok(file)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SecretsFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `wrangler deploy` in one worker directory. `--secrets-file` is appended when
/// a first deploy uploads its secrets with the Worker (#2055), and `--env` for a
/// non-default environment (slice 4) — the one place Wrangler needs telling
/// which of the synthesised `[env.<name>]` tables to read.
///
/// The process control is the adapter's ([`CloudflareApi::push`]): with a
/// secrets file, SIGINT and SIGTERM are held until wrangler exits, so the file's
/// owner can always remove it. What the exit means is decided here: wrangler's
/// exit is mapped as any push's is; if it exited 0 but `bynk` was asked to stop,
/// the run stops, as an interrupt for SIGINT and as a failure with the signal's
/// exit code otherwise.
pub(crate) fn push(
    api: &dyn CloudflareApi,
    worker_dir: &Path,
    worker: &str,
    wrangler_args: &[String],
    environment: &str,
    (secrets_file, first_deploy): (Option<&Path>, bool),
) -> Result<Pushed, DeployFailure> {
    let deployed = deployed_worker_name(environment, worker);
    // #3562 (G4-2): a first deploy is watched for wrangler's `10007` race, which
    // needs wrangler's own output (copied through as it arrives).
    let watch = first_deploy
        .then(|| first_deploy::Watch::begin(worker_dir, environment))
        .flatten();
    let run = api
        .push(PushCall {
            worker_dir,
            args: wrangler_args,
            secrets_file,
            environment,
            tee: watch.is_some(),
        })
        .map_err(|e| match e.failure() {
            Failure::Missing => DeployFailure::driver("wrangler not found".into()),
            Failure::Wait(_) => DeployFailure::driver(format!(
                "could not wait for wrangler deploy for `{deployed}`: {e}"
            )),
            _ => DeployFailure::driver(format!(
                "could not run wrangler deploy for `{deployed}`: {e}"
            )),
        })?;
    let exit = watch
        .as_ref()
        .map_or(run.exit, |watch| watch.settle(run.exit, &run.said));
    let pushed = match (wrangler_outcome(environment, worker, exit)?, run.signalled) {
        (Pushed::Ok, Some(sig)) if crate::stop::exit_byte(sig) == 0 => Pushed::Interrupted,
        (Pushed::Ok, Some(sig)) => {
            return Err(DeployFailure {
                message: format!(
                    "stopped by {} after wrangler deploy finished for `{deployed}`",
                    crate::stop::name(sig)
                ),
                code: crate::stop::exit_byte(sig),
            });
        }
        (pushed, _) => pushed,
    };
    // A forgiven failure left wrangler's triggers step unrun (see `Watch::forgave`).
    if pushed == Pushed::Ok && watch.as_ref().is_some_and(first_deploy::Watch::forgave) {
        deploy_forgiven_triggers(api, worker_dir, worker, environment)?;
    }
    Ok(pushed)
}

/// The triggers a forgiven first deploy never got: `wrangler triggers deploy`.
pub(crate) fn deploy_forgiven_triggers(
    api: &dyn CloudflareApi,
    worker_dir: &Path,
    worker: &str,
    environment: &str,
) -> Result<(), DeployFailure> {
    first_deploy::deploy_triggers(api, worker_dir, environment).map_err(|e| {
        let Some(e) = e else {
            return DeployFailure::driver("wrangler not found".into());
        };
        DeployFailure::driver(format!(
            "`{}` was uploaded, but its triggers (workers.dev, routes, crons) were not applied: {e}. Run `{}` again to finish",
            deployed_worker_name(environment, worker),
            deploy_command(environment)
        ))
    })
}

/// #2055: remove what a SIGKILLed first deploy left behind: each
/// `bynk-secrets-<pid>-…` directory in `temp_dir` whose `bynk` process is no
/// longer running, and which this user owns. Run at the start of every
/// deploy. A directory of a live process (this one's, or another deploy's) is
/// left alone, and so is one whose pid has since been reused, until that
/// process ends. Returns how many were removed. Never reads the files.
/// Windows has no liveness check here, so it removes nothing there.
pub(crate) fn sweep_stale_upload_dirs(temp_dir: &Path) -> usize {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(entries) = std::fs::read_dir(temp_dir) else {
            return 0;
        };
        // SAFETY: `getuid` cannot fail.
        let uid = unsafe { libc::getuid() };
        let mut removed = 0;
        for entry in entries.flatten() {
            let Some(pid) = secrets_dir_pid(&entry.file_name().to_string_lossy()) else {
                continue;
            };
            if pid == std::process::id() || process_alive(pid) {
                continue;
            }
            let path = entry.path();
            let owned =
                std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir() && m.uid() == uid);
            if owned && std::fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }
    #[cfg(not(unix))]
    {
        let _ = temp_dir;
        0
    }
}

/// The pid in a `bynk-secrets-<pid>-<nanos>-<n>` directory name, as
/// [`SecretsFile::write`] names them.
fn secrets_dir_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("bynk-secrets-")?;
    let (pid, _) = rest.split_once('-')?;
    pid.parse().ok()
}

/// Is a process with this pid running? `kill(pid, 0)` checks without
/// signalling: it fails with `ESRCH` only when there is none. A process of
/// another user (`EPERM`) counts as running.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return true;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

pub(crate) fn whoami(api: &dyn CloudflareApi) -> bool {
    api.whoami().is_ok()
}

/// Interop I5 (#1972): what the account says about a foreign Worker a Service
/// Binding names.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WorkerPresence {
    Present,
    /// Cloudflare's "Worker not found" (code 10007, or 10090 for a legacy
    /// environment), as wrangler 4.149.0 reads it (`isWorkerNotFoundError`).
    Missing,
    /// The listing failed for another reason (network, permissions): what
    /// wrangler said.
    Unknown(CloudflareError),
}

/// Interop I5 (#1972): ask the account whether Worker `name` exists, with
/// `wrangler deployments list --name <name> --json` (a read of
/// `/accounts/<id>/workers/scripts/<name>/deployments`). Run from the project
/// root, like the other account reads, so no generated config is loaded.
pub(crate) fn worker_presence(api: &dyn CloudflareApi, name: &str) -> WorkerPresence {
    match api.worker_deployments(name) {
        Ok(()) => WorkerPresence::Present,
        // A missing Worker is read only from Cloudflare's own code, so an auth or network
        // failure is never mistaken for one.
        Err(e) if e.worker_not_found() => WorkerPresence::Missing,
        Err(e) => WorkerPresence::Unknown(e.into()),
    }
}

/// Create `name`, or accept that it is already there. Returns whether this call
/// created it: a race-loser, finding it made in between, did not (#2579).
///
/// Unlike KV there is no id to record: Cloudflare addresses a queue by the name
/// `from queue("n")` already gave it, so a successful create and an
/// already-existing queue are the same end state and both return `Ok`
/// (ADR 0194 D2).
pub(crate) fn create_queue(api: &dyn CloudflareApi, name: &str) -> Result<bool, CloudflareError> {
    match api.queue_create(name) {
        Ok(()) => Ok(true),
        // The race-loser's path, not the common one: `queue_exists` has already
        // answered for a queue that was simply there. Someone else creating it
        // between that check and this call lands here. Since 1D.4d (#4056) the
        // answer is read from the account, not from wrangler's prose: the API
        // reference documents no code for a name already taken, so a create
        // Cloudflare refuses is a success only when the queue is now there.
        Err(e) if e.is_refusal() => match api.queue_find(name) {
            Ok(Some(_)) => Ok(false),
            Ok(None) => Err(e),
            Err(lookup) => {
                Err(format!("{e} (looking the queue up by name also failed: {lookup})").into())
            }
        },
        Err(e) => Err(e),
    }
}

/// Is `name` already on the account?
///
/// A lookup by the same name the config binds, over Cloudflare's REST API since
/// 1D.4d (the listing, matched exactly: the reference has no lookup by name).
/// Asking the account, not the ledger: the ledger's queue set is a planning aid,
/// and trusting it here would leave a queue deleted out-of-band un-recreated
/// (ADR 0194 D2).
///
/// A failed lookup is read as "not there — try to create it". That is also what
/// an auth or network failure produces, and the honest consequence is the right
/// one: the create then fails too and surfaces Cloudflare's real answer, rather
/// than this call inventing a diagnosis of its own.
pub(crate) fn queue_exists(api: &dyn CloudflareApi, name: &str) -> bool {
    matches!(api.queue_find(name), Ok(Some(_)))
}

/// Which of a context's queues still want a create attempt on this run, marking
/// them attempted as it goes.
///
/// A queue is an account resource, so two contexts consuming `"jobs"` mean one
/// queue (ADR 0194 D2) — and the emitter's duplicate-consumer check is scoped to
/// a single context, so that is a legal project rather than a hypothetical. But
/// provisioning runs per context, so without this a shared queue costs one
/// wrangler spawn per consumer, and the ADR's "one call per queue per deploy"
/// would be a claim the code did not honour.
///
/// Scoped to one run, deliberately: every queue is still attempted on every
/// fresh deploy, which is the property the self-healing rests on. Pure but for
/// the marker, so the rule is tested without spawning wrangler.
///
/// P2 (#2578): a context's producer queues are attempted with its consumed
/// ones, in name order, each once.
pub(crate) fn unattempted_queues(
    declared: &Resources,
    attempted: &mut BTreeSet<String>,
) -> Vec<String> {
    declared
        .all_queues()
        .into_iter()
        .filter(|queue| attempted.insert(queue.clone()))
        .collect()
}

/// P2 (#2578): the queues one context's deploy records in the ledger: every
/// queue it consumes, and a queue it only produces to when this run `created`
/// it. Pure, so the rule is tested without spawning wrangler.
pub(crate) fn queues_to_record(declared: &Resources, created: &BTreeSet<String>) -> Vec<String> {
    declared
        .all_queues()
        .into_iter()
        .filter(|queue| declared.queues.contains(queue) || created.contains(queue))
        .collect()
}

/// Delete a queue by name — `--prune`'s other mutation.
///
/// Found by name and deleted by id (1D.4d, #4056). Idempotent against a queue
/// already gone, so a re-run `--prune` is safe (#839): one the listing does not
/// show, or a delete answered with Cloudflare's `10104 QueueNotFound`, is
/// success.
pub(crate) fn delete_queue(api: &dyn CloudflareApi, name: &str) -> Result<(), CloudflareError> {
    match api.queue_delete(name) {
        Ok(_) => Ok(()),
        Err(e)
            if e.rest()
                .is_some_and(|r| r.has_code(crate::cloudflare::rest::queues::NOT_FOUND)) =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// The names already set on this Worker, from `wrangler secret list`.
///
/// `None` means "could not ask" — see [`secret_action`]. Unlike `secret put`,
/// `secret list` has no draft-Worker path: it simply fails for a Worker that
/// does not exist, which is exactly the answer we want on a first deploy.
pub(crate) fn list_secrets(
    api: &dyn CloudflareApi,
    worker_dir: &Path,
    environment: &str,
) -> Option<BTreeSet<String>> {
    // In the worker directory: `secret list` reads the Worker's name from the config beside it,
    // and any KV id is materialised by now, so wrangler can load a complete config.
    parse_secret_list(&api.secret_list(worker_dir, environment).ok()?)
}

/// The names in `wrangler secret list --format json`'s output.
///
/// Wrangler's shape, so this is a claim about another tool's output rather than
/// a contract — but a structural one (a JSON array of objects with a `name`),
/// not a prose match.
///
/// A shape change reads as `None`, "could not tell", which is the same answer as
/// a network or auth failure and is handled once in [`prepare_bindings`]: where a
/// value is available, every secret is re-set (noisy and idempotent); where one
/// is not, a **redeploy leaves the secret alone and says so** rather than failing
/// on the strength of a read failure in an advisory check. Only a first deploy
/// treats an unresolvable declared secret as fatal, because there its Worker
/// really is new.
pub(crate) fn parse_secret_list(stdout: &str) -> Option<BTreeSet<String>> {
    let parsed: serde_json::Value = serde_json::from_str(stdout).ok()?;
    Some(
        parsed
            .as_array()?
            .iter()
            .filter_map(|entry| entry.get("name")?.as_str().map(str::to_owned))
            .collect(),
    )
}

/// Set one secret, feeding the value on **stdin**.
///
/// Never argv, and not merely by preference: `wrangler secret put` has no value
/// option at all — the value is stdin or an interactive prompt — so this is the
/// only interface, and it is the one that keeps the value out of the process
/// list (ADR 0195 D1). Wrangler takes the stdin path exactly when *its* stdin is
/// not a TTY, and a pipe never is, so this works whether or not `bynk` itself
/// has a terminal.
pub(crate) fn set_secret(
    api: &dyn CloudflareApi,
    worker_dir: &Path,
    name: &str,
    value: &str,
    environment: &str,
) -> Result<(), CloudflareError> {
    api.secret_put(worker_dir, name, value, environment)
        .map_err(CloudflareError::from)
}

/// #3073: each agent class's current schema version, from the project's
/// `bynk.schema.lock` (what the build just compiled against). Empty when the
/// project has no lock or it cannot be read: the facts then record no agents,
/// and a revert treats the version as having none to compare.
pub(crate) fn schema_agent_versions(project_root: &Path) -> BTreeMap<String, i64> {
    let Ok(existing) = bynk_driver::schema_lock::read(project_root) else {
        return BTreeMap::new();
    };
    let Ok(registry) = bynk_project::schema_registry::parse(existing.as_deref(), project_root)
    else {
        return BTreeMap::new();
    };
    registry
        .agents()
        .map(|(class, entry)| (class.clone(), entry.version))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudflare::Op;
    use crate::cloudflare::fake::Recording;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // ---- #601 slice 2: what `wrangler deploy`'s status makes us exit with ----
    //
    // 1D.3b (#4043, Decision D): the rule is over `Exit`, which the adapter builds from the
    // process's status, so it is tested on every platform.

    #[test]
    fn a_clean_push_is_ok() {
        assert_eq!(
            wrangler_outcome("default", "api", Exit::Code(0)),
            Ok(Pushed::Ok)
        );
    }

    #[test]
    fn wranglers_exit_code_is_propagated_rather_than_flattened() {
        // Slice 0 exited with `exit_status_byte(&status)`; the multi-context
        // loop must not lose that. A CI job reads the code, so a wrangler exit
        // 2 must not surface as a generic 1.
        let Err(failure) = wrangler_outcome("default", "api", Exit::Code(2)) else {
            panic!("a non-zero wrangler exit is a failure");
        };
        assert_eq!(failure.code, 2, "wrangler's own code reaches the caller");
        assert!(
            failure.message.contains("(exit 2)"),
            "the message states the code it exits with: {}",
            failure.message
        );
    }

    /// #2047: a failed push names the Worker it was creating, `-staging`
    /// under `--env staging`.
    #[test]
    fn a_failed_push_names_the_deployed_worker() {
        for (environment, expected) in [
            (
                "default",
                "wrangler deploy failed for `commerce-orders` (exit 2)",
            ),
            (
                "staging",
                "wrangler deploy failed for `commerce-orders-staging` (exit 2)",
            ),
        ] {
            let Err(failure) = wrangler_outcome(environment, "commerce-orders", Exit::Code(2))
            else {
                panic!("a non-zero wrangler exit is a failure");
            };
            assert_eq!(failure.message, expected);
        }
    }

    #[test]
    fn a_shared_ctrl_c_is_a_clean_stop_not_a_failure() {
        // SIGINT reaches us too (shared foreground process group), which is why
        // `exit_status_byte` maps it to 0. Reporting it as a failure would print
        // "wrangler deploy failed … (exit 0)" — an error whose own code says
        // success — and slice 0 exited 0 here.
        assert_eq!(
            wrangler_outcome("default", "api", Exit::Signal(2)),
            Ok(Pushed::Interrupted)
        );
    }

    #[test]
    fn a_real_signal_death_is_a_failure_at_128_plus_the_signal() {
        // Not every signal is a clean stop: a SIGSEGV or the OOM killer's
        // SIGKILL is a genuine failure, and `exit_status_byte` says `128 + sig`.
        let Err(segv) = wrangler_outcome("default", "api", Exit::Signal(11)) else {
            panic!("a SIGSEGV is a failure, not a clean stop");
        };
        assert_eq!(segv.code, 139);
        let Err(kill) = wrangler_outcome("default", "api", Exit::Signal(9)) else {
            panic!("a SIGKILL is a failure, not a clean stop");
        };
        assert_eq!(kill.code, 137);
    }

    #[test]
    fn a_driver_side_failure_has_no_child_code_to_carry() {
        // KV, materialisation, a missing wrangler — nothing ran, so there is no
        // code to propagate and 1 is the honest answer.
        assert_eq!(DeployFailure::driver("nope".into()).code, 1);
    }

    /// A REST refusal of `status` with Cloudflare's `code`, as the queue calls answer.
    fn queue_refusal(method: &'static str, status: u16, code: u32) -> CloudflareError {
        crate::cloudflare::rest::RestError::answered(
            method,
            "/queues",
            status,
            &format!(r#"[{{"code":{code},"message":"stand-in"}}]"#),
        )
        .into()
    }

    #[test]
    fn a_queue_already_deleted_is_success() {
        // #839, over REST since 1D.4d: a queue the listing no longer shows, or a delete answered
        // with `10104 QueueNotFound`, is the end state `--prune` asked for.
        let absent = Recording::new(|_: &Op<'_>| Ok::<_, CloudflareError>("absent".into()));
        assert_eq!(delete_queue(&absent, "jobs"), Ok(()));
        let raced = Recording::new(|_: &Op<'_>| Err(queue_refusal("DELETE", 404, 10104)));
        assert_eq!(delete_queue(&raced, "jobs"), Ok(()));
        // Any other refusal is a real failure, in Cloudflare's words.
        let other = Recording::new(|_: &Op<'_>| Err(queue_refusal("DELETE", 400, 10107)));
        let e = delete_queue(&other, "jobs").unwrap_err();
        assert!(e.to_string().contains("400"), "{e}");
    }

    #[test]
    fn confirm_prune_short_circuits_on_yes_without_touching_stdin() {
        // The one path of confirm_prune testable without a terminal: --yes
        // must return true immediately, exactly as `confirm` does.
        assert!(confirm_prune(true, &Orphans::default(), "default"));
    }

    #[test]
    fn an_existing_queue_is_success_not_a_failure() {
        // D2, read from the account since 1D.4d: a create that loses the race against a
        // concurrent deploy is refused, and is a success only when the queue is now listed. The
        // API reference documents no code for a name already taken, so the code is not read.
        for code in [11009, 10101, 100] {
            let raced = Recording::new(move |op: &Op<'_>| match op {
                Op::QueueCreate { .. } => Err(queue_refusal("POST", 409, code)),
                _ => Ok(String::new()),
            });
            assert_eq!(create_queue(&raced, "jobs"), Ok(false), "code {code}");
            assert_eq!(raced.calls(), ["queues create jobs", "queues info jobs"]);
        }
        // Refused and still not listed: the create's own error.
        let refused = Recording::new(|op: &Op<'_>| match op {
            Op::QueueCreate { .. } => Err(queue_refusal("POST", 400, 11009)),
            _ => Ok("absent".to_string()),
        });
        let e = create_queue(&refused, "jobs").unwrap_err();
        assert!(e.to_string().contains("400"), "{e}");
        // A denied token or a server error is never adopted, and is not followed by a lookup.
        for status in [403, 500] {
            let failing =
                Recording::new(move |_: &Op<'_>| Err(queue_refusal("POST", status, 10000)));
            assert!(create_queue(&failing, "jobs").is_err());
            assert_eq!(failing.calls(), ["queues create jobs"]);
        }
        // A lookup that fails too is reported after the create's error.
        let both = Recording::new(|op: &Op<'_>| match op {
            Op::QueueCreate { .. } => Err(queue_refusal("POST", 409, 11009)),
            _ => Err(queue_refusal("GET", 500, 10000)),
        });
        let e = create_queue(&both, "jobs").unwrap_err().to_string();
        assert!(
            e.contains("looking the queue up by name also failed"),
            "{e}"
        );
    }

    #[test]
    fn a_queue_two_contexts_share_is_attempted_once_per_run() {
        // Provisioning runs per context, but a queue is an account resource and
        // two contexts consuming `"jobs"` mean one queue — and the emitter's
        // duplicate-consumer check is context-scoped, so that project is legal.
        // Without the dedup, a shared queue costs one wrangler spawn per
        // consumer and the ADR's "one call per queue per deploy" is a claim the
        // code does not honour.
        let orders = Resources::default().consumes(&["jobs", "orders-only"]);
        let billing = Resources::default().consumes(&["billing-only", "jobs"]);
        let mut run = BTreeSet::new();
        assert_eq!(
            unattempted_queues(&orders, &mut run),
            names(&["jobs", "orders-only"])
        );
        assert_eq!(
            unattempted_queues(&billing, &mut run),
            names(&["billing-only"]),
            "`jobs` was already attempted by the orders context this run"
        );

        // A *fresh* run attempts everything again — the dedup is per run, which
        // is what keeps a queue deleted out-of-band self-healing (ADR 0194 D2).
        let mut later = BTreeSet::new();
        assert_eq!(
            unattempted_queues(&billing, &mut later),
            names(&["billing-only", "jobs"])
        );
    }

    #[test]
    fn a_producer_queue_is_attempted_with_the_consumed_ones() {
        // P2 (#2578): created if missing, the same way, once per run.
        let shop = Resources::default()
            .consumes(&["returns"])
            .produces(&["order-events", "returns"]);
        let mut run = BTreeSet::new();
        assert_eq!(
            unattempted_queues(&shop, &mut run),
            names(&["order-events", "returns"])
        );
    }

    #[test]
    fn a_queue_only_produced_to_is_recorded_only_when_this_run_created_it() {
        // P2 (#2578): a queue only producers name may be another Worker's, so
        // reusing it records nothing (nothing `--prune` could later delete). A
        // consumed queue is recorded either way, even when another context
        // already checked it this run.
        let shop = Resources::default()
            .consumes(&["returns"])
            .produces(&["order-events", "audit"]);
        assert_eq!(
            queues_to_record(&shop, &BTreeSet::new()),
            names(&["returns"])
        );
        assert_eq!(
            queues_to_record(&shop, &BTreeSet::from(["audit".to_string()])),
            names(&["audit", "returns"])
        );
    }

    #[test]
    fn a_wrangler_complaint_is_read_from_whichever_stream_carries_it() {
        let op = crate::cloudflare::Op::WorkerDeployments { name: "jobs" };
        let said = |stdout: &str, stderr: &str| {
            crate::cloudflare::fake::failed(&op, stdout, stderr).to_string()
        };
        assert_eq!(said("", "boom"), "boom");
        assert_eq!(
            said("boom", ""),
            "boom",
            "wrangler is not consistent about which stream carries a complaint"
        );
        assert!(
            said("", "").contains("exited 1"),
            "a silent failure still reads as something, not a dangling colon"
        );
    }

    #[test]
    fn a_queue_the_run_only_found_is_recorded_as_reused() {
        // #2579: the decision that keeps `--prune` off another Worker's queue.
        let resources = BTreeMap::from([("orders".into(), Resources::default())]);
        let mut lock = DeployLock::default();
        assert!(record_provisioned_queue(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "theirs",
            false
        ));
        assert!(record_provisioned_queue(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "ours",
            true
        ));
        let orphans = crate::deploy::ledger::find_orphans(&lock, "default", &resources);
        assert_eq!(orphans.queues, vec!["ours".to_string()]);
    }

    #[test]
    fn a_queue_bynk_created_stays_prunable_when_a_redeploy_finds_it_there() {
        let resources = BTreeMap::from([("orders".into(), Resources::default())]);
        let mut lock = DeployLock::default();
        record_provisioned_queue(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "ours",
            true,
        );
        assert!(!record_provisioned_queue(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "ours",
            false
        ));
        let orphans = crate::deploy::ledger::find_orphans(&lock, "default", &resources);
        assert_eq!(orphans.queues, vec!["ours".to_string()]);
    }

    #[test]
    fn the_queue_ledger_records_once_and_reports_whether_it_changed() {
        // The provision step writes the ledger only when the set actually
        // gained a name, so a re-run that provisions nothing writes nothing.
        let mut lock = DeployLock::default();
        assert!(
            QueueTable::created(
                lock.environments
                    .entry(String::from("default"))
                    .or_default(),
                "intake"
            ),
            "the first is new"
        );
        assert!(
            !QueueTable::created(
                lock.environments
                    .entry(String::from("default"))
                    .or_default(),
                "intake"
            ),
            "the second changes nothing, so the ledger must not be rewritten"
        );
    }

    // ---- #2037: the bare namespace's title on the provisioning command ----
    //
    // the apply of one context against a stand-in `wrangler` that records its arguments
    // and answers `kv namespace create` with an id. Nothing reaches
    // Cloudflare.

    /// #3073: every push carries the facts variable (asserted here, for every
    /// test that runs one); the tests then compare the rest of the call.
    fn strip_facts_var(call: &str) -> String {
        if !call.starts_with("deploy") {
            return call.to_string();
        }
        let marker = format!(" --var {}:", facts::FACTS_VAR);
        let (head, rest) = call
            .split_once(&marker)
            .unwrap_or_else(|| panic!("a push carries the facts variable: {call}"));
        match rest.split_once(' ') {
            Some((_, tail)) => format!("{head} {tail}"),
            None => head.to_string(),
        }
    }

    /// The account the provisioning tests deploy to: it logs every call and answers as the
    /// on-disk stand-in did before 1D.3b. A migration of `D1_BROKEN…` fails; `d1 create` mints
    /// an id from the name's length, and adopts on a name containing `taken`, which `d1 list`
    /// shows; any other `--json` call is refused as an unknown argument; `kv namespace create`
    /// mints `minted-id`; everything else succeeds, saying nothing.
    fn provision_fake() -> crate::cloudflare::fake::Recording {
        use crate::cloudflare::Op;
        use crate::cloudflare::fake::{Recording, failed};
        Recording::new(|op| -> Result<String, crate::cloudflare::CloudflareError> {
            Ok(match op {
                Op::D1MigrationsApply { binding, .. } if binding.starts_with("D1_BROKEN") => {
                    return Err(failed(op, "", "✘ [ERROR] Migration 0001_bad.sql failed\n").into());
                }
                Op::D1Create { name, .. } if name.contains("taken") => {
                    return Err(crate::cloudflare::RestError::answered(
                        "POST",
                        "/d1/database",
                        400,
                        "[]",
                    )
                    .into());
                }
                Op::D1Create { name, .. } => {
                    format!("0d1d0000-0000-4000-8000-00000000000{}", name.len())
                }
                Op::D1List => "adadadad-0000-4000-8000-000000000000".into(),
                _ if op.line().contains("--json") => {
                    return Err(failed(op, "", "✘ [ERROR] Unknown argument: json\n").into());
                }
                Op::KvCreate { .. } => "minted-id".into(),
                Op::KvList => "[[kv_namespaces]]\nbinding = \"t\"\nid = \"minted-id\"\n".into(),
                _ => String::new(),
            })
        })
    }

    /// What the apply ran, one line per `wrangler` invocation, and the
    /// ledger it left, for one Worker `api` that binds only the bare `KV`.
    fn provision_bare_kv(environment: &str, lock: DeployLock) -> (Vec<String>, DeployLock) {
        let (calls, lock, pushed) =
            provision_api(environment, Resources::default().needs_kv(), lock, None);
        assert_eq!(pushed, Ok(Pushed::Ok));
        (calls, lock)
    }

    /// The apply of Worker `api` declaring `declared` (the whole project),
    /// with the account's live KV ids `live_kv_ids`: the `wrangler`
    /// invocations, the ledger it left, and its outcome.
    fn provision_api(
        environment: &str,
        declared: Resources,
        lock: DeployLock,
        live_kv_ids: Option<BTreeSet<String>>,
    ) -> (Vec<String>, DeployLock, Result<Pushed, DeployFailure>) {
        let (calls, lock, pushed, _) =
            provision_api_with_config(environment, declared, lock, live_kv_ids);
        (calls, lock, pushed)
    }

    /// [`provision_api`], and the generated configuration as the push left it.
    fn provision_api_with_config(
        environment: &str,
        declared: Resources,
        lock: DeployLock,
        live_kv_ids: Option<BTreeSet<String>>,
    ) -> (
        Vec<String>,
        DeployLock,
        Result<Pushed, DeployFailure>,
        String,
    ) {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "bynk-kv-title-{environment}-{}-{unique}",
            std::process::id()
        ));
        let workers_dir = root.join("workers");
        std::fs::create_dir_all(workers_dir.join("api")).unwrap();
        let mut config = String::from("name = \"api\"\nmain = \"index.ts\"\n");
        for binding in kv_bindings(&declared) {
            config.push_str(&format!(
                "\n[[kv_namespaces]]\nbinding = \"{binding}\"\nid = \"{}\" # set at deploy time\n",
                bynk_emit::emitter::wrangler::kv_id_placeholder(binding)
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
                bynk_emit::emitter::wrangler::d1_database_id_placeholder(&db.binding)
            ));
            if !db.migrations.is_empty() {
                config.push_str(&format!("migrations_dir = \"migrations/{}\"\n", db.name));
            }
        }
        for binding in &declared.ratelimits {
            config.push_str(&format!(
                "\n[[ratelimits]]\nname = \"{binding}\"\nnamespace_id = \"{}\" # set at deploy time\nsimple = {{ limit = 20, period = 60 }}\n",
                bynk_emit::emitter::wrangler::ratelimit_namespace_placeholder(binding)
            ));
        }
        std::fs::write(workers_dir.join("api/wrangler.toml"), config).unwrap();
        let api = provision_fake();
        let mut lock = lock;
        let source = SecretSource::default();
        let edge_keys = BTreeMap::new();
        let foreign_keys = BTreeMap::new();
        let foreign_secrets = BTreeSet::new();
        let listed = BTreeMap::from([("api".to_string(), Some(BTreeSet::new()))]);
        let mut resolved = BTreeMap::new();
        let project = BTreeMap::from([("api".to_string(), declared)]);
        // 1D.2b (#4001): as `run` applies a context, its plan's steps.
        let order = ["api".to_string()];
        let plan = derive_plan(
            &order,
            &project,
            &lock,
            &SecretSource::default(),
            false,
            environment,
        );
        let pushed = apply_worker(
            &mut RunCx {
                tools: Tools {
                    api: &api,
                    project_root: &root,
                },
                workers_dir: &workers_dir,
                project: &project,
                writer: &mut LedgerWriter::new(&mut lock, &root.join(LOCK_FILE)),
                secrets: &mut Secrets {
                    source: &source,
                    force: false,
                    edge_keys: &edge_keys,
                    expired: &std::collections::BTreeMap::new(),
                    cleaned: &std::cell::RefCell::new(std::collections::BTreeSet::new()),
                    foreign_keys: &foreign_keys,
                    foreign_secrets: &foreign_secrets,
                    listed: &listed,
                    resolved: &mut resolved,
                    with_upload: true,
                },
                wrangler_args: &[],
                environment,
                lives: Lives {
                    kv: live_kv_ids.as_ref(),
                    ..Lives::default()
                },
            },
            &plan.contexts[0],
        );
        let calls = api.calls().iter().map(|c| strip_facts_var(c)).collect();
        let config = std::fs::read_to_string(workers_dir.join("api/wrangler.toml")).unwrap();
        let _ = std::fs::remove_dir_all(root);
        (calls, lock, pushed, config)
    }

    // ---- P4a (#2602): D1 databases are created, recorded and bound by id ----

    fn d1_calls(calls: &[String]) -> Vec<&str> {
        calls
            .iter()
            .map(String::as_str)
            .filter(|c| c.starts_with("d1 "))
            .collect()
    }

    #[test]
    fn each_d1_database_is_created_recorded_and_bound_by_its_minted_id() {
        let (calls, lock, pushed, config) = provision_api_with_config(
            "default",
            Resources::default().d1_databases(&[("D1_ORDERS", "api-orders")]),
            DeployLock::default(),
            None,
        );
        assert_eq!(pushed, Ok(Pushed::Ok));
        assert_eq!(d1_calls(&calls), ["d1 create api-orders"]);
        let recorded = lock.recorded::<D1>("default", "api", "D1_ORDERS").unwrap();
        assert_eq!(recorded.name, "api-orders");
        assert_eq!(recorded.id, "0d1d0000-0000-4000-8000-0000000000010");
        assert!(
            config.contains("database_id = \"0d1d0000-0000-4000-8000-0000000000010\""),
            "{config}"
        );
        // Recorded, so the next deploy binds it without asking the account.
        let (calls, _, pushed) = provision_api(
            "default",
            Resources::default().d1_databases(&[("D1_ORDERS", "api-orders")]),
            lock,
            None,
        );
        assert_eq!(pushed, Ok(Pushed::Ok));
        assert_eq!(d1_calls(&calls), Vec::<&str>::new());
    }

    #[test]
    fn a_named_environment_creates_and_binds_its_own_database() {
        let (calls, lock, _, config) = provision_api_with_config(
            "staging",
            Resources::default().d1_databases(&[("D1_ORDERS", "api-orders")]),
            DeployLock::default(),
            None,
        );
        assert_eq!(d1_calls(&calls), ["d1 create api-orders-staging"]);
        assert_eq!(
            lock.recorded::<D1>("staging", "api", "D1_ORDERS")
                .unwrap()
                .name,
            "api-orders-staging"
        );
        assert!(lock.recorded::<D1>("default", "api", "D1_ORDERS").is_none());
        let doc: toml::Table = config.parse().unwrap();
        let block = &doc["env"]["staging"]["d1_databases"][0];
        assert_eq!(block["database_name"].as_str(), Some("api-orders-staging"));
        assert_eq!(
            block["database_id"].as_str(),
            Some("0d1d0000-0000-4000-8000-0000000000018")
        );
    }

    #[test]
    fn a_database_of_the_same_name_already_there_is_adopted() {
        let (calls, lock, pushed) = provision_api(
            "default",
            Resources::default().d1_databases(&[("D1_TAKEN", "api-taken")]),
            DeployLock::default(),
            None,
        );
        assert_eq!(pushed, Ok(Pushed::Ok));
        assert_eq!(d1_calls(&calls), ["d1 create api-taken", "d1 list --json"]);
        assert_eq!(
            lock.recorded::<D1>("default", "api", "D1_TAKEN")
                .unwrap()
                .id,
            "adadadad-0000-4000-8000-000000000000"
        );
    }

    // ---- P4b (#2608): migrations are applied before the push ----

    fn with_migrations(mut resources: Resources) -> Resources {
        for db in &mut resources.d1_databases {
            db.migrations = vec!["0001_create.sql".into(), "0002_more.sql".into()];
        }
        resources
    }

    #[test]
    fn migrations_are_applied_remotely_after_binding_and_before_the_push() {
        let (calls, _, pushed) = provision_api(
            "default",
            with_migrations(Resources::default().d1_databases(&[("D1_ORDERS", "api-orders")])),
            DeployLock::default(),
            None,
        );
        assert_eq!(pushed, Ok(Pushed::Ok));
        assert_eq!(
            d1_calls(&calls),
            [
                "d1 create api-orders",
                "d1 migrations apply D1_ORDERS --remote"
            ]
        );
        let at = |prefix: &str| calls.iter().position(|c| c.starts_with(prefix));
        assert!(
            at("d1 migrations apply") < at("deploy"),
            "applied before the push: {calls:?}"
        );
        // Stdin is closed, so wrangler takes its "yes" fallback: the adapter's to do, and
        // tested there (`cloudflare::process`).
    }

    #[test]
    fn a_named_environment_applies_its_own_databases_migrations() {
        let (calls, _, pushed, config) = provision_api_with_config(
            "staging",
            with_migrations(Resources::default().d1_databases(&[("D1_ORDERS", "api-orders")])),
            DeployLock::default(),
            None,
        );
        assert_eq!(pushed, Ok(Pushed::Ok));
        // Without the key there, wrangler would read its default
        // `migrations/`, find no `.sql` directly in it, and apply nothing.
        let doc: toml::Table = config.parse().unwrap();
        assert_eq!(
            doc["env"]["staging"]["d1_databases"][0]["migrations_dir"].as_str(),
            Some("migrations/api-orders")
        );
        assert_eq!(
            d1_calls(&calls),
            [
                "d1 create api-orders-staging",
                "d1 migrations apply D1_ORDERS --remote --env staging"
            ]
        );
    }

    #[test]
    fn a_failing_migration_stops_the_deploy_before_the_push() {
        let (calls, _, pushed) = provision_api(
            "default",
            with_migrations(Resources::default().d1_databases(&[("D1_BROKEN", "api-broken")])),
            DeployLock::default(),
            None,
        );
        let Err(failure) = pushed else {
            panic!("a failed migration must fail the deploy: {calls:?}");
        };
        let message = format!("{failure:?}");
        assert!(
            message.contains("`api-broken`")
                && message.contains("nothing was pushed")
                && message.contains("0001_bad.sql failed"),
            "{message}"
        );
        assert!(!calls.iter().any(|c| c.starts_with("deploy")), "{calls:?}");
    }

    // ---- P3 (#2342): rate limiter ids are allocated in the ledger ----

    fn api_with_limiters(bindings: &[&str]) -> Resources {
        Resources::default().ratelimits(bindings)
    }

    #[test]
    fn each_rate_limiter_gets_an_allocated_id_recorded_and_materialised() {
        let (calls, lock, pushed, config) = provision_api_with_config(
            "default",
            api_with_limiters(&["RATELIMIT_LOGIN", "RATELIMIT_PUBLISH"]),
            DeployLock::default(),
            None,
        );
        assert_eq!(pushed, Ok(Pushed::Ok));
        // No account call: the id is the ledger's to choose.
        assert!(calls.iter().all(|c| !c.contains("ratelimit")), "{calls:?}");
        let login = ratelimit_seed("default", "api", "RATELIMIT_LOGIN").to_string();
        assert_eq!(
            lock.recorded::<RateLimit>("default", "api", "RATELIMIT_LOGIN")
                .map(|r| r.namespace_id.as_str()),
            Some(login.as_str())
        );
        assert!(
            lock.recorded::<RateLimit>("default", "api", "RATELIMIT_PUBLISH")
                .map(|r| r.namespace_id.as_str())
                .is_some()
        );
        assert!(
            config.contains(&format!("name = \"RATELIMIT_LOGIN\"\nnamespace_id = \"{login}\" # set at deploy time\nsimple = {{ limit = 20, period = 60 }}")),
            "{config}"
        );
        assert!(!config.contains("_NAMESPACE>"), "{config}");
    }

    #[test]
    fn a_recorded_rate_limiter_keeps_its_id_and_a_removed_one_stays_until_pushed() {
        let (_, lock, _, _) = provision_api_with_config(
            "default",
            api_with_limiters(&["RATELIMIT_LOGIN", "RATELIMIT_PUBLISH"]),
            DeployLock::default(),
            None,
        );
        let (_, lock, pushed, config) = provision_api_with_config(
            "default",
            api_with_limiters(&["RATELIMIT_PUBLISH"]),
            lock,
            None,
        );
        assert_eq!(pushed, Ok(Pushed::Ok));
        let publish = ratelimit_seed("default", "api", "RATELIMIT_PUBLISH").to_string();
        assert_eq!(
            lock.recorded::<RateLimit>("default", "api", "RATELIMIT_PUBLISH")
                .map(|r| r.namespace_id.as_str()),
            Some(publish.as_str()),
            "a second deploy (a changed `@limit` is one) reuses the id"
        );
        // Forgotten by `plan::run` after the push succeeds, not here.
        assert!(
            lock.recorded::<RateLimit>("default", "api", "RATELIMIT_LOGIN")
                .map(|r| r.namespace_id.as_str())
                .is_some()
        );
        assert!(
            config.contains(&format!("namespace_id = \"{publish}\"")),
            "{config}"
        );
    }

    #[test]
    fn a_named_environment_gets_its_own_id_in_its_block() {
        let (_, lock, _, _) = provision_api_with_config(
            "default",
            api_with_limiters(&["RATELIMIT_LOGIN"]),
            DeployLock::default(),
            None,
        );
        let (_, lock, pushed, config) = provision_api_with_config(
            "staging",
            api_with_limiters(&["RATELIMIT_LOGIN"]),
            lock,
            None,
        );
        assert_eq!(pushed, Ok(Pushed::Ok));
        // Unique across environments, which may share an account.
        let staging = lock
            .recorded::<RateLimit>("staging", "api", "RATELIMIT_LOGIN")
            .map(|r| r.namespace_id.as_str())
            .unwrap()
            .to_string();
        assert_ne!(
            Some(staging.as_str()),
            lock.recorded::<RateLimit>("default", "api", "RATELIMIT_LOGIN")
                .map(|r| r.namespace_id.as_str())
        );
        let doc: toml::Table = config.parse().unwrap();
        let block = doc["env"]["staging"]["ratelimits"].as_array().unwrap();
        assert_eq!(block[0]["name"].as_str(), Some("RATELIMIT_LOGIN"));
        assert_eq!(block[0]["namespace_id"].as_str(), Some(staging.as_str()));
        assert_eq!(block[0]["simple"]["limit"].as_integer(), Some(20));
        assert_eq!(block[0]["simple"]["period"].as_integer(), Some(60));
    }

    // ---- P5 (#2323): R2 buckets are created by name and recorded ----

    fn api_with_buckets() -> Resources {
        Resources::default().r2_buckets(&[
            ("R2_ARCHIVES", "api-archives", None),
            ("R2_INDEXES", "api-indexes", Some("eu")),
        ])
    }

    /// The `r2` invocations among `calls`, in order.
    fn r2_calls(calls: &[String]) -> Vec<&str> {
        calls
            .iter()
            .map(String::as_str)
            .filter(|c| c.starts_with("r2 "))
            .collect()
    }

    #[test]
    fn each_r2_bucket_is_created_by_name_in_its_jurisdiction_and_recorded() {
        let (calls, lock, pushed) =
            provision_api("default", api_with_buckets(), DeployLock::default(), None);
        assert_eq!(pushed, Ok(Pushed::Ok));
        assert_eq!(
            r2_calls(&calls),
            [
                "r2 bucket create api-archives",
                "r2 bucket create api-indexes --jurisdiction eu"
            ]
        );
        let archives = lock
            .recorded::<R2>("default", "api", "R2_ARCHIVES")
            .unwrap();
        assert_eq!(archives.name, "api-archives");
        assert_eq!(archives.jurisdiction, None);
        let indexes = lock.recorded::<R2>("default", "api", "R2_INDEXES").unwrap();
        assert_eq!(indexes.name, "api-indexes");
        assert_eq!(indexes.jurisdiction.as_deref(), Some("eu"));
    }

    #[test]
    fn a_recorded_r2_bucket_is_not_created_again() {
        let (_, lock, _) =
            provision_api("default", api_with_buckets(), DeployLock::default(), None);
        let (calls, _, pushed) = provision_api("default", api_with_buckets(), lock, None);
        assert_eq!(pushed, Ok(Pushed::Ok));
        assert_eq!(r2_calls(&calls), Vec::<&str>::new());
    }

    #[test]
    fn a_named_environment_creates_its_own_qualified_buckets() {
        let (calls, lock, _) =
            provision_api("staging", api_with_buckets(), DeployLock::default(), None);
        assert_eq!(
            r2_calls(&calls),
            [
                "r2 bucket create api-archives-staging",
                "r2 bucket create api-indexes-staging --jurisdiction eu"
            ]
        );
        assert_eq!(
            lock.recorded::<R2>("staging", "api", "R2_ARCHIVES")
                .unwrap()
                .name,
            "api-archives-staging"
        );
        assert!(
            lock.recorded::<R2>("default", "api", "R2_ARCHIVES")
                .is_none()
        );
    }

    #[test]
    fn a_changed_jurisdiction_makes_a_new_bucket_and_records_it() {
        let (_, lock, _) =
            provision_api("default", api_with_buckets(), DeployLock::default(), None);
        let moved = Resources::default().r2_buckets(&[
            ("R2_ARCHIVES", "api-archives", Some("fedramp")),
            ("R2_INDEXES", "api-indexes", Some("eu")),
        ]);
        let (calls, lock, _) = provision_api("default", moved, lock, None);
        assert_eq!(
            r2_calls(&calls),
            ["r2 bucket create api-archives --jurisdiction fedramp"]
        );
        assert_eq!(
            lock.recorded::<R2>("default", "api", "R2_ARCHIVES")
                .unwrap()
                .jurisdiction
                .as_deref(),
            Some("fedramp")
        );
        // The bucket it replaces is still recorded, for the plan to report.
        let kept = &lock
            .recorded::<R2>("default", "api", "R2_ARCHIVES")
            .unwrap()
            .superseded;
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].jurisdiction, None);
    }

    // ---- #2059: a namespace created again re-runs the cross-environment
    // title check ----
    //
    // Worker `api` with the bare `Kv` and `capability Staging`. The default
    // environment records `api`'s `KV_STAGING`, titled `api-staging`, and
    // staging records the bare `KV`, also `api-staging` under `--env staging`.
    // The plan passes: staging's bare id is recorded, so nothing is created.

    fn api_with_staging_instance() -> (Resources, DeployLock) {
        let declared = Resources::default()
            .needs_kv()
            .kv_instances(&["KV_STAGING"]);
        let mut lock = DeployLock::default();
        lock.record::<Kv>("default", "api", "KV", KvNamespace::of("default-bare-id"));
        lock.record::<Kv>(
            "default",
            "api",
            "KV_STAGING",
            KvNamespace::of("default-staging-id"),
        );
        lock.record::<Kv>("staging", "api", "KV", KvNamespace::of("staging-bare-id"));
        (declared, lock)
    }

    /// Staging's bare id has gone from the account, so the apply would
    /// create it again as `api-staging`, the title the default environment's
    /// instance holds. Refused before any `kv namespace create` or push.
    #[test]
    fn a_namespace_created_again_under_a_title_another_environment_holds_is_refused() {
        let (declared, lock) = api_with_staging_instance();
        let plan_time = BTreeMap::from([("api".to_string(), api_with_staging_instance().0)]);
        assert_eq!(
            kv_title_collision(&plan_time, LedgerView::of(&lock, "staging")),
            None,
            "the plan cannot see it: the bare id is recorded"
        );
        let live = BTreeSet::from([
            "default-bare-id".to_string(),
            "default-staging-id".to_string(),
        ]);
        let (calls, lock, pushed) = provision_api("staging", declared, lock, Some(live));
        let failure = pushed.expect_err("refused");
        assert_eq!(
            failure.message,
            "KV namespace for `api` in `--env staging` and KV namespace `KV_STAGING` for `api` \
             in the default environment would both be titled `api-staging` on the account, \
             which holds one namespace per title; rename a capability or context so they differ"
        );
        assert!(calls.is_empty(), "nothing ran: {calls:?}");
        assert_eq!(
            lock.recorded::<Kv>("staging", "api", "KV")
                .map(|kv| kv.id.as_str()),
            Some("staging-bare-id"),
            "the ledger is untouched"
        );
    }

    /// The same project with staging's bare id still live: nothing is created
    /// for it, the instance's distinct `api-staging-staging` is created, and
    /// the push goes ahead.
    #[test]
    fn a_live_recorded_namespace_is_not_checked_again() {
        let (declared, lock) = api_with_staging_instance();
        let live = BTreeSet::from([
            "default-bare-id".to_string(),
            "default-staging-id".to_string(),
            "staging-bare-id".to_string(),
        ]);
        let (calls, _, pushed) = provision_api("staging", declared, lock, Some(live));
        assert_eq!(pushed, Ok(Pushed::Ok));
        assert_eq!(
            calls,
            [
                "kv namespace create api-staging-staging",
                "deploy --env staging"
            ]
        );
    }

    /// At the deploy-time re-check, another environment's recorded namespace
    /// whose id has gone, and which this source no longer declares, no longer
    /// holds its title. Only there: the plan-time check runs before the live
    /// ids are fetched, so it counts every recorded namespace.
    ///
    /// `api` no longer uses the bare `Kv`, but staging still records its bare
    /// namespace (`api-staging`), an orphan. The default environment records
    /// `capability Staging` (`api-staging`), and its id has gone too, so the
    /// default deploy re-creates it under that title.
    #[test]
    fn a_gone_namespace_in_another_environment_holds_no_title() {
        let declared = || Resources::default().kv_instances(&["KV_STAGING"]);
        let lock = || {
            let mut lock = DeployLock::default();
            lock.record::<Kv>(
                "default",
                "api",
                "KV_STAGING",
                KvNamespace::of("gone-default-id"),
            );
            lock.record::<Kv>("staging", "api", "KV", KvNamespace::of("orphan-id"));
            lock
        };
        // The plan passes: `KV_STAGING` is recorded, so it creates nothing.
        let plan_time = BTreeMap::from([("api".to_string(), declared())]);
        assert_eq!(
            kv_title_collision(&plan_time, LedgerView::of(&lock(), "default")),
            None
        );

        // The orphan is still on the account: the re-create is refused.
        let (calls, _, pushed) = provision_api(
            "default",
            declared(),
            lock(),
            Some(BTreeSet::from(["orphan-id".to_string()])),
        );
        assert!(
            pushed
                .expect_err("refused")
                .message
                .contains("KV namespace for `api` in `--env staging`"),
        );
        assert!(calls.is_empty(), "nothing ran: {calls:?}");

        // The orphan's id has gone too: it holds nothing, so the re-create
        // goes ahead and is recorded.
        let (calls, lock, pushed) =
            provision_api("default", declared(), lock(), Some(BTreeSet::new()));
        assert_eq!(pushed, Ok(Pushed::Ok));
        assert_eq!(calls, ["kv namespace create api-staging", "deploy"]);
        assert_eq!(
            lock.recorded::<Kv>("default", "api", "KV_STAGING")
                .map(|kv| kv.id.as_str()),
            Some("minted-id")
        );
    }

    #[test]
    fn the_default_environment_creates_the_bare_namespace_under_the_workers_name() {
        let (calls, lock) = provision_bare_kv("default", DeployLock::default());
        assert_eq!(calls, ["kv namespace create api", "deploy"]);
        assert_eq!(
            lock.recorded::<Kv>("default", "api", "KV")
                .map(|kv| kv.id.as_str()),
            Some("minted-id")
        );
    }

    #[test]
    fn a_named_environment_creates_the_bare_namespace_under_a_qualified_title() {
        // An account holds one namespace per title, so `api` would collide
        // with the default environment's.
        let (calls, lock) = provision_bare_kv("staging", DeployLock::default());
        assert_eq!(
            calls,
            ["kv namespace create api-staging", "deploy --env staging"]
        );
        assert_eq!(
            lock.recorded::<Kv>("staging", "api", "KV")
                .map(|kv| kv.id.as_str()),
            Some("minted-id")
        );
        assert_eq!(
            lock.recorded::<Kv>("default", "api", "KV")
                .map(|kv| kv.id.as_str()),
            None
        );
    }

    #[test]
    fn a_recorded_staging_namespace_is_reused_not_recreated() {
        // Whatever it was titled when made, a namespace the ledger records for
        // the environment is the one the deploy binds.
        let mut lock = DeployLock::default();
        lock.record::<Kv>("staging", "api", "KV", KvNamespace::of("staging-id"));
        let (calls, lock) = provision_bare_kv("staging", lock);
        assert_eq!(calls, ["deploy --env staging"]);
        assert_eq!(
            lock.recorded::<Kv>("staging", "api", "KV")
                .map(|kv| kv.id.as_str()),
            Some("staging-id")
        );
    }

    // ---- #2055: a new Worker's first version carries its secrets ----
    //
    // the apply of one context against a stand-in `wrangler` that records its arguments,
    // drains a `secret put`'s stdin, and notes whether a `--secrets-file` it
    // is handed exists and names the secret. Nothing reaches Cloudflare.

    const BINDING: &str = "LEDGER_CALL";
    const PLANTED: &str = "planted-2055";

    /// What one apply of a Worker `api` with one secret to set left
    /// behind.
    struct BindingRun {
        /// One line per `wrangler` invocation.
        calls: Vec<String>,
        /// What the stand-in saw of the `--secrets-file`, one line per upload.
        seen: Vec<String>,
        pushed: Result<Pushed, DeployFailure>,
        /// Whether the run left a `bynk-secrets-*` directory in the project.
        left_in_project: bool,
    }

    /// How the fake `wrangler deploy` ends.
    #[derive(Clone, Copy)]
    enum Tail {
        /// It succeeds.
        Ok,
        /// It exits 1.
        Fails,
        /// A shell script runs it (Unix), through the real process path: for the signal
        /// tests, which need a process to hold a secrets file across.
        #[cfg(unix)]
        Script(&'static str),
    }

    /// Whether `tail` runs on the recording fake rather than a script.
    fn recording_used(tail: Tail) -> bool {
        matches!(tail, Tail::Ok | Tail::Fails)
    }

    fn deploy_with_binding(
        environment: &str,
        already_deployed: bool,
        with_upload: bool,
        tail: Tail,
    ) -> BindingRun {
        // One at a time: a first deploy records SIGINT/SIGTERM process-wide
        // while its upload runs, and two tests signalling this process would
        // see each other's signals.
        let _lock = crate::stop::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "bynk-2055-{environment}-{}-{unique}",
            std::process::id()
        ));
        let workers_dir = root.join("workers");
        std::fs::create_dir_all(workers_dir.join("api")).unwrap();
        std::fs::write(
            workers_dir.join("api/wrangler.toml"),
            "name = \"api\"\nmain = \"index.ts\"\n",
        )
        .unwrap();
        let log = root.join("calls.log");
        // A `--secrets-file` is checked while the upload runs: it must exist
        // then, and hold the name. Its value is never echoed.
        let seen_by_fake = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let seen = seen_by_fake.clone();
        let recording = crate::cloudflare::fake::Recording::new(move |op| {
            use crate::cloudflare::Op;
            let Op::Push { secrets_file, .. } = op else {
                return Ok(String::new());
            };
            if let Some(file) = secrets_file {
                let held = std::fs::read_to_string(file)
                    .is_ok_and(|text| text.contains(&format!("\"{BINDING}\":\"{PLANTED}\"")));
                seen.borrow_mut()
                    .push(if held { "present" } else { "missing" }.into());
            }
            match tail {
                Tail::Fails => Err(crate::cloudflare::fake::failed(op, "", "")),
                _ => Ok(String::new()),
            }
        });
        let seen = root.join("seen.log");
        #[cfg(unix)]
        let (script, scripted);
        let api: &dyn CloudflareApi = match tail {
            #[cfg(unix)]
            Tail::Script(deploy_tail) => {
                script = format!(
                    "echo \"$*\" >> '{log}'\n\
                     if [ \"$1\" = secret ] && [ \"$2\" = put ]; then cat > /dev/null; fi\n\
                     if [ \"$1\" = deploy ]; then\n\
                     prev=\n\
                     for a in \"$@\"; do\n\
                     if [ \"$prev\" = --secrets-file ]; then\n\
                     if [ -f \"$a\" ] && grep -q '\"{BINDING}\":\"{PLANTED}\"' \"$a\"; then echo present >> '{seen}'; else echo missing >> '{seen}'; fi\n\
                     fi\n\
                     prev=$a\n\
                     done\n\
                     {deploy_tail}\n\
                     fi\n\
                     exit 0\n",
                    log = log.display(),
                    seen = seen.display(),
                );
                scripted = crate::cloudflare::Account::new(Wrangler::scripted(&script, &root));
                &scripted
            }
            Tail::Ok | Tail::Fails => &recording,
        };

        let mut lock = DeployLock::default();
        if already_deployed {
            lock.record_deployed(environment, "api", None);
        }
        let source = SecretSource::default();
        let edge_keys = BTreeMap::new();
        let foreign_keys = BTreeMap::from([(
            ("api".to_string(), BINDING.to_string()),
            ForeignKey::Set {
                value: PLANTED.to_string(),
                minted: false,
            },
        )]);
        let foreign_secrets = BTreeSet::from([BINDING.to_string()]);
        let listed = BTreeMap::from([("api".to_string(), Some(BTreeSet::new()))]);
        let mut resolved = BTreeMap::new();
        let project = BTreeMap::from([("api".to_string(), Resources::default())]);
        // 1D.2b (#4001): as `run` applies a context, its plan's steps.
        let order = ["api".to_string()];
        let plan = derive_plan(
            &order,
            &project,
            &lock,
            &SecretSource::default(),
            false,
            environment,
        );
        let pushed = apply_worker(
            &mut RunCx {
                tools: Tools {
                    api,
                    project_root: &root,
                },
                workers_dir: &workers_dir,
                project: &project,
                writer: &mut LedgerWriter::new(&mut lock, &root.join(LOCK_FILE)),
                secrets: &mut Secrets {
                    source: &source,
                    force: false,
                    edge_keys: &edge_keys,
                    expired: &std::collections::BTreeMap::new(),
                    cleaned: &std::cell::RefCell::new(std::collections::BTreeSet::new()),
                    foreign_keys: &foreign_keys,
                    foreign_secrets: &foreign_secrets,
                    listed: &listed,
                    resolved: &mut resolved,
                    with_upload,
                },
                wrangler_args: &[],
                environment,
                lives: Lives::default(),
            },
            &plan.contexts[0],
        );
        let read = |path: &Path| -> Vec<String> {
            std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .map(strip_facts_var)
                .collect()
        };
        let left_in_project = walk(&root)
            .iter()
            .any(|p| p.to_string_lossy().contains("bynk-secrets"));
        let lock_text = std::fs::read_to_string(root.join(LOCK_FILE)).unwrap_or_default();
        assert!(
            !lock_text.contains(PLANTED),
            "the ledger never carries a secret"
        );
        let (calls, seen) = if recording_used(tail) {
            (
                recording
                    .calls()
                    .iter()
                    .map(|c| strip_facts_var(c))
                    .collect(),
                seen_by_fake.borrow().clone(),
            )
        } else {
            (read(&log), read(&seen))
        };
        let run = BindingRun {
            calls,
            seen,
            pushed,
            left_in_project,
        };
        let _ = std::fs::remove_dir_all(root);
        run
    }

    fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            }
            out.push(path);
        }
        out
    }

    /// The `--secrets-file` path in the run's only call, checked to be a
    /// `deploy` with nothing else but `rest`, and to sit under the system temp
    /// dir. The asserts here and below compare without formatting what wrangler
    /// was handed into a failure message: a test of secret handling logs none
    /// of it, even its own planted values.
    fn secrets_file_of(calls: &[String], rest: &[&str]) -> std::path::PathBuf {
        assert!(calls.len() == 1, "a first deploy is one wrangler call");
        let call = &calls[0];
        let tokens: Vec<&str> = call.split(' ').collect();
        assert!(
            tokens.len() == 3 + rest.len() && tokens[..2] == ["deploy", "--secrets-file"],
            "the call is `deploy --secrets-file <path>`, then `rest`"
        );
        assert!(tokens[3..] == *rest, "and then exactly `rest`");
        let path = std::path::PathBuf::from(tokens[2]);
        assert!(
            path.starts_with(std::env::temp_dir()),
            "the secrets file is written under the temp dir, not the project"
        );
        assert!(
            !call.contains(PLANTED),
            "only the path reaches argv, never a value"
        );
        path
    }

    #[test]
    fn a_first_deploy_uploads_its_secrets_with_the_worker() {
        let run = deploy_with_binding("default", false, true, Tail::Ok);
        assert!(run.pushed == Ok(Pushed::Ok), "the push succeeds");
        // One call, and no `secret put` after it: the first version has them.
        let file = secrets_file_of(&run.calls, &[]);
        assert!(
            run.seen == ["present"],
            "the file existed during the upload"
        );
        assert!(!file.exists(), "and is removed once it succeeded");
        assert!(!file.parent().unwrap().exists(), "with its directory");
        assert!(!run.left_in_project);
    }

    #[test]
    fn a_failed_first_upload_still_removes_the_secrets_file() {
        let run = deploy_with_binding("default", false, true, Tail::Fails);
        assert!(run.pushed.is_err(), "wrangler's failure is the deploy's");
        let file = secrets_file_of(&run.calls, &[]);
        assert!(
            run.seen == ["present"],
            "the file existed during the upload"
        );
        assert!(!file.exists(), "removed although the upload failed");
        assert!(!file.parent().unwrap().exists());
    }

    #[test]
    fn a_first_deploy_under_an_environment_uploads_to_the_suffixed_worker() {
        // `--env staging` is what makes wrangler upload `api-staging`, the
        // secrets with it.
        let run = deploy_with_binding("staging", false, true, Tail::Ok);
        assert!(run.pushed == Ok(Pushed::Ok), "the push succeeds");
        let file = secrets_file_of(&run.calls, &["--env", "staging"]);
        assert!(
            run.seen == ["present"],
            "the file existed during the upload"
        );
        assert!(!file.exists());
    }

    // Unix: the upload runs as a process that signals this one, through the real process path.
    #[cfg(unix)]
    #[test]
    fn a_sigterm_during_the_upload_waits_for_wrangler_and_still_removes_the_file() {
        // What a CI cancel does to `bynk` alone, mid-upload. Without the
        // deferral this would end the test binary with the file on disk.
        let run = deploy_with_binding(
            "default",
            false,
            true,
            Tail::Script("kill -TERM $PPID; sleep 0.2; exit 0"),
        );
        let file = secrets_file_of(&run.calls, &[]);
        assert!(
            run.seen == ["present"],
            "the file existed during the upload"
        );
        assert!(!file.exists(), "removed although bynk was signalled");
        assert!(!file.parent().unwrap().exists());
        assert!(
            run.pushed.as_ref().is_err_and(|f| f.code == 143),
            "the run stops, exiting as SIGTERM does"
        );
    }

    // Unix: the upload runs as a process that signals this one, through the real process path.
    #[cfg(unix)]
    #[test]
    fn a_sigterm_to_bynk_is_passed_on_to_the_upload() {
        // Sent to `bynk` alone, a SIGTERM would otherwise leave wrangler
        // running and `bynk` waiting on it.
        let started = std::time::Instant::now();
        let run = deploy_with_binding(
            "default",
            false,
            true,
            Tail::Script("kill -TERM $PPID; exec sleep 30"),
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "wrangler was stopped, not waited out"
        );
        let file = secrets_file_of(&run.calls, &[]);
        assert!(!file.exists(), "and the file is removed");
        assert!(
            run.pushed.as_ref().is_err_and(|f| f.code == 143),
            "wrangler died of the SIGTERM"
        );
    }

    // Unix: process liveness by pid and directory ownership by uid.
    #[cfg(unix)]
    #[test]
    fn the_sweep_removes_only_a_dead_runs_secrets_directory() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp =
            std::env::temp_dir().join(format!("bynk-2055-sweep-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // A pid that has exited: a child, reaped.
        let mut gone = std::process::Command::new("true").spawn().unwrap();
        let dead = gone.id();
        gone.wait().unwrap();
        // A pid that is running: a child, until the end of the test.
        let mut alive = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let dir = |pid: u32| tmp.join(format!("bynk-secrets-{pid}-1-0"));
        for d in [dir(dead), dir(alive.id()), dir(std::process::id())] {
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("secrets.json"), "{}").unwrap();
        }
        let unrelated = [tmp.join("bynk-secrets-notapid"), tmp.join("other-1-0")];
        for d in &unrelated {
            std::fs::create_dir_all(d).unwrap();
        }

        let removed = sweep_stale_upload_dirs(&tmp);
        let _ = alive.kill();
        let _ = alive.wait();
        let kept = |d: &Path| d.exists();
        let result = (
            removed,
            kept(&dir(dead)),
            kept(&dir(alive.id())),
            kept(&dir(std::process::id())),
            unrelated.iter().all(|d| kept(d)),
        );
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(result.0, 1, "one directory removed");
        assert!(!result.1, "the dead run's directory is gone");
        assert!(result.2, "a live run's is kept");
        assert!(result.3, "this run's own is kept");
        assert!(result.4, "nothing else is touched");
    }

    #[test]
    fn a_secrets_directory_name_carries_its_pid() {
        assert_eq!(secrets_dir_pid("bynk-secrets-4242-17-0"), Some(4242));
        assert_eq!(secrets_dir_pid("bynk-secrets-x-17-0"), None);
        assert_eq!(secrets_dir_pid("bynk-other-4242-17-0"), None);
        let file = SecretsFile::write(&[]).unwrap();
        let name = file
            .path()
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(secrets_dir_pid(&name), Some(std::process::id()));
    }

    #[test]
    fn a_redeploy_sets_its_secrets_before_the_push_on_stdin() {
        // The Worker exists, so its secrets go first, as their own version,
        // and never through a file.
        let run = deploy_with_binding("default", true, true, Tail::Ok);
        assert!(run.pushed == Ok(Pushed::Ok), "the push succeeds");
        assert!(
            run.calls == [format!("secret put {BINDING}"), "deploy".into()],
            "`secret put`, then `deploy`"
        );
        assert!(run.seen.is_empty(), "no secrets file");
        let run = deploy_with_binding("staging", true, true, Tail::Ok);
        assert!(
            run.calls
                == [
                    format!("secret put {BINDING} --env staging"),
                    "deploy --env staging".into()
                ],
            "both under `--env staging`"
        );
    }

    #[test]
    fn a_wrangler_without_the_secrets_file_falls_back_to_setting_after_the_push() {
        let run = deploy_with_binding("default", false, false, Tail::Ok);
        assert!(run.pushed == Ok(Pushed::Ok), "the push succeeds");
        assert!(
            run.calls == ["deploy".to_string(), format!("secret put {BINDING}")],
            "`deploy`, then `secret put`"
        );
        // And a failed push sets nothing on a Worker that may not exist.
        let run = deploy_with_binding("default", false, false, Tail::Fails);
        assert!(run.pushed.is_err(), "the push fails");
        assert!(run.calls == ["deploy"], "and nothing is set after it");
    }

    // Unix: file modes 0600 and 0700.
    #[cfg(unix)]
    #[test]
    fn the_secrets_file_is_private_json_of_exactly_the_prepared_secrets() {
        use std::os::unix::fs::PermissionsExt;
        let prepared = vec![
            (
                "A".to_string(),
                "with \"quotes\"\nand a newline".to_string(),
            ),
            ("B".to_string(), "plain".to_string()),
        ];
        let file = SecretsFile::write(&prepared).unwrap();
        let path = file.path().to_path_buf();
        let dir = path.parent().unwrap().to_path_buf();
        assert!(path.starts_with(std::env::temp_dir()));
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);
        let parsed: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(
            parsed == prepared.into_iter().collect::<BTreeMap<_, _>>(),
            "the file holds exactly the prepared pairs"
        );
        drop(file);
        assert!(!dir.exists(), "dropping it removes the directory");
    }

    #[test]
    fn the_set_line_names_the_secret_and_the_deployed_worker_never_the_value() {
        let line = binding_set_line(BINDING, &deployed_worker_name("staging", "api"), true);
        assert_eq!(
            line,
            "bynk: set secret `LEDGER_CALL` on `api-staging`, with its first version"
        );
        assert!(!line.contains(PLANTED));
        assert_eq!(
            binding_set_line(BINDING, "api", false),
            "bynk: set secret `LEDGER_CALL` on `api`"
        );
    }

    #[test]
    fn only_an_installed_wrangler_known_to_be_older_lacks_the_secrets_file() {
        let wrangler = |version: Option<&str>| probe::Probe {
            tool: "wrangler".to_string(),
            version: version.map(|v| probe::Version::parse(v).unwrap()),
            provenance: Provenance::Path("/usr/bin/wrangler".into()),
        };
        assert!(!secrets_file_supported(&wrangler(Some("4.73.9"))));
        assert!(secrets_file_supported(&wrangler(Some(SECRETS_FILE_MIN))));
        assert!(secrets_file_supported(&wrangler(Some(
            bynk_emit::WRANGLER_MIN
        ))));
        assert!(secrets_file_supported(&wrangler(None)), "npx: newest 4.x");
    }
}
