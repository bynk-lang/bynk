//! Shared Workers-build plumbing for `bynk dev` and `bynk deploy`.
//!
//! Both commands do the same four things before they diverge: prepare a managed
//! build directory, compile the project into it, discover the worker directories
//! that came out, and shell `wrangler`. That machinery grew up inside `dev.rs`
//! and `deploy.rs` reached across for it, which left `dev` reading as `deploy`'s
//! utility library. It lives here instead — a sibling of both, owned by neither.

use std::path::Path;
use std::process::Command;

use bynk_driver::project::{BuildTarget, SchemaLock};

use crate::probe::Provenance;

/// One compile of the project into `build_dir`, in-process, on the same rooting
/// rule as `bynk compile <project_root>` (#524, via
/// [`bynk_driver::project_options`]). Returns `false` on failure with the
/// diagnostics already rendered.
///
/// `schema_registry` (#980): `true` for the real `bynk dev`/`bynk deploy`
/// call sites — otherwise a deploy could ship a `schemaVersion` computed
/// purely from `@schema(N)` annotations, diverging from what `bynk compile`
/// would have shipped for the same source (the write is a no-op when the
/// tree is already up to date). `false` for
/// `compile_once_warnings_behaviour.rs`, the only other caller: it compiles a
/// **committed** repo fixture in place, the same hazard
/// `bynk-tests/tests/e2e.rs`'s in-place fixtures have — an unconditional write
/// would leave a real `bynk.schema.lock` in the tree on every test run.
pub fn compile_once(project_root: &Path, build_dir: &Path, schema_registry: bool) -> bool {
    // #1078: `bynk-emit` touches no disk for `bynk.schema.lock` — read its
    // current content here (verified-absent `None` for a fresh project) and
    // hand it in; write the reconciled content back after a clean compile.
    let schema_lock = if schema_registry {
        match bynk_driver::schema_lock::read(project_root) {
            Ok(existing) => SchemaLock::On { existing },
            Err(e) => {
                eprintln!(
                    "bynk: could not read {}: {e}",
                    bynk_driver::schema_lock::lock_path(project_root).display()
                );
                return false;
            }
        }
    } else {
        SchemaLock::Off
    };
    let options = match bynk_driver::try_project_options(project_root) {
        // #1821 (ADR 0147 D3): the deployable carries no `suite`.
        Ok(o) => o
            .target(BuildTarget::Workers)
            .schema_registry(schema_lock)
            .tests(false),
        Err(e) => {
            eprintln!("bynk: {e}");
            return false;
        }
    };
    let output = match bynk_driver::project::compile_project(&options) {
        Ok(out) => out,
        Err(failure) => {
            // Render with full source context, exactly as the shelled `bynk
            // compile` did — the front-end's flatten-then-delegate (ADR 0100),
            // shared with `bynk check` (see `crate::diagnostics`).
            crate::diagnostics::render_project_failure(&failure);
            return false;
        }
    };
    // A write failure here is reported but does not fail the build — the
    // same non-fatal handling `schema_registry::write`'s own eprintln used
    // to give it when this lived inside `compile_project`.
    if let Some(content) = &output.schema_lock
        && let Err(e) = bynk_driver::schema_lock::write(project_root, content)
    {
        eprintln!(
            "bynk: could not write {}: {e}",
            bynk_driver::schema_lock::lock_path(project_root).display()
        );
    }
    if let Err(e) = bynk_driver::write_output(&output, build_dir) {
        eprintln!(
            "bynk: could not write build output under `{}`: {e}",
            build_dir.display()
        );
        return false;
    }
    // ADR 0117: surface non-failing warnings.
    crate::diagnostics::print_project_warnings(
        &output.warnings,
        &output.snapshots,
        &output.display_root,
    );
    true
}

