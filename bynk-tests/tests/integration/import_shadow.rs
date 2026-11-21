//! 1A.36a (#3737, Decision E): the shadow check for tree-derived imports.
//!
//! For every module the positive corpus emits, on both targets, this computes
//! the import set the built tree asks for ([`bynk_ts::unbound_names`] over the
//! module with its structured imports removed) and compares it with the names
//! the module actually imports, which today's predicates chose before the body
//! existed. Nothing here changes output: it is the evidence 1A.36b's derivation
//! is checked against.
//!
//! * The tree set must be a **subset** of the imported set. A name the tree uses
//!   and the module does not import is an under-import; the only one on record
//!   is [`KNOWN_UNDER_IMPORTS`] (#3807, the same defect `import_closure.rs`
//!   lists).
//! * Where the tree set is a **strict** subset, the module over-imports, and the
//!   extra names are the unused imports the prediction adds. Each such name is
//!   classed by whether it still appears as a whole word in the module's
//!   printed body: a name that does may be used by text the tree cannot see
//!   (a `Verbatim` or `Raw` statement, which `unbound_names` does not read), so
//!   dropping it needs a look; a name that does not is plainly unused.
//!
//! The report is printed to stderr (run with `--nocapture`), and written to the
//! file `BYNK_IMPORT_SHADOW_REPORT` names when that is set.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_driver::project::{BuildTarget, Document};
use bynk_ts::TsProgram;

#[path = "../par/mod.rs"]
mod par;

/// `(source, target, module, name)`: a name the tree uses that the module does not
/// import. Empty since 1A.36b-2: the imports are the tree's, so none can be missing; an entry
/// would be a defect to report, with its issue. (#3807's `lib` was the last.)
const KNOWN_UNDER_IMPORTS: &[(&str, &str, &str, &str)] = &[];

/// `(name, reason)`: a runtime export a derived module may spell as code without importing it,
/// because the spelling is not a use of the export. `words` reads identifiers, not scopes.
const UNIMPORTED_RUNTIME_OK: &[(&str, &str)] = &[
    (
        "invariantViolation",
        "a property key in a structural cast: `(e as { invariantViolation?: unknown })`",
    ),
    (
        "signature",
        "a parameter name in a binding module's signature, not the runtime's `signature`",
    ),
];

fn target_name(target: BuildTarget) -> &'static str {
    match target {
        BuildTarget::Workers => "workers",
        _ => "bundle",
    }
}

/// The identifiers `text` spells as code: string and comment contents are
/// skipped (a template literal's `${…}` is code), and so is a member name after
/// a `.`. What remains is every name TypeScript could read as a binding.
fn words(text: &str) -> BTreeSet<&str> {
    let bytes = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    let mut out = BTreeSet::new();
    // A stack of open template literals' `${` depths: the brace depth at which
    // each one resumes template text.
    let mut templates: Vec<usize> = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' | b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'`' => {
                i += 1;
                i = skip_template(bytes, i, &mut templates, &mut depth);
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                if templates.last() == Some(&depth) {
                    templates.pop();
                    depth -= 1;
                    i = skip_template(bytes, i + 1, &mut templates, &mut depth);
                } else {
                    depth = depth.saturating_sub(1);
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            _ if ident(b) => {
                let start = i;
                while i < bytes.len() && ident(bytes[i]) {
                    i += 1;
                }
                if start == 0 || bytes[start - 1] != b'.' {
                    out.insert(&text[start..i]);
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Skip template-literal text from `i` to its closing backtick or its next
/// `${`, which opens a brace level whose matching `}` resumes the text.
fn skip_template(
    bytes: &[u8],
    mut i: usize,
    templates: &mut Vec<usize>,
    depth: &mut usize,
) -> usize {
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'`' => return i + 1,
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                *depth += 1;
                templates.push(*depth);
                return i + 2;
            }
            _ => i += 1,
        }
    }
    i
}

