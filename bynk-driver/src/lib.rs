//! bynk-driver — the shared front-end of the `bynkc` and `bynk` CLIs (#521).
//!
//! Both binaries expose `fmt` and `check` with identical semantics; before
//! this crate each re-implemented the command bodies (and the project-failure
//! flattening layer, and the project-rooting rule) as by-hand copies pinned
//! only by comments and a skip-able parity test. The single implementation
//! lives here, parameterised by the program name that prefixes messages.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bynk_emit::project::{self, CompileOptions, ProjectPathsError, try_read_project_paths_with};
use bynk_fmt::{FormatOptions, IndentStyle, format_source};
use bynk_render::LabelHome;

pub mod coverage;
pub mod dev_run;
pub mod discovery;
pub mod output;
pub mod probe;
pub mod remote;
pub mod schema_lock;
pub mod test_filter;
pub mod test_json;
pub mod test_runner;
pub mod test_stream;

pub use output::{write_document, write_modules, write_modules_for_node, write_output};

/// Root a directory project the way every project command should (#46): a
/// `bynk.toml` or a `src/` subdir selects **project** mode, whose flat
/// `[paths] include`/`exclude` layout (v0.113, DECISION S) defaults to the
/// conventional roots that exist (`src`, `tests`) or the project root itself;
/// otherwise the legacy **single-tree** where `<dir>` is itself the root.
/// `check`, `compile`, `test`, and `dev` all route through this so the
/// conventional layout works the same from any of them.
///
/// #1077 (R2.3/T0.7 residue): reads and populates `.sources(...)` itself —
/// `bynk-emit` no longer discovers or reads project files on disk, so this is
/// now the one real place that walk happens for the live CLI path.
///
/// #1081 review: returns `Result` because that walk is real I/O against a
/// user-controlled `bynk.toml` (a missing `include` root, an unreadable
/// directory) — [`discovery::DiscoveryError`], not a panic.
pub fn project_options(input: &Path) -> Result<CompileOptions, discovery::DiscoveryError> {
    if input.join("bynk.toml").exists() || input.join("src").is_dir() {
        let overlay = manifest_overlay(input);
        let paths = try_read_project_paths_with(input, &overlay)
            .unwrap_or_else(|_| project::ProjectPaths::conventional(input));
        // #1890: degrades like `[paths]` above; `try_project_options` refuses.
        let workers = project::try_read_workers_config_with(input, &overlay).unwrap_or_default();
        Ok(options_for_split(input, paths)?
            .compatibility_flags(workers.compatibility_flags)
            .workers_dev(workers.workers_dev)
            .asset_files(discovery::list_asset_folder(input)?)
            .embed_files(discovery::list_embed_folder(input)?))
    } else {
        let sources = discovery::read_bynk_tree_single(input)?;
        Ok(CompileOptions::single(input.to_path_buf())
            .sources(sources)
            .asset_files(discovery::list_asset_folder(input)?)
            .embed_files(discovery::list_embed_folder(input)?))
    }
}

/// [`project_options`], but a malformed `bynk.toml` is an error rather than a
/// silent fall-back to the conventional layout — the one input a user
/// hand-edits that the compiler otherwise reads without checking, after which
/// a cascade of `bynk.uses.unknown_target` errors points at units that
/// plainly exist on disk.
///
/// #1890: project mode also reads `[workers] compatibility_flags` into the
/// options, so every CLI build (`bynkc compile`, `bynk dev`/`deploy`, which
/// all come through here) writes the project's flags into `wrangler.toml`.
pub fn try_project_options(input: &Path) -> Result<CompileOptions, ProjectOptionsError> {
    // #1883, slice 1b: the `assets/` folder is listed once here, beside the
    // sources; the build reads only the declared files through it, so
    // `bynk-emit` never touches the disk.
    let asset_files = discovery::list_asset_folder(input)?;
    // #2387, slice 2: the `embeds/` folder, listed the same way.
    let embed_files = discovery::list_embed_folder(input)?;
    Ok(match project_sources(input)? {
        (Some(paths), sources) => {
            let workers = project::try_read_workers_config_with(input, &manifest_overlay(input))?;
            CompileOptions::split(input.to_path_buf(), paths)
                .sources(sources)
                .compatibility_flags(workers.compatibility_flags)
                .workers_dev(workers.workers_dev)
                .asset_files(asset_files)
                .embed_files(embed_files)
        }
        (None, sources) => CompileOptions::single(input.to_path_buf())
            .sources(sources)
            .asset_files(asset_files)
            .embed_files(embed_files),
    })
}

/// The `.bynk` files [`try_project_options`] reads for the directory `input`,
/// sorted: what `check`, `test` and `compile` see. #1753: `fmt` expands a
/// directory argument through this, so `fmt --check <dir>` and
/// `check <dir>` cover the same files.
pub fn project_source_files(input: &Path) -> Result<Vec<PathBuf>, ProjectOptionsError> {
    let (_, sources) = project_sources(input)?;
    let mut files: Vec<PathBuf> = sources.into_keys().collect();
    files.sort();
    Ok(files)
}

/// [`try_project_options`]' rooting and walk, shared with
/// [`project_source_files`] so the two can't disagree on which files a
/// directory holds. A `bynk.toml` or a `src/` subdir selects project mode,
/// returning its `[paths]` layout alongside the sources; otherwise `input` is
/// a single tree and the layout is `None`.
fn project_sources(
    input: &Path,
) -> Result<(Option<project::ProjectPaths>, HashMap<PathBuf, String>), ProjectOptionsError> {
    if input.join("bynk.toml").exists() || input.join("src").is_dir() {
        let overlay = manifest_overlay(input);
        // #1665: an unknown table is refused here, on the CLIs' path, before
        // `[paths]` is read. (The language server reads `[paths]` alone.)
        project::check_manifest(input, &overlay)?;
        // #2207 (decision C): a workspace root is not a project. Walking it as
        // one would read every member's sources as a single tree.
        if project::is_workspace_root(input) {
            return Err(ProjectPathsError::WorkspaceRoot.into());
        }
        let paths = try_read_project_paths_with(input, &overlay)?;
        let sources = split_sources(input, &paths)?;
        Ok((Some(paths), sources))
    } else {
        Ok((None, discovery::read_bynk_tree_single(input)?))
    }
}

/// `bynk.toml`'s own content, keyed exactly as [`try_read_project_paths_with`]
/// looks it up (`project_root.join("bynk.toml")`, unmodified — the literal-path
/// branch of `discovery::read_source`'s overlay lookup, so this never needs to
/// match a canonicalised key).
///
/// #1077 review: without this, both entry points above read `bynk.toml`
/// through `bynk-emit`'s own disk-fallback (`read_source`'s `fs::read_to_string`
/// on an overlay miss) — the one on-disk read #1081 left the CLI path still
/// implicitly depending on `bynk-emit` for, despite that PR's claim of a fully
/// fallback-free CLI path. A missing/unreadable `bynk.toml` yields an empty
/// overlay, which `try_read_project_paths_with` already treats as "no
/// manifest" (falls back to the conventional layout) — the same degrade
/// `try_read_project_paths` itself provides.
fn manifest_overlay(input: &Path) -> HashMap<PathBuf, String> {
    let toml_path = input.join("bynk.toml");
    match fs::read_to_string(&toml_path) {
        Ok(text) => HashMap::from([(toml_path, text)]),
        Err(_) => HashMap::new(),
    }
}

