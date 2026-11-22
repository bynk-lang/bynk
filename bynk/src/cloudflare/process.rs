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
}

impl<'a> Wrangler<'a> {
    pub(crate) fn new(provenance: &'a Provenance, project_root: &'a Path) -> Self {
        Wrangler {
            provenance,
            project_root,
        }
    }

    fn command(&self, op: &Op<'_>, dir: &Path) -> Result<Command, WranglerError> {
        let argv = op.argv();
        let (subcommand, rest) = argv
            .split_first()
            .expect("an operation names its subcommand");
        let mut command = bynk_driver::tools::wrangler::command(self.provenance, subcommand)
            .ok_or_else(|| WranglerError::missing(op))?;
        command.args(rest).current_dir(dir);
        Ok(command)
    }

    /// [`command`](Self::command) run from wherever `bynk` was started.
    fn command_anywhere(&self, op: &Op<'_>) -> Result<Command, WranglerError> {
        let argv = op.argv();
        let (subcommand, rest) = argv
            .split_first()
            .expect("an operation names its subcommand");
        let mut command = bynk_driver::tools::wrangler::command(self.provenance, subcommand)
            .ok_or_else(|| WranglerError::missing(op))?;
        command.args(rest);
        Ok(command)
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
        {
            // Taken and dropped inside the block: wrangler reads to EOF, so the pipe must close
            // before `wait_with_output`, or both ends block.
            let mut pipe = child
                .stdin
                .take()
                .ok_or_else(|| WranglerError::new(op, Failure::Pipe(no_pipe)))?;
            pipe.write_all(stdin.as_bytes())
                .map_err(|e| WranglerError::new(op, Failure::Io(e)))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|e| WranglerError::new(op, Failure::Wait(e)))?;
        if output.status.success() {
            return Ok(());
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
        let id = value
            .get("id")
            .or_else(|| value.get("kv_namespaces")?.get(0)?.get("id"));
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
