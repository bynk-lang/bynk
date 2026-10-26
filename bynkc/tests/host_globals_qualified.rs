//! #1653: emitted code reaches every host global through `globalThis`.
//!
//! A user type or function may share its name with a host global (`type Error`,
//! `fn console`), and its declaration sits in the same module scope as the
//! emitter's own code. So the emitter never writes a bare `JSON.stringify` or
//! `new Error(…)` into a unit module: it writes `globalThis.JSON.stringify`, which
//! no user declaration can hide (`globalThis` itself is renamed or rejected).
//!
//! This test is the drift guard for that rule. It scans every blessed unit
//! module in the positive fixture corpus for a bare reference to a host global
//! the emitter is known to use, so a later change that adds an unqualified
//! `Date.now()` fails here rather than only in a program that happens to
//! declare a `Date`.
//!
//! Out of scope, because no user name can reach their module scope: the test
//! scaffolding (`tests/`), the platform bindings (`bynk*.ts`), user-authored
//! adapter bindings (`*.binding.ts`) and the runtime itself.

use std::fs;
use std::path::{Path, PathBuf};

/// The host globals emitted code references. Kept in step with the emitter:
/// add a name here when emitted code starts using a new global. `String` is
/// the one global emitted bare (`String(x)`): it is a hard Bynk keyword, so no
/// declaration can take the name.
const HOST_GLOBALS: &[&str] = &[
    "ArrayBuffer",
    "Array",
    "BigInt",
    "Blob",
    "Boolean",
    "Date",
    "Error",
    "Headers",
    "Intl",
    "JSON",
    "Map",
    "Math",
    "Number",
    "Object",
    "Parameters",
    "Promise",
    "Record",
    "RegExp",
    "Request",
    "Response",
    "ReturnType",
    "Set",
    "Symbol",
    "TextDecoder",
    "TextEncoder",
    "URL",
    "Uint8Array",
    "console",
    "crypto",
    "encodeURIComponent",
];

fn ts_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            ts_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "ts") {
            out.push(path);
        }
    }
}

/// Whether `rel` (a path under a fixture's `expected/`) is a unit module — one
/// whose module scope holds user declarations.
fn is_unit_module(rel: &Path) -> bool {
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let top_level = rel.components().count() == 1;
    !(rel.starts_with("tests")
        || name == "main.ts"
        || rel.starts_with("__bynk_runtime")
        || name.ends_with(".binding.ts")
        || (top_level && name.starts_with("bynk")))
}

/// `src` with comments and string/template literal contents blanked out, so a
/// global named in prose or in a JS string is not counted.
fn code_only(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            q @ (b'"' | b'\'' | b'`') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                out.push_str("\"\"");
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out
}

/// Whether the module declares or imports `name` itself (a user `type Error`),
/// in which case its bare references are the user's, not the emitter's.
fn declares(code: &str, name: &str) -> bool {
    ["interface", "type", "const", "function", "class"]
        .iter()
        .any(|kw| {
            [" ", "<", "("]
                .iter()
                .any(|t| code.contains(&format!("{kw} {name}{t}")))
        })
        || code.lines().any(|l| {
            l.trim_start().starts_with("import")
                && l.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
                    .any(|w| w == name)
        })
        || code.contains(&format!("{{ {name},")) // destructured from a namespace
}

/// Bare (unqualified) references to `name` in `code`: not preceded by `.` or
/// an identifier character, and not an object key (`name:`).
fn bare_references(code: &str, name: &str) -> usize {
    let b = code.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    code.match_indices(name)
        .filter(|&(at, _)| {
            let before = at.checked_sub(1).map(|j| b[j]);
            let after = b.get(at + name.len()).copied();
            let rest = code[at + name.len()..].trim_start();
            let qualified_or_longer =
                before.is_some_and(|c| ident(c) || c == b'.') || after.is_some_and(ident);
            let object_key = rest.starts_with(':') || rest.starts_with("?:");
            !(qualified_or_longer || object_key)
        })
        .count()
}

#[test]
fn unit_modules_reach_host_globals_through_global_this() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let mut violations = Vec::new();
    let mut scanned = 0;
    for fixture in fs::read_dir(&fixtures)
        .expect("positive fixtures")
        .flatten()
    {
        let expected = fixture.path().join("expected");
        let mut files = Vec::new();
        ts_files(&expected, &mut files);
        for file in files {
            let rel = file.strip_prefix(&expected).expect("under expected/");
            if !is_unit_module(rel) {
                continue;
            }
            scanned += 1;
            let code = code_only(&fs::read_to_string(&file).expect("readable"));
            for name in HOST_GLOBALS {
                if declares(&code, name) {
                    continue;
                }
                let n = bare_references(&code, name);
                if n > 0 {
                    violations.push(format!("{}: {n} bare `{name}`", file.display()));
                }
            }
        }
    }
    assert!(
        scanned > 100,
        "expected to scan the fixture corpus, scanned {scanned}"
    );
    assert!(
        violations.is_empty(),
        "emitted unit modules reference host globals without `globalThis.` (#1653):\n{}",
        violations.join("\n")
    );
}
