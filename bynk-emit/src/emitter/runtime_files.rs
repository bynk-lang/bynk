//! The runtime as per-module files (ADR 0615, #2426).
//!
//! A project emits `__bynk_runtime/<module>.{ts,js}` for the runtime modules its
//! emitted code reaches, and no others. Generated code imports each helper from
//! the module that defines it; this module is where a list of runtime names
//! becomes one import per defining module, and where the reached set is closed
//! over the modules' own imports.
//!
//! The directory name starts with `_`, which a Bynk identifier cannot (#1653),
//! and a user file mirrors its source path, so no user path can collide with it.

use std::collections::BTreeSet;
use std::path::PathBuf;

use bynk_ts::TsDecl;

use super::runtime_modules::{RUNTIME_EXPORTS, RUNTIME_MODULES};

/// The directory (at the root of the output) the runtime modules are emitted to.
pub const RUNTIME_DIR: &str = "__bynk_runtime";

/// The module that defines the exported runtime name `name`.
fn module_of(name: &str) -> Option<&'static str> {
    RUNTIME_EXPORTS
        .binary_search_by_key(&name, |(n, _)| n)
        .ok()
        .map(|i| RUNTIME_EXPORTS[i].1)
}

/// Split a runtime specifier (`./runtime.js`, `../../runtime.ts`) into the
/// directory prefix and the extension, or `None` if it is not one.
fn split_specifier(from: &str) -> Option<(&str, &str)> {
    let (prefix, file) = from.rsplit_once('/')?;
    let ext = file.strip_prefix("runtime.")?;
    matches!(ext, "js" | "ts").then_some((&from[..=prefix.len()], ext))
}

/// One import declaration per runtime module defining a name in `names`.
///
/// `from` is the project-relative runtime specifier the producer already
/// computes (`runtime_import_for`); each name keeps its `type ` prefix and any
/// `as` rename. A group whose names are all types is emitted as
/// `import type`, so a type-only edge never survives type-stripping as a value
/// import. Modules come out in runtime dependency order, which keeps the output
/// deterministic.
pub(crate) fn split_runtime_import(type_only: bool, names: &[String], from: &str) -> Vec<TsDecl> {
    let (prefix, ext) =
        split_specifier(from).unwrap_or_else(|| panic!("not a runtime specifier: {from}"));
    let mut groups: Vec<(&'static str, Vec<String>)> = RUNTIME_MODULES
        .iter()
        .map(|(m, _)| (*m, Vec::new()))
        .collect();
    for name in names {
        let bare = name.strip_prefix("type ").unwrap_or(name);
        let imported = bare.split(" as ").next().unwrap_or(bare);
        let module =
            module_of(imported).unwrap_or_else(|| panic!("`{imported}` is not a runtime export"));
        let group = groups
            .iter_mut()
            .find(|(m, _)| *m == module)
            .expect("module is in the table");
        group.1.push(name.clone());
    }
    groups
        .into_iter()
        .filter(|(_, names)| !names.is_empty())
        .map(|(module, names)| {
            let all_types = names.iter().all(|n| n.starts_with("type "));
            let (type_only, names) = if all_types && !type_only {
                (
                    true,
                    names
                        .iter()
                        .map(|n| n.trim_start_matches("type ").to_string())
                        .collect(),
                )
            } else {
                (type_only, names)
            };
            TsDecl::Import {
                type_only,
                names,
                from: format!("{prefix}{RUNTIME_DIR}/{module}.{ext}"),
            }
        })
        .collect()
}

/// Rewrite the `import … from "<…>/runtime.js"` lines of hand-written TypeScript
/// (adapter bindings, first-party or the user's) to per-module imports. Anything
/// that is not a single-statement named import of the runtime is left as is.
pub fn rewrite_runtime_imports(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        match rewrite_import_line(body) {
            Some(rewritten) => {
                out.push_str(&rewritten);
                if line.ends_with('\n') {
                    out.push('\n');
                }
            }
            None => out.push_str(line),
        }
    }
    out
}

