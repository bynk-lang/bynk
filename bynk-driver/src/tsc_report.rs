//! #3068: report a `tsc` failure on generated TypeScript as a Bynk diagnostic,
//! `bynk.codegen.tsc_failed`, at the Bynk source the failing line came from.
//!
//! A program that passed the checker never *should* produce TypeScript that
//! `tsc` rejects, so such a failure is a compiler bug, and a raw
//! `out/workers/room/handlers.ts(120,74): error TS2345` line is no use to the
//! person who hit it. This module parses `tsc`'s output, maps each error's line
//! back through the emitted `.ts.map` (ADR 0103: line-level, each generated line
//! mapped to its nearest enclosing statement), and builds a
//! [`ProjectFailure`] so the usual renderers print it like any other diagnostic.
//!
//! Mapping is line-level only (the maps carry no useful column), so the Bynk span
//! is the whole mapped source line; the TS column stays in the attached note. An
//! error that cannot be mapped is still reported, at the project level.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bynk_project::AttributedError;
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::coverage::SourceMap;
use crate::project::ProjectFailure;

/// Where to report a compiler bug.
const REPORT_URL: &str = "https://github.com/bynk-lang/bynk/issues";
/// At most this many distinct Bynk locations are reported; the rest are counted.
const MAX_LOCATIONS: usize = 10;

pub const CODE: &str = "bynk.codegen.tsc_failed";

/// One `error TSnnnn` record from `tsc`'s output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TscError {
    /// The file as `tsc` printed it.
    pub file: String,
    /// 1-based line and column in the generated file.
    pub line: usize,
    pub col: usize,
    /// `TS2345`.
    pub code: String,
    /// The message, continuation lines joined with newlines.
    pub message: String,
}

/// Parse `tsc --pretty false` output into its error records. Lines that are not
/// records (the `Found N errors` footer, blanks) are ignored; an indented line
/// continues the previous record's message.
pub fn parse_tsc_output(text: &str) -> Vec<TscError> {
    let mut out: Vec<TscError> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some(rec) = parse_record(line) {
            out.push(rec);
        } else if line.starts_with(char::is_whitespace)
            && !line.trim().is_empty()
            && let Some(last) = out.last_mut()
        {
            last.message.push('\n');
            last.message.push_str(line.trim_end());
        }
    }
    out
}

/// `path(line,col): error TS1234: message`. The path may itself contain
/// parentheses, so split on the *last* `(digits,digits): error TS` marker.
fn parse_record(line: &str) -> Option<TscError> {
    let marker = "): error TS";
    let end = line.find(marker)?;
    let head = &line[..end];
    let open = head.rfind('(')?;
    let file = &head[..open];
    let (l, c) = head[open + 1..].split_once(',')?;
    let (line_no, col) = (l.trim().parse().ok()?, c.trim().parse().ok()?);
    let rest = &line[end + marker.len()..];
    let (digits, message) = rest.split_once(':')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(TscError {
        file: file.to_string(),
        line: line_no,
        col,
        code: format!("TS{digits}"),
        message: message.trim().to_string(),
    })
}

/// Normalise a source-map `sources` entry to compare with a snapshot's path.
fn norm(p: &str) -> String {
    let p = p.replace('\\', "/");
    let mut s = p.as_str();
    while let Some(r) = s.strip_prefix("./").or_else(|| s.strip_prefix("../")) {
        s = r;
    }
    s.to_string()
}

/// The text of the snapshot a map's source name refers to.
fn find_snapshot<'a>(
    name: &str,
    snapshots: &'a [(PathBuf, String)],
) -> Option<(&'a Path, &'a str)> {
    let name = norm(name);
    snapshots
        .iter()
        .filter_map(|(p, t)| {
            let key = norm(&p.to_string_lossy());
            (key == name
                || key.ends_with(&format!("/{name}"))
                || name.ends_with(&format!("/{key}")))
            .then_some((key.len(), p.as_path(), t.as_str()))
        })
        .max_by_key(|(len, _, _)| *len)
        .map(|(_, p, t)| (p, t))
}

/// Where `tsc` may have meant `file` from: it prints paths relative to the
/// directory it ran in, which is not always the emit root.
fn generated_candidates(file: &str, emit_root: &Path) -> Vec<PathBuf> {
    let p = Path::new(file);
    let mut c = vec![p.to_path_buf()];
    if p.is_relative() {
        c.push(emit_root.join(p));
        if let Some(parent) = emit_root.parent() {
            c.push(parent.join(p));
        }
        // `out/workers/x.ts` printed against an emit root named `out`.
        if let Some(name) = emit_root.file_name()
            && let Ok(rest) = p.strip_prefix(name)
        {
            c.push(emit_root.join(rest));
        }
    }
    c
}

/// Where one `tsc` error resolved to, if it did.
struct Resolved {
    source: PathBuf,
    /// 0-based line in the Bynk source.
    line: usize,
    span: Span,
    /// The generated line's text, for the secondary note.
    generated: Option<String>,
}

