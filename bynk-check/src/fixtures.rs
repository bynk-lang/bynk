//! #2841: the checker-side rules for a suite's `fixture name = "path"`
//! declarations — the path rules and the content rules shared with `embed`
//! ([`crate::embeds`], [`crate::assets`]) but for the project's `fixtures/`
//! folder, the name rules, and the unused warning.
//!
//! A fixture is a project file's text, a `String` constant in the declaring
//! suite's cases. Unlike an embed it is test-only: its text goes into test
//! output and never into a deployable, so it comes from `fixtures/` (not
//! `embeds/`), may be large ([`MAX_FIXTURE_BYTES`]), and has no project total.
//!
//! The checker stays disk-free: the folder reaches it from the caller as a
//! [`bynk_project::assets::FixtureFolder`], and [`read_declared_fixtures`] reads
//! only the files a declaration names.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use bynk_syntax::ast::{
    Block, ExprKind, FixtureDecl, SourceUnit, SuiteDecl, expr_children, statement_exprs,
};
use bynk_syntax::error::CompileError;

use crate::project_model::ErrorSink;
use bynk_project::ParsedFile;

/// The largest fixture file, in bytes. A fixture never ships, so the cap only
/// keeps a test module a reasonable size; there is no project total.
pub const MAX_FIXTURE_BYTES: usize = 4 * 1024 * 1024;

/// `bynk.fixture.unsafe_path` for `f`, when its path is empty, absolute or
/// contains a `..` segment (the rule an asset's and an embed's paths follow).
pub fn unsafe_path_error(f: &FixtureDecl) -> Option<CompileError> {
    let reason = crate::assets::unsafe_asset_path_reason(&f.path)?;
    Some(
        CompileError::new(
            "bynk.fixture.unsafe_path",
            f.path_span,
            format!("fixture `{}` has an unsafe path: {reason}", f.name.name),
        )
        .with_note("a fixture path is relative to the project root and must stay inside it"),
    )
}

/// `bynk.fixture.outside_fixtures_dir` for `f`, when its path is safe but does
/// not lie inside the project's fixtures folder, so a declaration can never
/// read `.env` or a source file by mistake.
pub fn outside_fixtures_dir_error(f: &FixtureDecl) -> Option<CompileError> {
    if crate::assets::unsafe_asset_path_reason(&f.path).is_some()
        || bynk_project::assets::is_in_fixtures_dir(&f.path)
    {
        return None;
    }
    let dir = bynk_project::assets::PROJECT_FIXTURES_DIR;
    Some(
        CompileError::new(
            "bynk.fixture.outside_fixtures_dir",
            f.path_span,
            format!(
                "fixture `{}` must name a file in the project's `{dir}/` folder",
                f.name.name
            ),
        )
        .with_note(format!(
            "a fixture comes only from `{dir}/`; move the file there and name it `{dir}/…`"
        )),
    )
}

/// The path rules for `f`, in order (at most one applies).
pub fn path_errors(f: &FixtureDecl) -> Option<CompileError> {
    unsafe_path_error(f).or_else(|| outside_fixtures_dir_error(f))
}

/// The suite of a parsed file, when it is one.
fn suite_of(pf: &ParsedFile) -> Option<&SuiteDecl> {
    match pf.unit() {
        SourceUnit::Suite(s) => Some(s),
        _ => None,
    }
}

/// The path errors ([`path_errors`]) for each fixture, and
/// `bynk.fixture.missing_file` for each whose (otherwise valid) path
/// names no file in `available` — the project's `fixtures/` folder as the
/// caller read it. `None` skips the check (nothing on disk was consulted).
pub fn phase_fixture_files(
    parsed: &[ParsedFile],
    available: Option<&BTreeSet<String>>,
    errors: &mut ErrorSink,
) {
    for pf in parsed {
        let Some(suite) = suite_of(pf) else { continue };
        for f in &suite.fixtures {
            // The path rules need no folder, so they hold for a check too.
            if let Some(e) = path_errors(f) {
                errors.push_for(Some(&pf.identity_path()), e);
                continue;
            }
            let Some(available) = available else { continue };
            let key = bynk_project::assets::normalise_asset_path(&f.path);
            if !available.contains(&key) {
                errors.push_for(
                    Some(&pf.identity_path()),
                    CompileError::new(
                        "bynk.fixture.missing_file",
                        f.path_span,
                        format!(
                            "fixture `{}` names `{key}`, which does not exist",
                            f.name.name
                        ),
                    )
                    .with_note(
                        "a fixture must be a regular file in the project's `fixtures/` folder; symbolic links are not followed",
                    ),
                );
            }
        }
    }
}

/// The text of every declared fixture file in `folder`, keyed by normalised
/// path — read through the folder's reader once each, and only for a file a
/// declaration names. Content rules are an embed's ([`crate::embeds::embedded_text`]):
/// valid UTF-8 (`bynk.fixture.invalid_encoding`), no NUL byte
/// (`bynk.fixture.binary_content`), a BOM dropped and newlines normalised; and
/// at most [`MAX_FIXTURE_BYTES`] (`bynk.fixture.too_large`). A read failure is
/// `bynk.fixture.unreadable_file`. Each is reported on every declaration that
/// names the file, and the file is left out.
pub fn read_declared_fixtures(
    parsed: &[ParsedFile],
    folder: &bynk_project::assets::FixtureFolder,
    errors: &mut ErrorSink,
) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut refused: BTreeMap<String, (&'static str, String)> = BTreeMap::new();
    for pf in parsed {
        let Some(suite) = suite_of(pf) else { continue };
        for f in &suite.fixtures {
            if path_errors(f).is_some() {
                continue;
            }
            let key = bynk_project::assets::normalise_asset_path(&f.path);
            if !folder.files().contains(&key) || out.contains_key(&key) {
                continue;
            }
            if !refused.contains_key(&key) {
                match read_one(folder, &key) {
                    Ok(text) => {
                        out.insert(key.clone(), text);
                        continue;
                    }
                    Err(refusal) => {
                        refused.insert(key.clone(), refusal);
                    }
                }
            }
            let (code, detail) = &refused[&key];
            errors.push_for(
                Some(&pf.identity_path()),
                refusal_error(code, f, &key, detail),
            );
        }
    }
    out
}

