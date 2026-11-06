//! `bynk new` — scaffold a new project.
//!
//! The zero-to-one step of the driver arc `doctor → new → dev` (proposal
//! v0.58): it writes a **complete, runnable** single-context HTTP service that
//! `bynk dev` serves unmodified. Unlike `dev`, `new` shells nothing, compiles
//! nothing, and reads no network — it is pure, offline file-writing, so it
//! works before `bynk`, Node, or `wrangler` are installed (D4).
//!
//! The starter, manifest, `.gitignore` and `.gitattributes` are **embedded**
//! via `include_str!` (the first-party precedent, ADR 0086): each template carries a
//! [`PLACEHOLDER`] identifier substituted for the project name at write time.
//! A standing test (`tests/new.rs`) renders the starter with a non-default name
//! and asserts it compiles and is `bynk-fmt`-clean, so the scaffold can never
//! rot into something that doesn't build.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The sentinel identifier in the embedded templates, replaced by the project
/// name when the scaffold is written. Chosen as a legal Bynk identifier so each
/// template is itself parseable, and distinctive enough that a plain
/// substring substitution only ever hits the intended occurrences.
pub const PLACEHOLDER: &str = "appname";

const STARTER_BYNK: &str = include_str!("templates/starter.bynk");
const BYNK_TOML: &str = include_str!("templates/bynk.toml");
const GITIGNORE: &str = include_str!("templates/gitignore");
/// #1763: `*.bynk text eol=lf`, so a Windows checkout keeps `fmt`'s LF form.
const GITATTRIBUTES: &str = include_str!("templates/gitattributes");

/// Directory entries that don't count as "non-empty" for the clobber check
/// (D5): VCS metadata and OS cruft a freshly-`mkdir`ed or `git init`ed
/// directory commonly carries. Mirrors `cargo`'s look-the-other-way set.
const SCAFFOLD_IGNORES: &[&str] = &[
    ".git",
    ".gitattributes",
    ".gitignore",
    ".hg",
    ".hgignore",
    ".svn",
    ".DS_Store",
];

/// Parsed `bynk new` arguments.
#[derive(Debug, Clone)]
pub struct NewOptions {
    /// Directory to create for the new project.
    pub path: PathBuf,
    /// `--name` override for the project / context identifier. Defaults to
    /// `path`'s final component.
    pub name: Option<String>,
}

/// Scaffold a new project: derive & validate the name, refuse to clobber, write
/// the tree, and print next steps. Returns a non-zero exit (touching nothing)
/// on an underivable/invalid name or a non-empty target.
pub fn run(opts: &NewOptions) -> ExitCode {
    let name = match opts.name.clone().or_else(|| derive_name(&opts.path)) {
        Some(name) => name,
        None => {
            eprint!("{}", cannot_derive_message(&display(&opts.path)));
            return ExitCode::FAILURE;
        }
    };

    if !is_legal_name(&name) {
        eprint!("{}", invalid_name_message(&name));
        return ExitCode::FAILURE;
    }

    match target_is_nonempty(&opts.path) {
        Ok(true) => {
            eprint!("{}", clobber_message(&display(&opts.path)));
            return ExitCode::FAILURE;
        }
        Ok(false) => {}
        Err(e) => {
            eprintln!("bynk: cannot inspect `{}`: {e}", display(&opts.path));
            return ExitCode::FAILURE;
        }
    }

    if let Err(e) = write_scaffold(&opts.path, &name) {
        eprintln!("bynk: failed to write the scaffold: {e}");
        return ExitCode::FAILURE;
    }

    print!("{}", next_steps_message(&display(&opts.path)));
    ExitCode::SUCCESS
}

/// The project name implied by a target path: its final component. `None` when
/// the path has no final component (e.g. `.` or `/`), in which case `--name` is
/// required.
fn derive_name(path: &Path) -> Option<String> {
    path.file_name().map(|s| s.to_string_lossy().into_owned())
}

