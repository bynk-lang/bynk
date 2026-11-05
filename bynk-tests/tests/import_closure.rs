//! #1831: every name an emitted module uses is bound in that module.
//!
//! Runs [`bynk_ts::unbound_names`] over every TypeScript module the positive
//! corpus emits. That catches the `TS2304`/`TS2552` family (#1736, #1778,
//! #1815, #1818, #1823, #1829), where the emitter names a type or calls a codec
//! the module never imports, without `tsc` on `PATH`.
//!
//! **Namespace members** (#2036). A module that names `handlers.Batch` binds
//! `handlers`, so the name check passes even when `handlers.ts` exports no
//! `Batch` (`TS2694`). Each member a module names through a namespace import
//! of another emitted module is checked against that module's exports
//! ([`bynk_ts::module_exports`], following `export *`), and a missing one is
//! reported as `<specifier>::<member>`.
//!
//! **Worker isolation** (#2045). A Worker's module never imports another
//! Worker's module as a value, so that module stays out of its bundle (#661):
//! a named import from another `workers/<dir>/` is reported unless every
//! entry is `type X`. A type-only name used as a value (`TS1361`) is reported
//! too, the regression a type-only import would otherwise hide without `tsc`.
//!
//! **Both targets.** A project fixture is compiled for bundle *and* Workers,
//! whatever its `target.txt` says. A fixture built only for bundle never had
//! its Workers output type-checked, and that is where #1845 and #1846 were
//! found. `bynkc test`'s layout is the bundle output with the Workers output
//! overlaid, so its modules are covered too. A single-file fixture is compiled
//! in memory through the project pipeline (its own path has no tree to walk).
//!
//! **Combinations no fixture has.** [`repros`] holds minimal programs for open
//! defects whose shape the corpus lacks.
//!
//! **Known defects** are listed in [`KNOWN`] with their issue, strictly: an
//! unlisted report fails, and so does a listed one that no longer reports.
//! The fix for an issue deletes its entry, the same discipline as
//! `behaviour_fixtures.rs`'s `fail` lines. The check cannot see into opaque
//! text (`bynk_ts::unbound_names`' module doc), so a clean run here does not
//! replace `tsc_verify`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use bynkc::BuildTarget;

/// `(source, target, module, unbound names, issue)`. `source` is a fixture
/// directory name or a [`repros`] name.
const KNOWN: &[(&str, &str, &str, &[&str], u32)] = &[];

/// Minimal projects for open defects no positive fixture exhibits:
/// `(name, [(path, source)])`, compiled for bundle.
fn repros() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    // #1829's repro became `1829_agent_state_unread_commons_field`, #1977's
    // `1977_map_query_entries_and_branded_keys`.
    Vec::new()
}

fn target_name(target: BuildTarget) -> &'static str {
    match target {
        BuildTarget::Workers => "workers",
        _ => "bundle",
    }
}

