//! [`Wrangler`]: each account operation as a wrangler process (#4043, Decision A).
//!
//! The command comes from [`bynk_driver::tools::wrangler::command`], the one place a wrangler
//! spawn is built. The resource operations run from the project root, where wrangler cannot load
//! and reject a generated config still carrying a placeholder (the KV id before the namespace
//! exists); `d1 migrations apply` runs from the Worker's directory, where its config names the
//! migrations directory.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;

use crate::probe::Provenance;

use super::api::{CloudflareApi, Op, PushCall, PushRun, VectorizeOp};
use super::error::{Exit, Failure, WranglerError};

/// The process adapter: wrangler as `provenance` resolves it, run from `project_root`.
pub(crate) struct Wrangler<'a> {
    provenance: &'a Provenance,
    project_root: &'a Path,
    /// Tests only: run `sh -c <script> sh <argv…>` instead of wrangler, so a test of the
    /// process path (signals, stdin, the tee) needs no stand-in written to disk.
    #[cfg(test)]
    script: Option<String>,
}

/// The test-only `sh -c` behind [`Wrangler::scripted`], in a test module so that
/// `tool_spawn_sites` reads it as the test code it is.
#[cfg(test)]
mod scripting {
    use std::process::Command;

    /// `sh -c <script> sh <argv…>`, so `"$*"` is the call's arguments.
    pub(super) fn command(script: &str, argv: &[String]) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script, "sh"]).args(argv);
        command
    }
}

impl<'a> Wrangler<'a> {
    pub(crate) fn new(provenance: &'a Provenance, project_root: &'a Path) -> Self {
        Wrangler {
            provenance,
            project_root,
            #[cfg(test)]
            script: None,
        }
    }

    /// The adapter over a shell script held in memory: each call runs `sh -c <script> sh` with
    /// the call's arguments, so `"$*"` reads as an on-disk stand-in's did. Unix only, as `sh` is.
    #[cfg(test)]
    pub(crate) fn scripted(script: &str, project_root: &'a Path) -> Self {
        static NONE: Provenance = Provenance::Missing;
        Wrangler {
            provenance: &NONE,
            project_root,
            script: Some(script.to_string()),
        }
    }

    /// The program for `argv`: wrangler as resolved, or a test's script.
    fn base(&self, op: &Op<'_>, argv: &[String]) -> Result<Command, WranglerError> {
        let (subcommand, rest) = argv
            .split_first()
            .expect("an operation names its subcommand");
        #[cfg(test)]
        if let Some(script) = &self.script {
            return Ok(scripting::command(script, argv));
        }
        let mut command = bynk_driver::tools::wrangler::command(self.provenance, subcommand)
            .ok_or_else(|| WranglerError::missing(op))?;
        command.args(rest);
        Ok(command)
    }

    fn command(&self, op: &Op<'_>, dir: &Path) -> Result<Command, WranglerError> {
        let mut command = self.base(op, &op.argv())?;
        command.current_dir(dir);
        Ok(command)
    }

    /// [`command`](Self::command) run from wherever `bynk` was started.
    fn command_anywhere(&self, op: &Op<'_>) -> Result<Command, WranglerError> {
        self.base(op, &op.argv())
    }