/// Is `name` a legal Bynk identifier — a single, dotless `Ident`? Answered by
/// the real lexer rather than a hand-rolled regex, so it tracks the language
/// exactly: a dash, dot, leading digit, or reserved keyword all yield something
/// other than one lone `Ident` token and are rejected.
pub fn is_legal_name(name: &str) -> bool {
    match bynk_syntax::lexer::tokenize(name) {
        Ok(tokens) => tokens.len() == 1 && tokens[0].kind == bynk_syntax::lexer::TokenKind::Ident,
        Err(_) => false,
    }
}

/// Render an embedded template for `name` by substituting [`PLACEHOLDER`].
pub fn render(template: &str, name: &str) -> String {
    template.replace(PLACEHOLDER, name)
}

/// The rendered starter source for `name` — the `context <name>` HTTP service
/// written to `src/<name>.bynk`.
pub fn starter_source(name: &str) -> String {
    render(STARTER_BYNK, name)
}

/// Does the target exist and hold anything that isn't [`SCAFFOLD_IGNORES`]
/// cruft? A missing or cruft-only directory is fine to scaffold into (D5).
fn target_is_nonempty(target: &Path) -> io::Result<bool> {
    let entries = match fs::read_dir(target) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if !SCAFFOLD_IGNORES.contains(&name.to_string_lossy().as_ref()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Create the directory tree and write the four files. Never overwrites: the
/// clobber check has already cleared the target of everything except
/// [`SCAFFOLD_IGNORES`] cruft. `.gitignore` and `.gitattributes` (#1763) are
/// the members of that set the scaffold also writes — a hand-written one (e.g.
/// in a `git init`ed target) passes the clobber check, so we write ours only
/// when none is present rather than clobbering the user's.
fn write_scaffold(target: &Path, name: &str) -> io::Result<()> {
    let src_dir = target.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(target.join("bynk.toml"), render(BYNK_TOML, name))?;
    let gitignore = target.join(".gitignore");
    if !gitignore.exists() {
        fs::write(gitignore, render(GITIGNORE, name))?;
    }
    // #1763: likewise, never over the user's own.
    let gitattributes = target.join(".gitattributes");
    if !gitattributes.exists() {
        fs::write(gitattributes, render(GITATTRIBUTES, name))?;
    }
    fs::write(src_dir.join(format!("{name}.bynk")), starter_source(name))?;
    Ok(())
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

// ---------------------------------------------------------------------------
// Output surface — pinned by goldens (proposal §5). Built here as pure
// functions so the tests can assert them without touching the filesystem.
// ---------------------------------------------------------------------------

/// The success "next steps" message, printed to stdout.
pub fn next_steps_message(dir: &str) -> String {
    format!(
        "Created a new Bynk project in `{dir}`.\n\
         \n\
         Next steps:\n  \
         cd {dir}\n  \
         bynk dev          # build and serve it locally\n\
         \n\
         New to Bynk? `bynk doctor` checks your toolchain is ready.\n"
    )
}

/// The failure message for a name that isn't a legal Bynk identifier.
pub fn invalid_name_message(name: &str) -> String {
    format!(
        "bynk: `{name}` isn't a valid Bynk name.\n      \
         A name must be a single identifier — a letter followed by letters, \
         digits, or underscores (no dashes or dots).\n      \
         Pass `--name <ident>` to choose the project's identifier.\n"
    )
}

/// The failure message when a name can't be derived from the path.
pub fn cannot_derive_message(path: &str) -> String {
    format!(
        "bynk: couldn't derive a project name from `{path}`.\n      \
         Pass `--name <ident>` to name the project.\n"
    )
}

/// The failure message when the target exists and isn't empty (D5).
pub fn clobber_message(dir: &str) -> String {
    format!(
        "bynk: `{dir}` already exists and isn't empty — refusing to overwrite.\n      \
         Choose a different path, or empty that directory first.\n"
    )
}