/// One file's text, or the diagnostic code and detail that refuse it.
fn read_one(
    folder: &bynk_project::assets::FixtureFolder,
    key: &str,
) -> Result<String, (&'static str, String)> {
    let bytes = folder.read(key).map_err(|e| {
        (
            "bynk.fixture.unreadable_file",
            format!("the read failed: {e}"),
        )
    })?;
    if bytes.len() > MAX_FIXTURE_BYTES {
        return Err((
            "bynk.fixture.too_large",
            format!(
                "it is {} bytes, over the {MAX_FIXTURE_BYTES}-byte limit for one fixture file",
                bytes.len()
            ),
        ));
    }
    crate::embeds::embedded_text(&bytes).map_err(|code| {
        if code == "bynk.embed.invalid_encoding" {
            (
                "bynk.fixture.invalid_encoding",
                "it is not valid UTF-8".to_string(),
            )
        } else {
            (
                "bynk.fixture.binary_content",
                "it contains a NUL byte, so it is a binary file".to_string(),
            )
        }
    })
}

fn refusal_error(code: &'static str, f: &FixtureDecl, key: &str, detail: &str) -> CompileError {
    let what = match code {
        "bynk.fixture.unreadable_file" => "could not be read",
        "bynk.fixture.invalid_encoding" => "is not UTF-8 text",
        "bynk.fixture.binary_content" => "is not text",
        "bynk.fixture.too_large" => "is too large for a fixture",
        other => unreachable!("`{other}` is not a fixture refusal code"),
    };
    CompileError::new(
        code,
        f.path_span,
        format!("fixture `{}` names `{key}`, which {what}", f.name.name),
    )
    .with_note(detail.to_string())
    .with_note("a fixture is a `String`; read a binary file another way")
}

/// The name rules and the unused warning for one suite's fixtures.
///
/// `taken` is every value name in the suite's case scope (its view's, with the
/// target's services and agents; see `test_suites::bound_value_names`): a
/// fixture of the same name is `bynk.resolve.name_conflict`, as an embed's is.
/// A repeated fixture name is `bynk.fixture.duplicate`, and a name a helper
/// `fn` of the suite declares is the helper's `bynk.suite.fn_name_clash`. A fixture no `case`,
/// `property` or helper mentions is `bynk.fixture.unused`, a warning.
pub fn check_suite_fixtures(
    suite: &SuiteDecl,
    taken: &HashSet<String>,
    errors: &mut Vec<CompileError>,
) {
    let mut seen: BTreeMap<&str, &FixtureDecl> = BTreeMap::new();
    for f in &suite.fixtures {
        let name = f.name.name.as_str();
        if let Some(prev) = seen.get(name) {
            errors.push(
                CompileError::new(
                    "bynk.fixture.duplicate",
                    f.name.span,
                    format!("fixture `{name}` is already declared"),
                )
                .with_label(prev.name.span, "previously declared here"),
            );
            continue;
        }
        seen.insert(name, f);
        if taken.contains(name) || name.starts_with("__") || name == "deps" {
            errors.push(CompileError::new(
                "bynk.resolve.name_conflict",
                f.name.span,
                format!("fixture `{name}` conflicts with a declaration of the same name in this suite's scope"),
            ));
        }
    }
    if suite.fixtures.is_empty() {
        return;
    }
    let mut mentioned: HashSet<&str> = HashSet::new();
    for c in &suite.cases {
        collect_idents(&c.body, &mut mentioned);
    }
    for p in &suite.properties {
        if let Some(w) = &p.forall.where_pred {
            collect_expr_idents(w, &mut mentioned);
        }
        collect_idents(&p.forall.body, &mut mentioned);
    }
    for f in &suite.fns {
        collect_idents(&f.body, &mut mentioned);
    }
    for f in &suite.fixtures {
        if !mentioned.contains(f.name.name.as_str()) {
            errors.push(CompileError::new(
                "bynk.fixture.unused",
                f.name.span,
                format!("fixture `{}` is never used in this suite", f.name.name),
            ));
        }
    }
}

fn collect_idents<'a>(b: &'a Block, out: &mut HashSet<&'a str>) {
    let mut roots = Vec::new();
    for s in &b.statements {
        statement_exprs(s, &mut roots);
    }
    roots.push(&b.tail);
    for e in roots {
        collect_expr_idents(e, out);
    }
}

fn collect_expr_idents<'a>(e: &'a bynk_syntax::ast::Expr, out: &mut HashSet<&'a str>) {
    if let ExprKind::Ident(id) = &e.kind {
        out.insert(id.name.as_str());
    }
    if let ExprKind::Block(b) = &e.kind {
        collect_idents(b, out);
    }
    for c in expr_children(e) {
        collect_expr_idents(c, out);
    }
}