    /// Run `op` from `dir`, capturing both streams: its stdout on success.
    fn run(&self, op: &Op<'_>, dir: &Path, close_stdin: bool) -> Result<String, WranglerError> {
        let mut command = self.command(op, dir)?;
        if close_stdin {
            command.stdin(Stdio::null());
        }
        let output = command
            .output()
            .map_err(|e| WranglerError::new(op, Failure::Start(e)))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return Ok(stdout);
        }
        Err(WranglerError::exit(
            op,
            Exit::of(&output.status),
            &stdout,
            &String::from_utf8_lossy(&output.stderr),
        ))
    }

    fn run_from_root(&self, op: Op<'_>) -> Result<String, WranglerError> {
        self.run(&op, self.project_root, false)
    }

    /// Run `op` from `dir` with both streams discarded: answered by its exit code alone.
    fn status(&self, op: &Op<'_>, dir: &Path) -> Result<(), WranglerError> {
        let status = self
            .command(op, dir)?
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| WranglerError::new(op, Failure::Start(e)))?;
        if status.success() {
            Ok(())
        } else {
            Err(WranglerError::exited(op, &status, "", ""))
        }
    }

    /// Run `command` (spawned for `op`) with `stdin` on its stdin and both streams captured.
    /// `no_pipe` is the text of a stdin that could not be opened.
    fn feed(
        op: &Op<'_>,
        mut command: Command,
        stdin: &str,
        no_pipe: &'static str,
    ) -> Result<(), WranglerError> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| WranglerError::new(op, Failure::Start(e)))?;
        let mut unread: Option<std::io::Error> = None;
        {
            // Taken and dropped inside the block: wrangler reads to EOF, so the pipe must close
            // before `wait_with_output`, or both ends block.
            let mut pipe = child
                .stdin
                .take()
                .ok_or_else(|| WranglerError::new(op, Failure::Pipe(no_pipe)))?;
            // #4103: wrangler may exit without reading stdin (it fails early on auth, config or a
            // bad Worker name), which closes the pipe under the write. That is wrangler's
            // failure to report, not an I/O error of ours: carry on to its exit and streams.
            if let Err(e) = pipe.write_all(stdin.as_bytes()) {
                if e.kind() != std::io::ErrorKind::BrokenPipe {
                    return Err(WranglerError::new(op, Failure::Io(e)));
                }
                unread = Some(e);
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|e| WranglerError::new(op, Failure::Wait(e)))?;
        if output.status.success() {
            // Exited 0 without reading the whole value: it may not have been set.
            return unread.map_or(Ok(()), |e| Err(WranglerError::new(op, Failure::Io(e))));
        }
        Err(WranglerError::exited(
            op,
            &output.status,
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        ))
    }

    /// Run `command` (spawned for `op`) without letting a signal end `bynk` first, so the caller
    /// can always remove the secrets file it carries (#2055).
    ///
    /// SIGINT and SIGTERM are recorded for as long as wrangler runs ([`crate::stop::defer`]),
    /// then the earlier dispositions are restored. A Ctrl-C has already reached wrangler through
    /// the foreground group, so only a SIGTERM is passed on to it: one sent to `bynk` alone (a
    /// CI step's cancel) would otherwise leave wrangler running and `bynk` waiting on it. The
    /// signal received, if any, is returned with the exit for the caller to act on. SIGKILL
    /// cannot be deferred.
    fn hold_signals(op: &Op<'_>, mut command: Command) -> Result<PushRun, WranglerError> {
        let deferred = crate::stop::defer();
        let mut child = command
            .spawn()
            .map_err(|e| WranglerError::new(op, Failure::Start(e)))?;
        let said = tee_stderr(&mut child);
        #[cfg_attr(not(unix), allow(unused_mut, unused_variables))]
        let mut forwarded = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => return Err(WranglerError::new(op, Failure::Wait(e))),
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
        let said = said.and_then(|t| t.join().ok()).unwrap_or_default();
        Ok(PushRun {
            exit: Exit::of(&status),
            said,
            signalled,
        })
    }
}

/// `wrangler <subcommand>` for the long-running and streamed uses no account operation covers:
/// `bynk dev`'s servers, `bynk tail`, and `bynk dev`'s warm-up (#1957). `None` when wrangler is
/// missing. Their streaming and process-group handling stay with their commands.
pub(crate) fn spawn_command(provenance: &Provenance, subcommand: &str) -> Option<Command> {
    bynk_driver::tools::wrangler::command(provenance, subcommand)
}

/// Copy a child's stderr to ours as it arrives, and keep it, so a watched push shows what
/// wrangler shows and can still be read afterwards. `None` unless the child was spawned with a
/// piped stderr.
pub(crate) fn tee_stderr(child: &mut Child) -> Option<JoinHandle<String>> {
    let stderr = child.stderr.take()?;
    Some(std::thread::spawn(move || {
        let mut kept = String::new();
        let mut out = std::io::stderr();
        let mut reader = BufReader::new(stderr);
        let mut line = Vec::new();
        // Bytes, not `lines()`: a line that is not UTF-8 must not end the copy, which would drop
        // the pipe under wrangler and lose everything after it.
        while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
            let _ = out.write_all(&line);
            kept.push_str(&String::from_utf8_lossy(&line));
            line.clear();
        }
        kept
    }))
}

