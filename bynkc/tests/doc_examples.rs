//! The example-compilation gate.
//!
//! Extracts every fenced ```bynk block from the Book
//! (`site/src/content/docs/book/**`) and the Developer Documentation surface
//! (`site/src/content/docs/docs/**`) plus the landing page
//! (`site/src/content/docs/index.mdx`, which ships the most visible example), the
//! repository README and the LLM primer (`site/public/llms.txt`), and compiles
//! each, so a doc example can never fall out of step with the compiler.
//!
//! Block handling, by the fence's info string and the block's first line
//! (#1661: every block is checked unless it says otherwise):
//! - ```bynk           → must compile:
//!     * a block starting `commons …` is compiled as a single file;
//!     * a block starting `context …` is compiled as a one-file project;
//!     * (a leading doc-block — a module doc above the header — is skipped
//!       when reading the "starting" line, #1885);
//!     * anything else (declarations shown on their own) is compiled inside a
//!       synthetic `commons doc`, or failing that a `context doc`.
//! - ```bynk,fail      → must FAIL to compile (negative examples);
//!   ```bynk,fail=<code> also requires `<code>` among the errors.
//! - ```bynk,fragment  → must **parse**, inside at least one of the wrappers a
//!   partial snippet comes from (a unit, an agent or service body, a function
//!   body, a suite or a test case). For blocks that name types the page
//!   declared elsewhere, or show a single member or statement; parsing still
//!   catches syntax that has drifted from the grammar.
//! - ```bynk,ignore    → skipped. Only for pseudo-syntax that is not Bynk at all
//!   (`<Name>` placeholders, `…` elisions, lines from different contexts set
//!   side by side).
//!
//! The site renders all four alike (it keeps only the language before the
//! comma). Counts are printed so unchecked blocks are never silently assumed
//! good. Run with `--nocapture` to see the summary.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct Block {
    file: String,
    line: usize,
    info: String,
    body: String,
}

fn docs_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site/src/content/docs")
}

/// The prose surfaces whose ```bynk blocks are gated: the Book and the Developer
/// Documentation surface. (By Example is `.mdx` and has its own extraction gate.)
fn doc_surfaces() -> [PathBuf; 2] {
    [docs_root().join("book"), docs_root().join("docs")]
}

fn landing_page() -> PathBuf {
    docs_root().join("index.mdx")
}

fn repo_readme() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../README.md")
}

fn llms_primer() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site/public/llms.txt")
}

fn collect_blocks() -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut files = Vec::new();
    for surface in doc_surfaces() {
        gather_md(&surface, &mut files);
    }
    files.sort();
    // The landing page lives outside the Book but ships the most visible bynk
    // example; gate it too so the front door can't drift from the compiler.
    files.push(landing_page());
    // Same for the repository README — its front-page showcase example had
    // drifted through three syntax revisions before this gate covered it.
    files.push(repo_readme());
    // And the LLM primer (#2464): the file agents are pointed at to learn Bynk,
    // which had drifted to teaching syntax the compiler no longer accepts.
    files.push(llms_primer());
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let rel = file
            .strip_prefix(docs_root())
            .unwrap_or(&file)
            .display()
            .to_string();
        let mut lines = text.lines().enumerate();
        while let Some((idx, line)) = lines.next() {
            let trimmed = line.trim_start();
            // #2461: a four-backtick fence shows fences as text (a doc comment
            // with a fenced example inside it); nothing inside it is a block.
            if trimmed.starts_with("````") {
                let fence = trimmed.trim_end();
                for (_, l) in lines.by_ref() {
                    if l.trim() == fence {
                        break;
                    }
                }
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("```bynk") {
                // `rest` is "" for ```bynk, or ",fail" / ",ignore" etc.
                let info = rest.trim_start_matches(',').to_string();
                let mut body = String::new();
                for (_, l) in lines.by_ref() {
                    if l.trim() == "```" {
                        break;
                    }
                    body.push_str(l);
                    body.push('\n');
                }
                blocks.push(Block {
                    file: rel.clone(),
                    line: idx + 1,
                    info,
                    body,
                });
            }
        }
    }
    blocks
}