fn rewrite_import_line(line: &str) -> Option<String> {
    let rest = line.strip_prefix("import ")?;
    let (type_only, rest) = match rest.strip_prefix("type ") {
        Some(r) => (true, r),
        None => (false, rest),
    };
    let rest = rest.strip_prefix('{')?;
    let (list, rest) = rest.split_once('}')?;
    let rest = rest.trim_start().strip_prefix("from")?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let rest = &rest[1..];
    let (spec, tail) = rest.split_once(quote)?;
    split_specifier(spec)?;
    if tail.trim() != ";" {
        return None;
    }
    let names: Vec<String> = list
        .split(',')
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    let decls = split_runtime_import(type_only, &names, spec);
    let lines: Vec<String> = decls
        .iter()
        .map(|d| match d {
            TsDecl::Import {
                type_only,
                names,
                from,
            } => format!(
                "import {}{{ {} }} from {quote}{from}{quote};",
                if *type_only { "type " } else { "" },
                names.join(", ")
            ),
            _ => unreachable!("split_runtime_import yields named imports"),
        })
        .collect();
    Some(lines.join("\n"))
}

/// The runtime module named by an import specifier, with its extension.
fn reached_module(specifier: &str) -> Option<(&str, &str)> {
    let rest = specifier.rsplit_once(&format!("{RUNTIME_DIR}/"))?.1;
    let (module, ext) = rest.rsplit_once('.')?;
    Some((module, ext))
}

/// Every runtime module an emitted file's imports name, found in the
/// specifiers it imports and in the `import … from` lines of its verbatim text.
pub(crate) fn reached_in(program: &bynk_ts::TsProgram, out: &mut BTreeSet<(String, String)>) {
    let mut note = |spec: &str| {
        if let Some((m, e)) = reached_module(spec) {
            out.insert((m.to_string(), e.to_string()));
        }
    };
    for spec in program.import_specifiers() {
        note(spec);
    }
    for (_, text) in program.verbatim_content() {
        for line in text.lines().filter(|l| l.starts_with("import ")) {
            if let Some((_, tail)) = line.rsplit_once(" from ") {
                note(tail.trim().trim_end_matches(';').trim_matches(['"', '\'']));
            }
        }
    }
}

/// The files for `reached` (module, extension) pairs closed over the runtime
/// modules' own imports. Type-only edges count: `tsc` over the emitted `.ts`
/// needs the files, and one rule keeps the output deterministic.
pub(crate) fn closure_files(reached: &BTreeSet<(String, String)>) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    for ext in reached
        .iter()
        .map(|(_, e)| e.as_str())
        .collect::<BTreeSet<_>>()
    {
        let mut want: Vec<&str> = reached
            .iter()
            .filter(|(_, e)| e == ext)
            .map(|(m, _)| m.as_str())
            .collect();
        let mut done: BTreeSet<&str> = BTreeSet::new();
        while let Some(m) = want.pop() {
            if !done.insert(m) {
                continue;
            }
            let (_, src) = RUNTIME_MODULES
                .iter()
                .find(|(n, _)| *n == m)
                .unwrap_or_else(|| panic!("unknown runtime module `{m}`"));
            for line in src.lines().filter(|l| l.starts_with("import ")) {
                if let Some(dep) = line
                    .rsplit_once("\"./")
                    .and_then(|(_, t)| t.strip_suffix(".ts\";"))
                {
                    want.push(dep);
                }
            }
        }
        for m in done {
            files.push((module_path(m), module_source(m, ext)));
        }
    }
    files
}

/// Always a `.ts` file: specifiers say `.js` (resolved by `tsc` / stripped by
/// `--emit js`) or `.ts` (the debug build), but the document is TypeScript.
fn module_path(module: &str) -> PathBuf {
    PathBuf::from(format!("{RUNTIME_DIR}/{module}.ts"))
}

/// A module's source with its sibling imports spelled for the target extension.
fn module_source(module: &str, ext: &str) -> String {
    let (_, src) = RUNTIME_MODULES
        .iter()
        .find(|(n, _)| *n == module)
        .expect("module is in the table");
    if ext == "ts" {
        return src.to_string();
    }
    src.lines()
        .map(|l| {
            if l.starts_with("import ") {
                l.replace(".ts\";", &format!(".{ext}\";"))
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Every runtime module as `(path, source)`, for tests that stage a runtime by
/// hand beside emitted code. Extension `js` spells sibling imports as `.js`.
pub fn emit_all_runtime_modules() -> Vec<(PathBuf, String)> {
    RUNTIME_MODULES
        .iter()
        .map(|(m, _)| (module_path(m), module_source(m, "js")))
        .collect()
}

/// [`split_runtime_import`] as top-level statements, for producers building a
/// `TsProgram`.
pub(crate) fn runtime_import_stmts(
    type_only: bool,
    names: &[String],
    from: &str,
) -> Vec<bynk_ts::TsStmt> {
    split_runtime_import(type_only, names, from)
        .into_iter()
        .map(|d| bynk_ts::TsStmt::decl(d, None))
        .collect()
}