impl CloudflareApi for Wrangler<'_> {
    fn queue_create(&self, name: &str) -> Result<(), WranglerError> {
        self.run_from_root(Op::QueueCreate { name }).map(drop)
    }

    /// Answered by the exit code alone, both streams discarded: the reason to ask `queues info`
    /// rather than read a create's complaint (ADR 0194 D2).
    fn queue_info(&self, name: &str) -> Result<(), WranglerError> {
        self.status(&Op::QueueInfo { name }, self.project_root)
    }

    fn queue_delete(&self, name: &str) -> Result<(), WranglerError> {
        self.run_from_root(Op::QueueDelete { name }).map(drop)
    }

    fn kv_create(&self, title: &str) -> Result<String, WranglerError> {
        let op = Op::KvCreate { title };
        let stdout = self.run(&op, self.project_root, false)?;
        parse_kv_id(&stdout).ok_or_else(|| {
            WranglerError::new(
                &op,
                Failure::Unreadable("wrangler did not return a namespace id".into()),
            )
        })
    }

    fn kv_list(&self) -> Result<String, WranglerError> {
        self.run_from_root(Op::KvList)
    }

    fn kv_delete(&self, id: &str) -> Result<(), WranglerError> {
        self.run_from_root(Op::KvDelete { id }).map(drop)
    }

    fn d1_migrations_apply(
        &self,
        worker_dir: &Path,
        binding: &str,
        environment: &str,
    ) -> Result<(), WranglerError> {
        let op = Op::D1MigrationsApply {
            binding,
            environment,
        };
        self.run(&op, worker_dir, true).map(drop)
    }

    fn d1_create(&self, name: &str) -> Result<String, WranglerError> {
        let op = Op::D1Create { name };
        let stdout = self.run(&op, self.project_root, false)?;
        parse_d1_id(&stdout).ok_or_else(|| {
            WranglerError::new(
                &op,
                Failure::Unreadable(format!(
                    "`wrangler d1 create` succeeded but printed no `database_id`:\n{stdout}"
                )),
            )
        })
    }

    fn d1_list(&self) -> Result<String, WranglerError> {
        self.run_from_root(Op::D1List)
    }

    fn r2_create(&self, name: &str, jurisdiction: Option<&str>) -> Result<(), WranglerError> {
        self.run_from_root(Op::R2Create { name, jurisdiction })
            .map(drop)
    }

    fn vectorize(&self, op: VectorizeOp<'_>) -> Result<String, WranglerError> {
        self.run_from_root(Op::Vectorize(op))
    }

    fn push(&self, call: PushCall<'_>) -> Result<PushRun, WranglerError> {
        let op = Op::Push {
            args: call.args,
            secrets_file: call.secrets_file,
            environment: call.environment,
        };
        let mut command = self.command(&op, call.worker_dir)?;
        if call.tee {
            command.stderr(Stdio::piped());
        }
        if call.secrets_file.is_some() {
            return Self::hold_signals(&op, command);
        }
        let ran = if call.tee {
            command.spawn().and_then(|mut child| {
                let said = tee_stderr(&mut child);
                let status = child.wait()?;
                Ok((status, said.and_then(|t| t.join().ok()).unwrap_or_default()))
            })
        } else {
            command.status().map(|status| (status, String::new()))
        };
        let (status, said) = ran.map_err(|e| WranglerError::new(&op, Failure::Start(e)))?;
        Ok(PushRun {
            exit: Exit::of(&status),
            said,
            signalled: None,
        })
    }

    fn triggers_deploy(&self, worker_dir: &Path, environment: &str) -> Result<(), WranglerError> {
        let op = Op::TriggersDeploy { environment };
        let status = self
            .command(&op, worker_dir)?
            .status()
            .map_err(|e| WranglerError::new(&op, Failure::Start(e)))?;
        if status.success() {
            Ok(())
        } else {
            Err(WranglerError::exited(&op, &status, "", ""))
        }
    }

    fn whoami(&self) -> Result<(), WranglerError> {
        // Wherever bynk runs: `whoami` reads no config.
        let op = Op::Whoami;
        let status = self
            .command_anywhere(&op)?
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| WranglerError::new(&op, Failure::Start(e)))?;
        if status.success() {
            Ok(())
        } else {
            Err(WranglerError::exited(&op, &status, "", ""))
        }
    }

    fn worker_deployments(&self, name: &str) -> Result<(), WranglerError> {
        self.run_from_root(Op::WorkerDeployments { name }).map(drop)
    }

    fn rollback(&self, script: &str, version: &str) -> Result<(), WranglerError> {
        self.run_from_root(Op::Rollback { script, version })
            .map(drop)
    }

    fn secret_list(&self, worker_dir: &Path, environment: &str) -> Result<String, WranglerError> {
        // In the Worker's directory: `secret list` reads the Worker's name from the config
        // beside it, and any KV id is materialised by now.
        self.run(&Op::SecretList { environment }, worker_dir, false)
    }

    fn secret_put(
        &self,
        worker_dir: &Path,
        name: &str,
        value: &str,
        environment: &str,
    ) -> Result<(), WranglerError> {
        let op = Op::SecretPut { name, environment };
        let command = self.command(&op, worker_dir)?;
        // No trailing newline: wrangler trims trailing whitespace from what it reads, so one
        // would be dropped anyway.
        Self::feed(
            &op,
            command,
            value,
            "could not open a pipe for the secret value",
        )
    }

    fn named_secret(
        &self,
        dir: &Path,
        verb: &str,
        script: &str,
        name: &str,
        stdin: &str,
    ) -> Result<(), WranglerError> {
        let op = Op::NamedSecret { verb, script, name };
        let command = self.command(&op, dir)?;
        std::fs::create_dir_all(dir).map_err(|e| WranglerError::new(&op, Failure::Io(e)))?;
        Self::feed(&op, command, stdin, "could not open wrangler's stdin")
    }

    fn d1_migrations_apply_local(
        &self,
        worker_dir: &Path,
        binding: &str,
        persist: &[String],
    ) -> Result<(), WranglerError> {
        let op = Op::D1MigrationsApplyLocal { binding, persist };
        self.run(&op, worker_dir, true).map(drop)
    }

    fn secrets_store_list_local(
        &self,
        worker_dir: &Path,
        store: &str,
        persist: &[String],
    ) -> Result<String, WranglerError> {
        let op = Op::SecretsStoreListLocal { store, persist };
        self.run(&op, worker_dir, true)
    }
}

