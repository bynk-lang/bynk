//! [`Wrangler`]: each account operation as a wrangler process (#4043, Decision A).
//!
//! The command comes from [`bynk_driver::tools::wrangler::command`], the one place a wrangler
//! spawn is built. The resource operations run from the project root, where wrangler cannot load
//! and reject a generated config still carrying a placeholder (the KV id before the namespace
//! exists); `d1 migrations apply` runs from the Worker's directory, where its config names the
//! migrations directory.

use std::path::Path;
use std::process::Stdio;

use crate::probe::Provenance;

use super::api::{CloudflareApi, Op, VectorizeOp};
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

    fn command(&self, op: &Op<'_>, dir: &Path) -> Result<std::process::Command, WranglerError> {
        let argv = op.argv();
        let (subcommand, rest) = argv
            .split_first()
            .expect("an operation names its subcommand");
        let mut command = bynk_driver::tools::wrangler::command(self.provenance, subcommand)
            .ok_or_else(|| WranglerError::missing(op))?;
        command.args(rest).current_dir(dir);
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
}

impl CloudflareApi for Wrangler<'_> {
    fn queue_create(&self, name: &str) -> Result<(), WranglerError> {
        self.run_from_root(Op::QueueCreate { name }).map(drop)
    }

    /// Answered by the exit code alone, both streams discarded: the reason to ask `queues info`
    /// rather than read a create's complaint (ADR 0194 D2).
    fn queue_info(&self, name: &str) -> Result<(), WranglerError> {
        let op = Op::QueueInfo { name };
        let status = self
            .command(&op, self.project_root)?
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| WranglerError::new(&op, Failure::Start(e)))?;
        if status.success() {
            Ok(())
        } else {
            Err(WranglerError::exit(&op, Exit::of(&status), "", ""))
        }
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