fn resolve(err: &TscError, emit_root: &Path, snapshots: &[(PathBuf, String)]) -> Option<Resolved> {
    for ts in generated_candidates(&err.file, emit_root) {
        let Ok(map_text) = std::fs::read_to_string(format!("{}.map", ts.display())) else {
            continue;
        };
        let map = SourceMap::parse(&map_text)?;
        let (name, src_line) = map.resolve(err.line.checked_sub(1)?)?;
        let (path, text) = find_snapshot(name, snapshots)?;
        let line = src_line as usize;
        let mut start = 0usize;
        let mut found = None;
        for (i, l) in text.split_inclusive('\n').enumerate() {
            if i == line {
                let body = l.trim_end_matches(['\n', '\r']);
                let lead = body.len() - body.trim_start().len();
                found = Some((start + lead, start + body.trim_end().len().max(lead)));
                break;
            }
            start += l.len();
        }
        let (s, e) = found?;
        let generated = std::fs::read_to_string(&ts)
            .ok()
            .and_then(|g| g.lines().nth(err.line - 1).map(|l| l.trim().to_string()));
        return Some(Resolved {
            source: path.to_path_buf(),
            line,
            span: Span::new(s, e),
            generated,
        });
    }
    None
}

/// Build the diagnostics for a failed `tsc` run, or `None` when its output holds
/// no `error TS` record (a crash, an out-of-memory, a missing install): that is a
/// toolchain failure, not evidence of a compiler bug, and the caller reports the
/// raw output as it always did.
///
/// `emit_root` is the directory the TypeScript (and its `.map`s) was written to;
/// `snapshots` and `display_root` are the build's, so the Bynk file renders with
/// source context.
pub fn diagnose(
    tsc_output: &str,
    emit_root: &Path,
    snapshots: &[(PathBuf, String)],
    display_root: &Path,
) -> Option<ProjectFailure> {
    let errors = parse_tsc_output(tsc_output);
    if errors.is_empty() {
        return None;
    }
    // One diagnostic per Bynk location; several TS errors on one statement
    // are notes on the same diagnostic. `None` collects the unmapped ones.
    type Group = (Option<Resolved>, Vec<TscError>);
    let mut order: Vec<(Option<(PathBuf, usize)>, Group)> = Vec::new();
    let mut index: BTreeMap<Option<(PathBuf, usize)>, usize> = BTreeMap::new();
    for err in errors {
        let r = resolve(&err, emit_root, snapshots);
        let key = r.as_ref().map(|r| (r.source.clone(), r.line));
        match index.get(&key) {
            Some(&i) => order[i].1.1.push(err),
            None => {
                index.insert(key.clone(), order.len());
                order.push((key, (r, vec![err])));
            }
        }
    }
    let total = order.len();
    let mut out = Vec::new();
    for (_, (resolved, errs)) in order.into_iter().take(MAX_LOCATIONS) {
        let (source_path, span, mapped) = match &resolved {
            Some(r) => (Some(r.source.clone()), r.span, true),
            None => (None, Span::new(0, 0), false),
        };
        let mut message =
            String::from("the generated TypeScript for this code does not type-check");
        if !mapped {
            message.push_str(" (no Bynk source location could be determined for it)");
        }
        let mut e = CompileError::new(CODE, span, message).with_note(format!(
            "this is a bug in the Bynk compiler, not in your program. Please report it at {REPORT_URL} with this output and, if you can, a minimal repro"
        ));
        for t in &errs {
            e = e.with_note(format!(
                "tsc: {} at {}({},{}): {}",
                t.code,
                t.file,
                t.line,
                t.col,
                t.message.replace('\n', "\n      ")
            ));
        }
        if let Some(g) = resolved.as_ref().and_then(|r| r.generated.as_deref()) {
            e = e.with_note(format!("generated: {g}"));
        }
        out.push(AttributedError::new(source_path, e));
    }
    if total > MAX_LOCATIONS {
        out.push(AttributedError::new(
            None,
            CompileError::new(
                CODE,
                Span::new(0, 0),
                format!(
                    "and {} more location(s) where the generated TypeScript does not type-check",
                    total - MAX_LOCATIONS
                ),
            ),
        ));
    }
    Some(ProjectFailure {
        errors: out,
        snapshots: snapshots.to_vec(),
        display_root: display_root.to_path_buf(),
    })
}

/// What running `tsc` over an emitted tree came to.
enum Ran {
    Passed,
    /// `tsc` ran and failed; its combined stdout and stderr.
    Failed(String),
    /// No runner could be started; why, for the message.
    Unavailable(Vec<String>),
}