/// One module's comparison: what the tree needs, what is imported, and the
/// split of the surplus.
#[derive(Default)]
struct Shadow {
    under: Vec<String>,
    /// Imported, unused by the tree, and absent from the printed body.
    plainly_unused: Vec<String>,
    /// Imported, unused by the tree, but a word in the printed body: text the
    /// tree cannot see may use it.
    in_text: Vec<String>,
    /// For each `in_text` name, the first printed body line that spells it.
    spelled: Vec<String>,
    /// The names the module imports from the runtime (`__bynk_runtime/…`).
    from_runtime: BTreeSet<String>,
    /// Runtime exports the printed body spells as code that the module neither imports nor
    /// declares: text the tree cannot see, using a name nothing imported (1A.36b-2).
    unimported_runtime: Vec<String>,
}

fn compare(program: &TsProgram) -> Shadow {
    let body = program.without_imports();
    let imported = program.imported_names();
    // A name the module imports is its own use even where it shares a global's
    // name (`Request`, `Response` of `bynk`): the globals must not hide it (1A.36b-1).
    let tree: BTreeSet<String> = bynk_ts::unbound_names_providing(&body, |n| imported.contains(n))
        .into_iter()
        .collect();
    let printed: String = body
        .stmts
        .iter()
        .map(|s| bynk_ts::print_stmt(s, 0))
        .collect::<Vec<_>>()
        .join("\n");
    let in_body = words(&printed);
    let mut shadow = Shadow {
        under: tree.difference(&imported).cloned().collect(),
        from_runtime: program.names_imported_from(|from| from.contains("__bynk_runtime/")),
        ..Shadow::default()
    };
    let runtime: BTreeSet<&str> =
        bynk_emit::emitter::runtime_files::runtime_export_names().collect();
    let declared = declared_names(&printed);
    shadow.unimported_runtime = in_body
        .iter()
        .filter(|w| runtime.contains(**w) && !imported.contains(**w) && !declared.contains(**w))
        .map(|w| w.to_string())
        .collect();
    for name in imported.difference(&tree) {
        if in_body.contains(name.as_str()) {
            shadow.in_text.push(name.clone());
            let line = printed
                .lines()
                .find(|l| words(l).contains(name.as_str()))
                .unwrap_or_default();
            shadow.spelled.push(format!(
                "{name}: {}",
                line.trim().chars().take(110).collect::<String>()
            ));
        } else {
            shadow.plainly_unused.push(name.clone());
        }
    }
    shadow
}

/// The names `printed` declares or imports in text (`function f`, `const f`, `const { a, b } =`,
/// `type F`, `import { X as Y } from`): a spelled runtime name the module binds itself is not an
/// under-import. Text that still carries its own `import` line (an adapter's binding module)
/// binds the names that way.
fn declared_names(printed: &str) -> BTreeSet<&str> {
    let split = |t| {
        let line: &str = t;
        line.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
    };
    let mut out = BTreeSet::new();
    for line in printed.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("import ") || trimmed.starts_with("export {") {
            let head = trimmed.split(" from ").next().unwrap_or(trimmed);
            out.extend(split(head));
            continue;
        }
        let words = split(line);
        let mut ws = words.iter().copied();
        while let Some(w) = ws.next() {
            if matches!(
                w,
                "function" | "const" | "let" | "var" | "class" | "type" | "interface" | "enum"
            ) {
                if let Some(n) = ws.next() {
                    out.insert(n);
                }
                // A destructuring pattern binds every name inside the braces.
                if let Some(open) = line.find(&format!("{w} {{")) {
                    let inner = line[open..].split('}').next().unwrap_or("");
                    out.extend(split(inner));
                }
            }
        }
    }
    out
}