/// The split-layout half of `project_options`: walk the project's sources
/// ([`split_sources`]) and hand them to `CompileOptions::split` alongside the
/// layout.
fn options_for_split(
    input: &Path,
    paths: project::ProjectPaths,
) -> Result<CompileOptions, discovery::DiscoveryError> {
    let sources = split_sources(input, &paths)?;
    Ok(CompileOptions::split(input.to_path_buf(), paths).sources(sources))
}

/// The sources of a project rooted at `input` with layout `paths`: build the
/// one `Roots` value the project resolves to and walk exactly that (via
/// [`discovery::sources_for_roots`] — #1081 review, so the CLI's walk can't
/// drift from what `Roots::trees`/`Roots::excludes` themselves say). The one
/// copy of the split-layout walk, shared by [`project_options`] and
/// [`try_project_options`]/[`project_source_files`] (#1755 review).
fn split_sources(
    input: &Path,
    paths: &project::ProjectPaths,
) -> Result<HashMap<PathBuf, String>, discovery::DiscoveryError> {
    let roots = project::Roots::Split {
        project_root: input.to_path_buf(),
        paths: paths.clone(),
    };
    discovery::sources_for_roots(&roots)
}

/// Why [`try_project_options`] could not produce a usable [`CompileOptions`]:
/// either the manifest itself is unreadable ([`ProjectPathsError`]), or a
/// well-formed manifest names a project tree that can't be walked
/// ([`discovery::DiscoveryError`]) — #1081 review.
#[derive(Debug)]
pub enum ProjectOptionsError {
    Paths(ProjectPathsError),
    Discovery(discovery::DiscoveryError),
}

impl std::fmt::Display for ProjectOptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Paths(e) => write!(f, "{e}"),
            Self::Discovery(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProjectOptionsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            // `ProjectPathsError` doesn't itself implement `Error` (no
            // further cause to chain to — it's already a leaf).
            Self::Paths(_) => None,
            Self::Discovery(e) => Some(e),
        }
    }
}

impl From<ProjectPathsError> for ProjectOptionsError {
    fn from(e: ProjectPathsError) -> Self {
        Self::Paths(e)
    }
}

impl From<discovery::DiscoveryError> for ProjectOptionsError {
    fn from(e: discovery::DiscoveryError) -> Self {
        Self::Discovery(e)
    }
}

/// Render a project build failure with per-file ariadne context, exactly as
/// single-file mode had rich rendering. Unattributed (project-level) errors
/// keep the plain form.
///
/// This is the **flattening layer** (ADR 0100): it attributes each
/// `AttributedError` to its file snapshot and delegates the actual rendering to
/// [`bynk_render::print_errors`]. The `ProjectFailure → CompileError` flattening
/// stays here, above `bynk-render`, so there is no `render → emit` edge.
pub fn print_project_failure(failure: &project::ProjectFailure) {
    for ae in &failure.errors {
        match attributed_snapshot(ae, &failure.snapshots, &failure.display_root) {
            Some((label, text)) => {
                print_attributed(ae, &label, text, &failure.snapshots, &failure.display_root);
            }
            None => {
                eprintln!("[{}] {}", ae.error.category, ae.error.message);
                for note in &ae.error.notes {
                    eprintln!("  note: {note}");
                }
                // Finding #47: a label's text still surfaces even with no
                // file to underline it against.
                for (_, label) in &ae.error.labels {
                    eprintln!("  label: {label}");
                }
            }
        }
    }
}

/// v0.89 (ADR 0117): print a successful build's non-failing warnings, with
/// real per-file ariadne context now that a successful build's `snapshots`
/// (mirroring `ProjectFailure::snapshots`) make that possible. A warning whose
/// source isn't attributable (or doesn't fit the snapshot) falls back to the
/// plain `warning[<category>]: <message>` form.
pub fn print_project_warnings(
    warnings: &[project::AttributedError],
    snapshots: &[(PathBuf, String)],
    display_root: &Path,
) {
    for w in warnings {
        match attributed_snapshot(w, snapshots, display_root) {
            Some((label, text)) => print_attributed(w, &label, text, snapshots, display_root),
            None => {
                let where_ = w
                    .source_path
                    .as_deref()
                    .map(|p| format!("{}: ", p.to_string_lossy().replace('\\', "/")))
                    .unwrap_or_default();
                eprintln!("{where_}warning[{}]: {}", w.error.category, w.error.message);
                for note in &w.error.notes {
                    eprintln!("  note: {note}");
                }
                for (_, label) in &w.error.labels {
                    eprintln!("  label: {label}");
                }
            }
        }
    }
}

/// [`print_project_warnings`]'s `--format short` analogue: one
/// `path:line:col: warning[category]: message` line per warning, falling
/// back to `warning[category]: message` when unattributed. Strictly one
/// line per warning throughout (like [`bynk_render::render_errors_short`],
/// this mirrors the VS Code problem-matcher's contract), so — unlike
/// [`print_project_warnings`] — finding #47 doesn't reach this one.
pub fn print_project_warnings_short(
    warnings: &[project::AttributedError],
    snapshots: &[(PathBuf, String)],
    display_root: &Path,
) {
    for w in warnings {
        match attributed_snapshot(w, snapshots, display_root) {
            Some((label, text)) => eprintln!("{}", bynk_render::short_line(&label, text, &w.error)),
            // Every entry in `warnings` is warning-severity by construction
            // (ADR 0117's own split), so `severity_word` here is always
            // "warning" — read off the shared helper (finding #48) rather
            // than hardcoding the string a second time.
            None => eprintln!(
                "{}[{}]: {}",
                bynk_render::severity_word(&w.error),
                w.error.category,
                w.error.message
            ),
        }
    }
}

/// The `(label, source text)` an `AttributedError`'s `source_path` resolves
/// to in `snapshots`, if any — the one attribution lookup every renderer in
/// this file shares (finding #48; previously `print_project_failure` and
/// [`project_failure_short_lines`] each hand-rolled their own copy).
///
/// #1772: the label is the file's path as typed from the working directory
/// ([`display_path`]), not its identity path. `snapshots` stay keyed by
/// identity.
fn attributed_snapshot<'a>(
    ae: &project::AttributedError,
    snapshots: &'a [(PathBuf, String)],
    display_root: &Path,
) -> Option<(String, &'a str)> {
    let path = ae.source_path.as_deref()?;
    let text = snapshots
        .iter()
        .find(|(p, _)| p.as_path() == path)
        .map(|(_, t)| t.as_str())?;
    Some((display_path(display_root, path), text))
}

/// Render `ae` with ariadne against its own file's `text` (shown as `label`),
/// each secondary label against the file it lies in: a "previously declared
/// here" in a sibling file of a multi-file unit is drawn in a snippet of that
/// file, from `snapshots`, not at the same offsets in this one. A label into a
/// first-party unit (#1956) is drawn against that unit's own source, named by
/// its bare identity path (`bynk.bynk`) — it is not under the project root, so
/// [`display_path`] would name a file that doesn't exist. A label whose file
/// has no text becomes a note.
fn print_attributed(
    ae: &project::AttributedError,
    label: &str,
    text: &str,
    snapshots: &[(PathBuf, String)],
    display_root: &Path,
) {
    let others: Vec<Option<(String, &str)>> = (0..ae.error.labels.len())
        .map(|i| match ae.label_file(i) {
            project::LabelFile::Other(path) => snapshots
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, t)| (display_path(display_root, path), t.as_str())),
            project::LabelFile::FirstParty {
                identity_path,
                source,
            } => Some(((*identity_path).to_string(), *source)),
            _ => None,
        })
        .collect();
    let homes: Vec<LabelHome<'_>> = others
        .iter()
        .enumerate()
        .map(|(i, other)| match (ae.label_file(i), other) {
            (project::LabelFile::Primary, _) => LabelHome::Primary,
            (_, Some((filename, source))) => LabelHome::File { filename, source },
            (_, None) => LabelHome::Unavailable,
        })
        .collect();
    bynk_render::print_error_in(&ae.error, text, label, &homes);
}

