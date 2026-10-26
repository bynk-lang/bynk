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
    try_split_runtime_import(type_only, names, from)
        .unwrap_or_else(|| panic!("an import from {from} names a non-runtime export: {names:?}"))
}

/// [`split_runtime_import`] that answers `None` for a specifier that is not a
/// runtime one or a name the runtime does not export, so hand-written input
/// (a user's binding) is left for `tsc` to report instead of crashing.
fn try_split_runtime_import(type_only: bool, names: &[String], from: &str) -> Option<Vec<TsDecl>> {
    let (prefix, ext) = split_specifier(from)?;
    let mut groups: Vec<(&'static str, Vec<String>)> = RUNTIME_MODULES
        .iter()
        .map(|(m, _)| (*m, Vec::new()))
        .collect();
    for name in names {
        let bare = name.strip_prefix("type ").unwrap_or(name);
        let imported = bare.split(" as ").next().unwrap_or(bare);
        let module = module_of(imported)?;
        let group = groups
            .iter_mut()
            .find(|(m, _)| *m == module)
            .expect("module is in the table");
        group.1.push(name.clone());
    }
    Some(
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
            .collect(),
    )
}

/// The `import … from "…";` statements of hand-written TypeScript as
/// `(first line, last line, statement text)` ranges, a multi-line braced import
/// joined into one. Only statements that start a line (after indentation).
fn import_statements(lines: &[&str]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if t.starts_with("import ") || t.starts_with("import{") {
            let mut j = i;
            let mut text = t.trim_end().to_string();
            // A braced list may span lines; stop at the `}` line, never run past
            // a statement that is not closed within a few hundred lines.
            while text.contains('{') && !text.contains('}') && j + 1 < lines.len() && j - i < 500 {
                j += 1;
                text.push(' ');
                text.push_str(lines[j].trim());
            }
            out.push((i, j, text));
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Rewrite the `import … from "<…>/runtime.js"` statements of hand-written
/// TypeScript (adapter bindings, first-party or the user's) to per-module
/// imports. Multi-line, unterminated, indented and trailing-comment forms are
/// handled. A statement that is not a braced named import of runtime exports (a
/// namespace import, a typo'd name) is left as written, for `tsc` to report.
pub fn rewrite_runtime_imports(source: &str) -> String {
    let crlf = source.contains("\r\n");
    let nl = if crlf { "\r\n" } else { "\n" };
    let lines: Vec<&str> = source
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    for (first, last, text) in import_statements(&lines) {
        out.extend(lines[i..first].iter().map(|l| l.to_string()));
        match rewrite_import_statement(&text) {
            Some(rewritten) => {
                let indent = &lines[first][..lines[first].len() - lines[first].trim_start().len()];
                out.extend(rewritten.into_iter().map(|l| format!("{indent}{l}")));
            }
            None => out.extend(lines[first..=last].iter().map(|l| l.to_string())),
        }
        i = last + 1;
    }
    out.extend(lines[i..].iter().map(|l| l.to_string()));
    out.join(nl)
}

fn rewrite_import_statement(stmt: &str) -> Option<Vec<String>> {
    let rest = stmt.strip_prefix("import")?.trim_start();
    let (type_only, rest) = match rest.strip_prefix("type ") {
        Some(r) => (true, r.trim_start()),
        None => (false, rest),
    };
    let rest = rest.strip_prefix('{')?;
    let (list, rest) = rest.split_once('}')?;
    let rest = rest.trim_start().strip_prefix("from")?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let rest = &rest[1..];
    let (spec, tail) = rest.split_once(quote)?;
    split_specifier(spec)?;
    // Whatever follows the specifier may be `;`, nothing, or a comment.
    let tail = tail.trim();
    let comment = tail.strip_prefix(';').unwrap_or(tail).trim();
    if !(comment.is_empty() || comment.starts_with("//") || comment.starts_with("/*")) {
        return None;
    }
    let names: Vec<String> = list
        .split(',')
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    let decls = try_split_runtime_import(type_only, &names, spec)?;
    let mut lines: Vec<String> = decls
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
    if !comment.is_empty() {
        if let Some(last) = lines.last_mut() {
            last.push(' ');
            last.push_str(comment);
        }
    }
    Some(lines)
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
        let lines: Vec<&str> = text.lines().collect();
        for (_, _, stmt) in import_statements(&lines) {
            if let Some((_, tail)) = stmt.rsplit_once(" from ") {
                let spec = tail.trim().trim_start_matches(['"', '\'']);
                if let Some(end) = spec.find(['"', '\'']) {
                    note(&spec[..end]);
                }
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
            // A name that is not a runtime module (a hand-written binding's typo)
            // emits nothing; `tsc` reports the missing file.
            let Some((_, src)) = RUNTIME_MODULES.iter().find(|(n, _)| *n == m) else {
                done.remove(m);
                continue;
            };
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
