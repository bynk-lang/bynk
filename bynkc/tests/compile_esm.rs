//! #2148: `bynkc compile` output is checked and built as ES modules by a plain
//! `tsc -p out/tsconfig.json`, emit on, with no user `package.json` in scope.
//!
//! The emitted modules are ESM (`.js`-suffixed imports, `export`), and the
//! emitted tsconfig says `module: NodeNext`, under which tsc picks each file's
//! format from the nearest `package.json`. With none declaring
//! `"type": "module"`, tsc used to down-level the output to CommonJS. That
//! transform reserves the top-level names `require` (TypeScript 5 and 7) and
//! `Object` (TypeScript 7), so a Bynk `fn require` or `type Object` failed with
//! TS2441, which tsc reports only while emitting. The compiler now writes
//! `out/package.json` declaring `"type": "module"`.
//!
//! The marker keeps `NodeNext`'s checking, so `tsc` holds a hand-written
//! adapter binding to the rules Node runs it under: an extensionless relative
//! import is a `tsc` error (TS2835), not a passing check followed by
//! `ERR_MODULE_NOT_FOUND` at run time.
//!
//! Each project is staged under the system temp directory, not the
//! repository, so no `package.json` above it decides the format.
//!
//! Skips loudly without a TypeScript toolchain; `BYNK_REQUIRE_TSC=1` turns the
//! skip into a failure (CI).

mod require;
mod tools;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const NAMES_SOURCE: &str = "context demo.names

-- Names the CommonJS transform reserves at module top level.
type Object = { n: Int }

fn require(x: Int) -> Int {
  x + 10
}

fn wrap(x: Int) -> Object {
  Object { n: require(x) }
}
";

const ADAPTER_SOURCE: &str = "adapter clock {
  binding \"./clock.binding.ts\"

  exports capability { Clock }

  capability Clock {
    fn now() -> Effect[Int]
  }

  provides Clock = FixedClock
}
";

/// A binding whose value import has no `.js` extension: `tsc` under a
/// bundler's resolution accepts it, Node's ESM loader does not.
const EXTENSIONLESS_BINDING: &str = "import type { Clock } from \"./clock.js\";
import { Some } from \"./runtime\";

export class FixedClock implements Clock {
  async now(): Promise<number> {
    return Some(0).tag === \"Some\" ? 0 : 1;
  }
}
";

/// Stage a project under the system temp directory: `bynk.toml` plus each
/// `(path under src/, contents)`.
fn stage(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-2148-compile-esm-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    for (rel, text) in files {
        let path = root.join("src").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"compile-esm\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    root
}

fn output_text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The nearest `package.json` strictly above `dir`, if any.
fn package_json_above(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .skip(1)
        .map(|d| d.join("package.json"))
        .find(|p| p.is_file())
}

/// `bynkc compile` the staged project, then run a plain emit-on
/// `tsc -p out/tsconfig.json`, exactly as a user building for deployment
/// would: no `--noEmit`, no module overrides. `None` when the test skips.
fn compile_then_tsc(what: &str, root: &Path) -> Option<Output> {
    let Some(mut tsc) = tools::tsc_command() else {
        tools::skip(
            what,
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "no TypeScript toolchain (`tsc` or `npx`) is on PATH",
        );
        return None;
    };
    if let Some(p) = package_json_above(root) {
        // A `package.json` above the temp directory would decide the format,
        // so the run would not show what a user without one gets.
        tools::skip(
            what,
            false,
            REQUIRE_ENV,
            &format!("{} is in scope here", p.display()),
        );
        return None;
    }

    let compiled = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["compile", "-o", "out", "."])
        .current_dir(root)
        .env("NO_COLOR", "1")
        .output()
        .expect("bynkc runs");
    assert!(
        compiled.status.success(),
        "bynkc compile failed:\n{}",
        output_text(&compiled)
    );
    let marker = fs::read_to_string(root.join("out/package.json"))
        .expect("bynkc compile writes out/package.json");
    assert!(
        marker.contains("\"type\": \"module\""),
        "out/package.json does not declare ES modules:\n{marker}"
    );

    Some(
        tsc.arg("-p")
            .arg(root.join("out/tsconfig.json"))
            .current_dir(root)
            .output()
            .expect("run tsc"),
    )
}

#[test]
fn compiled_output_emits_as_esm_without_a_user_package_json() {
    let root = stage("names", &[("demo/names.bynk", NAMES_SOURCE)]);
    let Some(built) = compile_then_tsc("COMPILE ESM (tsc)", &root) else {
        let _ = fs::remove_dir_all(&root);
        return;
    };
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

#[test]
fn an_extensionless_import_in_a_binding_fails_tsc() {
    let root = stage(
        "binding",
        &[
            ("clock.bynk", ADAPTER_SOURCE),
            ("clock.binding.ts", EXTENSIONLESS_BINDING),
        ],
    );
    let Some(built) = compile_then_tsc("COMPILE ESM BINDING (tsc)", &root) else {
        let _ = fs::remove_dir_all(&root);
        return;
    };
    let text = output_text(&built);
    assert!(
        !built.status.success() && text.contains("TS2835"),
        "tsc should reject the binding's extensionless relative import as Node's \
         ESM loader would (TS2835), but reported:\n{text}"
    );
    assert!(
        text.contains("clock.binding.ts"),
        "the TS2835 error should point at the binding:\n{text}"
    );

    // The same binding with the `.js` specifier Node needs builds cleanly, so
    // the failure above is the extension and nothing else.
    fs::write(
        root.join("src/clock.binding.ts"),
        EXTENSIONLESS_BINDING.replace("\"./runtime\"", "\"./runtime.js\""),
    )
    .unwrap();
    let Some(fixed) = compile_then_tsc("COMPILE ESM BINDING (tsc)", &root) else {
        let _ = fs::remove_dir_all(&root);
        return;
    };
    assert!(
        fixed.status.success(),
        "the binding with a `.js` specifier fails tsc:\n{}",
        output_text(&fixed)
    );

    let _ = fs::remove_dir_all(&root);
}
