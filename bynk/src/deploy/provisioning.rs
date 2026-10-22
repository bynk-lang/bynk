use super::*;

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
    status: &std::process::ExitStatus,
) -> Result<Pushed, DeployFailure> {
    if status.success() {
        return Ok(Pushed::Ok);
    }
    // `exit_status_byte` is the driver's one place for "what should a child's
    // status make us exit with?" — including the deliberate SIGINT → 0 (a
    // shared Ctrl-C is a clean stop) and the `128 + signal` convention for the
    // signals that are real deaths. Reuse it rather than re-deciding here.
    match exit_status_byte(status) {
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

/// Provision and push exactly one context. Slice 0's body, lifted so the
/// multi-context loop and `--context` share one path.
///
/// The phase order is the contract (ADR 0194 D3): everything the upload needs
/// to already exist — the KV namespace, the queues a `[[queues.consumers]]`
/// binding would otherwise fail against — is provisioned first; Durable
/// Object namespaces are not provisioned at all, because Cloudflare creates
/// them from the `exports` declaration in the same config `wrangler deploy`
/// is pushing (#1796).
#[allow(clippy::too_many_arguments)]
pub(crate) fn deploy_one(
    provenance: &Provenance,
    project_root: &Path,
    workers_dir: &Path,
    worker: &str,
    declared: &Resources,
    // #2059: the whole project, for the cross-environment title check a
    // create re-runs ([`cross_environment_title_clash`]).
    project: &BTreeMap<String, Resources>,
    lock: &mut DeployLock,
    lock_path: &Path,
    attempted_queues: &mut BTreeSet<String>,
    secrets: &mut Secrets<'_>,
    wrangler_args: &[String],
    environment: &str,
    // Slice 5, DECISION B: `Some` when `run()` fetched the account's live KV
    // namespace ids once, up front. A recorded id absent from this set is
    // exactly as untrustworthy as no record at all — re-provision rather than
    // inject a dead id. `None` means the fetch was skipped or failed; the
    // recorded id is trusted unconditionally, as it always was pre-slice-5,
    // rather than blocking a deploy on a fetch that didn't happen.
    live_kv_ids: Option<&BTreeSet<String>>,
) -> Result<Pushed, DeployFailure> {
    let worker_dir = workers_dir.join(worker);
    let config = worker_dir.join("wrangler.toml");
    // Every KV namespace the Worker binds: the bare `KV`, then each derived
    // instance (S4b, #1967), each its own namespace with its own ledger entry.
    let mut kv_ids = BTreeMap::new();
    for binding in kv_bindings(declared) {
        let recorded = recorded_kv_binding(lock, worker, binding, environment).map(str::to_owned);
        let trust_recorded = should_trust_recorded_kv(recorded.as_deref(), live_kv_ids);
        let id = if trust_recorded {
            recorded.expect("trust_recorded is only true when recorded is Some")
        } else {
            // #2059: the plan-time check covered only namespaces with no
            // recorded id. One whose recorded id has gone from the account is
            // created again here, under its current title, so compare that
            // title across environments now, before `kv namespace create`.
            if let Some(message) = cross_environment_title_clash(
                project,
                lock,
                environment,
                &[(worker, binding)],
                live_kv_ids,
            ) {
                return Err(DeployFailure::driver(message));
            }
            let what = kv_namespace_noun(worker, binding);
            let id = create_kv(
                provenance,
                &kv_title(worker, binding, environment),
                project_root,
            )
            .map_err(|e| DeployFailure::driver(format!("could not create {what}: {e}")))?;
            lock.record_kv(environment, worker, binding, &id);
            // Recorded before the push, so an interrupted run never makes a
            // second namespace (ADR 0180).
            write_lock(lock_path, lock).map_err(|e| {
                DeployFailure::driver(format!(
                    "created {what} but could not record it in {}: {e}",
                    lock_path.display()
                ))
            })?;
            id
        };
        kv_ids.insert(binding.to_string(), id);
    }
    // P5 (#2323): each `R2` bucket, made by name (the name is the identity, so
    // an existing one is the same end state) and recorded before the push. A
    // bucket the ledger already holds under the same jurisdiction is not asked
    // of the account again. Buckets are named as `environment` names them in
    // the config the push reads: the top level already carries the default
    // environment's, and the synthesised `[env.<name>]` block the rest.
    for bucket in &declared.r2_buckets {
        if r2_action(lock, worker, bucket, environment) == "reuse" {
            continue;
        }
        let physical = r2_bucket_name(environment, &bucket.name);
        create_r2_bucket(
            provenance,
            &physical,
            bucket.jurisdiction.as_deref(),
            project_root,
        )
        .map_err(|e| {
            DeployFailure::driver(format!(
                "could not create the R2 bucket `{physical}` for `{worker}`'s `{}`: {e}",
                bucket.binding
            ))
        })?;
        lock.record_r2(
            environment,
            worker,
            &bucket.binding,
            &physical,
            bucket.jurisdiction.as_deref(),
        );
        write_lock(lock_path, lock).map_err(|e| {
            DeployFailure::driver(format!(
                "created the R2 bucket `{physical}` but could not record it in {}: {e}",
                lock_path.display()
            ))
        })?;
    }
    if !kv_ids.is_empty() && !materialise_kv_ids(&config, &kv_ids) {
        return Err(DeployFailure::driver(format!(
            "could not materialise the KV namespace ids into `{worker}`'s generated configuration"
        )));
    }
    // Reconciled against the account on every run, never against the ledger:
    // the ledger's queue set is a planning aid, so trusting it to skip would
    // leave a queue deleted out-of-band un-recreated and the push failing
    // against a binding with nothing behind it (ADR 0194 D2). Once per run,
    // not once per consuming context — a queue two contexts consume is one
    // queue. `wrangler deploy` will not create it for us: it checks and fails
    // with "To create it, run: wrangler queues create", so this step is the one
    // that makes such a project deployable at all.
    //
    // The account-facing name is environment-qualified (slice 4, DECISION C):
    // queues reconcile by bare name account-wide, so two environments sharing
    // an account would otherwise create-or-reuse the same physical queue. The
    // ledger still keys by the *logical* name — the outer `environments` map
    // already separates `staging` from `default`, so no schema change is
    // needed, only the wrangler-facing name changes.
    for queue in unattempted_queues(declared, attempted_queues) {
        let physical = env_qualify(environment, &queue);
        if !queue_exists(provenance, &physical, project_root) {
            create_queue(provenance, &physical, project_root).map_err(|e| {
                DeployFailure::driver(format!("could not create the queue `{physical}`: {e}"))
            })?;
        }
        // Recorded before the push, as KV is: what the ledger claims is only
        // ever what it watched succeed. Recorded for a queue that was already
        // there, too — the set's use is the plan's `create`/`reuse` wording, and
        // "we confirmed this exists" is exactly what makes `reuse` the true word.
        if lock.record_queue(environment, &queue) {
            write_lock(lock_path, lock).map_err(|e| {
                DeployFailure::driver(format!(
                    "provisioned the queue `{physical}` but could not record it in {}: {e}",
                    lock_path.display()
                ))
            })?;
        }
    }
    // Slice 4 (DECISION B+C): Cloudflare does not inherit bindings into a named
    // environment, so a non-default `--env` needs its own `[env.<name>]` table —
    // synthesised here, not by the emitter, since the environment name is a
    // deploy-time concept the compiler never sees. The top-level stanza is left
    // untouched; it continues to serve the plain, no-`--env` `bynk deploy`.
    if environment != "default" {
        let config_text = std::fs::read_to_string(&config).map_err(|e| {
            DeployFailure::driver(format!(
                "could not read `{worker}`'s generated configuration: {e}"
            ))
        })?;
        let foreign: BTreeSet<String> = declared
            .foreign_services
            .iter()
            .map(|f| f.binding.clone())
            .collect();
        let synthesised = synthesise_environment_block(
            &config_text,
            environment,
            &kv_ids,
            &foreign,
        )
        .map_err(|e| {
            DeployFailure::driver(format!(
                "could not synthesise the `[env.{environment}]` configuration for `{worker}`: {e}"
            ))
        })?;
        std::fs::write(&config, synthesised).map_err(|e| {
            DeployFailure::driver(format!(
                "could not write `{worker}`'s `[env.{environment}]` configuration: {e}"
            ))
        })?;
    }
    // Secrets straddle the push, and which side depends on whether the Worker
    // already exists (ADR 0195 D6).
    //
    // `wrangler secret put` against a Worker that is not on the account yet does
    // not fail — it creates a **stub draft Worker** (`export default { fetch()
    // {} }`) and puts the secret on that. Non-interactively it does so without
    // asking (its confirm falls back to yes), and interactively it prompts
    // mid-deploy — where a decline makes it exit **0** having set nothing, which
    // `deploy` would read as success and push behind. Neither is a thing to do
    // on a plan that said "deploy `api`".
    //
    // So: a Worker the ledger has pushed before exists, and its secrets are set
    // **before** the push, as the phase order intends — the running code never
    // sees a request without them.
    //
    // A first deploy uploads its secrets **with** the push (#2055):
    // `wrangler deploy --secrets-file` puts them in the new Worker's first
    // version, so it is never live without them. Setting them after the push,
    // as this did before, left a new Worker serving for a few seconds with no
    // auth secret (fail-closed, answering 401, but contrary to what the guide
    // promised). The file is the only way in for a Worker that does not exist
    // yet, and it is used only there: a value piped to `secret put` never
    // touches disk, and a file — however briefly, 0600, in a private temp
    // directory — does, so a redeploy keeps the stdin path.
    //
    // What does **not** straddle is the deciding and resolving: `prepare_bindings`
    // runs before the push on *both* paths, so a missing value is found before
    // anything is uploaded.
    let first_deploy = !lock.is_deployed(environment, worker);
    let prepared = prepare_bindings(
        provenance,
        &worker_dir,
        worker,
        declared,
        secrets,
        first_deploy,
        environment,
    )?;
    let deployed = deployed_worker_name(environment, worker);
    if !first_deploy {
        apply_secrets(provenance, &worker_dir, worker, &prepared, environment)?;
        return push(
            provenance,
            &worker_dir,
            worker,
            wrangler_args,
            environment,
            None,
        );
    }
    if prepared.is_empty() {
        return push(
            provenance,
            &worker_dir,
            worker,
            wrangler_args,
            environment,
            None,
        );
    }
    if !secrets.with_upload {
        // An installed wrangler older than `--secrets-file`: the old order,
        // said out loud, since the guide promises otherwise.
        eprintln!(
            "bynk: this wrangler is older than {SECRETS_FILE_MIN}, which cannot upload secrets with a new Worker, so `{deployed}` will be live for a moment before its secrets are set. Upgrade wrangler to close that window."
        );
        let pushed = push(
            provenance,
            &worker_dir,
            worker,
            wrangler_args,
            environment,
            None,
        )?;
        if pushed == Pushed::Ok {
            apply_secrets(provenance, &worker_dir, worker, &prepared, environment)?;
        }
        return Ok(pushed);
    }
    let file = SecretsFile::write(&prepared).map_err(|e| {
        DeployFailure::driver(format!(
            "could not write the secrets for `{deployed}`'s first version to a temporary file: {e}"
        ))
    })?;
    let pushed = push(
        provenance,
        &worker_dir,
        worker,
        wrangler_args,
        environment,
        Some(file.path()),
    );
    // Removed whatever the push did, before anything else can fail.
    drop(file);
    let pushed = pushed?;
    if pushed == Pushed::Ok {
        report_first_version_bindings(&prepared, &deployed);
    }
    Ok(pushed)
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
/// are deferred until wrangler has exited ([`push_holding_secrets_file`]), so
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
fn push(
    provenance: &Provenance,
    worker_dir: &Path,
    worker: &str,
    wrangler_args: &[String],
    environment: &str,
    secrets_file: Option<&Path>,
) -> Result<Pushed, DeployFailure> {
    let Some(mut command) = workers::wrangler_command(provenance, "deploy") else {
        return Err(DeployFailure::driver("wrangler not found".into()));
    };
    command.current_dir(worker_dir).args(wrangler_args);
    if let Some(file) = secrets_file {
        command.arg("--secrets-file").arg(file);
    }
    if environment != "default" {
        command.arg("--env").arg(environment);
    }
    if secrets_file.is_some() {
        return push_holding_secrets_file(command, environment, worker);
    }
    match command.status() {
        Ok(status) => wrangler_outcome(environment, worker, &status),
        Err(e) => Err(DeployFailure::driver(format!(
            "could not run wrangler deploy for `{}`: {e}",
            deployed_worker_name(environment, worker)
        ))),
    }
}

/// #2055: run the upload that carries a secrets file without letting a
/// signal end `bynk` first, so the caller can always remove the file.
///
/// SIGINT and SIGTERM are recorded for as long as wrangler runs
/// ([`crate::stop::defer`]), then the earlier dispositions are restored. A
/// Ctrl-C has already reached wrangler through the foreground group, so only
/// a SIGTERM is passed on to it: one sent to `bynk` alone (a CI step's
/// cancel) would otherwise leave wrangler running and `bynk` waiting on it.
/// Wrangler's exit is mapped as any push's is. If it exited 0 but `bynk` was
/// asked to stop, the run stops: as an interrupt for SIGINT, as a failure
/// with the signal's exit code otherwise. SIGKILL cannot be deferred, and
/// leaves the file for the next deploy's [`sweep_stale_upload_dirs`].
fn push_holding_secrets_file(
    mut command: std::process::Command,
    environment: &str,
    worker: &str,
) -> Result<Pushed, DeployFailure> {
    let deployed = deployed_worker_name(environment, worker);
    let deferred = crate::stop::defer();
    let mut child = command.spawn().map_err(|e| {
        DeployFailure::driver(format!(
            "could not run wrangler deploy for `{deployed}`: {e}"
        ))
    })?;
    #[cfg_attr(not(unix), allow(unused_mut, unused_variables))]
    let mut forwarded = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                return Err(DeployFailure::driver(format!(
                    "could not wait for wrangler deploy for `{deployed}`: {e}"
                )));
            }
        }
        #[cfg(unix)]
        if !forwarded && crate::stop::received() == Some(libc::SIGTERM) {
            // SAFETY: signalling our own child, which has not been reaped.
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
            }
            forwarded = true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let signalled = crate::stop::received();
    drop(deferred);
    match (wrangler_outcome(environment, worker, &status)?, signalled) {
        (Pushed::Ok, Some(sig)) if crate::stop::exit_byte(sig) == 0 => Ok(Pushed::Interrupted),
        (Pushed::Ok, Some(sig)) => Err(DeployFailure {
            message: format!(
                "stopped by {} after wrangler deploy finished for `{deployed}`",
                crate::stop::name(sig)
            ),
            code: crate::stop::exit_byte(sig),
        }),
        (pushed, _) => Ok(pushed),
    }
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

pub(crate) fn whoami(provenance: &Provenance) -> bool {
    let Some(mut command) = workers::wrangler_command(provenance, "whoami") else {
        return false;
    };
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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
    Unknown(String),
}

/// Interop I5 (#1972): ask the account whether Worker `name` exists, with
/// `wrangler deployments list --name <name> --json` (a read of
/// `/accounts/<id>/workers/scripts/<name>/deployments`). Run from the project
/// root, like the other account reads, so no generated config is loaded.
pub(crate) fn worker_presence(
    provenance: &Provenance,
    name: &str,
    project_root: &Path,
) -> WorkerPresence {
    let Some(mut command) = workers::wrangler_command(provenance, "deployments") else {
        return WorkerPresence::Unknown("wrangler not found".into());
    };
    match command
        .args(["list", "--name", name, "--json"])
        .current_dir(project_root)
        .output()
    {
        Ok(output) => classify_worker_listing(output.status.success(), &wrangler_said(&output)),
        Err(e) => WorkerPresence::Unknown(e.to_string()),
    }
}

/// [`worker_presence`]'s reading of one listing: success is presence, and a
/// failure names a missing Worker only by Cloudflare's own code, so an auth
/// or network failure is never mistaken for one.
pub(crate) fn classify_worker_listing(success: bool, said: &str) -> WorkerPresence {
    if success {
        WorkerPresence::Present
    } else if said.contains("[code: 10007]") || said.contains("[code: 10090]") {
        WorkerPresence::Missing
    } else {
        WorkerPresence::Unknown(said.to_string())
    }
}

/// How a message names one of `worker`'s KV namespaces: `KV namespace for
/// `api`` for the bare one, as it always has, `KV namespace `KV_SESSIONS` for
/// `shop-cart`` for a derived instance (S4b, #1967).
pub(crate) fn kv_namespace_noun(worker: &str, binding: &str) -> String {
    if is_bare_kv(binding) {
        format!("KV namespace for `{worker}`")
    } else {
        format!("KV namespace `{binding}` for `{worker}`")
    }
}

fn create_kv(provenance: &Provenance, name: &str, project_root: &Path) -> Result<String, String> {
    let Some(mut command) = workers::wrangler_command(provenance, "kv") else {
        return Err("wrangler not found".into());
    };
    let output = command
        .arg("namespace")
        .arg("create")
        .arg(name)
        .arg("--json")
        // The generated worker config still carries the placeholder until the
        // namespace exists. Create from the project root so Wrangler cannot
        // load and validate that incomplete config.
        .current_dir(project_root)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(wrangler_said(&output));
    }
    parse_kv_id(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| "wrangler did not return a namespace id".into())
}

fn parse_kv_id(output: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(output)
        .ok()?
        .get("id")?
        .as_str()
        .map(str::to_owned)
}

/// Every KV namespace id currently on the account — slice 5, DECISION B.
///
/// Fetched **once per `bynk deploy` run**, not once per context: this is an
/// account-wide list, so calling it from inside `deploy_one` would pay one
/// identical round-trip per KV-bearing context on every redeploy. `run()`
/// calls this once, before the per-context loop, and threads the result
/// through — see `deploy_one`'s `live_kv_ids` parameter.
///
/// `None` means "could not ask" (no wrangler, network/auth failure, or an
/// unparseable response) — read the same way [`list_secrets`]'s `None` is:
/// the caller falls back to trusting whatever the ledger already says, rather
/// than blocking every deploy on this call succeeding.
///
/// **Completeness (review, #840):** Cloudflare's list-namespaces endpoint is
/// itself paginated, but `wrangler`'s `listKVNamespaces` (`workers-sdk`
/// `packages/wrangler/src/kv/helpers.ts`) loops `page`/`per_page` until a
/// short page comes back, aggregating every page before this command prints
/// — confirmed against wrangler's own source, not assumed. A truncated
/// response is not a live risk on the CLI path this calls.
pub(crate) fn live_kv_namespace_ids(
    provenance: &Provenance,
    project_root: &Path,
) -> Option<BTreeSet<String>> {
    let mut command = workers::wrangler_command(provenance, "kv")?;
    // No `--format json`: unlike `secret list`/`queues`, this command has no
    // such flag at all — confirmed against wrangler 4.103 (`Unknown argument:
    // format`, exit 1). Its *default* output is already a raw JSON array, so
    // this is not a missing-flag gap, just a different default per command.
    let output = command
        .arg("namespace")
        .arg("list")
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_kv_namespace_ids(&String::from_utf8_lossy(&output.stdout))
}

/// The ids in `wrangler kv namespace list`'s (default, already-JSON) output —
/// the same structural shape [`parse_secret_list`] reads, `"id"` instead of
/// `"name"`.
fn parse_kv_namespace_ids(stdout: &str) -> Option<BTreeSet<String>> {
    let parsed: serde_json::Value = serde_json::from_str(stdout).ok()?;
    Some(
        parsed
            .as_array()?
            .iter()
            .filter_map(|entry| entry.get("id")?.as_str().map(str::to_owned))
            .collect(),
    )
}

/// Create `name`, or accept that it is already there.
///
/// Unlike KV there is no id to scrape: Cloudflare addresses a queue by the name
/// `from queue("n")` already gave it, so a successful create and an
/// already-existing queue are the same end state and both return `Ok`
/// (ADR 0194 D2).
fn create_queue(provenance: &Provenance, name: &str, project_root: &Path) -> Result<(), String> {
    let Some(mut command) = workers::wrangler_command(provenance, "queues") else {
        return Err("wrangler not found".into());
    };
    let output = command
        .arg("create")
        .arg(name)
        // As for KV: the generated config may still carry the KV placeholder,
        // so run from the project root, where wrangler cannot load and reject
        // an incomplete config that has nothing to do with this call.
        .current_dir(project_root)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    // The race-loser's path, not the common one: `queue_exists` has already
    // answered for a queue that was simply there. Someone else creating it
    // between that check and this call lands here, so the "already exists"
    // complaint is still read as the success it describes — on both streams,
    // wrangler being inconsistent about which carries one.
    //
    // This is the driver's one claim about another tool's prose, and it cannot
    // be pinned by a test (ADR 0194 D2). It is deliberately reached only by a
    // concurrent deploy, so being wrong about it costs a spurious failure on a
    // rare race that a re-run fixes — not, as it would on the create-every-time
    // shape, every re-deploy of every queue project.
    let said = wrangler_said(&output);
    if queue_already_exists(&said) {
        return Ok(());
    }
    Err(said)
}

/// Create the R2 bucket `name`, in `jurisdiction` when given, or accept that it
/// is already there (P5, #2323): as for a queue, Cloudflare addresses a bucket
/// by the name the config binds, so a created bucket and an existing one are
/// the same end state. Run from the project root, as KV and queues are, so
/// wrangler cannot load and reject an incomplete config.
fn create_r2_bucket(
    provenance: &Provenance,
    name: &str,
    jurisdiction: Option<&str>,
    project_root: &Path,
) -> Result<(), String> {
    let Some(mut command) = workers::wrangler_command(provenance, "r2") else {
        return Err("wrangler not found".into());
    };
    command.arg("bucket").arg("create").arg(name);
    if let Some(jurisdiction) = jurisdiction {
        command.arg("--jurisdiction").arg(jurisdiction);
    }
    let output = command
        .current_dir(project_root)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let said = wrangler_said(&output);
    if r2_bucket_already_exists(&said) {
        return Ok(());
    }
    Err(said)
}

/// Does this `wrangler r2 bucket create` failure just mean the bucket is
/// already there (and ours)? Cloudflare answers "The bucket you tried to
/// create already exists, and you own it." (code 10004); a bucket another
/// account owns says it is "not available" instead, and stays a failure.
fn r2_bucket_already_exists(said: &str) -> bool {
    let said = said.to_ascii_lowercase();
    said.contains("already exists") && said.contains("you own it")
}

/// Is `name` already on the account?
///
/// `wrangler queues info <name>` is a lookup by the same name the config binds,
/// and it answers with an **exit code** rather than prose — which is the whole
/// reason to ask it. Cloudflare's own deploy path reconciles queues this way
/// (`getQueue` in wrangler's queues client), rather than by creating and reading
/// the complaint.
///
/// Asking the account, not the ledger: the ledger's queue set is a planning aid,
/// and trusting it here would leave a queue deleted out-of-band un-recreated
/// (ADR 0194 D2).
///
/// A non-zero exit is read as "not there — try to create it". That is also what
/// an auth or network failure produces, and the honest consequence is the right
/// one: the create then fails too and surfaces wrangler's real complaint, rather
/// than this call inventing a diagnosis of its own.
fn queue_exists(provenance: &Provenance, name: &str, project_root: &Path) -> bool {
    let Some(mut command) = workers::wrangler_command(provenance, "queues") else {
        return false;
    };
    command
        .arg("info")
        .arg(name)
        // As for the create: run from the project root, where wrangler cannot
        // load and reject a config still carrying the KV placeholder.
        .current_dir(project_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Everything a failed wrangler call said, both streams, never empty — a
/// non-zero exit with nothing to say must still read as something rather than
/// as a dangling colon.
fn wrangler_said(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let said: Vec<&str> = [stderr.trim(), stdout.trim()]
        .into_iter()
        .filter(|stream| !stream.is_empty())
        .collect();
    if said.is_empty() {
        return format!("wrangler exited {}", exit_status_byte(&output.status));
    }
    said.join("\n")
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
fn unattempted_queues(declared: &Resources, attempted: &mut BTreeSet<String>) -> Vec<String> {
    declared
        .queues
        .iter()
        .filter(|queue| attempted.insert((*queue).to_string()))
        .cloned()
        .collect()
}

/// Does this `wrangler queues create` failure just mean the queue is already
/// there?
///
/// Matching wrangler's message is the only seam available — it has no
/// `--if-not-exists`, and the create is the *only* call that would tell us
/// (checking with `queues list` first would be the same race, one call later).
/// The failure mode is benign and visible: an unrecognised wording surfaces as
/// a plain deploy failure carrying wrangler's own text, never as a silent
/// mis-provision. Pure, so the rule is tested without an account.
fn queue_already_exists(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("already exists")
}

/// Delete a KV namespace by id — `--prune`'s one KV-side mutation.
///
/// `--skip-confirmation`: wrangler's own prompt is redundant with
/// [`confirm_prune`], which already named every resource about to go and
/// asked once for the whole batch: a second, per-resource prompt would be
/// noise, not safety. Idempotent against a namespace already gone (matched
/// via [`kv_namespace_already_deleted`]) — confirmed empirically (#839):
/// Cloudflare answers a not-found delete with an error, never silent success,
/// so this driver-side match is what makes re-running `--prune` safe.
pub(crate) fn delete_kv_namespace(
    provenance: &Provenance,
    id: &str,
    project_root: &Path,
) -> Result<(), String> {
    let Some(mut command) = workers::wrangler_command(provenance, "kv") else {
        return Err("wrangler not found".into());
    };
    let output = command
        .arg("namespace")
        .arg("delete")
        .arg("--namespace-id")
        .arg(id)
        .arg("--skip-confirmation")
        .current_dir(project_root)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let said = wrangler_said(&output);
    if kv_namespace_already_deleted(&said) {
        return Ok(());
    }
    Err(said)
}

/// Does this `wrangler kv namespace delete` failure just mean the namespace
/// is already gone? Confirmed empirically (#839) against a real account:
/// `namespace not found [code: 10013]`. Matching the stable error code rather
/// than the prose, unlike [`queue_already_exists`]'s text match — Cloudflare
/// structures its API errors with a `code`, and `10013` is the more durable
/// half of the message if the wording is ever revised. Pure, so the rule is
/// tested without an account.
fn kv_namespace_already_deleted(stderr: &str) -> bool {
    stderr.contains("code: 10013") || stderr.to_ascii_lowercase().contains("namespace not found")
}

/// Delete a queue by name — `--prune`'s other mutation.
///
/// No confirmation flag to pass: confirmed empirically (#839) that
/// `wrangler queues delete` has no prompt or force flag at all (`--help`
/// shows only global flags) — it is already non-interactive by default.
/// Idempotent against a queue already gone, matched the same way
/// [`queue_already_exists`] matches the create side's inverse case.
pub(crate) fn delete_queue(
    provenance: &Provenance,
    name: &str,
    project_root: &Path,
) -> Result<(), String> {
    let Some(mut command) = workers::wrangler_command(provenance, "queues") else {
        return Err("wrangler not found".into());
    };
    let output = command
        .arg("delete")
        .arg(name)
        .current_dir(project_root)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let said = wrangler_said(&output);
    if queue_already_deleted(&said) {
        return Ok(());
    }
    Err(said)
}

/// Does this `wrangler queues delete` failure just mean the queue is already
/// gone? Confirmed empirically (#839): `Queue "<name>" does not exist. To
/// create it, run: wrangler queues create <name>`. Pure, so the rule is
/// tested without an account.
fn queue_already_deleted(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("does not exist")
}

/// The names already set on this Worker, from `wrangler secret list`.
///
/// `None` means "could not ask" — see [`secret_action`]. Unlike `secret put`,
/// `secret list` has no draft-Worker path: it simply fails for a Worker that
/// does not exist, which is exactly the answer we want on a first deploy.
pub(crate) fn list_secrets(
    provenance: &Provenance,
    worker_dir: &Path,
    environment: &str,
) -> Option<BTreeSet<String>> {
    let mut command = workers::wrangler_command(provenance, "secret")?;
    command
        .arg("list")
        .arg("--format")
        .arg("json")
        // In the worker directory: `secret list` reads the Worker's name from
        // the config beside it, and any KV id is materialised by now, so
        // wrangler can load a complete config.
        .current_dir(worker_dir);
    if environment != "default" {
        command.arg("--env").arg(environment);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_secret_list(&String::from_utf8_lossy(&output.stdout))
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
    provenance: &Provenance,
    worker_dir: &Path,
    name: &str,
    value: &str,
    environment: &str,
) -> Result<(), String> {
    let Some(mut command) = workers::wrangler_command(provenance, "secret") else {
        return Err("wrangler not found".into());
    };
    command.arg("put").arg(name).current_dir(worker_dir);
    if environment != "default" {
        command.arg("--env").arg(environment);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    {
        // Taken and dropped inside the block: wrangler reads to EOF, so the
        // pipe must close before `wait_with_output`, or both ends block.
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "could not open a pipe for the secret value".to_string())?;
        // No trailing newline: wrangler trims trailing whitespace from what it
        // reads, so one would be dropped anyway — sending exactly the value
        // keeps this end honest about what it sent.
        stdin
            .write_all(value.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    Err(wrangler_said(&output))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // ---- #601 slice 2: what `wrangler deploy`'s status makes us exit with ----
    //
    // Unix-only: `ExitStatus` is only constructible from a raw wait status
    // there, and the raw encoding (`code << 8`, or a bare signal number) is a
    // Unix concept. The rule itself is platform-independent.

    #[cfg(unix)]
    fn status(raw: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(raw)
    }

    #[cfg(unix)]
    #[test]
    fn a_clean_push_is_ok() {
        assert_eq!(
            wrangler_outcome("default", "api", &status(0)),
            Ok(Pushed::Ok)
        );
    }

    #[cfg(unix)]
    #[test]
    fn wranglers_exit_code_is_propagated_rather_than_flattened() {
        // Slice 0 exited with `exit_status_byte(&status)`; the multi-context
        // loop must not lose that. A CI job reads the code, so a wrangler exit
        // 2 must not surface as a generic 1.
        let Err(failure) = wrangler_outcome("default", "api", &status(2 << 8)) else {
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
    #[cfg(unix)]
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
            let Err(failure) = wrangler_outcome(environment, "commerce-orders", &status(2 << 8))
            else {
                panic!("a non-zero wrangler exit is a failure");
            };
            assert_eq!(failure.message, expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_shared_ctrl_c_is_a_clean_stop_not_a_failure() {
        // SIGINT reaches us too (shared foreground process group), which is why
        // `exit_status_byte` maps it to 0. Reporting it as a failure would print
        // "wrangler deploy failed … (exit 0)" — an error whose own code says
        // success — and slice 0 exited 0 here.
        assert_eq!(
            wrangler_outcome("default", "api", &status(2)),
            Ok(Pushed::Interrupted)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_real_signal_death_is_a_failure_at_128_plus_the_signal() {
        // Not every signal is a clean stop: a SIGSEGV or the OOM killer's
        // SIGKILL is a genuine failure, and `exit_status_byte` says `128 + sig`.
        let Err(segv) = wrangler_outcome("default", "api", &status(11)) else {
            panic!("a SIGSEGV is a failure, not a clean stop");
        };
        assert_eq!(segv.code, 139);
        let Err(kill) = wrangler_outcome("default", "api", &status(9)) else {
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

    #[test]
    fn parses_wrangler_namespace_json() {
        assert_eq!(
            parse_kv_id(r#"{"id":"abc","title":"api"}"#),
            Some("abc".into())
        );
    }

    #[test]
    fn should_trust_recorded_kv_only_when_the_id_is_live_or_unchecked() {
        let live = BTreeSet::from(["kv-abc".to_string()]);
        assert!(
            should_trust_recorded_kv(Some("kv-abc"), Some(&live)),
            "recorded and present in the live set"
        );
        assert!(
            !should_trust_recorded_kv(Some("kv-deleted"), Some(&live)),
            "recorded but Cloudflare no longer has it — treat as unrecorded"
        );
        assert!(
            should_trust_recorded_kv(Some("kv-abc"), None),
            "the fetch never ran or failed — trust the ledger, as pre-slice-5"
        );
        assert!(
            !should_trust_recorded_kv(None, Some(&live)),
            "nothing recorded at all"
        );
        assert!(!should_trust_recorded_kv(None, None));
    }

    #[test]
    fn kv_namespace_already_deleted_matches_the_confirmed_error_shape() {
        // Confirmed empirically against a real account (#839): `namespace not
        // found [code: 10013]`. The code is matched, not just the prose — the
        // more durable half if Cloudflare ever reword the message.
        assert!(kv_namespace_already_deleted(
            "✘ [ERROR] A request to the Cloudflare API (...) failed.\n  namespace not found [code: 10013]"
        ));
        assert!(kv_namespace_already_deleted("code: 10013"));
        assert!(!kv_namespace_already_deleted(
            "✘ [ERROR] Authentication error [10000]"
        ));
        assert!(!kv_namespace_already_deleted(""));
    }

    #[test]
    fn queue_already_deleted_matches_the_confirmed_error_shape() {
        // Confirmed empirically (#839): `Queue "<name>" does not exist. To
        // create it, run: wrangler queues create <name>`.
        assert!(queue_already_deleted(
            "✘ [ERROR] Queue \"jobs\" does not exist. To create it, run: wrangler queues create jobs"
        ));
        assert!(!queue_already_deleted(
            "✘ [ERROR] A queue with this name already exists"
        ));
        assert!(!queue_already_deleted(""));
    }

    #[test]
    fn confirm_prune_short_circuits_on_yes_without_touching_stdin() {
        // The one path of confirm_prune testable without a terminal: --yes
        // must return true immediately, exactly as `confirm` does.
        assert!(confirm_prune(true, &Orphans::default(), "default"));
    }

    #[test]
    fn an_existing_queue_is_success_not_a_failure() {
        // D2: `wrangler queues create` has no `--if-not-exists` (verified
        // against wrangler 4.103's `queues create --help`), so a create that
        // loses the race against a concurrent deploy reads its "already there"
        // complaint as the success it describes.
        //
        // This match is the driver's one unpinnable claim about wrangler's
        // prose: the wording is Cloudflare's API text, which wrangler renders
        // verbatim as `{message} [code: {code}]` and has no queue-specific
        // handling for. `queue_exists` is what keeps it off the common path.
        assert!(queue_already_exists(
            "✘ [ERROR] A queue with this name already exists"
        ));
        assert!(
            queue_already_exists("queue already exists"),
            "the match is on the phrase, whatever wrangler wraps it in"
        );
        // Anything else is a real failure and must surface with wrangler's own
        // words rather than being swallowed as idempotency.
        assert!(!queue_already_exists(
            "✘ [ERROR] Authentication error [10000]"
        ));
        assert!(!queue_already_exists(
            "✘ [ERROR] A request to the Cloudflare API failed."
        ));
        assert!(!queue_already_exists(""));
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

    #[cfg(unix)]
    #[test]
    fn a_wrangler_complaint_is_read_from_whichever_stream_carries_it() {
        let said = |stdout: &str, stderr: &str| {
            wrangler_said(&std::process::Output {
                status: status(1 << 8),
                stdout: stdout.as_bytes().to_vec(),
                stderr: stderr.as_bytes().to_vec(),
            })
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
        // The consequence that makes this worth doing: a create whose
        // "already exists" lands on stdout must still be read as success, or
        // every re-deploy of a queue project fails.
        assert!(queue_already_exists(&said(
            "A queue with this name already exists",
            ""
        )));
    }

    #[test]
    fn the_queue_ledger_records_once_and_reports_whether_it_changed() {
        // The provision step writes the ledger only when the set actually
        // gained a name, so a re-run that provisions nothing writes nothing.
        let mut lock = DeployLock::default();
        assert!(lock.record_queue("default", "intake"), "the first is new");
        assert!(
            !lock.record_queue("default", "intake"),
            "the second changes nothing, so the ledger must not be rewritten"
        );
    }

    // ---- #2037: the bare namespace's title on the provisioning command ----
    //
    // `deploy_one` against a stand-in `wrangler` that records its arguments
    // and answers `kv namespace create` with an id. Nothing reaches
    // Cloudflare.

    /// Write an executable stand-in `wrangler` at `path`.
    ///
    /// By a child `sh`, not by this process: a test thread that forks while
    /// this process holds the script open for writing carries that descriptor
    /// into its child until exec, and running the script in that window fails
    /// with ETXTBSY ("Text file busy"). Seen once the #2055 tests ran beside
    /// these in parallel.
    #[cfg(unix)]
    fn write_stand_in(path: &Path, script: &str) {
        let wrote = std::process::Command::new("sh")
            .arg("-c")
            .arg("printf '%s' \"$1\" > \"$2\" && chmod 755 \"$2\"")
            .arg("sh")
            .arg(script)
            .arg(path)
            .status()
            .unwrap();
        assert!(wrote.success(), "the stand-in wrangler is written");
    }

    /// What `deploy_one` ran, one line per `wrangler` invocation, and the
    /// ledger it left, for one Worker `api` that binds only the bare `KV`.
    #[cfg(unix)]
    fn provision_bare_kv(environment: &str, lock: DeployLock) -> (Vec<String>, DeployLock) {
        let (calls, lock, pushed) =
            provision_api(environment, Resources::default().needs_kv(), lock, None);
        assert_eq!(pushed, Ok(Pushed::Ok));
        (calls, lock)
    }

    /// `deploy_one` for Worker `api` declaring `declared` (the whole project),
    /// with the account's live KV ids `live_kv_ids`: the `wrangler`
    /// invocations, the ledger it left, and its outcome.
    #[cfg(unix)]
    fn provision_api(
        environment: &str,
        declared: Resources,
        lock: DeployLock,
        live_kv_ids: Option<BTreeSet<String>>,
    ) -> (Vec<String>, DeployLock, Result<Pushed, DeployFailure>) {
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
        std::fs::write(workers_dir.join("api/wrangler.toml"), config).unwrap();
        let log = root.join("calls.log");
        let fake = root.join("wrangler");
        write_stand_in(
            &fake,
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\nif [ \"$1\" = kv ]; then echo '{{\"id\":\"minted-id\",\"title\":\"t\"}}'; fi\nexit 0\n",
                log.display()
            ),
        );

        let mut lock = lock;
        let source = SecretSource::default();
        let edge_keys = BTreeMap::new();
        let foreign_keys = BTreeMap::new();
        let foreign_secrets = BTreeSet::new();
        let listed = BTreeMap::from([("api".to_string(), Some(BTreeSet::new()))]);
        let mut resolved = BTreeMap::new();
        let project = BTreeMap::from([("api".to_string(), declared)]);
        let pushed = deploy_one(
            &Provenance::Path(fake),
            &root,
            &workers_dir,
            "api",
            &project["api"],
            &project,
            &mut lock,
            &root.join(LOCK_FILE),
            &mut BTreeSet::new(),
            &mut Secrets {
                source: &source,
                force: false,
                edge_keys: &edge_keys,
                foreign_keys: &foreign_keys,
                foreign_secrets: &foreign_secrets,
                listed: &listed,
                resolved: &mut resolved,
                with_upload: true,
            },
            &[],
            environment,
            live_kv_ids.as_ref(),
        );
        let calls = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        let _ = std::fs::remove_dir_all(root);
        (calls, lock, pushed)
    }

    // ---- P5 (#2323): R2 buckets are created by name and recorded ----

    #[cfg(unix)]
    fn api_with_buckets() -> Resources {
        Resources::default().r2_buckets(&[
            ("R2_ARCHIVES", "api-archives", None),
            ("R2_INDEXES", "api-indexes", Some("eu")),
        ])
    }

    /// The `r2` invocations among `calls`, in order.
    #[cfg(unix)]
    fn r2_calls(calls: &[String]) -> Vec<&str> {
        calls
            .iter()
            .map(String::as_str)
            .filter(|c| c.starts_with("r2 "))
            .collect()
    }

    #[cfg(unix)]
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
        let archives = recorded_r2(&lock, "api", "R2_ARCHIVES", "default").unwrap();
        assert_eq!(archives.name, "api-archives");
        assert_eq!(archives.jurisdiction, None);
        let indexes = recorded_r2(&lock, "api", "R2_INDEXES", "default").unwrap();
        assert_eq!(indexes.name, "api-indexes");
        assert_eq!(indexes.jurisdiction.as_deref(), Some("eu"));
    }

    #[cfg(unix)]
    #[test]
    fn a_recorded_r2_bucket_is_not_created_again() {
        let (_, lock, _) =
            provision_api("default", api_with_buckets(), DeployLock::default(), None);
        let (calls, _, pushed) = provision_api("default", api_with_buckets(), lock, None);
        assert_eq!(pushed, Ok(Pushed::Ok));
        assert_eq!(r2_calls(&calls), Vec::<&str>::new());
    }

    #[cfg(unix)]
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
            recorded_r2(&lock, "api", "R2_ARCHIVES", "staging")
                .unwrap()
                .name,
            "api-archives-staging"
        );
        assert!(recorded_r2(&lock, "api", "R2_ARCHIVES", "default").is_none());
    }

    #[cfg(unix)]
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
            recorded_r2(&lock, "api", "R2_ARCHIVES", "default")
                .unwrap()
                .jurisdiction
                .as_deref(),
            Some("fedramp")
        );
        // The bucket it replaces is still recorded, for the plan to report.
        let kept = &recorded_r2(&lock, "api", "R2_ARCHIVES", "default")
            .unwrap()
            .superseded;
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].jurisdiction, None);
    }

    #[test]
    fn an_existing_bucket_you_own_is_success_and_anothers_is_not() {
        assert!(r2_bucket_already_exists(
            "The bucket you tried to create already exists, and you own it. [code: 10004]"
        ));
        assert!(!r2_bucket_already_exists(
            "The requested bucket name is not available. [code: 10006]"
        ));
        assert!(!r2_bucket_already_exists("authentication error"));
    }

    // ---- #2059: a namespace created again re-runs the cross-environment
    // title check ----
    //
    // Worker `api` with the bare `Kv` and `capability Staging`. The default
    // environment records `api`'s `KV_STAGING`, titled `api-staging`, and
    // staging records the bare `KV`, also `api-staging` under `--env staging`.
    // The plan passes: staging's bare id is recorded, so nothing is created.

    #[cfg(unix)]
    fn api_with_staging_instance() -> (Resources, DeployLock) {
        let declared = Resources::default()
            .needs_kv()
            .kv_instances(&["KV_STAGING"]);
        let mut lock = DeployLock::default();
        lock.record_kv("default", "api", "KV", "default-bare-id");
        lock.record_kv("default", "api", "KV_STAGING", "default-staging-id");
        lock.record_kv("staging", "api", "KV", "staging-bare-id");
        (declared, lock)
    }

    /// Staging's bare id has gone from the account, so `deploy_one` would
    /// create it again as `api-staging`, the title the default environment's
    /// instance holds. Refused before any `kv namespace create` or push.
    #[cfg(unix)]
    #[test]
    fn a_namespace_created_again_under_a_title_another_environment_holds_is_refused() {
        let (declared, lock) = api_with_staging_instance();
        let plan_time = BTreeMap::from([("api".to_string(), api_with_staging_instance().0)]);
        assert_eq!(
            crate::deploy::plan::kv_title_collision(&plan_time, &lock, "staging"),
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
            recorded_kv_binding(&lock, "api", "KV", "staging"),
            Some("staging-bare-id"),
            "the ledger is untouched"
        );
    }

    /// The same project with staging's bare id still live: nothing is created
    /// for it, the instance's distinct `api-staging-staging` is created, and
    /// the push goes ahead.
    #[cfg(unix)]
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
                "kv namespace create api-staging-staging --json",
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
    #[cfg(unix)]
    #[test]
    fn a_gone_namespace_in_another_environment_holds_no_title() {
        let declared = || Resources::default().kv_instances(&["KV_STAGING"]);
        let lock = || {
            let mut lock = DeployLock::default();
            lock.record_kv("default", "api", "KV_STAGING", "gone-default-id");
            lock.record_kv("staging", "api", "KV", "orphan-id");
            lock
        };
        // The plan passes: `KV_STAGING` is recorded, so it creates nothing.
        let plan_time = BTreeMap::from([("api".to_string(), declared())]);
        assert_eq!(
            crate::deploy::plan::kv_title_collision(&plan_time, &lock(), "default"),
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
        assert_eq!(calls, ["kv namespace create api-staging --json", "deploy"]);
        assert_eq!(
            recorded_kv_binding(&lock, "api", "KV_STAGING", "default"),
            Some("minted-id")
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_default_environment_creates_the_bare_namespace_under_the_workers_name() {
        let (calls, lock) = provision_bare_kv("default", DeployLock::default());
        assert_eq!(calls, ["kv namespace create api --json", "deploy"]);
        assert_eq!(
            recorded_kv_binding(&lock, "api", "KV", "default"),
            Some("minted-id")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_named_environment_creates_the_bare_namespace_under_a_qualified_title() {
        // An account holds one namespace per title, so `api` would collide
        // with the default environment's.
        let (calls, lock) = provision_bare_kv("staging", DeployLock::default());
        assert_eq!(
            calls,
            [
                "kv namespace create api-staging --json",
                "deploy --env staging"
            ]
        );
        assert_eq!(
            recorded_kv_binding(&lock, "api", "KV", "staging"),
            Some("minted-id")
        );
        assert_eq!(recorded_kv_binding(&lock, "api", "KV", "default"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_recorded_staging_namespace_is_reused_not_recreated() {
        // Whatever it was titled when made, a namespace the ledger records for
        // the environment is the one the deploy binds.
        let mut lock = DeployLock::default();
        lock.record_kv("staging", "api", "KV", "staging-id");
        let (calls, lock) = provision_bare_kv("staging", lock);
        assert_eq!(calls, ["deploy --env staging"]);
        assert_eq!(
            recorded_kv_binding(&lock, "api", "KV", "staging"),
            Some("staging-id")
        );
    }

    // ---- #2055: a new Worker's first version carries its secrets ----
    //
    // `deploy_one` against a stand-in `wrangler` that records its arguments,
    // drains a `secret put`'s stdin, and notes whether a `--secrets-file` it
    // is handed exists and names the secret. Nothing reaches Cloudflare.

    const BINDING: &str = "LEDGER_CALL";
    const PLANTED: &str = "planted-2055";

    /// What one `deploy_one` of a Worker `api` with one secret to set left
    /// behind.
    #[cfg(unix)]
    struct BindingRun {
        /// One line per `wrangler` invocation.
        calls: Vec<String>,
        /// What the stand-in saw of the `--secrets-file`, one line per upload.
        seen: Vec<String>,
        pushed: Result<Pushed, DeployFailure>,
        /// Whether the run left a `bynk-secrets-*` directory in the project.
        left_in_project: bool,
    }

    #[cfg(unix)]
    fn deploy_with_binding(
        environment: &str,
        already_deployed: bool,
        with_upload: bool,
        deploy_tail: &str,
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
        let seen = root.join("seen.log");
        let fake = root.join("wrangler");
        // A `--secrets-file` is checked while the upload runs: it must exist
        // then, and hold the name. Its value is never echoed.
        let script = format!(
            "#!/bin/sh\n\
                 echo \"$*\" >> '{log}'\n\
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
        write_stand_in(&fake, &script);

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
        let pushed = deploy_one(
            &Provenance::Path(fake),
            &root,
            &workers_dir,
            "api",
            &project["api"],
            &project,
            &mut lock,
            &root.join(LOCK_FILE),
            &mut BTreeSet::new(),
            &mut Secrets {
                source: &source,
                force: false,
                edge_keys: &edge_keys,
                foreign_keys: &foreign_keys,
                foreign_secrets: &foreign_secrets,
                listed: &listed,
                resolved: &mut resolved,
                with_upload,
            },
            &[],
            environment,
            None,
        );
        let read = |path: &Path| -> Vec<String> {
            std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
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
        let run = BindingRun {
            calls: read(&log),
            seen: read(&seen),
            pushed,
            left_in_project,
        };
        let _ = std::fs::remove_dir_all(root);
        run
    }

    #[cfg(unix)]
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
    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn a_first_deploy_uploads_its_secrets_with_the_worker() {
        let run = deploy_with_binding("default", false, true, "exit 0");
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

    #[cfg(unix)]
    #[test]
    fn a_failed_first_upload_still_removes_the_secrets_file() {
        let run = deploy_with_binding("default", false, true, "exit 1");
        assert!(run.pushed.is_err(), "wrangler's failure is the deploy's");
        let file = secrets_file_of(&run.calls, &[]);
        assert!(
            run.seen == ["present"],
            "the file existed during the upload"
        );
        assert!(!file.exists(), "removed although the upload failed");
        assert!(!file.parent().unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_first_deploy_under_an_environment_uploads_to_the_suffixed_worker() {
        // `--env staging` is what makes wrangler upload `api-staging`, the
        // secrets with it.
        let run = deploy_with_binding("staging", false, true, "exit 0");
        assert!(run.pushed == Ok(Pushed::Ok), "the push succeeds");
        let file = secrets_file_of(&run.calls, &["--env", "staging"]);
        assert!(
            run.seen == ["present"],
            "the file existed during the upload"
        );
        assert!(!file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_sigterm_during_the_upload_waits_for_wrangler_and_still_removes_the_file() {
        // What a CI cancel does to `bynk` alone, mid-upload. Without the
        // deferral this would end the test binary with the file on disk.
        let run = deploy_with_binding(
            "default",
            false,
            true,
            "kill -TERM $PPID; sleep 0.2; exit 0",
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

    #[cfg(unix)]
    #[test]
    fn a_sigterm_to_bynk_is_passed_on_to_the_upload() {
        // Sent to `bynk` alone, a SIGTERM would otherwise leave wrangler
        // running and `bynk` waiting on it.
        let started = std::time::Instant::now();
        let run = deploy_with_binding("default", false, true, "kill -TERM $PPID; exec sleep 30");
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

    #[cfg(unix)]
    #[test]
    fn a_redeploy_sets_its_secrets_before_the_push_on_stdin() {
        // The Worker exists, so its secrets go first, as their own version,
        // and never through a file.
        let run = deploy_with_binding("default", true, true, "exit 0");
        assert!(run.pushed == Ok(Pushed::Ok), "the push succeeds");
        assert!(
            run.calls == [format!("secret put {BINDING}"), "deploy".into()],
            "`secret put`, then `deploy`"
        );
        assert!(run.seen.is_empty(), "no secrets file");
        let run = deploy_with_binding("staging", true, true, "exit 0");
        assert!(
            run.calls
                == [
                    format!("secret put {BINDING} --env staging"),
                    "deploy --env staging".into()
                ],
            "both under `--env staging`"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_wrangler_without_the_secrets_file_falls_back_to_setting_after_the_push() {
        let run = deploy_with_binding("default", false, false, "exit 0");
        assert!(run.pushed == Ok(Pushed::Ok), "the push succeeds");
        assert!(
            run.calls == ["deploy".to_string(), format!("secret put {BINDING}")],
            "`deploy`, then `secret put`"
        );
        // And a failed push sets nothing on a Worker that may not exist.
        let run = deploy_with_binding("default", false, false, "exit 1");
        assert!(run.pushed.is_err(), "the push fails");
        assert!(run.calls == ["deploy"], "and nothing is set after it");
    }

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