fn gather_md(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            gather_md(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

fn first_line(body: &str) -> &str {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

/// #1885: the line that decides how a block is compiled — its first non-blank
/// line that is not trivia, skipping any leading doc-blocks and `--` line
/// comments. A module doc (or a header comment) sits above the unit header, so
/// a block that opens `--- … --- commons x` is a `commons` block, not
/// declarations to wrap in a synthetic unit. Falls back to the first non-blank
/// line when the block is all trivia (or a doc-block is unclosed).
fn header_line(body: &str) -> &str {
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = first_line(body);
    while let Some(l) = lines.next() {
        if l.len() >= 3 && l.bytes().all(|b| b == b'-') {
            // A doc-block marker: skip to the marker that closes it.
            if !lines.by_ref().any(|c| c == l) {
                break;
            }
        } else if !l.starts_with("//") {
            return l;
        }
    }
    first
}

/// #1661 (Decision A): an un-headed block — the declarations a page shows on
/// their own — compiled inside a synthetic unit. A `commons` is tried first; a
/// block declaring context-only items (an agent, a service, a capability) is
/// then tried as a `context`. Either succeeding means the block compiles; the
/// errors reported are the `commons` attempt's unless the `context` one got
/// further.
fn compile_wrapped(body: &str, idx: usize) -> Result<(), String> {
    let as_commons = bynkc::compile(&format!("commons doc\n\n{body}"), "doc.bynk")
        .map(|_| ())
        .map_err(|errs| {
            errs.iter()
                .map(|e| format!("{}: {}", e.category, e.message))
                .collect::<Vec<_>>()
                .join("; ")
        });
    match as_commons {
        Ok(()) => Ok(()),
        Err(commons_err) => {
            compile_context(&format!("context doc\n\n{body}"), idx).map_err(|context_err| {
                format!("as commons: {commons_err} | as context: {context_err}")
            })
        }
    }
}

/// #1661: the wrappers a `fragment` block is parsed in — one per place a page
/// shows a partial snippet from: declarations, a whole unit, an agent or service
/// body (members and handlers), a function body (statements and expressions),
/// a suite, a test case's body, and an adapter body. Each is `(prefix, suffix)` around the block.
const FRAGMENT_WRAPPERS: &[(&str, &str)] = &[
    ("", ""),
    ("commons doc\n\n", ""),
    ("context doc\n\n", ""),
    (
        "context doc\n\nagent Doc {\n  key id: String\n  store docProbeCell: Cell[Int]\n",
        "\n  on call docProbe() -> Effect[()] {\n    ()\n  }\n}\n",
    ),
    ("context doc\n\nservice doc from http {\n", "\n}\n"),
    (
        "context doc\n\nservice doc from queue(\"doc\") {\n",
        "\n}\n",
    ),
    (
        "context doc\n\nservice doc from websocket(in: Doc, out: Doc) {\n",
        "\n}\n",
    ),
    ("commons doc\n\nfn doc() -> Int {\n", "\n}\n"),
    ("suite doc\n\n", ""),
    ("suite doc\n\ncase \"doc\" {\n", "\n}\n"),
    ("adapter doc {\n", "\n}\n"),
];

/// #1661: a `fragment` block cannot compile on its own (it names types the page
/// declared earlier, or is a single member or statement), but it must still be
/// **syntactically** current: it has to parse inside at least one of
/// [`FRAGMENT_WRAPPERS`]. On failure, the error from the wrapper that got
/// furthest into the block is reported, as the likeliest intended reading.
fn parses_as_fragment(body: &str) -> Result<(), String> {
    // A page may show several units in one file (an atomic commons beside its
    // suite), which only the multi-unit parser accepts.
    if let Ok(tokens) = bynk_syntax::lexer::tokenize(body)
        && bynk_syntax::parser::parse_units(&tokens, body).is_ok()
    {
        return Ok(());
    }
    let mut best: Option<(usize, String)> = None;
    for (prefix, suffix) in FRAGMENT_WRAPPERS {
        let src = format!("{prefix}{body}{suffix}");
        let errs = match bynk_syntax::lexer::tokenize(&src) {
            Ok(tokens) => match bynk_syntax::parser::parse_unit(&tokens, &src) {
                Ok(_) => return Ok(()),
                Err(errs) => errs,
            },
            Err(e) => vec![e],
        };
        // A parser that fails must say why; an empty error list is a gate
        // failure, never a silent pass (#1662 review).
        let Some(first) = errs.first() else {
            return Err(format!(
                "{prefix:?} wrapper: parse failed with no diagnostic"
            ));
        };
        let progress = first.span.start.saturating_sub(prefix.len());
        if best.as_ref().is_none_or(|(p, _)| progress > *p) {
            let line = body[..progress.min(body.len())].lines().count().max(1);
            best = Some((
                progress,
                format!("block line {line}: {}: {}", first.category, first.message),
            ));
        }
    }
    Err(best.map(|(_, e)| e).unwrap_or_default())
}

/// Compile a `context …` block as a one-file project under a unique temp dir.
fn compile_context(body: &str, idx: usize) -> Result<(), String> {
    let first = header_line(body);
    let name = first
        .strip_prefix("context")
        .unwrap_or("")
        .trim()
        .trim_end_matches('{')
        .trim();
    if name.is_empty() {
        return Err("could not parse context name".to_string());
    }
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("doc-ctx-{idx}"));
    let _ = fs::remove_dir_all(&root);
    let rel: PathBuf = name.split('.').collect::<PathBuf>().with_extension("bynk");
    let file = root.join(&rel);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, body).unwrap();

    let result = bynkc::compile_project(&bynk_testkit::compile_options_single(root.clone()))
        .map_err(bynkc::ProjectFailure::flatten)
        .map(|_| ())
        .map_err(|errs| {
            errs.iter()
                .map(|e| format!("{}: {}", e.category, e.message))
                .collect::<Vec<_>>()
                .join("; ")
        });
    let _ = fs::remove_dir_all(&root);
    result
}

#[test]
fn every_doc_example_compiles() {
    let blocks = collect_blocks();
    assert!(!blocks.is_empty(), "found no ```bynk blocks under the Book");

    let (mut checked_ok, mut checked_fail, mut skip_ignored, mut parsed_fragment, mut skip_include) =
        (0, 0, 0, 0, 0);
    let mut failures: Vec<String> = Vec::new();

    for (idx, b) in blocks.iter().enumerate() {
        let loc = format!("{}:{} (`{}…`)", b.file, b.line, first_line(&b.body));

        if b.info.contains("ignore") {
            skip_ignored += 1;
            continue;
        }
        // Display-only blocks: a body that is just `{{#include …}}` directive(s)
        // is inlined at site-build time from a fixture that lives outside the Book
        // (site/src/diagnostics/*.bynk). The fixture's own compile is checked by
        // tests/doc_diagnostics.rs, so don't demand it stand alone here.
        let nonempty: Vec<&str> = b
            .body
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if !nonempty.is_empty() && nonempty.iter().all(|l| l.starts_with("{{#include")) {
            skip_include += 1;
            continue;
        }
        let expect_fail = b.info.contains("fail");
        let first = header_line(&b.body);

        // An explicitly marked fragment is parsed, not compiled — even a headed
        // one (a context that consumes another the page declares separately).
        if b.info.contains("fragment") {
            if expect_fail {
                failures.push(format!(
                    "{loc}: marked `fail` but is a `fragment`, which is parsed, not compiled"
                ));
            }
            match parses_as_fragment(&b.body) {
                Ok(()) => parsed_fragment += 1,
                Err(e) => {
                    failures.push(format!("{loc}: marked `fragment` but does not parse: {e}"))
                }
            }
            continue;
        }
        let result: Result<(), String> = if first.starts_with("commons ") {
            bynkc::compile(&b.body, &b.file)
                .map(|_| ())
                .map_err(|errs| {
                    errs.iter()
                        .map(|e| format!("{}: {}", e.category, e.message))
                        .collect::<Vec<_>>()
                        .join("; ")
                })
        } else if first.starts_with("context ") {
            compile_context(&b.body, idx)
        } else {
            compile_wrapped(&b.body, idx)
        };

        // `bynk,fail=<code>` also pins the diagnostic the prose names
        // (#1662 review): the block must fail *with* that code.
        let pinned = b.info.split(',').find_map(|f| f.strip_prefix("fail="));
        match (expect_fail, result) {
            (false, Ok(())) => checked_ok += 1,
            (true, Err(e)) => match pinned {
                Some(code) if !e.contains(code) => {
                    failures.push(format!("{loc}: marked `fail={code}` but failed with: {e}"))
                }
                _ => checked_fail += 1,
            },
            (false, Err(e)) => failures.push(format!("{loc}: expected to compile, but: {e}")),
            (true, Ok(())) => {
                failures.push(format!("{loc}: marked `fail` but compiled successfully"))
            }
        }
    }

    eprintln!(
        "doc examples: {checked_ok} compiled, {checked_fail} failed-as-expected, \
         {parsed_fragment} fragments parsed, {skip_ignored} ignored, \
         {skip_include} include-only skipped ({} total)",
        blocks.len()
    );

    assert!(
        failures.is_empty(),
        "doc example compilation gate failed:\n  {}",
        failures.join("\n  ")
    );
}