/// The namespace id in `wrangler kv namespace create`'s printed binding snippet: `"id": "…"` in a
/// JSON config, `id = "…"` in a TOML one.
pub(crate) fn parse_kv_id(output: &str) -> Option<String> {
    // A whole-output JSON document first (compact or pretty), then the line scan.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(output) {
        let id = value.get("id").or_else(|| {
            value
                .get(bynk_project::wrangler_keys::WranglerKey::KvNamespaces.top_key())?
                .get(0)?
                .get("id")
        });
        if let Some(id) = id.and_then(|id| id.as_str()) {
            return Some(id.to_owned());
        }
    }
    output.lines().find_map(|line| {
        let line = line.trim().trim_start_matches(['{', '[']).trim_start();
        let rest = line
            .strip_prefix("\"id\"")
            .or_else(|| line.strip_prefix("id"))?
            .trim_start();
        let rest = rest
            .strip_prefix([':', '='])?
            .trim_start()
            .strip_prefix('"')?;
        let id = &rest[..rest.find('"')?];
        (!id.is_empty()).then(|| id.to_owned())
    })
}

/// The `database_id` in `wrangler d1 create`'s output: the config snippet it prints, as TOML
/// (`database_id = "…"`) or JSON (`"database_id": "…"`).
pub(crate) fn parse_d1_id(output: &str) -> Option<String> {
    let at = output.find("database_id")?;
    let rest = &output[at + "database_id".len()..];
    let rest = rest.trim_start_matches('"').trim_start();
    let rest = rest.strip_prefix(['=', ':'])?.trim_start();
    let id = rest.strip_prefix('"')?.split('"').next()?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_kv_id_from_the_printed_binding_snippet() {
        let toml = "🌀 Creating namespace with title \"api\"\n✨ Success!\n\
                    [[kv_namespaces]]\nbinding = \"api\"\nid = \"abc\"\n";
        assert_eq!(parse_kv_id(toml), Some("abc".into()));
        let json = "{\n  \"kv_namespaces\": [\n    {\n      \"binding\": \"api\",\n      \"id\": \"abc\"\n    }\n  ]\n}\n";
        assert_eq!(parse_kv_id(json), Some("abc".into()));
        let compact = r#"{"kv_namespaces":[{"binding":"api","id":"abc"}]}"#;
        assert_eq!(parse_kv_id(compact), Some("abc".into()));
        assert_eq!(parse_kv_id("✘ [ERROR] nope"), None);
    }

    #[test]
    fn the_d1_id_is_read_from_either_snippet_format() {
        let toml = "✅ Successfully created DB 'shop-orders' in region WEUR\nCreated your new D1 database.\n\n[[d1_databases]]\nbinding = \"shop_orders\"\ndatabase_name = \"shop-orders\"\ndatabase_id = \"9b3c2a1e-0000-4000-8000-0123456789ab\"\n";
        assert_eq!(
            parse_d1_id(toml).as_deref(),
            Some("9b3c2a1e-0000-4000-8000-0123456789ab")
        );
        let json = "{\n  \"d1_databases\": [\n    {\n      \"binding\": \"DB\",\n      \"database_name\": \"shop-orders\",\n      \"database_id\": \"9b3c2a1e-0000-4000-8000-0123456789ab\"\n    }\n  ]\n}";
        assert_eq!(
            parse_d1_id(json).as_deref(),
            Some("9b3c2a1e-0000-4000-8000-0123456789ab")
        );
        assert_eq!(parse_d1_id("Created your new D1 database."), None);
    }

    /// The tee copies stderr through and keeps it, through a line that is not UTF-8, with the
    /// kept text readable afterwards. Unix: it runs `sh`.
    #[cfg(unix)]
    #[test]
    fn the_tee_keeps_everything_wrangler_says_even_past_invalid_utf8() {
        let mut child = Command::new("sh")
            .args([
                "-c",
                "printf 'first\\n\\377\\n' >&2; echo '[code: 10007]' >&2; exit 1",
            ])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let said = tee_stderr(&mut child).unwrap();
        let status = child.wait().unwrap();
        let said = said.join().unwrap();
        assert!(!status.success());
        assert!(said.starts_with("first\n"), "{said:?}");
        assert!(said.contains("[code: 10007]"), "{said:?}");
    }

    /// A scratch directory and the log a scripted adapter's calls append to.
    #[cfg(unix)]
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bynk-adapter-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Each call runs where it always ran: the resource calls from the project root, the
    /// Worker's calls from its directory. Unix: it runs `sh`.
    #[cfg(unix)]
    #[test]
    fn each_call_runs_from_its_directory_with_its_arguments() {
        let root = scratch("cwd");
        let worker = root.join("workers/api");
        std::fs::create_dir_all(&worker).unwrap();
        let log = root.join("calls.log");
        let script = format!("echo \"$PWD $*\" >> '{}'", log.display());
        let api = Wrangler::scripted(&script, &root);
        api.queue_create("jobs").unwrap();
        api.secret_list(&worker, "staging").unwrap();
        api.d1_migrations_apply(&worker, "DB", "default").unwrap();
        let calls = std::fs::read_to_string(&log).unwrap();
        let root_at = std::fs::canonicalize(&root).unwrap();
        let worker_at = std::fs::canonicalize(&worker).unwrap();
        assert_eq!(
            calls,
            format!(
                "{} queues create jobs\n{} secret list --format json --env staging\n{} d1 migrations apply DB --remote\n",
                root_at.display(),
                worker_at.display(),
                worker_at.display()
            )
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `d1 migrations apply` runs with stdin closed, so wrangler takes its "yes" fallback; a
    /// secret's value goes on stdin and never in argv (ADR 0195 D1). Unix: it runs `sh`.
    #[cfg(unix)]
    #[test]
    fn stdin_is_closed_for_migrations_and_carries_a_secrets_value() {
        let root = scratch("stdin");
        let log = root.join("calls.log");
        let script = format!(
            "if read -r line; then echo \"$* stdin:$line\"; else echo \"$* stdin:closed\"; fi >> '{}'",
            log.display()
        );
        let api = Wrangler::scripted(&script, &root);
        api.d1_migrations_apply(&root, "DB", "default").unwrap();
        api.secret_put(&root, "TOKEN", "s3cret\n", "default")
            .unwrap();
        api.named_secret(&root.join("sweep"), "put", "shop-api", "K", "k3y\n")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "d1 migrations apply DB --remote stdin:closed\n\
             secret put TOKEN stdin:s3cret\n\
             secret put K --name shop-api stdin:k3y\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #4103: a wrangler that exits without reading stdin is reported as wrangler's own failure,
    /// not as the broken pipe under the write. The value is larger than any pipe buffer, so the
    /// write cannot finish before wrangler exits, whatever the timing. Unix: it runs `sh`.
    #[cfg(unix)]
    #[test]
    fn a_wrangler_that_never_reads_stdin_reports_its_own_failure() {
        let root = scratch("unread");
        let big = "x".repeat(1 << 20);
        let failing = Wrangler::scripted("echo 'it went wrong on stderr' >&2; exit 1", &root);
        let put = failing.secret_put(&root, "TOKEN", &big, "default");
        assert_eq!(put.unwrap_err().said(), "it went wrong on stderr");
        let named = failing.named_secret(&root.join("sweep"), "put", "shop-api", "K", &big);
        assert_eq!(named.unwrap_err().said(), "it went wrong on stderr");
        let silent = Wrangler::scripted("exit 3", &root)
            .secret_put(&root, "TOKEN", &big, "default")
            .unwrap_err();
        assert_eq!(silent.said(), "wrangler exited 3");
        // Exiting 0 without reading the value may mean it was not set: still an error.
        let unread = Wrangler::scripted("exit 0", &root)
            .secret_put(&root, "TOKEN", &big, "default")
            .unwrap_err();
        assert!(unread.said().contains("Broken pipe"), "{}", unread.said());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The four renderings of a failed call, from one real failed process. Unix: it runs `sh`.
    #[cfg(unix)]
    #[test]
    fn a_failed_call_renders_as_each_caller_always_rendered_it() {
        let root = scratch("render");
        let api = Wrangler::scripted("echo ' out '; echo ' err ' >&2; exit 2", &root);
        let e = api.rollback("s", "v").unwrap_err();
        assert_eq!(e.said(), "err\nout");
        assert_eq!(e.stderr_only(), "err");
        assert_eq!(e.said_or("silent"), "err\nout");
        assert_eq!(e.stderr_else_stdout(), "err");
        let quiet = Wrangler::scripted("exit 4", &root)
            .rollback("s", "v")
            .unwrap_err();
        assert_eq!(quiet.said(), "wrangler exited 4");
        assert_eq!(
            quiet.said_or("wrangler secret put failed"),
            "wrangler secret put failed"
        );
        let only_out = Wrangler::scripted("echo ' out '; exit 1", &root)
            .rollback("s", "v")
            .unwrap_err();
        assert_eq!(only_out.stderr_else_stdout(), "out");
        assert_eq!(only_out.stderr_only(), "");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A wrangler that cannot be started is its own failure, with the OS's text.
    #[test]
    fn a_wrangler_that_cannot_start_says_why() {
        let absent = Provenance::Path(std::env::temp_dir().join("bynk-no-such-wrangler"));
        let e = Wrangler::new(&absent, Path::new("."))
            .kv_list()
            .unwrap_err();
        assert!(matches!(e.failure(), Failure::Start(_)), "{e:?}");
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn a_missing_wrangler_is_an_error_before_anything_runs() {
        let missing = Provenance::Missing;
        let api = Wrangler::new(&missing, Path::new("."));
        assert_eq!(api.kv_list().unwrap_err().to_string(), "wrangler not found");
        assert_eq!(
            api.queue_info("q").unwrap_err().to_string(),
            "wrangler not found"
        );
    }
}
