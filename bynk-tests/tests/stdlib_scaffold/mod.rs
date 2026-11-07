//! A project that consumes the whole standard library on one platform, generated
//! from the `bynk-std` registry (standard-library S2, #1952).
//!
//! The first-party bindings import the modules the compiler generates from the
//! first-party sources (`./bynk.js`, `./bynk/locale/types.js`, …), so they can
//! only be type-checked or run beside a compiled project. Fixtures that happen
//! to consume a capability used to be that project, which made binding coverage
//! incidental. The scaffold is that project on purpose: one context that
//! `consumes` every exported capability of every adapter available on the
//! platform, and `uses` every commons. A unit added to `UNITS` is in it without
//! an edit here.

#![allow(dead_code)]

use bynk_check::firstparty::{Platform, UNITS, Unit, UnitKind, experimental_items};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Every platform the toolchain ships a binding for.
pub const PLATFORMS: [Platform; 3] = [Platform::Cloudflare, Platform::Node, Platform::Browser];

/// The registry units available on `platform`: the portable ones, and any
/// platform-native one only on its own platform (the platform lock).
pub fn units(platform: Platform) -> impl Iterator<Item = &'static Unit> {
    UNITS
        .iter()
        .filter(move |u| u.platform.is_none_or(|p| p == platform))
}

/// The capabilities an adapter's source exports: its `exports capability { … }`
/// line.
pub fn exported_capabilities(unit: &Unit) -> Vec<&'static str> {
    let line = unit
        .source
        .lines()
        .find(|l| l.trim_start().starts_with("exports capability {"))
        .unwrap_or_else(|| panic!("{} declares no `exports capability`", unit.name));
    line.split_once('{')
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(names, _)| names.split(',').map(str::trim).collect())
        .unwrap_or_default()
}

/// The scaffold's one source file for `platform`.
pub fn source(platform: Platform) -> String {
    let mut uses = String::new();
    let mut consumes = String::new();
    for unit in units(platform) {
        match unit.kind {
            UnitKind::Commons => uses.push_str(&format!("uses {}\n", unit.name)),
            UnitKind::Adapter => consumes.push_str(&format!(
                "consumes {} {{ {} }}\n",
                unit.name,
                exported_capabilities(unit).join(", ")
            )),
        }
    }
    format!(
        "context stdlib.scaffold\n\n{uses}{consumes}\nservice probe {{\n\ton call() -> Effect[Int] {{\n\t\tEffect.pure(0)\n\t}}\n}}\n"
    )
}

/// The binding modules `platform`'s scaffold ships: `(output path, TS source)`
/// for every available unit that has one. The path is relative to the output
/// root and ends in `.ts`.
pub fn bindings(platform: Platform) -> Vec<(&'static str, &'static str)> {
    units(platform)
        .filter_map(|u| u.binding_for(platform))
        .collect()
}

/// A binding's file stem (`bynk-node`, `cloudflare.binding`): what its test file
/// and its test names are called after.
pub fn stem(output_path: &str) -> &str {
    let file = output_path.rsplit('/').next().unwrap_or(output_path);
    file.strip_suffix(".ts").unwrap_or(file)
}

/// Write `platform`'s scaffold project under `dir` and compile it with `bynk`,
/// to TypeScript or (with `js`) to JavaScript. Returns the output root.
pub fn compile(platform: Platform, dir: &Path, js: bool) -> PathBuf {
    let project = dir.join(format!("scaffold-{}", platform.as_str()));
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(project.join("src/stdlib")).unwrap();
    // Every experimental item allowed, so the scaffold consumes the whole
    // library, the experimental parts included (`bynk.cloudflare.Service`).
    let allow = experimental_items()
        .iter()
        .map(|item| format!("{item:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        project.join("bynk.toml"),
        format!("[project]\nname = \"stdlib-scaffold\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [{allow}]\n"),
    )
    .unwrap();
    fs::write(project.join("src/stdlib/scaffold.bynk"), source(platform)).unwrap();

    let out = project.join(if js { "out-js" } else { "out-ts" });
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"));
    cmd.arg("compile");
    match platform {
        // The Workers target, so the platform adapter's binding is emitted the
        // way a deployed Worker gets it.
        Platform::Cloudflare => cmd.args(["--target", "workers"]),
        other => cmd.args(["--platform", other.as_str()]),
    };
    if js {
        cmd.args(["--emit", "js"]);
    }
    let compiled = cmd
        .arg("-o")
        .arg(&out)
        .arg(&project)
        .output()
        .expect("run bynk");
    assert!(
        compiled.status.success(),
        "the {} scaffold does not compile:\n{}\n--- source ---\n{}",
        platform.as_str(),
        String::from_utf8_lossy(&compiled.stderr),
        source(platform)
    );
    out
}

/// A fresh scratch directory for one test, removed by the caller on success.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bynk-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}