/// #1772: the path a diagnostic names, as a user would type it from the
/// working directory: the build's root as the caller spelled it, joined with
/// the file's identity path (relative to that root). A relative root is
/// already relative to the working directory; an absolute one (`bynk dev`
/// resolves the project root) is shown relative to the working directory when
/// it lies inside it. This matches the path `fmt` reports for the same file,
/// so a problem matcher resolving against the working directory finds it.
/// Separators print as `/` on every platform.
///
/// #1774 review: a `.` component is dropped (`bynk check`'s default input is
/// `.`, and `Roots::trees` already keeps `./` out of identity paths for the
/// same reason), and an absolute path is compared against the cwd both as
/// given and canonicalised, since a canonical root on Windows is a verbatim
/// `\\?\C:\…` path that a plain cwd never prefixes. One that still isn't
/// under the cwd is shown absolute, without a verbatim prefix.
pub fn display_path(display_root: &Path, identity: &Path) -> String {
    use std::path::Component;
    let joined: PathBuf = display_root
        .join(identity)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    let shown = if joined.is_absolute() {
        let cwd = std::env::current_dir().ok();
        let canonical_cwd = cwd.as_ref().and_then(|c| c.canonicalize().ok());
        cwd.iter()
            .chain(canonical_cwd.iter())
            .find_map(|base| joined.strip_prefix(base).ok().map(Path::to_path_buf))
            .unwrap_or(joined)
    } else {
        joined
    };
    let text = shown.to_string_lossy().replace('\\', "/");
    // A verbatim prefix (`//?/C:/…`, `//?/UNC/server/…`) is Windows-internal.
    if let Some(unc) = text.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else if let Some(rest) = text.strip_prefix("//?/") {
        rest.to_string()
    } else {
        text
    }
}

/// #1774: a path the user named directly (a `fmt` input, a single-file
/// `check`), shown by the same rule as [`display_path`], so `fmt` and `check`
/// print one file identically on every platform. On Windows a directory's
/// expansion joined `\` onto a `/`-typed input, giving mixed separators.
fn shown(path: &Path) -> String {
    display_path(Path::new(""), path)
}

/// The project-failure analogue of [`bynk_render::print_errors_short`]: each
/// attributed error is positioned against its file's snapshot; an unattributed
/// (project-level) error falls back to `<severity>[<category>]: <message>`.
pub fn print_project_failure_short(failure: &project::ProjectFailure) {
    for line in project_failure_short_lines(failure) {
        eprintln!("{line}");
    }
}

/// The string form of [`print_project_failure_short`]: one `path:line:col:
/// severity[category]: message` line per attributed error (an unattributed
/// project-level error falls back to `severity[category]: message`). Backs both
/// the printer above and the `bynkc test --format json` compile-error document,
/// whose `diagnostics` the VS Code `bynkc` problem-matcher re-parses — each
/// `Vec` entry is exactly one line by that contract, so unlike the other
/// renderers in this file this one deliberately does *not* grow note/label
/// continuation lines (finding #47): doing so would break a machine consumer
/// that re-parses every entry as a single diagnostic line.
///
/// The flattening layer (ADR 0100): it delegates the per-error formatting to
/// [`bynk_render::short_line`] / [`bynk_render::severity_word`], and the
/// attribution lookup to the crate-private `attributed_snapshot` (finding #48).
pub fn project_failure_short_lines(failure: &project::ProjectFailure) -> Vec<String> {
    failure
        .errors
        .iter()
        .map(
            |ae| match attributed_snapshot(ae, &failure.snapshots, &failure.display_root) {
                Some((label, text)) => bynk_render::short_line(&label, text, &ae.error),
                None => format!(
                    "{}[{}]: {}",
                    bynk_render::severity_word(&ae.error),
                    ae.error.category,
                    ae.error.message
                ),
            },
        )
        .collect()
}

/// Render every diagnostic from a [`project::ProjectCheck`] (finding #64) with
/// the same per-file ariadne context [`print_project_failure`] gives its own,
/// errors-only list. Unlike that renderer, a `ProjectCheck`'s list can
/// legitimately mix both severities — the unattributed fallback line names its
/// actual severity ([`bynk_render::severity_word`]) rather than
/// `print_project_failure`'s bare `[category]: message` (silently correct only
/// because that list is errors-only by construction).
pub fn print_project_check(check: &project::ProjectCheck) {
    for ae in &check.errors {
        match attributed_snapshot(ae, &check.snapshots, &check.display_root) {
            Some((label, text)) => {
                print_attributed(ae, &label, text, &check.snapshots, &check.display_root);
            }
            None => {
                eprintln!(
                    "{}[{}]: {}",
                    bynk_render::severity_word(&ae.error),
                    ae.error.category,
                    ae.error.message
                );
                for note in &ae.error.notes {
                    eprintln!("  note: {note}");
                }
                for (_, label) in &ae.error.labels {
                    eprintln!("  label: {label}");
                }
            }
        }
    }
}

/// [`print_project_check`]'s `--format short` analogue, mirroring
/// [`project_failure_short_lines`].
pub fn project_check_short_lines(check: &project::ProjectCheck) -> Vec<String> {
    check
        .errors
        .iter()
        .map(
            |ae| match attributed_snapshot(ae, &check.snapshots, &check.display_root) {
                Some((label, text)) => bynk_render::short_line(&label, text, &ae.error),
                None => format!(
                    "{}[{}]: {}",
                    bynk_render::severity_word(&ae.error),
                    ae.error.category,
                    ae.error.message
                ),
            },
        )
        .collect()
}

/// [`print_project_check`] via [`project_check_short_lines`].
pub fn print_project_check_short(check: &project::ProjectCheck) {
    for line in project_check_short_lines(check) {
        eprintln!("{line}");
    }
}

/// How `--indent` spells the two [`IndentStyle`] variants. The words match the
/// `[fmt] indent` key in `bynk.toml`, which the language server already reads,
/// so a project states the same choice the same way in either place.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentKind {
    /// One tab per nesting level. The default — a reader sets their own tab
    /// width in the editor, which space indentation takes away from them.
    Tab,
    /// `--indent-width` spaces per nesting level.
    Spaces,
}