type Key = (String, &'static str, String);

fn record(
    out: &mut BTreeMap<Key, Shadow>,
    source: &str,
    target: BuildTarget,
    project: &bynk_driver::project::ProjectOutput,
) -> usize {
    let mut modules = 0;
    for (path, doc) in &project.artefacts.docs {
        if let Document::Ts(program) = doc {
            modules += 1;
            let shadow = compare(program);
            if !(shadow.under.is_empty()
                && shadow.plainly_unused.is_empty()
                && shadow.in_text.is_empty()
                && shadow.unimported_runtime.is_empty())
            {
                let module = path
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert((source.to_string(), target_name(target), module), shadow);
            }
        }
    }
    modules
}

/// Compiles a project fixture for `target`; `None` where the target refuses the
/// fixture (a host-supplied capability on Workers), which `import_closure.rs`
/// names and checks.
fn compile(dir: &Path, target: BuildTarget) -> Option<bynk_driver::project::ProjectOutput> {
    let options = if dir.join("bynk.toml").exists() {
        let paths = bynk_driver::project::try_read_project_paths(dir)
            .expect("well-formed fixture manifest");
        bynk_testkit::compile_options_split(dir.to_path_buf(), paths)
    } else {
        bynk_testkit::compile_options_single(dir.join("src"))
    };
    bynk_driver::project::compile_project(&options.target(target)).ok()
}

#[test]
fn the_tree_derived_imports_are_a_subset_of_the_predicted_ones() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
        .expect("positive fixtures")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    let per_fixture = par::par_map(&dirs, |dir| {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let mut found = BTreeMap::new();
        let mut modules = 0;
        if dir.join("src").is_dir() {
            for target in [BuildTarget::Bundle, BuildTarget::Workers] {
                match compile(dir, target) {
                    Some(out) => modules += record(&mut found, &name, target, &out),
                    // Only the Workers target may refuse a positive fixture (a host-supplied
                    // capability); a Bundle failure would silently shrink the subset check.
                    None if target == BuildTarget::Workers => {}
                    None => panic!("{name} must compile for the Bundle target"),
                }
            }
        } else {
            let source = fs::read_to_string(dir.join("input.bynk")).expect("input.bynk");
            let out = bynk_driver::project::compile_in_memory(
                &source,
                BuildTarget::Bundle,
                Default::default(),
            )
            .unwrap_or_else(|_| panic!("{name} must compile"));
            modules += record(&mut found, &name, BuildTarget::Bundle, &out);
        }
        (found, modules)
    });
    let mut found: BTreeMap<Key, Shadow> = BTreeMap::new();
    let mut modules = 0;
    for (f, m) in per_fixture {
        found.extend(f);
        modules += m;
    }
    assert!(modules > 1000, "only {modules} modules checked");

    // The subset claim: no module's tree asks for a name it does not import,
    // bar the one recorded defect.
    let under: Vec<String> = found
        .iter()
        .flat_map(|((s, t, m), sh)| {
            sh.under
                .iter()
                .filter(move |n| {
                    !KNOWN_UNDER_IMPORTS.contains(&(s.as_str(), t, m.as_str(), n.as_str()))
                })
                .map(move |n| format!("  {s} [{t}] {m}: {n}"))
        })
        .collect();
    let known_gone: Vec<String> = KNOWN_UNDER_IMPORTS
        .iter()
        .filter(|(s, t, m, n)| {
            !found
                .get(&(s.to_string(), *t, m.to_string()))
                .is_some_and(|sh| sh.under.iter().any(|u| u == n))
        })
        .map(|(s, t, m, n)| format!("  {s} [{t}] {m}: {n}"))
        .collect();
    assert!(
        under.is_empty() && known_gone.is_empty(),
        "{modules} modules checked.\nThe tree uses a name the module does not import:\n{}\n\
         Known under-imports that no longer occur (delete their entry):\n{}",
        under.join("\n"),
        known_gone.join("\n"),
    );

    // 1A.36b-1: a module whose header the emitter derives from its tree imports no
    // runtime name it does not use. What still over-imports is predicted elsewhere: the
    // driver's `tests/*.test.ts` modules (1B.2) and the Workers' `index.ts` and
    // `compose.ts`, which `workers_entry.rs` and `workers.rs` build.
    let over_imported: Vec<String> = found
        .iter()
        .filter(|((_, _, m), _)| !predicts_its_runtime_imports(m))
        .flat_map(|((s, t, m), sh)| {
            sh.plainly_unused
                .iter()
                .filter(|n| sh.from_runtime.contains(*n))
                .map(move |n| format!("  {s} [{t}] {m}: {n}"))
        })
        .collect();
    assert!(
        over_imported.is_empty(),
        "a module with a derived header imports a runtime name its tree does not use:\n{}",
        over_imported.join("\n"),
    );

    // 1A.36b-2: the under-import net for derived modules. The tree derives the header, so a
    // runtime name that only text spells (`Verbatim`, a `Raw` without `raw_using`) would go
    // unimported and nothing else reports it: no emitter-built module (every module bar the
    // driver's test modules and the runtime's own files) spells a runtime export it does not
    // import. It runs over the Worker `index.ts` and `compose.ts` too, before and after their
    // headers are derived.
    let unimported: Vec<String> = found
        .iter()
        .filter(|((_, _, m), _)| !is_driver_test_module(m) && !m.starts_with("__bynk_runtime/"))
        .flat_map(|((s, t, m), sh)| {
            sh.unimported_runtime
                .iter()
                .filter(|n| !UNIMPORTED_RUNTIME_OK.iter().any(|(ok, _)| ok == n))
                .map(move |n| format!("  {s} [{t}] {m}: {n}"))
        })
        .collect();
    assert!(
        unimported.is_empty(),
        "a module with a derived header spells a runtime export it does not import \
         (text the tree cannot see):\n{}",
        unimported.join("\n"),
    );

    let report = report(modules, &found);
    eprintln!("{report}");
    if let Ok(path) = std::env::var("BYNK_IMPORT_SHADOW_REPORT") {
        fs::write(path, &report).expect("write the shadow report");
    }
}

