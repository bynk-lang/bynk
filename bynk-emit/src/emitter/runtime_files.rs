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

use super::runtime_modules::{RUNTIME_EXPORTS, RUNTIME_MODULES, RUNTIME_TYPE_ONLY};

/// The directory (at the root of the output) the runtime modules are emitted to.
pub const RUNTIME_DIR: &str = "__bynk_runtime";

/// The module that defines the exported runtime name `name`.
fn module_of(name: &str) -> Option<&'static str> {
    RUNTIME_EXPORTS
        .binary_search_by_key(&name, |(n, _)| n)
        .ok()
        .map(|i| RUNTIME_EXPORTS[i].1)
}

/// Every name a runtime module exports. The import-shadow test reads it to find a runtime name
/// a module spells in text its tree cannot see.
pub fn runtime_export_names() -> impl Iterator<Item = &'static str> {
    RUNTIME_EXPORTS.iter().map(|(name, _)| *name)
}

/// Whether `name` is a name a runtime module exports.
pub(crate) fn is_runtime_export(name: &str) -> bool {
    module_of(name).is_some()
}

/// Whether the runtime exports `name` as a type and nothing else, so that an
/// import of it is a `type` import. A name that is also a value (`HttpResult`,
/// a const beside its type) is not: one bare binding serves both.
pub(crate) fn is_type_only_export(name: &str) -> bool {
    RUNTIME_TYPE_ONLY.binary_search(&name).is_ok()
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
            TsDecl::ImportNamespace { .. }
            | TsDecl::ImportDefault { .. }
            | TsDecl::ReExport { .. }
            | TsDecl::ExportNames { .. }
            | TsDecl::ReExportAll { .. }
            | TsDecl::Export(_)
            | TsDecl::Interface { .. }
            | TsDecl::ConstDecl { .. }
            | TsDecl::Class { .. }
            | TsDecl::Function { .. }
            | TsDecl::TypeAlias { .. }
            | TsDecl::ExportDefault(_)
            | TsDecl::DeclareConst { .. } => {
                unreachable!("split_runtime_import yields named imports")
            }
        })
        .collect();
    if let (false, Some(last)) = (comment.is_empty(), lines.last_mut()) {
        last.push(' ');
        last.push_str(comment);
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
pub fn reached_in(program: &bynk_ts::TsProgram, out: &mut BTreeSet<(String, String)>) {
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
pub fn closure_files(reached: &BTreeSet<(String, String)>) -> Vec<(PathBuf, String)> {
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
                if let Some(dep) = imported_module(m, line) {
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

/// The runtime module an `import … from "<relative>.ts";` line of `module`'s source names, resolved
/// against the module's own directory: `./result.ts` from `storage`, and `../storage.ts` from
/// `testing/bindings`.
fn imported_module(module: &str, line: &str) -> Option<&'static str> {
    let spec = line
        .rsplit_once('"')
        .and_then(|(head, _)| head.rsplit_once('"'))
        .map(|(_, spec)| spec)?;
    let spec = spec.strip_suffix(".ts")?;
    let mut parts: Vec<&str> = module.split('/').collect();
    parts.pop();
    for seg in spec.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    let resolved = parts.join("/");
    RUNTIME_MODULES
        .iter()
        .find(|(n, _)| *n == resolved)
        .map(|(n, _)| *n)
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

/// `split_runtime_import` as top-level statements, for producers building a
/// `TsProgram`.
pub fn runtime_import_stmts(type_only: bool, names: &[String], from: &str) -> Vec<bynk_ts::TsStmt> {
    split_runtime_import(type_only, names, from)
        .into_iter()
        .map(|d| bynk_ts::TsStmt::decl(d, None))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::runtime_modules;
    use super::*;

    /// Every relative import in a per-module source names another module in the
    /// table, so any reachable subset can be closed over its own imports.
    #[test]
    fn per_module_runtime_imports_resolve_within_the_table() {
        let names: Vec<&str> = runtime_modules::RUNTIME_MODULES
            .iter()
            .map(|(n, _)| *n)
            .collect();
        for (name, src) in runtime_modules::RUNTIME_MODULES {
            for line in src.lines().filter(|l| {
                (l.starts_with("import ") || l.starts_with("} from ")) && l.contains('"')
            }) {
                let from = line.rsplit('"').nth(1).expect("import specifier");
                // ADR 0799 C: a module may import the first-party npm package
                // `bynk-std` declares for it, and nothing else outside the table.
                if bynk_check::firstparty::FIRST_PARTY_NPM.iter().any(|d| {
                    // Hyperdrive H5 (#3322): or a subpath of it
                    // (`mysql2/promise`, mysql2's promise API).
                    d.runtime_module == *name
                        && (d.package == from
                            || from
                                .strip_prefix(d.package)
                                .is_some_and(|r| r.starts_with('/')))
                }) {
                    continue;
                }
                // Resolved against the module's own directory: `./result.ts` from `storage`,
                // `../storage.ts` from `testing/bindings`.
                let mut parts: Vec<&str> = name.split('/').collect();
                parts.pop();
                let target = from.strip_suffix(".ts").map(|f| {
                    for seg in f.split('/') {
                        match seg {
                            "." => {}
                            ".." => {
                                parts.pop();
                            }
                            seg => parts.push(seg),
                        }
                    }
                    parts.join("/")
                });
                assert!(
                    target.is_some_and(|t| names.contains(&t.as_str()) && t != *name),
                    "{name}: import {from:?} does not name another runtime module"
                );
            }
        }
    }

    /// ADR 0799 C: each first-party npm pin is exact and is the version the
    /// runtime's own tests and type-check run against (`runtime/package.json`),
    /// so what a project installs is what was tested, and the module it names
    /// really imports the package.
    #[test]
    fn first_party_pins_match_the_runtime() {
        let manifest =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/package.json"))
                .expect("runtime/package.json");
        for d in bynk_check::firstparty::FIRST_PARTY_NPM {
            assert!(
                manifest.contains(&format!("\"{}\": \"{}\"", d.package, d.version)),
                "`{}` is pinned at {} in bynk-std but not in runtime/package.json",
                d.package,
                d.version
            );
            let (_, src) = runtime_modules::RUNTIME_MODULES
                .iter()
                .find(|(n, _)| *n == d.runtime_module)
                .expect("the runtime module exists");
            assert!(
                src.contains(&format!("from \"{}\"", d.package))
                    || src.contains(&format!("from \"{}/", d.package)),
                "runtime module `{}` does not import `{}`",
                d.runtime_module,
                d.package
            );
        }
    }

    /// The `__` aliases emitted code imports live in the module that defines
    /// each name, so a per-module import line finds them there.
    #[test]
    fn runtime_import_splits_per_defining_module() {
        let names: Vec<String> = ["Ok", "type Result", "__callService", "type __JsonValue"]
            .map(String::from)
            .to_vec();
        let got: Vec<(bool, Vec<String>, String)> =
            split_runtime_import(false, &names, "../runtime.js")
                .into_iter()
                .map(|d| match d {
                    bynk_ts::TsDecl::Import {
                        type_only,
                        names,
                        from,
                    } => (type_only, names, from),
                    bynk_ts::TsDecl::ImportNamespace { .. }
                    | bynk_ts::TsDecl::ImportDefault { .. }
                    | bynk_ts::TsDecl::ReExport { .. }
                    | bynk_ts::TsDecl::ExportNames { .. }
                    | bynk_ts::TsDecl::ReExportAll { .. }
                    | bynk_ts::TsDecl::Export(_)
                    | bynk_ts::TsDecl::Interface { .. }
                    | bynk_ts::TsDecl::ConstDecl { .. }
                    | bynk_ts::TsDecl::Class { .. }
                    | bynk_ts::TsDecl::Function { .. }
                    | bynk_ts::TsDecl::TypeAlias { .. }
                    | bynk_ts::TsDecl::ExportDefault(_)
                    | bynk_ts::TsDecl::DeclareConst { .. } => unreachable!(),
                })
                .collect();
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            got,
            vec![
                (
                    false,
                    v(&["Ok", "type Result"]),
                    "../__bynk_runtime/result.js".to_string()
                ),
                (
                    false,
                    v(&["__callService", "type __JsonValue"]),
                    "../__bynk_runtime/boundary.js".to_string()
                ),
            ]
        );
    }

    /// #2443: `HttpResult` lives apart from the HTTP response helpers, so a
    /// bundle-target service that only names it does not reach `http` or
    /// (through its type edges) `boundary`.
    #[test]
    fn http_result_is_reached_without_the_http_helpers() {
        use std::collections::BTreeSet;
        let names = vec!["HttpResult".to_string()];
        let decls = split_runtime_import(false, &names, "./runtime.js");
        assert!(matches!(
            &decls[..],
            [bynk_ts::TsDecl::Import { from, .. }] if from == "./__bynk_runtime/httpresult.js"
        ));
        let reached: BTreeSet<(String, String)> =
            [("httpresult".to_string(), "js".to_string())].into();
        let paths: Vec<String> = closure_files(&reached)
            .iter()
            .map(|(p, _)| p.display().to_string())
            .collect();
        assert_eq!(paths, ["__bynk_runtime/httpresult.ts"]);
    }

    #[test]
    fn runtime_files_close_over_module_imports() {
        use std::collections::BTreeSet;
        let reached: BTreeSet<(String, String)> =
            [("equality".to_string(), "js".to_string())].into();
        let files = closure_files(&reached);
        let paths: Vec<String> = files.iter().map(|(p, _)| p.display().to_string()).collect();
        // equality -> bytes -> result, and equality -> decimal (#2622).
        assert_eq!(
            paths,
            [
                "__bynk_runtime/bytes.ts",
                "__bynk_runtime/decimal.ts",
                "__bynk_runtime/equality.ts",
                "__bynk_runtime/result.ts"
            ]
        );
        assert!(
            files.iter().all(|(_, s)| !s.contains(".ts\";")),
            "siblings are spelled for the target extension"
        );
        assert!(closure_files(&BTreeSet::new()).is_empty());
    }

    #[test]
    fn handwritten_runtime_imports_are_rewritten() {
        let src = "import { None, Some, type Option, type KVNamespace } from \"../runtime.js\";\nimport { x } from \"./other.js\";\n";
        let out = rewrite_runtime_imports(src);
        assert!(
            out.contains(
                "import { None, Some, type Option } from \"../__bynk_runtime/result.js\";"
            ),
            "{out}"
        );
        assert!(
            out.contains("import type { KVNamespace } from \"../__bynk_runtime/storage.js\";"),
            "{out}"
        );
        assert!(out.contains("import { x } from \"./other.js\";"), "{out}");
    }

    #[test]
    fn handwritten_runtime_import_forms_are_handled_or_left_alone() {
        let r = rewrite_runtime_imports;
        // Multi-line, unterminated, indented, trailing comment, CRLF.
        let multi = "import {\n  Ok,\n  Err,\n} from \"./runtime.js\";\nconst x = 1;\n";
        assert_eq!(
            r(multi),
            "import { Ok, Err } from \"./__bynk_runtime/result.js\";\nconst x = 1;\n"
        );
        assert_eq!(
            r("  import { Ok } from \"./runtime.js\" // keep\n"),
            "  import { Ok } from \"./__bynk_runtime/result.js\"; // keep\n"
        );
        assert_eq!(
            r("import { Ok } from \"./runtime.js\"\r\nx\r\n"),
            "import { Ok } from \"./__bynk_runtime/result.js\";\r\nx\r\n"
        );
        // An unknown name, a namespace import and a foreign specifier stay as written.
        for keep in [
            "import { Ok, Reslt } from \"./runtime.js\";\n",
            "import * as rt from \"./runtime.js\";\n",
            "import { a } from \"./lib/other.js\";\n",
        ] {
            assert_eq!(r(keep), keep);
        }
    }

    #[test]
    fn an_unknown_runtime_module_emits_nothing_instead_of_panicking() {
        use std::collections::BTreeSet;
        let reached: BTreeSet<(String, String)> = [("nope".to_string(), "js".to_string())].into();
        assert!(closure_files(&reached).is_empty());
    }

    #[test]
    fn per_module_runtime_carries_its_aliases() {
        let all: String = runtime_modules::RUNTIME_MODULES
            .iter()
            .map(|(_, s)| *s)
            .collect();
        for alias in [
            "__callService",
            "__matchPath",
            "__makeAgent",
            "__JsonValue",
            "__formatIcuDate",
        ] {
            assert!(
                all.contains(alias),
                "{alias} missing from the per-module runtime"
            );
        }
        let (_, http) = runtime_modules::RUNTIME_MODULES
            .iter()
            .find(|(n, _)| *n == "http")
            .unwrap();
        assert!(http.contains("matchPath as __matchPath"));
    }

    #[test]
    fn runtime_emits_all_required_exports() {
        let s: String = emit_all_runtime_modules()
            .into_iter()
            .map(|(_, src)| src)
            .collect();
        // Core types and constructors used by every emitted module.
        assert!(s.contains("export type Result<T, E>"));
        assert!(s.contains("export const Ok"));
        assert!(s.contains("export const Err"));
        assert!(s.contains("export type Option<T>"));
        assert!(s.contains("export const Some"));
        assert!(s.contains("export const None"));
        assert!(s.contains("export interface ValidationError"));
        // Durable Object surface used by agent classes.
        assert!(s.contains("export interface DurableObjectStorage"));
        assert!(s.contains("export interface DurableObjectState"));
        assert!(s.contains("export class InMemoryStorage"));
        assert!(s.contains("export function makeTestState"));
        // Discriminator must be `tag` to match emitted code.
        assert!(s.contains("tag: \"Ok\""));
        assert!(s.contains("tag: \"Err\""));
        assert!(s.contains("tag: \"Some\""));
        assert!(s.contains("tag: \"None\""));
    }
}