/// The `fmt` subcommand's arguments, flattened by both `bynkc::cli` and
/// `bynk::cli` so the two spell one contract rather than two copies of it (the
/// [`test_runner::TestArgs`] pattern, findings #40/#72). Field docs here are
/// the CLI help text for both commands' flags.
///
/// Every style field is an `Option`, and deliberately carries no clap
/// `default_value` (#972). The three sources are layered — spec default, then
/// the project's `bynk.toml` `[fmt]`, then the flag — and a clap default would
/// make "the user asked for 100" indistinguishable from "the user said
/// nothing", so a manifest's `max_line_width = 120` would be overwritten by a
/// flag nobody passed. `None` means *defer to the layer below*.
#[derive(clap::Args, Debug)]
pub struct FmtArgs {
    /// Files or directories to format. A directory formats the `.bynk` files
    /// `check` would read in it: a project root's `[paths] include` trees minus
    /// `exclude`, a workspace root's members, or any other directory walked
    /// recursively. Use `-` for stdin → stdout.
    pub inputs: Vec<PathBuf>,
    /// Check formatting without writing changes. Exits non-zero if any
    /// file is not already canonical.
    #[arg(long)]
    pub check: bool,
    /// Indent with tabs or spaces. Defaults to the project's `[fmt] indent`,
    /// or tabs.
    #[arg(long, value_enum)]
    pub indent: Option<IndentKind>,
    /// Spaces per nesting level, with spaces indentation. Defaults to the
    /// project's `[fmt] indent_width`, or 2. Rejected when the effective
    /// indentation is tabs, where it would have no effect.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(0..=64))]
    pub indent_width: Option<u8>,
    /// Soft target line width in columns. A construct wider than this wraps
    /// across lines where the grammar allows; one with no break point in it
    /// (a long string literal) is left long. Defaults to the project's
    /// `[fmt] max_line_width`, or 100.
    #[arg(long, value_name = "COLUMNS", value_parser = clap::value_parser!(u32).range(1..))]
    pub max_line_width: Option<u32>,
    /// Emit a trailing comma in multi-line records, sums, list literals and
    /// `exports` clauses. Overrides a project's `trailing_comma = false`, and
    /// overrides an earlier `--no-trailing-comma`.
    #[arg(long, overrides_with = "no_trailing_comma")]
    pub trailing_comma: bool,
    /// Omit the trailing comma in multi-line records, sums, list literals and
    /// `exports` clauses. (Parameter and argument lists never carry one — the
    /// grammar rejects it — regardless of this flag.)
    #[arg(long, overrides_with = "trailing_comma")]
    pub no_trailing_comma: bool,
    /// Ignore the project's `bynk.toml` `[fmt]` section and format to the
    /// canonical style, plus whatever flags this run passes. For a script that
    /// wants one fixed rendering whatever project it is pointed at.
    #[arg(long)]
    pub no_config: bool,
}

impl FmtArgs {
    /// Layer these arguments over `base` — the manifest-resolved options for
    /// the file about to be formatted — or report why they describe nothing
    /// usable. A field the run did not state leaves `base` untouched.
    pub fn apply_to(&self, base: FormatOptions) -> Result<FormatOptions, String> {
        // The width already in `base` (from `[fmt] indent_width`, or the spec
        // default), so `--indent spaces` alone over a manifest's `indent_width
        // = 4` lands on four spaces rather than resetting to two.
        let base_width = match base.indent {
            IndentStyle::Spaces(n) => Some(n),
            IndentStyle::Tab => None,
        };
        let kind = self.indent.unwrap_or(match base.indent {
            IndentStyle::Tab => IndentKind::Tab,
            IndentStyle::Spaces(_) => IndentKind::Spaces,
        });
        let indent = match (kind, self.indent_width) {
            (IndentKind::Tab, None) => IndentStyle::Tab,
            // A width alongside tab indentation is silently meaningless, which
            // is exactly the kind of ignored flag that costs an hour to
            // notice. Say so instead — naming the *effective* indentation,
            // since it may have come from the manifest rather than this run.
            (IndentKind::Tab, Some(_)) => {
                return Err(
                    "`--indent-width` applies only to spaces indentation, and this run resolves \
                     to tabs (pass `--indent spaces`, or set `[fmt] indent` in bynk.toml)"
                        .to_string(),
                );
            }
            // 2 matches the `bynk.toml` `[fmt] indent_width` fallback, so the
            // CLI and the editor agree from the same words.
            (IndentKind::Spaces, width) => IndentStyle::Spaces(width.or(base_width).unwrap_or(2)),
        };
        Ok(FormatOptions {
            indent,
            max_line_width: self.max_line_width.unwrap_or(base.max_line_width),
            // Neither flag set defers to `base`; clap's `overrides_with` pair
            // makes the last one given win.
            trailing_comma: if self.no_trailing_comma {
                false
            } else if self.trailing_comma {
                true
            } else {
                base.trailing_comma
            },
        })
    }
}

/// Why a run could not settle on the options to format an input with.
enum FmtOptionsError {
    /// The project's `bynk.toml` `[fmt]` section is unusable.
    Manifest(PathBuf, bynk_fmt::ConfigError),
    /// #1665: the project's `bynk.toml` has a table (or a `[project]`/`[lsp]`
    /// key) it doesn't define.
    ManifestTables(PathBuf, project::ProjectPathsError),
    /// The flags this run passed contradict each other or the manifest.
    Args(String),
}

impl std::fmt::Display for FmtOptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Named, because the manifest governing a file is not necessarily
            // the one in the working directory.
            Self::Manifest(path, e) => write!(f, "{}: {e}", path.display()),
            Self::ManifestTables(path, e) => write!(f, "{}: {e}", path.display()),
            Self::Args(e) => write!(f, "{e}"),
        }
    }
}

/// Per-directory memo of the `[fmt]` section governing an input.
///
/// A run typically formats many files under one project (`fmt src/*.bynk`), so
/// the upward walk for `bynk.toml` and its parse happen once per starting
/// directory rather than once per file.
struct ManifestCache {
    /// `--no-config`: skip discovery entirely and hand back the spec defaults.
    disabled: bool,
    by_dir: std::collections::HashMap<PathBuf, FormatOptions>,
}

impl ManifestCache {
    fn new(disabled: bool) -> Self {
        Self {
            disabled,
            by_dir: std::collections::HashMap::new(),
        }
    }

    /// The options `input` inherits from its project, before this run's flags.
    fn options_for(&mut self, input: &Path) -> Result<FormatOptions, FmtOptionsError> {
        if self.disabled {
            return Ok(FormatOptions::default());
        }
        // Stdin carries no path to search from; the working directory is the
        // only project context a pipe has.
        let start: PathBuf = if input.as_os_str() == "-" {
            PathBuf::from(".")
        } else {
            match input.parent() {
                // A bare `x.bynk` has an empty parent, which is the cwd.
                Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => PathBuf::from("."),
            }
        };
        // Absolutise before walking. A relative start has no ancestors to walk
        // *through*: `Path::new("src").parent()` is `""` and `""`'s parent is
        // `None`, so the search stops at the working directory and never
        // reaches the project root above it. Run from `src/`, `fmt calc.bynk`
        // therefore missed the very manifest `fmt src/calc.bynk` from the root
        // found — silently formatting to the canonical style, and (under
        // `--check`) gating CI on a style the editor never produces. Joining
        // onto the cwd also collapses `src` and `/abs/src` to one cache key.
        //
        // `current_dir()` rather than `std::path::absolute`: same result here,
        // and it does not raise the crate's MSRV.
        let start = std::env::current_dir()
            .map(|cwd| cwd.join(&start))
            .unwrap_or(start);
        if let Some(hit) = self.by_dir.get(&start) {
            return Ok(*hit);
        }
        let opts = match bynk_fmt::find_manifest(&start) {
            None => FormatOptions::default(),
            Some(manifest) => {
                let text = std::fs::read_to_string(&manifest).map_err(|e| {
                    FmtOptionsError::Manifest(
                        manifest.clone(),
                        bynk_fmt::ConfigError::Read(e.to_string()),
                    )
                })?;
                // `[fmt]` first: its reader owns the detailed TOML parse error
                // and its own keys. #1665: then the manifest's table set.
                let config = bynk_fmt::FmtConfig::from_manifest_str(&text)
                    .map_err(|e| FmtOptionsError::Manifest(manifest.clone(), e))?;
                let options = config.apply(FormatOptions::default());
                // #2207: path-aware, so a workspace member's own
                // `[registries]` or `[workspace]` is refused here as in `check`.
                let root = manifest.parent().unwrap_or(Path::new("."));
                project::check_manifest(root, &HashMap::from([(root.join("bynk.toml"), text)]))
                    .map_err(|e| FmtOptionsError::ManifestTables(manifest, e))?;
                options
            }
        };
        self.by_dir.insert(start, opts);
        Ok(opts)
    }
}