/// A module the driver generates for `test` and `integration test` units
/// (`tests/<unit>.test.ts`): opaque test text, whose imports stay predicted until 1B.2.
fn is_driver_test_module(module: &str) -> bool {
    module.starts_with("tests/") && module.ends_with(".test.ts")
}

/// Whether a module's runtime import is still predicted rather than derived: the driver's test
/// modules, and the Worker entry (`index.ts`) and composition root (`compose.ts`), whose import
/// lists `workers_entry.rs` and `workers.rs` assemble from `RuntimeUse`.
fn predicts_its_runtime_imports(module: &str) -> bool {
    is_driver_test_module(module)
        || module
            .rsplit('/')
            .next()
            .is_some_and(|f| f == "index.ts" || f == "compose.ts")
}

/// The over-import findings, grouped by name.
fn report(modules: usize, found: &BTreeMap<Key, Shadow>) -> String {
    let over: Vec<(&Key, &Shadow)> = found
        .iter()
        .filter(|(_, s)| !s.plainly_unused.is_empty() || !s.in_text.is_empty())
        .collect();
    let mut plain: BTreeMap<&str, usize> = BTreeMap::new();
    let mut text: BTreeMap<&str, usize> = BTreeMap::new();
    let mut text_outside_tests: BTreeMap<&str, usize> = BTreeMap::new();
    for ((_, _, module), s) in &over {
        for n in &s.plainly_unused {
            *plain.entry(n).or_default() += 1;
        }
        for n in &s.in_text {
            *text.entry(n).or_default() += 1;
            if !is_driver_test_module(module) {
                *text_outside_tests.entry(n).or_default() += 1;
            }
        }
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "import shadow: {modules} modules, {} over-import ({} plainly unused name \
         occurrences, {} names the printed body still spells)",
        over.len(),
        plain.values().sum::<usize>(),
        text.values().sum::<usize>(),
    );
    let _ = writeln!(out, "plainly unused, by name (modules):");
    for (n, c) in &plain {
        let _ = writeln!(out, "  {n}: {c}");
    }
    let _ = writeln!(
        out,
        "imported, unused by the tree, spelled in the body (modules):"
    );
    for (n, c) in &text {
        let _ = writeln!(out, "  {n}: {c}");
    }
    let _ = writeln!(
        out,
        "of which outside the driver's tests/*.test.ts modules ({} occurrences):",
        text_outside_tests.values().sum::<usize>(),
    );
    for (n, c) in &text_outside_tests {
        let _ = writeln!(out, "  {n}: {c}");
    }
    let _ = writeln!(out, "per module:");
    for ((s, t, m), sh) in over {
        let _ = writeln!(
            out,
            "  {s} [{t}] {m}: unused {:?}; in text {:?}",
            sh.plainly_unused, sh.in_text
        );
        for line in &sh.spelled {
            let _ = writeln!(out, "      {line}");
        }
    }
    out
}

#[test]
fn words_reads_code_and_skips_strings_comments_and_members() {
    let w = words(
        "const a = Some(x.None) + \"Err\" + 'Ok' /* Result */ + `t ${Option} Request` // Response\n\
         foo(bar)",
    );
    let want: BTreeSet<&str> = ["const", "a", "Some", "x", "Option", "foo", "bar"].into();
    assert_eq!(w, want);
}