/// Ensure `.bynk/` is gitignored on first build (cargo's `target/.gitignore`
/// precedent — a `dev` run never dirties `git status`), then clear the
/// `workers/` tree so selection only ever sees this build's contexts (D1).
pub fn prepare_build_dir(project_root: &Path, build_dir: &Path) -> std::io::Result<()> {
    let bynk_dir = project_root.join(".bynk");
    std::fs::create_dir_all(&bynk_dir)?;
    let gitignore = bynk_dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, "*\n")?;
    }
    let workers = build_dir.join("workers");
    match std::fs::remove_dir_all(&workers) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The worker directories under `<build>/workers/` that carry a `wrangler.toml`
/// (the unit `wrangler dev` can serve), sorted for deterministic messages.
pub fn discover_workers(workers_dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(entries) = std::fs::read_dir(workers_dir) else {
        return names;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.join("wrangler.toml").is_file()
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    names
}

/// Why context selection failed — rendered to the user with the next step.
#[derive(Debug, PartialEq, Eq)]
pub enum SelectError {
    /// No worker was produced by the compile (e.g. an empty project).
    NoneBuilt,
    /// `--context NAME` named a context that doesn't exist.
    NotFound {
        requested: String,
        available: Vec<String>,
    },
}

impl std::fmt::Display for SelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectError::NoneBuilt => {
                write!(
                    f,
                    "no workers were built — does the project define any contexts?"
                )
            }
            SelectError::NotFound {
                requested,
                available,
            } => write!(
                f,
                "no context `{requested}` — available: {}",
                available.join(", ")
            ),
        }
    }
}

/// Match one requested context against the built worker dirs, accepting either
/// the dotted name or its dasherised form (`commerce.payment` /
/// `commerce-payment`).
fn resolve_one(available: &[String], name: &str) -> Result<String, SelectError> {
    let dashed = name.replace('.', "-");
    available
        .iter()
        .find(|d| d.as_str() == name || d.as_str() == dashed)
        .cloned()
        .ok_or_else(|| SelectError::NotFound {
            requested: name.to_string(),
            available: available.to_vec(),
        })
}

/// Pick the workers `dev` will serve **together** (#552). No `--context` serves
/// every context in the project — the whole point of the increment, since a
/// cross-context call only resolves when its callee is up too. `--context` is
/// repeatable and narrows to a subset, in `available`'s deterministic order
/// rather than the order they were typed, and duplicates collapse.
///
/// There is no `Ambiguous` case: several contexts is the expected shape, not a
/// failure. Pure (the FS scan is the caller's) so the rule is unit-tested.
pub fn select_contexts(
    available: &[String],
    requested: &[String],
) -> Result<Vec<String>, SelectError> {
    if available.is_empty() {
        return Err(SelectError::NoneBuilt);
    }
    if requested.is_empty() {
        return Ok(available.to_vec());
    }
    let mut chosen = Vec::new();
    for name in requested {
        let worker = resolve_one(available, name)?;
        if !chosen.contains(&worker) {
            chosen.push(worker);
        }
    }
    chosen.sort();
    Ok(chosen)
}

/// Build the `wrangler dev` invocation for a resolved provenance: an installed
/// binary is run directly; an npx-provisionable one goes through `npx --yes`.
/// `None` when wrangler is genuinely missing.
pub fn wrangler_command(provenance: &Provenance, subcommand: &str) -> Option<Command> {
    match provenance {
        Provenance::Path(p) | Provenance::ProjectLocal(p) => {
            let mut cmd = Command::new(p);
            cmd.arg(subcommand);
            Some(cmd)
        }
        Provenance::Npx => {
            // #1758: the resolved `npx`, so Windows runs its `npx.cmd` shim.
            let mut cmd = Command::new(crate::probe::program_path("npx"));
            // #524: pinned provisioning, per the repo's npx convention — an
            // unpinned `wrangler` here meant the dev server could drift from
            // the wrangler the tests and deploys run.
            cmd.arg("--yes").arg("wrangler@4").arg(subcommand);
            Some(cmd)
        }
        Provenance::Missing => None,
    }
}
