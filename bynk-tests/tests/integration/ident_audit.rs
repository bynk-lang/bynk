//! The identifier audit (1A.39b, #3930 Decision D).
//!
//! For every module the positive corpus emits, on both targets, every `TsExpr::Ident` is an
//! identifier or a dotted identifier chain (`this.state`, `globalThis.Number`) and every
//! `TsBindingName::Ident` is an identifier ([`bynk_ts::text_idents`]). An `Ident` that holds
//! expression text hides the names it uses from `name_closure` and the structure it spells from
//! the printer, whatever built it; the `ts_text_idents` line probe cannot see an `Ident(` fed from
//! a variable or a helper, and this test does. It changes no output.
//!
//! Every module the emitter or the driver builds is covered bar the driver's `tests/*.test.ts`
//! modules, whose text stays predicted until 1B.2 and is 1B.5's. What is still text is accounted by
//! class in [`TEXT_CLASSES`], each with a reason and an owner; the list can only shrink.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_driver::project::{BuildTarget, Document};

#[path = "../par/mod.rs"]
mod par;

/// One class of expression text that an `Ident` still holds. Every find must match a class, and
/// every class must still occur: the list can only shrink (a class that no longer occurs is
/// deleted, and a find outside every class fails). A class is structural, a named shape and not
/// "anything".
struct TextClass {
    name: &'static str,
    matches: fn(&str) -> bool,
    reason: &'static str,
    owner: &'static str,
}

const OWNER: &str = "1A.40";

const TEXT_CLASSES: &[TextClass] = &[
    TextClass {
        name: "subscript",
        matches: is_subscript,
        reason: "`obj[\"f\"]` / `argsObj[\"f\"]`: the `json: &str` threaded through the serialise family",
        owner: OWNER,
    },
    TextClass {
        name: "template path",
        matches: is_template_path,
        reason: "`${path}.f` / `${path}[${i}]`: the `path_expr: &str` threaded through the serialise family",
        owner: OWNER,
    },
    TextClass {
        name: "rendered literal",
        matches: is_rendered_literal,
        reason: "a pre-rendered default, zero or wire literal (`\"\"`, `0`, `[]`, `{ kind: \"None\" }`)",
        owner: OWNER,
    },
    TextClass {
        name: "events fan-out block",
        matches: is_fanout_block,
        reason: "the events fan-out handler's block text (`{ for (const ev of events) { .. } }`, `{  }`)",
        owner: OWNER,
    },
    TextClass {
        name: "predicate, call or cast text",
        matches: is_rendered_expression,
        reason: "a rendered predicate, call, comparison or cast (`!(..)`, `f(x)`, `a === b`, `(x as T)`)",
        owner: OWNER,
    },
];

fn is_identifier(t: &str) -> bool {
    let mut c = t.chars();
    c.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// `<identifier>["<name>"]`.
fn is_subscript(t: &str) -> bool {
    t.split_once("[\"").is_some_and(|(head, rest)| {
        is_identifier(head)
            && rest
                .strip_suffix("\"]")
                .is_some_and(|k| !k.contains('"') && !k.contains('\\'))
    })
}

/// A template literal made of `${path}`, `.name`, `[${i}]` and `[<digits>]` only.
fn is_template_path(t: &str) -> bool {
    let Some(inner) = t.strip_prefix('`').and_then(|t| t.strip_suffix('`')) else {
        return false;
    };
    let Some(mut rest) = inner.strip_prefix("${path}") else {
        return false;
    };
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("[${i}]") {
            rest = r;
        } else if let Some(r) = rest.strip_prefix('[') {
            let digits = r.chars().take_while(char::is_ascii_digit).count();
            match r[digits..].strip_prefix(']') {
                Some(r2) if digits > 0 => rest = r2,
                _ => return false,
            }
        } else if let Some(r) = rest.strip_prefix('.') {
            let n = r
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                .count();
            if n == 0 {
                return false;
            }
            rest = &r[n..];
        } else {
            return false;
        }
    }
    true
}

/// A literal: strings, numbers, arrays and objects of literals. No operator, call or cast.
fn is_rendered_literal(t: &str) -> bool {
    let mut out = String::new();
    let mut chars = t.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            out.push_str("\"\"");
            loop {
                match chars.next() {
                    Some('\\') => {
                        chars.next();
                    }
                    Some('"') | None => break,
                    Some(_) => {}
                }
            }
        } else {
            out.push(c);
        }
    }
    let out = out.trim();
    if out.is_empty() || out.contains("for ") {
        return false;
    }
    let bytes = out.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'$'))
            {
                i += 1;
            }
            let word = &out[start..i];
            // A bare word is an object key (`kind:`), or a `null`/`true`/`false` literal.
            let follows_colon = out[i..].trim_start().starts_with(':');
            if !follows_colon && !matches!(word, "null" | "true" | "false" | "undefined") {
                return false;
            }
        } else if matches!(
            b,
            b'"' | b'[' | b']' | b'{' | b'}' | b':' | b',' | b' ' | b'-' | b'.'
        ) || b.is_ascii_digit()
        {
            i += 1;
        } else {
            return false;
        }
    }
    true
}