/// A module's output path, `/`-separated on every platform.
fn module_key(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

type Reports = BTreeMap<(String, &'static str, String), Vec<String>>;

/// Record every module of `out` with an unbound name, or (#2036) a member
/// named through a namespace import that the imported module does not
/// export, reported as `<specifier>::<member>`.
fn record(
    reports: &mut Reports,
    source: &str,
    target: BuildTarget,
    out: &bynkc::ProjectOutput,
) -> usize {
    let mut modules = 0;
    for (path, doc) in &out.artefacts.docs {
        if let bynkc::Document::Ts(program) = doc {
            modules += 1;
            let mut unbound = bynk_ts::unbound_names(program);
            unbound.extend(missing_members(path, program, &out.artefacts.docs));
            unbound.extend(
                bynk_ts::type_only_value_uses(program)
                    .into_iter()
                    .map(|n| format!("{n} (type-only, TS1361)")),
            );
            unbound.extend(cross_worker_value_imports(source, path, program));
            if !unbound.is_empty() {
                reports.insert(
                    (source.to_string(), target_name(target), module_key(path)),
                    unbound,
                );
            }
        }
    }
    modules
}

/// #2036: each `ns.member` `program` names through a namespace import of
/// another emitted module that the module does not export: a `TS2694`
/// (`Namespace 'handlers' has no exported member 'Batch'`) or `TS2339` that
/// `unbound_names` cannot see, the namespace itself being bound. A specifier
/// that resolves to no emitted TypeScript module is not checked.
fn missing_members(
    path: &Path,
    program: &bynk_ts::TsProgram,
    docs: &BTreeMap<PathBuf, bynkc::Document>,
) -> Vec<String> {
    bynk_ts::namespace_member_uses(program)
        .into_iter()
        .filter(|u| {
            resolve(path, &u.from)
                .and_then(|target| exports_of(&target, docs, 0))
                .is_some_and(|names| !names.contains(&u.member))
        })
        .map(|u| format!("{}::{}", u.from, u.member))
        .collect()
}

/// #2045: each name a Worker's module imports *as a value* from another
/// Worker's directory (`workers/a/…` importing `{ Batch }` from
/// `workers/b/handlers.js`), reported as `value import <specifier>::<name>`.
/// Another context's module must not enter this Worker's bundle (#661), so
/// its types import type-only and its sum variants are built inline. Two
/// value imports are by design and so not reported: a consumed type with
/// attached methods (`CONSUMED_METHOD_TYPES`), whose methods run from the
/// callee's companion, and the `import * as handlers_<ns>` of a
/// cross-context capability provider, which the caller instantiates (a
/// namespace import, which this check does not inspect).
fn cross_worker_value_imports(
    source: &str,
    path: &Path,
    program: &bynk_ts::TsProgram,
) -> Vec<String> {
    let worker_of = |p: &Path| -> Option<String> {
        let mut comps = p.components().map(|c| c.as_os_str().to_string_lossy());
        (comps.next()? == "workers").then(|| comps.next().map(|c| c.to_string()))?
    };
    let Some(own) = worker_of(path) else {
        return Vec::new();
    };
    bynk_ts::value_imports(program)
        .into_iter()
        .filter(|(from, name)| {
            resolve(path, from)
                .as_deref()
                .and_then(worker_of)
                .is_some_and(|other| other != own)
                && !CONSUMED_METHOD_TYPES.contains(&(source, name.as_str()))
        })
        .map(|(from, name)| format!("value import {from}::{name}"))
        .collect()
}

/// #2045: `(source, type)` for each consumed type a fixture calls an attached
/// method on, imported as a value by design (see
/// [`cross_worker_value_imports`]). Scoped to its fixture, so the same name
/// imported as a value anywhere else is still reported.
const CONSUMED_METHOD_TYPES: &[(&str, &str)] =
    &[("2045_consumed_method_value_import_workers", "Batch")];

/// The emitted module a relative specifier names from `from_module`
/// (`./handlers.js` from `workers/x/compose.ts` is `workers/x/handlers.ts`).
fn resolve(from_module: &Path, specifier: &str) -> Option<PathBuf> {
    if !specifier.starts_with('.') {
        return None;
    }
    let spec = specifier.strip_suffix(".js").unwrap_or(specifier);
    let mut out = from_module.parent().unwrap_or(Path::new("")).to_path_buf();
    for seg in format!("{spec}.ts").split('/') {
        match seg {
            "." => {}
            ".." => {
                if !out.pop() {
                    return None;
                }
            }
            s => out.push(s),
        }
    }
    Some(out)
}

/// Every name the emitted module at `path` exports, following its
/// `export * from` re-exports. `None` when `path` is not an emitted
/// TypeScript module.
fn exports_of(
    path: &Path,
    docs: &BTreeMap<PathBuf, bynkc::Document>,
    depth: usize,
) -> Option<std::collections::BTreeSet<String>> {
    let Some(bynkc::Document::Ts(program)) = docs.get(path) else {
        return None;
    };
    let exports = bynk_ts::module_exports(program);
    let mut names = exports.names;
    if depth < 8 {
        for from in &exports.star_from {
            if let Some(more) = resolve(path, from).and_then(|p| exports_of(&p, docs, depth + 1)) {
                names.extend(more);
            }
        }
    }
    Some(names)
}

/// The bundle programs whose host supplies a capability through
/// `__makeSurface(deps)` (#1822): a Workers build refuses them with
/// `bynk.capability.not_provided`, so they are checked on bundle only. Named,
/// so a new fixture refused for the same reason fails loudly instead.
const HOST_SUPPLIED: &[&str] = &[
    "217_field_op_index",
    "219_given_unused_capability_warns",
    "86_service_with_given",
    "87_service_chained_effects",
    "88_service_with_result_propagation",
];

/// Compiles a project fixture for `target`. `None` for a [`HOST_SUPPLIED`]
/// fixture's Workers build, refused only because a capability is left to the
/// host.
fn compile_project_fixture(dir: &Path, target: BuildTarget) -> Option<bynkc::ProjectOutput> {
    let options = if dir.join("bynk.toml").exists() {
        let paths = bynkc::try_read_project_paths(dir).expect("well-formed fixture manifest");
        bynk_testkit::compile_options_split(dir.to_path_buf(), paths)
    } else {
        bynk_testkit::compile_options_single(dir.join("src"))
    };
    match bynkc::compile_project(&options.target(target)) {
        Ok(out) => Some(out),
        Err(f) => {
            let errors = f.flatten();
            let name = dir.file_name().unwrap_or_default().to_string_lossy();
            if target == BuildTarget::Workers
                && HOST_SUPPLIED.contains(&name.as_ref())
                && errors
                    .iter()
                    .all(|e| e.category == "bynk.capability.not_provided")
            {
                return None;
            }
            panic!(
                "{} must compile for {}:\n{}",
                dir.display(),
                target_name(target),
                bynkc::render_project_errors(&errors)
            )
        }
    }
}

#[test]
fn every_emitted_module_binds_every_name_it_uses() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
        .expect("positive fixtures")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    let mut reports = Reports::new();
    let mut modules = 0;
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        if dir.join("src").is_dir() {
            for target in [BuildTarget::Bundle, BuildTarget::Workers] {
                if let Some(out) = compile_project_fixture(dir, target) {
                    modules += record(&mut reports, &name, target, &out);
                }
            }
        } else {
            let source = fs::read_to_string(dir.join("input.bynk")).expect("input.bynk");
            let out = bynk_emit::project::compile_in_memory(
                &source,
                BuildTarget::Bundle,
                Default::default(),
            )
            .unwrap_or_else(|_| panic!("{name} must compile"));
            modules += record(&mut reports, &name, BuildTarget::Bundle, &out);
        }
    }

    let scratch = std::env::temp_dir().join(format!("bynk-import-closure-{}", std::process::id()));
    for (name, files) in repros() {
        let dir = scratch.join(name);
        let _ = fs::remove_dir_all(&dir);
        for (path, src) in &files {
            let p = dir.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, src).unwrap();
        }
        let out = compile_project_fixture(&dir, BuildTarget::Bundle).expect("a bundle build");
        modules += record(&mut reports, name, BuildTarget::Bundle, &out);
    }
    let _ = fs::remove_dir_all(&scratch);

    assert!(modules > 1000, "only {modules} modules checked");

    let known: Reports = KNOWN
        .iter()
        .map(|(source, target, module, names, _)| {
            (
                (source.to_string(), *target, module.to_string()),
                names.iter().map(|n| n.to_string()).collect(),
            )
        })
        .collect();
    let unexpected: Vec<String> = reports
        .iter()
        .filter(|(k, v)| known.get(*k) != Some(*v))
        .map(|((s, t, m), v)| format!("  {s} [{t}] {m}: {v:?}"))
        .collect();
    let fixed: Vec<String> = KNOWN
        .iter()
        .filter(|(s, t, m, ..)| !reports.contains_key(&(s.to_string(), *t, m.to_string())))
        .map(|(s, t, m, _, issue)| format!("  {s} [{t}] {m} (#{issue})"))
        .collect();
    assert!(
        unexpected.is_empty() && fixed.is_empty(),
        "{modules} modules checked.\n\
         Modules using a name they never bind, or a namespace member the module does not \
         export (each is a TS2304/TS2552/TS2694 tsc would report):\n{}\n\
         Known defects that no longer report (delete their `KNOWN` entries):\n{}",
        unexpected.join("\n"),
        fixed.join("\n"),
    );
}
