//! #2148: `bynkc compile` output builds as ES modules with a plain
//! `tsc -p out/tsconfig.json`, emit on, and no `package.json` in scope.
//!
//! The emitted modules are ESM (`.js`-suffixed imports, `export`). The emitted
//! tsconfig used to say `module: NodeNext`, under which tsc picks each file's
//! format from the nearest `package.json`; with none declaring
//! `"type": "module"` it down-levelled the output to CommonJS. That transform
//! reserves the top-level names `require` (TypeScript 5 and 7) and `Object`
//! (TypeScript 7), so a Bynk `fn require` or `type Object` failed with TS2441.
//! `tsc` reports this only while emitting, which is why `--noEmit` checks
//! never saw it.
//!
//! The project is staged under the system temp directory, not the repository,
//! so no `package.json` above it decides the format.
//!
//! Skips loudly without a TypeScript toolchain; `BYNK_REQUIRE_TSC=1` turns the
//! skip into a failure (CI).

mod require;
mod tools;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const SOURCE: &str = "context demo.names

-- Names the CommonJS transform reserves at module top level.
type Object = { n: Int }

fn require(x: Int) -> Int {
  x + 10
}

fn wrap(x: Int) -> Object {
  Object { n: require(x) }
}
";

fn stage() -> PathBuf {
    let root = std::env::temp_dir().join(format!("bynk-2148-compile-esm-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/demo")).unwrap();
    fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"compile-esm\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(root.join("src/demo/names.bynk"), SOURCE).unwrap();
    root
}

/// The nearest `package.json` at or above `dir`, if any.
fn package_json_above(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .map(|d| d.join("package.json"))
        .find(|p| p.is_file())
}

fn output_text(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn compiled_output_emits_as_esm_without_a_package_json() {
    let Some(mut tsc) = tools::tsc_command() else {
        tools::skip(
            "COMPILE ESM (tsc)",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "no TypeScript toolchain (`tsc` or `npx`) is on PATH",
        );
        return;
    };
    let root = stage();

    let compiled = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["compile", "-o", "out", "."])
        .current_dir(&root)
        .env("NO_COLOR", "1")
        .output()
        .expect("bynkc runs");
    assert!(
        compiled.status.success(),
        "bynkc compile failed:\n{}",
        output_text(&compiled)
    );

    let out = root.join("out");
    if let Some(p) = package_json_above(&out) {
        // The temp directory sits under a `package.json` on this machine, so
        // the run would not show what a user without one gets.
        tools::skip(
            "COMPILE ESM (tsc)",
            false,
            REQUIRE_ENV,
            &format!("{} decides the module format here", p.display()),
        );
        let _ = fs::remove_dir_all(&root);
        return;
    }

    // Emit on, exactly as a user building for deployment would run it: no
    // `--noEmit`, no module overrides.
    let built = tsc
        .arg("-p")
        .arg(out.join("tsconfig.json"))
        .current_dir(&root)
        .output()
        .expect("run tsc");
    assert!(
        built.status.success(),
        "tsc -p out/tsconfig.json (emit on) fails on the compiled output:\n{}",
        output_text(&built)
    );

    // The emitted JavaScript is ESM, whichever names a TypeScript version
    // happens to reserve under CommonJS.
    let js_path = root.join("out-js/demo/names.js");
    let js = fs::read_to_string(&js_path)
        .unwrap_or_else(|e| panic!("tsc wrote no {}: {e}", js_path.display()));
    assert!(
        js.contains("export "),
        "{} is not an ES module:\n{js}",
        js_path.display()
    );
    assert!(
        !js.contains("exports.") && !js.contains("Object.defineProperty(exports"),
        "{} was emitted as CommonJS:\n{js}",
        js_path.display()
    );

    let _ = fs::remove_dir_all(&root);
}