/// The `tsc` ladder `bynk test` uses (`tsc`, then `npx -p typescript@<major>
/// tsc`), type-checking only. There is no `tsx` rung: it does not type-check.
fn run_tsc(tsconfig: &Path) -> Ran {
    use std::process::{Command, Stdio};
    let pkg = format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED);
    let runners: [(&str, Vec<&str>); 2] = [
        ("tsc", vec![]),
        ("npx", vec!["--yes", "-p", pkg.as_str(), "tsc"]),
    ];
    let mut failures = Vec::new();
    for (prog, prefix) in &runners {
        let Some(path) = crate::test_runner::resolve_tool(prog) else {
            continue;
        };
        let mut cmd = Command::new(&path);
        cmd.args(prefix)
            .arg("-p")
            .arg(tsconfig)
            .args(["--noEmit", "--pretty", "false"]);
        match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output() {
            Ok(o) if o.status.success() => return Ran::Passed,
            Ok(o) => {
                let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                let err = String::from_utf8_lossy(&o.stderr);
                if !err.trim().is_empty() {
                    text.push('\n');
                    text.push_str(&err);
                }
                return Ran::Failed(text);
            }
            Err(e) => failures.push(crate::test_runner::start_failure(&path, &e)),
        }
    }
    Ran::Unavailable(failures)
}

/// `bynk check --emit` for one project (#3068): generate the Workers output into
/// a temporary directory and type-check it, reporting a `tsc` failure as
/// `bynk.codegen.tsc_failed`. Run after the ordinary check has passed. `true`
/// when the generated TypeScript type-checks.
pub fn check_emit(prog: &str, input: &Path, short: bool) -> bool {
    let options = match crate::try_project_options(input) {
        Ok(o) => o.target(crate::project::BuildTarget::Workers),
        Err(e) => {
            eprintln!("{prog}: {e}");
            return false;
        }
    };
    let out = match crate::project::compile_project(&options) {
        Ok(o) => o,
        Err(failure) => {
            // The check just passed, so this is itself unexpected; say so plainly.
            crate::print_project_failure(&failure);
            return false;
        }
    };
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("bynk-check-emit-{}-{n}", std::process::id()));
    let root = tmp.join("out");
    let result = (|| {
        for (path, doc) in &out.artefacts.docs {
            if let Err(e) = crate::write_document(path, doc, &out.artefacts.docs, &root) {
                eprintln!("{prog}: could not write {}: {e}", root.join(path).display());
                return false;
            }
        }
        match run_tsc(&root.join("tsconfig.json")) {
            Ran::Passed => true,
            Ran::Unavailable(why) => {
                eprintln!(
                    "{prog}: `check --emit` needs `tsc` (with Node.js) to type-check the generated TypeScript, and found none that runs."
                );
                for w in &why {
                    eprintln!("  {w}");
                }
                eprintln!(
                    "  Install it with `npm install -g typescript@{}`.",
                    bynk_emit::TYPESCRIPT_MAJOR_TESTED
                );
                false
            }
            Ran::Failed(text) => {
                match diagnose(&text, &root, &out.snapshots, &out.display_root) {
                    Some(f) if short => crate::print_project_failure_short(&f),
                    Some(f) => crate::print_project_failure(&f),
                    // No `error TS` record: a crash, not evidence of a compiler bug.
                    None => eprintln!("{prog}: tsc failed on the generated TypeScript:\n{text}"),
                }
                false
            }
        }
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// [`check_emit`] over a project or, at a workspace root, every member.
pub fn check_emit_all(prog: &str, input: &Path, short: bool) -> bool {
    if let Some(Ok(ws)) = crate::workspace_at(input) {
        let mut ok = true;
        for m in &ws.members {
            ok &= check_emit(prog, &crate::member_dir(input, m), short);
        }
        return ok;
    }
    check_emit(prog, input, short)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_records_footer_and_continuations() {
        let text = "out/a.ts(120,74): error TS2345: Argument of type 'X' is not assignable.\n  Type 'X' is missing property 'Clock'.\nout/b (1).ts(3,4): error TS2322: Nope.\n\nFound 2 errors in 2 files.\n";
        let errs = parse_tsc_output(text);
        assert_eq!(errs.len(), 2);
        assert_eq!(errs[0].file, "out/a.ts");
        assert_eq!((errs[0].line, errs[0].col), (120, 74));
        assert_eq!(errs[0].code, "TS2345");
        assert!(errs[0].message.contains("missing property 'Clock'"));
        assert_eq!(errs[1].file, "out/b (1).ts");
        assert_eq!(errs[1].line, 3);
    }

    #[test]
    fn non_records_are_not_errors() {
        assert!(parse_tsc_output("error: spawn tsc ENOENT\nkilled\n").is_empty());
        assert!(
            diagnose(
                "FATAL ERROR: heap out of memory",
                Path::new("out"),
                &[],
                Path::new("")
            )
            .is_none()
        );
    }

    #[test]
    fn unmapped_errors_are_still_reported() {
        let f = diagnose(
            "out/gone.ts(1,1): error TS2322: Nope.\n",
            Path::new("/nonexistent/out"),
            &[],
            Path::new(""),
        )
        .expect("a record is a failure");
        assert_eq!(f.errors.len(), 1);
        assert!(f.errors[0].source_path.is_none());
        assert_eq!(f.errors[0].error.category, CODE);
        assert!(
            f.errors[0]
                .error
                .message
                .contains("no Bynk source location")
        );
    }
}