fn is_fanout_block(t: &str) -> bool {
    t == "{  }" || t.starts_with("{ for (const ev of events) {")
}

/// Expression text that is not a literal: it carries an operator, a call, a cast or a ternary.
fn is_rendered_expression(t: &str) -> bool {
    !is_rendered_literal(t)
        && !is_fanout_block(t)
        && !is_subscript(t)
        && !is_template_path(t)
        && ["(", " === ", " >= ", " + ", " as ", " ? ", " => "]
            .iter()
            .any(|n| t.contains(n))
}

fn target_name(target: BuildTarget) -> &'static str {
    match target {
        BuildTarget::Workers => "workers",
        _ => "bundle",
    }
}

/// A module the driver generates for `test` and `integration test` units: opaque test text.
fn is_driver_test_module(module: &str) -> bool {
    module.starts_with("tests/") && module.ends_with(".test.ts")
}

fn record(
    out: &mut Vec<(String, String)>,
    source: &str,
    target: BuildTarget,
    project: &bynk_driver::project::ProjectOutput,
) -> usize {
    let mut modules = 0;
    for (path, doc) in &project.artefacts.docs {
        let Document::Ts(program) = doc else { continue };
        let module = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if is_driver_test_module(&module) {
            continue;
        }
        modules += 1;
        for text in bynk_ts::text_idents(program) {
            out.push((format!("{source} [{}] {module}", target_name(target)), text));
        }
    }
    modules
}

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
fn every_ident_in_the_built_trees_is_an_identifier_path() {
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
        let mut found = Vec::new();
        let mut modules = 0;
        if dir.join("src").is_dir() {
            for target in [BuildTarget::Bundle, BuildTarget::Workers] {
                match compile(dir, target) {
                    Some(out) => modules += record(&mut found, &name, target, &out),
                    // Only the Workers target may refuse a positive fixture (a host-supplied
                    // capability); a Bundle failure would silently shrink the audit.
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
    let mut found: Vec<(String, String)> = Vec::new();
    let mut modules = 0;
    for (f, m) in per_fixture {
        found.extend(f);
        modules += m;
    }
    assert!(modules > 1000, "only {modules} modules checked");

    // Classify each find by the first class that matches; count occurrences (one per module and
    // text) and distinct texts per class.
    let mut counts = vec![0usize; TEXT_CLASSES.len()];
    let mut distinct: Vec<BTreeMap<&str, usize>> = vec![BTreeMap::new(); TEXT_CLASSES.len()];
    let mut unmatched: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    for (location, text) in &found {
        match TEXT_CLASSES.iter().position(|c| (c.matches)(text)) {
            Some(i) => {
                counts[i] += 1;
                *distinct[i].entry(text.as_str()).or_default() += 1;
            }
            None => {
                unmatched
                    .entry(text.as_str())
                    .or_insert((0, location.as_str()))
                    .0 += 1;
            }
        }
    }
    let mut report = format!(
        "ident audit: {modules} modules, {} text idents\n",
        found.len()
    );
    for (i, c) in TEXT_CLASSES.iter().enumerate() {
        report.push_str(&format!(
            "  {:<32} {:>6} occurrences, {:>4} distinct texts (owner {}): {}\n",
            c.name,
            counts[i],
            distinct[i].len(),
            c.owner,
            c.reason
        ));
    }
    eprintln!("{report}");

    assert!(
        unmatched.is_empty(),
        "{modules} modules checked. An `Ident` holds expression text that matches no class in \
         TEXT_CLASSES (convert it to the node it spells, or add a structural class with a reason \
         and an owner):\n{}",
        unmatched
            .iter()
            .take(60)
            .map(|(t, (n, loc))| format!("  {n:>5}  {t}   (first: {loc})"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    // The list can only shrink: a class that no longer occurs is deleted.
    let stale: Vec<&str> = TEXT_CLASSES
        .iter()
        .zip(&counts)
        .filter(|(_, n)| **n == 0)
        .map(|(c, _)| c.name)
        .collect();
    assert!(
        stale.is_empty(),
        "these TEXT_CLASSES no longer occur anywhere; delete their entries: {stale:?}"
    );
}