/// The `fmt` command body shared by `bynkc fmt` and `bynk fmt`: each input is
/// formatted and rewritten only when it changes; `--check` reports
/// non-canonical files without writing; `-` reads stdin and writes the
/// formatted result to stdout. `prog` prefixes messages (`bynk fmt: …`).
pub fn run_fmt(prog: &str, args: &FmtArgs) -> ExitCode {
    let check = args.check;
    if args.inputs.is_empty() {
        eprintln!("{prog} fmt: no input files (pass file or directory paths, or `-` for stdin)");
        return ExitCode::FAILURE;
    }
    let inputs = match expand_fmt_inputs(&args.inputs) {
        Ok(inputs) => inputs,
        Err(e) => {
            eprintln!("{prog} fmt: {e}");
            return ExitCode::FAILURE;
        }
    };
    let inputs = &inputs;
    // Resolve *every* input's options before formatting any of them. Options
    // are per-input — `[fmt]` belongs to the project the file sits in, so a
    // path outside the current project obeys that project's style — but a
    // manifest error found on the third input must not land after the first two
    // have already been rewritten. Configuration is a whole-run precondition:
    // it fails before a byte is written, or not at all.
    let mut manifests = ManifestCache::new(args.no_config);
    let mut resolved: Vec<FormatOptions> = Vec::with_capacity(inputs.len());
    for input in inputs {
        match manifests
            .options_for(input)
            .and_then(|base| args.apply_to(base).map_err(FmtOptionsError::Args))
        {
            Ok(opts) => resolved.push(opts),
            Err(e) => {
                eprintln!("{prog} fmt: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let mut had_diff = false;
    let mut had_error = false;
    for (input, opts) in inputs.iter().zip(resolved) {
        if input.as_os_str() == "-" {
            use std::io::Read;
            let mut source = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut source) {
                eprintln!("{prog} fmt: read from stdin: {e}");
                return ExitCode::FAILURE;
            }
            // #1763: compared, and errors rendered, modulo line endings.
            let source = bynk_fmt::normalize_line_endings(&source);
            match format_source(&source, &opts) {
                Ok(formatted) => {
                    if check {
                        // `--check` on stdin must not print the formatted text
                        // (it would pollute a CI log) and must report a diff the
                        // same way the file path does — a `generator | bynk fmt
                        // --check -` gate is otherwise dead, passing green on
                        // non-canonical input.
                        if formatted != source {
                            eprintln!("{prog} fmt: <stdin> is not canonically formatted");
                            had_diff = true;
                        }
                    } else {
                        print!("{formatted}");
                    }
                }
                Err(e) => {
                    bynk_render::print_errors(&e.errors, &source, "<stdin>");
                    return ExitCode::FAILURE;
                }
            }
            continue;
        }
        let raw = match std::fs::read_to_string(input) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{prog} fmt: read `{}`: {e}", shown(input));
                had_error = true;
                continue;
            }
        };
        // #1763: line endings are not a formatting difference. A CRLF copy of a
        // canonical file passes `--check` and isn't rewritten; a file that does
        // need formatting is written in the LF canonical form. Errors render
        // against the normalised text, whose spans they carry.
        let source = bynk_fmt::normalize_line_endings(&raw);
        let filename = shown(input);
        match format_source(&source, &opts) {
            Ok(formatted) => {
                if check {
                    if formatted != source {
                        eprintln!("{prog} fmt: {} is not canonically formatted", filename);
                        had_diff = true;
                    }
                } else if formatted != source
                    && let Err(e) = atomic_write(input, &formatted)
                {
                    eprintln!("{prog} fmt: write `{}`: {e}", filename);
                    had_error = true;
                }
            }
            Err(e) => {
                bynk_render::print_errors(&e.errors, &source, &filename);
                had_error = true;
            }
        }
    }
    if had_error || (check && had_diff) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// #1753: expand `fmt`'s arguments to the files it formats. A directory
/// becomes the `.bynk` files [`project_source_files`] finds in it, which is what
/// `check` and `test` read for the same argument: a project root's `[paths]`
/// `include` trees minus `exclude`, or a plain directory walked recursively,
/// hidden directories skipped either way. A file or `-` passes through. A file
/// named more than once, directly or through a directory, is formatted once.
///
/// A directory with no `.bynk` files is an error rather than an empty run, so
/// a mistyped path cannot pass `--check`. Expansion happens before any file is
/// read, so its errors, like a manifest error, land before anything is written.
fn expand_fmt_inputs(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for input in inputs {
        // #2207 (decision C): a workspace root formats every member's files,
        // named from the root as the command was given it.
        if input.as_os_str() != "-"
            && let Some(workspace) = workspace_at(input)
        {
            let workspace = workspace.map_err(|e| format!("`{}`: {e}", input.display()))?;
            for member in &workspace.members {
                let member = member_dir(input, member);
                let files = project_source_files(&member)
                    .map_err(|e| format!("`{}`: {e}", shown(&member)))?;
                for file in files {
                    let key = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
                    if seen.insert(key) {
                        out.push(file);
                    }
                }
            }
            continue;
        }
        let files = if input.as_os_str() != "-" && input.is_dir() {
            let files =
                project_source_files(input).map_err(|e| format!("`{}`: {e}", input.display()))?;
            if files.is_empty() {
                return Err(format!("no `.bynk` files under `{}`", input.display()));
            }
            files
        } else {
            vec![input.clone()]
        };
        for file in files {
            let key = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
            if seen.insert(key) {
                out.push(file);
            }
        }
    }
    Ok(out)
}

/// Write `contents` to `path` atomically: the bytes land in a sibling temp
/// file that is then `rename`d over `path`. A plain `std::fs::write` truncates
/// the destination *before* writing, so an ENOSPC, a signal, or a crash
/// mid-write leaves the file truncated or empty — and for `fmt`, whose only
/// copy of the original is the in-memory `source`, that original is then gone.
/// The rename is atomic on POSIX and Windows, so a reader sees either the whole
/// old file or the whole new one, never a half-written mix.
///
/// The temp file is a sibling (same directory) so the rename stays within one
/// filesystem — a cross-device rename would fail with `EXDEV`. Its name carries
/// the PID and a per-process counter so concurrent `fmt` runs, or two files in
/// one run, never collide, and it is opened with `create_new` (`O_EXCL`): a
/// pre-existing path — a stale temp from an earlier crashed run, or a symlink a
/// local actor pre-planted to redirect the formatted bytes — is refused rather
/// than opened, and we bump the counter and retry. On any failure the temp file
/// is removed so a botched write leaves no litter beside the untouched original.
///
/// The `rename` swaps in a fresh inode, so if `path` was a symlink or a
/// hardlink the formatted file replaces the link rather than being written
/// through it (the old `std::fs::write` wrote through). Uncommon for source
/// files, and the atomicity is worth it.
fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Open a fresh sibling temp file exclusively, bumping the counter past any
    // name that is already taken (stale temp or planted symlink).
    let (mut file, tmp) = loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_name = format!(".{file_name}.bynk-fmt.{}.{n}.tmp", std::process::id());
        let tmp = match dir {
            Some(d) => d.join(tmp_name),
            None => PathBuf::from(tmp_name),
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(f) => break (f, tmp),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };

    // Scope the write so the handle is flushed and closed before the rename.
    // The rename replaces the destination inode, so carry the original file's
    // permissions onto the temp file first — otherwise a formatted file would
    // silently pick up the process umask's default mode (e.g. an executable or
    // group-restricted source would lose its bits).
    let write_result = (|| {
        // Best-effort: a filesystem that cannot honour the mode must not fail
        // the whole write.
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = file.set_permissions(meta.permissions());
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// The `check` command body shared by `bynkc check` and `bynk check`: a
/// directory routes through [`project::check_project`] (finding #64 —
/// non-bailing, so a structural error anywhere does not hide diagnostics
/// elsewhere the way `compile_project`'s bail-fast `Mode::Build` would), a
/// single file through [`bynk_emit::check_with_warnings`]. `short` selects
/// the one-line `--format short` rendering. `prog` prefixes messages
/// (`bynk: …`).
pub fn run_check(prog: &str, input: &Path, short: bool) -> ExitCode {
    // #2207 (decision C): at a workspace root, check every member in order.
    // Every member runs, and the run fails if any does. `rich` heads each
    // member's report with its path; `short` adds no headers, and its paths
    // are already relative to where the command ran.
    if let Some(workspace) = workspace_at(input) {
        let workspace = match workspace {
            Ok(w) => w,
            Err(e) => {
                eprintln!("{prog}: {}: {e}", shown(&input.join("bynk.toml")));
                return ExitCode::FAILURE;
            }
        };
        let mut failed = false;
        for member in &workspace.members {
            let member = member_dir(input, member);
            if !short {
                eprintln!("{}", member_header(&member));
            }
            failed |= !check_one(prog, &member, short, Some(&member));
        }
        return if failed {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        };
    }
    if check_one(prog, input, short, None) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// #2207: if `input` is a workspace root (a directory whose `bynk.toml` holds
/// `[workspace]`), the workspace it loads to, or why it doesn't.
pub fn workspace_at(input: &Path) -> Option<Result<project::Workspace, ProjectPathsError>> {
    (input.is_dir() && project::is_workspace_root(input)).then(|| project::load_workspace(input))
}

/// #2207: a member's directory as the command names it: `root` (as the user
/// typed it) joined with the member's relative path, without `.` components,
/// so a run at `.` names `packages/money`, not `./packages/money`.
pub fn member_dir(root: &Path, member: &Path) -> PathBuf {
    root.join(member)
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

/// #2207 (decision C): the line that heads one member's report in a
/// workspace run, `── packages/money ──`, its path as typed from the working
/// directory.
pub fn member_header(member: &Path) -> String {
    format!("── {} ──", shown(member))
}

/// [`run_check`] over one project or file; `true` when it passes. `member`
/// names the workspace member being checked, so a project-level error says
/// which one it is.
fn check_one(prog: &str, input: &Path, short: bool, member: Option<&Path>) -> bool {
    let prog = match member {
        Some(m) => format!("{prog}: {}", shown(m)),
        None => prog.to_string(),
    };
    let prog = prog.as_str();
    if input.is_dir() {
        let options = match try_project_options(input) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("{prog}: {e}");
                return false;
            }
        };
        // #2078: reconcile events against the committed `bynk.schema.lock`,
        // read-only, so a non-additive change fails `check` as it fails a
        // build. Nothing is written: an additive change is left for a build
        // to record.
        let options = match schema_lock::read_only(input) {
            Ok(lock) => options.schema_registry(lock),
            Err(e) => {
                eprintln!("{prog}: {e}");
                return false;
            }
        };
        let check = project::check_project(&options);
        let has_errors = check.has_errors();
        if short {
            print_project_check_short(&check);
        } else {
            print_project_check(&check);
        }
        !has_errors
    } else {
        let source = match std::fs::read_to_string(input) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{prog}: could not read `{}`: {e}", shown(input));
                return false;
            }
        };
        let filename = shown(input);
        match bynk_emit::check_with_warnings(&source, &filename) {
            Ok(warnings) => {
                if !warnings.is_empty() {
                    if short {
                        bynk_render::print_errors_short(&warnings, &source, &filename);
                    } else {
                        bynk_render::print_errors(&warnings, &source, &filename);
                    }
                }
                true
            }
            Err(errors) => {
                if short {
                    bynk_render::print_errors_short(&errors, &source, &filename);
                } else {
                    bynk_render::print_errors(&errors, &source, &filename);
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A minimal parser around [`FmtArgs`], so these assert what the real CLIs
    /// parse rather than what a hand-built struct claims.
    #[derive(clap::Parser, Debug)]
    struct Harness {
        #[command(flatten)]
        args: FmtArgs,
    }

    fn parse(argv: &[&str]) -> FmtArgs {
        let mut full = vec!["fmt"];
        full.extend_from_slice(argv);
        Harness::parse_from(full).args
    }

    /// The options a run resolves with no manifest in play.
    fn resolve(argv: &[&str]) -> Result<FormatOptions, String> {
        parse(argv).apply_to(FormatOptions::default())
    }

    /// A stand-in for what `bynk.toml` `[fmt]` resolved to.
    fn manifest(toml: &str) -> FormatOptions {
        bynk_fmt::FmtConfig::from_manifest_str(toml)
            .expect("manifest parses")
            .apply(FormatOptions::default())
    }

    #[test]
    fn no_flags_is_the_canonical_style() {
        let opts = resolve(&["a.bynk"]).expect("valid");
        assert_eq!(opts, FormatOptions::default());
    }

    #[test]
    fn spaces_without_a_width_falls_back_to_two() {
        // The same fallback `bynk.toml`'s `[fmt] indent_width` uses, so the CLI
        // and the language server land on the same style from the same words.
        let opts = resolve(&["--indent", "spaces", "a.bynk"]).expect("valid");
        assert_eq!(opts.indent, IndentStyle::Spaces(2));
    }

    #[test]
    fn spaces_takes_the_given_width() {
        let opts =
            resolve(&["--indent", "spaces", "--indent-width", "4", "a.bynk"]).expect("valid");
        assert_eq!(opts.indent, IndentStyle::Spaces(4));
    }

    #[test]
    fn a_width_with_tabs_is_an_error_not_a_silent_no_op() {
        let err = resolve(&["--indent", "tab", "--indent-width", "4", "a.bynk"])
            .expect_err("a meaningless width must be reported");
        assert!(err.contains("--indent-width"), "{err}");
        assert!(err.contains("spaces"), "{err}");
    }

    #[test]
    fn the_trailing_comma_pair_is_last_one_wins() {
        // `overrides_with` in both directions: a script may append either flag
        // to a shared argument list and have it win rather than conflict-error.
        assert!(
            resolve(&["--no-trailing-comma", "--trailing-comma", "a.bynk"])
                .expect("valid")
                .trailing_comma
        );
        assert!(
            !resolve(&["--trailing-comma", "--no-trailing-comma", "a.bynk"])
                .expect("valid")
                .trailing_comma
        );
    }

    #[test]
    fn max_line_width_is_taken_verbatim_and_zero_is_refused() {
        assert_eq!(
            resolve(&["--max-line-width", "60", "a.bynk"])
                .expect("valid")
                .max_line_width,
            60
        );
        // A zero-column budget is a nonsense input; clap rejects it at parse
        // time rather than the formatter wrapping every construct maximally.
        assert!(
            Harness::try_parse_from(["fmt", "--max-line-width", "0", "a.bynk"]).is_err(),
            "`--max-line-width 0` must not parse"
        );
    }

    // -- #972: the `bynk.toml` `[fmt]` layer beneath the flags --

    #[test]
    fn an_unflagged_run_takes_the_manifest_whole() {
        let base = manifest(
            "[fmt]\nindent = \"spaces\"\nindent_width = 4\nmax_line_width = 120\ntrailing_comma = false\n",
        );
        let opts = parse(&["a.bynk"]).apply_to(base).expect("valid");
        assert_eq!(opts.indent, IndentStyle::Spaces(4));
        assert_eq!(opts.max_line_width, 120);
        assert!(!opts.trailing_comma);
    }

    #[test]
    fn a_flag_beats_the_manifest_field_it_names_and_no_other() {
        let base = manifest("[fmt]\nindent = \"spaces\"\nindent_width = 4\nmax_line_width = 120\n");
        let opts = parse(&["--max-line-width", "80", "a.bynk"])
            .apply_to(base)
            .expect("valid");
        assert_eq!(opts.max_line_width, 80, "the flag wins where it speaks");
        assert_eq!(
            opts.indent,
            IndentStyle::Spaces(4),
            "and stays silent everywhere else"
        );
    }

    #[test]
    fn an_absent_flag_does_not_reset_the_manifest_to_the_default() {
        // The regression a clap `default_value` would have caused: "the user
        // said 100" is indistinguishable from "the user said nothing", so a
        // project's 120 would be overwritten by a flag nobody passed.
        let base = manifest("[fmt]\nmax_line_width = 120\n");
        assert_eq!(
            parse(&["a.bynk"])
                .apply_to(base)
                .expect("valid")
                .max_line_width,
            120
        );
    }

    #[test]
    fn indent_spaces_alone_keeps_the_manifest_width() {
        let base = manifest("[fmt]\nindent = \"spaces\"\nindent_width = 4\n");
        let opts = parse(&["--indent", "spaces", "a.bynk"])
            .apply_to(base)
            .expect("valid");
        assert_eq!(opts.indent, IndentStyle::Spaces(4), "not reset to 2");
    }

    #[test]
    fn indent_width_alone_applies_to_a_manifest_that_chose_spaces() {
        let base = manifest("[fmt]\nindent = \"spaces\"\n");
        let opts = parse(&["--indent-width", "8", "a.bynk"])
            .apply_to(base)
            .expect("valid");
        assert_eq!(opts.indent, IndentStyle::Spaces(8));
    }

    #[test]
    fn indent_width_alone_is_refused_when_the_run_resolves_to_tabs() {
        // No `--indent`, and a manifest that says tabs (or none at all): the
        // width has nothing to apply to, and the message says so rather than
        // the flag vanishing.
        let err = parse(&["--indent-width", "8", "a.bynk"])
            .apply_to(manifest("[fmt]\nindent = \"tab\"\n"))
            .expect_err("refused");
        assert!(err.contains("resolves"), "{err}");
    }

    #[test]
    fn an_explicit_tab_flag_overrides_a_manifest_choosing_spaces() {
        let base = manifest("[fmt]\nindent = \"spaces\"\nindent_width = 4\n");
        let opts = parse(&["--indent", "tab", "a.bynk"])
            .apply_to(base)
            .expect("valid");
        assert_eq!(opts.indent, IndentStyle::Tab);
    }

    #[test]
    fn trailing_comma_flag_overrides_a_manifest_that_turned_it_off() {
        let base = manifest("[fmt]\ntrailing_comma = false\n");
        assert!(
            parse(&["--trailing-comma", "a.bynk"])
                .apply_to(base)
                .expect("valid")
                .trailing_comma
        );
        // …and an unflagged run still honours the manifest.
        assert!(
            !parse(&["a.bynk"])
                .apply_to(base)
                .expect("valid")
                .trailing_comma
        );
    }

    #[test]
    fn no_config_is_parsed_and_defaults_are_used_in_its_presence() {
        // `--no-config` is honoured by the manifest *lookup* (ManifestCache),
        // so here it is enough that it parses and leaves the flag layer alone.
        let args = parse(&["--no-config", "a.bynk"]);
        assert!(args.no_config);
        assert_eq!(
            args.apply_to(FormatOptions::default()).expect("valid"),
            FormatOptions::default()
        );
    }

    /// A throwaway on-disk directory, removed on drop (including on panic) —
    /// mirrors `bynk-driver/tests/project_diagnostics.rs`'s own `Scratch`.
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scratch_dir(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "bynk_1077_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    /// #1077 review: `manifest_overlay` keys its entry exactly as
    /// `try_read_project_paths_with` looks it up — `root.join("bynk.toml")`,
    /// literal, no canonicalisation — and reads `bynk.toml`'s real content.
    /// This is what stops that lookup from falling through to `bynk-emit`'s
    /// own disk fallback; a mismatched key would silently degrade to the
    /// conventional layout instead of surfacing as a test failure here, so
    /// this asserts the map entry directly rather than only the end-to-end
    /// behaviour (which the integration test in `project_diagnostics.rs`
    /// covers).
    #[test]
    fn manifest_overlay_keys_and_reads_a_real_bynk_toml() {
        let dir = scratch_dir("manifest_overlay");
        let toml = "[paths]\ninclude = [\"lib\"]\n";
        fs::write(dir.0.join("bynk.toml"), toml).unwrap();

        let overlay = manifest_overlay(&dir.0);

        assert_eq!(
            overlay.get(&dir.0.join("bynk.toml")).map(String::as_str),
            Some(toml)
        );
    }

    #[test]
    fn manifest_overlay_is_empty_with_no_bynk_toml() {
        let dir = scratch_dir("manifest_overlay_missing");
        assert!(manifest_overlay(&dir.0).is_empty());
    }

    /// Review of #1084: `ProjectOptionsError::Paths` had no test anywhere in
    /// the repo, despite being the one arm where an overlay/disk divergence
    /// in the manifest read would actually be observable — everywhere else,
    /// `read_source`'s still-present disk fallback quietly reproduces the
    /// same result either way. This also re-pins that `?`'s automatic
    /// `From<ProjectPathsError>` conversion (not an explicit `map_err`) still
    /// reaches the caller correctly.
    #[test]
    fn try_project_options_surfaces_an_unknown_paths_key() {
        let dir = scratch_dir("try_project_options_unknown_key");
        fs::write(dir.0.join("bynk.toml"), "[paths]\ninculde = [\"src\"]\n").unwrap();
        fs::create_dir_all(dir.0.join("src")).unwrap();
        fs::write(dir.0.join("src/thing.bynk"), "context thing\n").unwrap();

        let err = match try_project_options(&dir.0) {
            Err(e) => e,
            Ok(_) => panic!("an unrecognised [paths] key must be reported, not silently ignored"),
        };
        assert!(
            matches!(
                &err,
                ProjectOptionsError::Paths(ProjectPathsError::UnknownKey(k)) if k == "inculde"
            ),
            "expected Paths(UnknownKey(\"inculde\")), got: {err:?}"
        );
    }

    /// #1665: an unknown table reaches the caller from the same strict path,
    /// even with a valid `[paths]`. Before, `[pahts]` built cleanly.
    #[test]
    fn try_project_options_surfaces_an_unknown_table() {
        let dir = scratch_dir("try_project_options_unknown_table");
        fs::write(
            dir.0.join("bynk.toml"),
            "[paths]\ninclude = [\"src\"]\n\n[pahts]\nexclude = []\n",
        )
        .unwrap();
        fs::create_dir_all(dir.0.join("src")).unwrap();
        fs::write(dir.0.join("src/thing.bynk"), "context thing\n").unwrap();

        let err = match try_project_options(&dir.0) {
            Err(e) => e,
            Ok(_) => panic!("an unknown table must be reported, not silently ignored"),
        };
        assert!(
            matches!(
                &err,
                ProjectOptionsError::Paths(ProjectPathsError::UnknownTable(t)) if t == "pahts"
            ),
            "expected Paths(UnknownTable(\"pahts\")), got: {err:?}"
        );
        assert!(err.to_string().contains("did you mean `[paths]`?"), "{err}");
    }

    /// A one-context project with `bynk.toml` `manifest`, for the #1890 tests.
    fn workers_project(name: &str, manifest: &str) -> Scratch {
        let dir = scratch_dir(name);
        fs::write(dir.0.join("bynk.toml"), manifest).unwrap();
        fs::create_dir_all(dir.0.join("src")).unwrap();
        fs::write(
            dir.0.join("src/greet.bynk"),
            "context greet\n\nservice hello {\n  on call(name: String) -> Effect[Result[String, ()]] {\n    Ok(name)\n  }\n}\n",
        )
        .unwrap();
        dir
    }

    /// The `wrangler.toml` a Workers build of `options` writes for `greet`,
    /// and the build's warnings' codes.
    fn greet_wrangler(options: CompileOptions) -> (String, Vec<&'static str>) {
        let out = project::compile_project(&options.target(project::BuildTarget::Workers))
            .unwrap_or_else(|f| {
                let msgs: Vec<&str> = f.errors.iter().map(|e| e.error.message.as_str()).collect();
                panic!("builds: {msgs:?}")
            });
        let text = out.artefacts.docs[Path::new("workers/greet/wrangler.toml")].text();
        (
            text,
            out.warnings.iter().map(|w| w.error.category).collect(),
        )
    }

    /// #1890: `[workers] compatibility_flags` reaches every emitted
    /// `wrangler.toml`, after the default, from both option builders. A flag
    /// already on by default is dropped with a warning.
    #[test]
    fn manifest_compatibility_flags_reach_wrangler_toml() {
        let dir = workers_project(
            "compat_flags_1890",
            "[workers]\ncompatibility_flags = [\"nodejs_compat\", \"global_fetch_strictly_public\"]\n",
        );
        let want = "compatibility_flags = [\"global_fetch_strictly_public\", \"nodejs_compat\"]\n";
        for options in [
            try_project_options(&dir.0).expect("reads"),
            project_options(&dir.0).expect("reads"),
        ] {
            let (text, warnings) = greet_wrangler(options);
            assert!(text.contains(want), "{text}");
            assert_eq!(
                warnings,
                ["bynk.project.duplicate_compatibility_flag"],
                "the repeated default is reported"
            );
        }
    }

    /// `[workers] workers_dev` is written into every `wrangler.toml` when set,
    /// and left out when it isn't; a non-boolean is refused.
    #[test]
    fn manifest_workers_dev_reaches_wrangler_toml() {
        let dir = workers_project("workers_dev_off", "[workers]\nworkers_dev = false\n");
        for options in [
            try_project_options(&dir.0).expect("reads"),
            project_options(&dir.0).expect("reads"),
        ] {
            let (text, _) = greet_wrangler(options);
            assert!(text.contains("workers_dev = false\n"), "{text}");
        }
        let dir = workers_project("workers_dev_unset", "[workers]\ncompatibility_flags = []\n");
        let (text, _) = greet_wrangler(try_project_options(&dir.0).expect("reads"));
        assert!(!text.contains("workers_dev"), "{text}");
        let dir = workers_project("workers_dev_bad", "[workers]\nworkers_dev = \"no\"\n");
        assert!(matches!(
            try_project_options(&dir.0),
            Err(ProjectOptionsError::Paths(
                ProjectPathsError::NotABool { .. }
            ))
        ));
    }

    /// #1890: with no `[workers]` table, a Worker still gets the default flag.
    #[test]
    fn a_worker_gets_the_default_compatibility_flag_without_a_workers_table() {
        let dir = workers_project("compat_flags_default_1890", "[project]\nname = \"p\"\n");
        let (text, warnings) = greet_wrangler(try_project_options(&dir.0).expect("reads"));
        assert!(
            text.contains("compatibility_flags = [\"global_fetch_strictly_public\"]\n"),
            "{text}"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// #1890: a non-string flag is refused on the CLIs' strict path.
    #[test]
    fn try_project_options_refuses_a_non_string_compatibility_flag() {
        let dir = workers_project(
            "compat_flags_bad_1890",
            "[workers]\ncompatibility_flags = [\"nodejs_compat\", 2]\n",
        );
        match try_project_options(&dir.0) {
            Err(ProjectOptionsError::Paths(ProjectPathsError::NotAStringList { .. })) => {}
            Err(e) => panic!("wrong error: {e:?}"),
            Ok(_) => panic!("a non-string flag must be refused"),
        }
    }
}

#[cfg(test)]
mod display_path_tests {
    use super::display_path;
    use std::path::Path;

    #[test]
    fn a_relative_root_is_joined_as_typed() {
        assert_eq!(
            display_path(Path::new("test/fixtures/x"), Path::new("src/a.bynk")),
            "test/fixtures/x/src/a.bynk"
        );
    }

    /// `bynk check` defaults its input to `.`; that adds no `./`.
    #[test]
    fn a_dot_root_adds_no_prefix() {
        assert_eq!(
            display_path(Path::new("."), Path::new("src/a.bynk")),
            "src/a.bynk"
        );
        assert_eq!(
            display_path(Path::new(""), Path::new("./src/a.bynk")),
            "src/a.bynk"
        );
    }

    /// `bynk dev` hands an absolute root; inside the cwd it shows relative.
    #[test]
    fn an_absolute_root_inside_the_cwd_is_shown_relative() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            display_path(&cwd.join("proj"), Path::new("src/a.bynk")),
            "proj/src/a.bynk"
        );
        // A canonicalised root (on Windows, a verbatim `\\?\` path) too.
        let canonical = cwd.canonicalize().unwrap();
        assert_eq!(
            display_path(&canonical.join("proj"), Path::new("src/a.bynk")),
            "proj/src/a.bynk"
        );
    }

    /// Outside the cwd, the path stays absolute, with `/` separators.
    #[test]
    fn an_absolute_root_outside_the_cwd_is_shown_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let outside = cwd.parent().unwrap().join("bynk-display-path-elsewhere");
        let shown = display_path(&outside, Path::new("src/a.bynk"));
        assert!(
            shown.ends_with("bynk-display-path-elsewhere/src/a.bynk"),
            "{shown}"
        );
        assert!(
            !shown.contains('\\') && !shown.starts_with("//?/"),
            "{shown}"
        );
    }

    /// On Windows, a verbatim root outside the cwd loses its `\\?\` prefix.
    #[cfg(windows)]
    #[test]
    fn a_verbatim_root_outside_the_cwd_loses_its_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\C:\elsewhere"), Path::new("src/a.bynk")),
            "C:/elsewhere/src/a.bynk"
        );
    }
}
