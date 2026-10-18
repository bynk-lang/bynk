use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::discovery::read_source;
use crate::json::json_string;

/// v0.17 [DECISION L] stub: a version range is *unpinned* — and rejected — when
/// it is empty, `*`/`x`/`latest`, or otherwise carries no concrete version
/// number. A pinned range names at least one digit (`^5`, `~1.2`, `1.2.3`,
/// `>=1.0 <2`). No allow-list or registry check yet.
pub fn is_unpinned_range(range: &str) -> bool {
    let r = range.trim();
    if r.is_empty() || r == "*" || r.eq_ignore_ascii_case("x") || r.eq_ignore_ascii_case("latest") {
        return true;
    }
    !r.chars().any(|c| c.is_ascii_digit())
}

/// Render the build output's minimal `package.json`: `"type": "module"`, and
/// the adapter-declared dependencies when there are any.
///
/// #2148: the emitted modules are ES modules, and the emitted tsconfig's
/// `module: NodeNext` takes each file's format from the nearest
/// `package.json`. Declaring `"type": "module"` beside the output makes `tsc`
/// check and build it as ES modules, under the same rules Node applies when it
/// runs them (`.js` specifiers on relative imports, no CommonJS idioms).
pub fn render_package_json(deps: &std::collections::BTreeMap<String, String>) -> String {
    let mut out = String::from("{\n  \"type\": \"module\"");
    if !deps.is_empty() {
        out.push_str(",\n  \"dependencies\": {\n");
        let entries: Vec<String> = deps
            .iter()
            .map(|(pkg, range)| format!("    {}: {}", json_string(pkg), json_string(range)))
            .collect();
        out.push_str(&entries.join(",\n"));
        out.push_str("\n  }");
    }
    out.push_str("\n}\n");
    out
}

/// Normalise a relative path by resolving `.` and `..` components, so a binding
/// clause like `./tokens.binding.ts` beside `src/tokens.bynk` yields the output
/// path `tokens.binding.ts`.
pub fn normalize_rel(p: &Path) -> PathBuf {
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s.to_os_string()),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    out.iter().collect()
}

/// v0.113 (DECISION S): the project's source tree, read from `bynk.toml`'s
/// `[paths]` section. Test-ness is a property of the `suite` declaration, not of
/// a directory, so the layout is a flat **`include`** list of trees to compile
/// and an **`exclude`** list of subtrees to skip — not the role-named
/// `src`/`tests` split. Each `include` entry is a root walked for `.bynk` files;
/// a file's identity path is relative to the `include` root that contains it.
#[derive(Debug, Clone)]
pub struct ProjectPaths {
    /// Trees to compile, relative to the project root. Defaults to the
    /// conventional roots that exist (`src`, and `tests` when present), else the
    /// project root itself.
    pub include: Vec<PathBuf>,
    /// Subtrees to skip during discovery (monorepo, vendored, or generated
    /// `.bynk`), relative to the project root.
    pub exclude: Vec<PathBuf>,
}

impl ProjectPaths {
    /// The default layout when `bynk.toml` declares no `[paths] include`: the
    /// conventional `src`/`tests` roots that exist under `project_root`, or the
    /// project root itself when neither does. This keeps a conventional
    /// `src/`(+`tests/`) project working with no config, and lets a flat project
    /// (`.bynk` at the root, no `src/`) compile with no config either.
    pub fn conventional(project_root: &Path) -> Self {
        let mut include = Vec::new();
        for role in ["src", "tests"] {
            if project_root.join(role).is_dir() {
                include.push(PathBuf::from(role));
            }
        }
        if include.is_empty() {
            include.push(PathBuf::from("."));
        }
        ProjectPaths {
            include,
            exclude: Vec::new(),
        }
    }
}

/// Like [`try_read_project_paths`], but honours `overlay` for `bynk.toml`
/// itself — the in-memory test seam's (#57) one remaining disk read outside
/// `discovery::read_source`, now routed through the same helper so a test
/// can supply a virtual `bynk.toml` with no on-disk file at all. `#[cfg(test)]`
/// because that's its only consumer today; drop the gate if a non-test caller
/// needs it (`try_read_project_paths_with`, which this wraps, has none of that
/// restriction — production code already reaches it through the always-on
/// `try_read_project_paths`).
#[cfg(test)]
pub(crate) fn read_project_paths_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> ProjectPaths {
    try_read_project_paths_with(project_root, overlay)
        .unwrap_or_else(|_| ProjectPaths::conventional(project_root))
}

/// A problem in `bynk.toml` that [`try_read_project_paths`] surfaces instead
/// of silently falling back to the conventional layout.
#[derive(Debug)]
pub enum ProjectPathsError {
    /// `bynk.toml` exists but does not parse as TOML (e.g. a trailing comma).
    Malformed,
    /// `[paths]` has a key other than `include`/`exclude` — most likely a typo
    /// (`inculde`) that was silently read as "no include list".
    UnknownKey(String),
    /// #1665: a top-level entry that isn't one of the manifest's tables
    /// ([`MANIFEST_TABLES`]): a typo (`[pahts]`), or a table for a planned
    /// feature (`[dependencies]`) that would otherwise read as working.
    UnknownTable(String),
    /// #1665: a key at the top level of `bynk.toml`, outside any table.
    TopLevelKey(String),
    /// #1665: a key in `[project]`, `[lsp]` or `[workers]` that the table
    /// doesn't have. (`[paths]` reports [`Self::UnknownKey`]; `[fmt]` is
    /// checked by its own reader, which owns its keys.)
    UnknownTableKey { table: &'static str, key: String },
    /// #1890: a key that must be a list of strings holds something else: a
    /// bare string, a number, or a list with a non-string entry
    /// (`[workers] compatibility_flags = ["a", 1]`).
    NotAStringList {
        table: &'static str,
        key: &'static str,
    },
}

/// #1665: the tables `bynk.toml` may hold, each with the keys it accepts.
/// `[fmt]`'s keys are `bynk-fmt`'s to check (`FmtConfig`, `deny_unknown_fields`),
/// so its list here is empty and unchecked.
pub const MANIFEST_TABLES: &[(&str, &[&str])] = &[
    ("project", &["name", "version"]),
    ("paths", &["include", "exclude"]),
    ("fmt", &[]),
    ("lsp", &["diagnostics_mode", "diagnostics_debounce_ms"]),
    ("workers", &["compatibility_flags"]),
    ("unstable", &["allow"]),
];

/// Tables a user might write for a feature that is designed but not built,
/// with the issue that tracks it.
const PLANNED_TABLES: &[(&str, &str)] = &[
    ("dependencies", "#843"),
    ("dev-dependencies", "#843"),
    ("workspace", "#843"),
    ("deploy", "#551"),
];

/// The manifest's tables as
/// `` `[project]`, `[paths]`, `[fmt]`, `[lsp]` <conj> `[workers]` ``, from
/// [`MANIFEST_TABLES`], so a message can't fall behind it.
fn table_list(conj: &str) -> String {
    let names: Vec<String> = MANIFEST_TABLES
        .iter()
        .map(|(t, _)| format!("`[{t}]`"))
        .collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} {conj} {last}", rest.join(", ")),
        _ => names.join(""),
    }
}

/// The closest of `candidates` to `name`, if any is within two edits.
fn did_you_mean<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|c| (edit_distance(name, c), c))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Levenshtein distance over chars.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

impl std::fmt::Display for ProjectPathsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectPathsError::Malformed => write!(f, "`bynk.toml` is not valid TOML"),
            ProjectPathsError::UnknownKey(k) => {
                write!(
                    f,
                    "`[paths]` has no key named `{k}` — did you mean `include` or `exclude`?"
                )
            }
            ProjectPathsError::UnknownTable(name) => {
                write!(f, "`bynk.toml` has no table named `[{name}]` — ")?;
                if let Some((_, issue)) = PLANNED_TABLES.iter().find(|(t, _)| *t == name) {
                    write!(f, "it is planned but not yet supported ({issue})")
                } else if let Some(t) = did_you_mean(name, MANIFEST_TABLES.iter().map(|(t, _)| *t))
                {
                    write!(f, "did you mean `[{t}]`?")
                } else {
                    write!(f, "the tables are {}", table_list("and"))
                }
            }
            ProjectPathsError::TopLevelKey(key) => {
                // #1770 review: a table's own name written as a plain value
                // (`paths = "src"`).
                if MANIFEST_TABLES.iter().any(|(t, _)| t == key) {
                    return write!(
                        f,
                        "`{key}` in `bynk.toml` must be a table — write it as `[{key}]`"
                    );
                }
                write!(f, "`bynk.toml` has a key `{key}` outside any table — ")?;
                match MANIFEST_TABLES
                    .iter()
                    .find(|(_, keys)| keys.contains(&key.as_str()))
                {
                    Some((table, _)) => write!(f, "did you mean it under `[{table}]`?"),
                    None => write!(f, "keys belong in {}", table_list("or")),
                }
            }
            ProjectPathsError::UnknownTableKey { table, key } => {
                let keys = MANIFEST_TABLES
                    .iter()
                    .find(|(t, _)| t == table)
                    .map(|(_, k)| *k)
                    .unwrap_or_default();
                write!(f, "`[{table}]` has no key named `{key}` — ")?;
                match did_you_mean(key, keys.iter().copied()) {
                    Some(k) => write!(f, "did you mean `{k}`?"),
                    None => {
                        let list: Vec<String> = keys.iter().map(|k| format!("`{k}`")).collect();
                        write!(f, "its keys are {}", list.join(", "))
                    }
                }
            }
            ProjectPathsError::NotAStringList { table, key } => write!(
                f,
                "`[{table}]` `{key}` must be a list of strings, like `{key} = [\"nodejs_compat\"]`"
            ),
        }
    }
}

/// #1665: check `bynk.toml`'s table set and the keys of `[project]` and
/// `[lsp]` ([`MANIFEST_TABLES`]). Every other reader takes only its own table
/// and ignores the rest, so an unknown table (`[dependencies]`, a typo'd
/// `[pahts]`) used to build cleanly with none of its intended behaviour.
///
/// Kept apart from [`try_read_project_paths_with`] on purpose: the CLIs call
/// both and refuse an unknown table, while the language server reads `[paths]`
/// alone and keeps serving, so an extra table can't cost an editor its
/// `include` layout. `Ok` when there is no `bynk.toml`. A manifest that doesn't
/// parse is [`ProjectPathsError::Malformed`].
pub fn check_manifest(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<(), ProjectPathsError> {
    let toml_path = project_root.join("bynk.toml");
    let Ok(content) = read_source(&toml_path, overlay) else {
        return Ok(());
    };
    check_manifest_str(&content)
}

/// [`check_manifest`] over a manifest's text.
pub fn check_manifest_str(content: &str) -> Result<(), ProjectPathsError> {
    let doc = content
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    for (name, value) in &doc {
        // #1770 review: a planned table names its issue whatever shape it is
        // written in (`[[dependencies]]`, `dependencies = [...]`).
        if PLANNED_TABLES.iter().any(|(t, _)| t == name) {
            return Err(ProjectPathsError::UnknownTable(name.clone()));
        }
        let Some((table, keys)) = MANIFEST_TABLES.iter().find(|(t, _)| t == name) else {
            return Err(if value.is_table() {
                ProjectPathsError::UnknownTable(name.clone())
            } else {
                ProjectPathsError::TopLevelKey(name.clone())
            });
        };
        // #1770 review: a known table's name holding a plain value
        // (`paths = "src"`) would otherwise read as an absent table.
        let Some(entries) = value.as_table() else {
            return Err(ProjectPathsError::TopLevelKey(name.clone()));
        };
        if keys.is_empty() {
            continue;
        }
        if *table == "workers" {
            workers_config_from(entries)?;
        }
        if *table == "unstable" {
            unstable_config_from(entries)?;
        }
        for key in entries.keys() {
            if !keys.contains(&key.as_str()) {
                return Err(match *table {
                    "paths" => ProjectPathsError::UnknownKey(key.clone()),
                    _ => ProjectPathsError::UnknownTableKey {
                        table,
                        key: key.clone(),
                    },
                });
            }
        }
    }
    Ok(())
}

/// Read `bynk.toml`'s `[paths]` section, surfacing a malformed manifest — a
/// parse failure or an unrecognised `[paths]` key — as an error instead of
/// silently falling back to the conventional layout (the previous
/// `read_project_paths` total form's behaviour, R3.8 — deleted in favour of
/// this at all 18 of its callers, #1113).
///
/// R3.9 (#1113): `[paths] include` is no longer capped at one or two trees —
/// [`crate::roots::Roots::trees`] walks every entry, so this no longer rejects
/// a longer list.
pub fn try_read_project_paths(project_root: &Path) -> Result<ProjectPaths, ProjectPathsError> {
    let toml_path = project_root.join("bynk.toml");
    let overlay = match fs::read_to_string(&toml_path) {
        Ok(text) => HashMap::from([(toml_path, text)]),
        Err(_) => HashMap::new(),
    };
    try_read_project_paths_with(project_root, &overlay)
}

/// Like [`try_read_project_paths`], but honours `overlay` for `bynk.toml`
/// itself, the same way `discovery::read_source` does for every other file.
pub fn try_read_project_paths_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<ProjectPaths, ProjectPathsError> {
    let toml_path = project_root.join("bynk.toml");
    let Ok(content) = read_source(&toml_path, overlay) else {
        return Ok(ProjectPaths::conventional(project_root));
    };
    let Ok(doc) = content.parse::<toml::Table>() else {
        return Err(ProjectPathsError::Malformed);
    };
    let paths = doc.get("paths").and_then(|v| v.as_table());
    if let Some(t) = paths {
        for k in t.keys() {
            if k != "include" && k != "exclude" {
                return Err(ProjectPathsError::UnknownKey(k.clone()));
            }
        }
    }
    let list = |key: &str| -> Vec<PathBuf> {
        match paths.and_then(|t| t.get(key)) {
            Some(toml::Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str())
                .map(PathBuf::from)
                .collect(),
            Some(toml::Value::String(s)) => vec![PathBuf::from(s)],
            _ => Vec::new(),
        }
    };
    let mut include = list("include");
    let exclude = list("exclude");
    if include.is_empty() {
        include = ProjectPaths::conventional(project_root).include;
    }
    Ok(ProjectPaths { include, exclude })
}

/// #1890: the project's `[workers]` table, settings for the Cloudflare
/// Workers a `workers`-target build emits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkersConfig {
    /// Extra Cloudflare compatibility flags, in manifest order, for every
    /// Worker's `wrangler.toml`, after the compiler's default set. The names
    /// pass through unvalidated, since Cloudflare adds flags faster than a
    /// list here could follow. Duplicates are kept here: the emitter drops
    /// them and warns.
    pub compatibility_flags: Vec<String>,
}

/// Read `bynk.toml`'s `[workers]` table, honouring `overlay` for `bynk.toml`
/// the way [`try_read_project_paths_with`] does. No manifest, or no
/// `[workers]` table, is the empty [`WorkersConfig`]. A manifest that doesn't
/// parse is [`ProjectPathsError::Malformed`], and a `compatibility_flags`
/// that isn't a list of strings is [`ProjectPathsError::NotAStringList`]. The
/// shape is checked here as well as in [`check_manifest`], because some
/// callers (the e2e harness) read the table without the whole-manifest check.
pub fn try_read_workers_config_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<WorkersConfig, ProjectPathsError> {
    let toml_path = project_root.join("bynk.toml");
    let Ok(content) = read_source(&toml_path, overlay) else {
        return Ok(WorkersConfig::default());
    };
    let doc = content
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    match doc.get("workers").and_then(|v| v.as_table()) {
        Some(t) => workers_config_from(t),
        None => Ok(WorkersConfig::default()),
    }
}

/// [`try_read_workers_config_with`], reading `bynk.toml` from disk.
pub fn try_read_workers_config(project_root: &Path) -> Result<WorkersConfig, ProjectPathsError> {
    let toml_path = project_root.join("bynk.toml");
    let overlay = match fs::read_to_string(&toml_path) {
        Ok(text) => HashMap::from([(toml_path, text)]),
        Err(_) => HashMap::new(),
    };
    try_read_workers_config_with(project_root, &overlay)
}

/// #1890: the `bynk.project.duplicate_compatibility_flag` warning for one
/// flag `[workers] compatibility_flags` repeats, against `manifest` (the
/// project's `bynk.toml`). `default` says the repeat is of a flag every
/// Worker already has, rather than of an earlier entry in the list. Built
/// here, with the manifest's other diagnostics, rather than in `bynk-emit`,
/// which only finds the duplicate while merging the list (R3.5 keeps
/// diagnostics out of emission).
pub fn duplicate_compatibility_flag_warning(
    flag: &str,
    default: bool,
    manifest: PathBuf,
) -> crate::AttributedError {
    let message = if default {
        format!(
            "`bynk.toml`'s `[workers] compatibility_flags` lists `{flag}`, which every Bynk Worker already has"
        )
    } else {
        format!("`bynk.toml`'s `[workers] compatibility_flags` lists `{flag}` more than once")
    };
    let mut error = bynk_syntax::CompileError::new(
        "bynk.project.duplicate_compatibility_flag",
        bynk_syntax::span::Span::default(),
        message,
    );
    error
        .notes
        .push("the duplicate is dropped; remove it from `bynk.toml`".to_string());
    crate::AttributedError::new(Some(manifest), error)
}

/// The `[workers]` table's typed content. Only the value shapes are checked
/// here; an unknown key is [`check_manifest_str`]'s to refuse.
fn workers_config_from(table: &toml::Table) -> Result<WorkersConfig, ProjectPathsError> {
    let not_a_list = || ProjectPathsError::NotAStringList {
        table: "workers",
        key: "compatibility_flags",
    };
    let compatibility_flags = match table.get("compatibility_flags") {
        None => Vec::new(),
        Some(toml::Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(not_a_list))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(not_a_list()),
    };
    Ok(WorkersConfig {
        compatibility_flags,
    })
}

/// The project's `[unstable]` table (standard-library S3, #1963): the
/// experimental first-party items it opts into using.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnstableConfig {
    /// Item paths (`bynk.cloudflare.D1`, `bynk.cloudflare.Kv.list`), in
    /// manifest order. An entry covers itself and everything under it. Whether
    /// each one names something experimental is the checker's to say, since it
    /// holds the standard library's registry.
    pub allow: Vec<String>,
}

/// Read `bynk.toml`'s `[unstable]` table, honouring `overlay` the way
/// [`try_read_workers_config_with`] does. No manifest, or no `[unstable]`
/// table, is the empty [`UnstableConfig`]; an `allow` that isn't a list of
/// strings is [`ProjectPathsError::NotAStringList`].
pub fn try_read_unstable_config_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<UnstableConfig, ProjectPathsError> {
    let toml_path = project_root.join("bynk.toml");
    let Ok(content) = read_source(&toml_path, overlay) else {
        return Ok(UnstableConfig::default());
    };
    let doc = content
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    match doc.get("unstable").and_then(|v| v.as_table()) {
        Some(t) => unstable_config_from(t),
        None => Ok(UnstableConfig::default()),
    }
}

/// [`try_read_unstable_config_with`], reading `bynk.toml` from disk.
pub fn try_read_unstable_config(project_root: &Path) -> Result<UnstableConfig, ProjectPathsError> {
    let toml_path = project_root.join("bynk.toml");
    let overlay = match fs::read_to_string(&toml_path) {
        Ok(text) => HashMap::from([(toml_path, text)]),
        Err(_) => HashMap::new(),
    };
    try_read_unstable_config_with(project_root, &overlay)
}

/// The project's `[unstable]` table as both the build and the editor see it
/// (#1963 Decision E): the overlay's `bynk.toml` when it holds one (the LSP puts
/// the open buffer there), otherwise the one on disk (a CLI build's overlay holds
/// only `.bynk` sources). A missing or malformed manifest reads as empty; its
/// shape errors are `check_manifest`'s to report.
pub fn read_unstable_config(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> UnstableConfig {
    let manifest = project_root.join("bynk.toml");
    let read = if overlay.contains_key(&manifest) {
        try_read_unstable_config_with(project_root, overlay)
    } else {
        try_read_unstable_config(project_root)
    };
    read.unwrap_or_default()
}

/// The `[unstable]` table's typed content. Only the value shape is checked
/// here; an unknown key is [`check_manifest_str`]'s to refuse.
fn unstable_config_from(table: &toml::Table) -> Result<UnstableConfig, ProjectPathsError> {
    let not_a_list = || ProjectPathsError::NotAStringList {
        table: "unstable",
        key: "allow",
    };
    let allow = match table.get("allow") {
        None => Vec::new(),
        Some(toml::Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(not_a_list))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(not_a_list()),
    };
    Ok(UnstableConfig { allow })
}

pub fn commons_dir_for(name: &str) -> PathBuf {
    let parts: Vec<&str> = name.split('.').collect();
    let mut p = PathBuf::new();
    for part in parts {
        p.push(part);
    }
    p
}

pub fn ts_output_path(source: &Path) -> PathBuf {
    let mut out = source.to_path_buf();
    out.set_extension("ts");
    out
}

/// v0.8: directory name of a Worker for a given context, with dots replaced
/// by dashes (`commerce.payment` → `commerce-payment`).
pub fn worker_dir_name(context: &str) -> String {
    context.replace('.', "-")
}

/// v0.8: project-relative synthetic source path of the workers-mode
/// handlers file for a given context. Used so the emitter's relative-import
/// machinery resolves correctly against the workers layout.
pub fn worker_handlers_source_path(context: &str) -> PathBuf {
    PathBuf::from(format!(
        "workers/{}/handlers.bynk",
        worker_dir_name(context)
    ))
}

/// v0.8: project-relative output path of the workers-mode handlers file.
pub fn worker_handlers_output_path(context: &str) -> PathBuf {
    PathBuf::from(format!("workers/{}/handlers.ts", worker_dir_name(context)))
}

/// #1820: project-relative synthetic source path of one file's module in a
/// context split across files, on Workers. Each file is its own module under
/// `workers/<dir>/handlers/`, and `handlers.ts` re-exports them all, so the
/// Worker's entry point and composition root, and every consumer, still
/// import the context from `handlers.ts`. A file keeps its stem
/// (`shop/orders/place.bynk` → `handlers/place.bynk`). Distinct files map to
/// distinct modules because every file of a split unit sits directly in its
/// `<name>/` directory: a `<name>.bynk` beside it is rejected
/// (`bynk.project.file_and_directory`), as is a file in any other directory
/// (`bynk.project.inconsistent_commons_name`). The emitted module is
/// [`ts_output_path`] of this path.
pub fn worker_file_source_path(context: &str, source: &Path) -> PathBuf {
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    PathBuf::from(format!(
        "workers/{}/handlers/{stem}.bynk",
        worker_dir_name(context)
    ))
}

/// The src-stripped stem components of a path (`learner/uln.bynk` → `["learner",
/// "uln"]`), dropping the extension and any non-`Normal` components.
fn stem_parts(rel_path: &Path) -> Vec<String> {
    rel_path
        .with_extension("")
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect()
}

/// v0.9.1: shared between source-unit and test-unit path validation. The
/// caller decides which root to strip from the file path before calling.
///
/// A file belongs to `qualified_name` when it is either the single file
/// `<name>.bynk` (`single_file_match`: stem parts == name parts) or one file of
/// the directory layout `<name>/*.bynk` (`multi_file_match`: parent-dir parts ==
/// name parts). These two branches are the single source of truth the v0.132
/// barrel trigger reads via [`is_multi_file_layout`].
pub fn unit_path_matches(rel_path: &Path, qualified_name: &str) -> bool {
    let name_parts: Vec<&str> = qualified_name.split('.').collect();
    let stem_parts = stem_parts(rel_path);
    let single_file_match = stem_parts.len() == name_parts.len()
        && stem_parts
            .iter()
            .zip(name_parts.iter())
            .all(|(a, b)| a == b);
    single_file_match || is_multi_file_parts(&stem_parts, &name_parts)
}

/// True when `stem_parts` is one file of the `<name>/*.bynk` directory layout —
/// the file's parent-directory parts equal the name parts.
fn is_multi_file_parts(stem_parts: &[String], name_parts: &[&str]) -> bool {
    if stem_parts.is_empty() {
        return false;
    }
    let parent_parts = &stem_parts[..stem_parts.len() - 1];
    parent_parts.len() == name_parts.len()
        && parent_parts
            .iter()
            .zip(name_parts.iter())
            .all(|(a, b)| a == b)
}

/// v0.132: does `rel_path` (src-stripped) place `qualified_name` under a
/// directory of that name — the `multi_file_match` branch of
/// [`unit_path_matches`]?
///
/// This is the layout where production emits `out/<name>/*.ts` per file and no
/// aggregate `out/<name>.ts`, so the test path's `import * as ns from
/// "./<name>.js"` dangles and needs an aggregating barrel. A single-file commons
/// (`<name>.bynk`) already owns `out/<name>.ts` and returns false, so a barrel
/// keyed on this predicate can never collide with it.
pub fn is_multi_file_layout(rel_path: &Path, qualified_name: &str) -> bool {
    let name_parts: Vec<&str> = qualified_name.split('.').collect();
    is_multi_file_parts(&stem_parts(rel_path), &name_parts)
}

/// #302: the qualified name a file moved from `old_rel` to `new_rel` should now
/// declare, preserving whichever [`unit_path_matches`] arrangement `old_rel`
/// used to satisfy against `old_name` — the dotted stem for a single-file
/// unit, or the dotted parent-directory for one file of a multi-file unit.
/// Returns `None` if `old_rel`/`old_name` don't actually satisfy either
/// arrangement (a pre-existing inconsistency the caller should not guess at).
///
/// `old_name` is matched as a **suffix** of `old_rel`'s stem/parent, not the
/// whole thing: the LSP's caller passes project-relative paths, which (unlike
/// the `source_path` `unit_path_matches` itself is checked against) still
/// carry a leading `include`-root segment (e.g. `src/`) that the qualified
/// name never mentions. Whatever prefix length that suffix match implies for
/// `old_rel` is applied unchanged to `new_rel` — correct as long as the file
/// stays under the same `include` root, which a rename/move normally does.
pub fn renamed_unit_name(old_rel: &Path, old_name: &str, new_rel: &Path) -> Option<String> {
    let name_parts: Vec<&str> = old_name.split('.').collect();
    let old_stem = stem_parts(old_rel);
    let new_stem = stem_parts(new_rel);

    let suffix_matches = |haystack: &[String]| {
        haystack.len() >= name_parts.len() && {
            let prefix_len = haystack.len() - name_parts.len();
            haystack[prefix_len..]
                .iter()
                .zip(name_parts.iter())
                .all(|(a, b)| a == b)
        }
    };

    if suffix_matches(&old_stem) {
        let prefix_len = old_stem.len() - name_parts.len();
        return (new_stem.len() >= prefix_len).then(|| new_stem[prefix_len..].join("."));
    }
    if !old_stem.is_empty() {
        let old_parent = &old_stem[..old_stem.len() - 1];
        if suffix_matches(old_parent) {
            let prefix_len = old_parent.len() - name_parts.len();
            if new_stem.is_empty() {
                return None;
            }
            let new_parent = &new_stem[..new_stem.len() - 1];
            return (new_parent.len() >= prefix_len).then(|| new_parent[prefix_len..].join("."));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    // -- is_unpinned_range ----------------------------------------------------
    #[test]
    fn is_unpinned_range_true_for_wildcards_and_digitless() {
        assert!(is_unpinned_range(""));
        assert!(is_unpinned_range("*"));
        assert!(is_unpinned_range("x"));
        assert!(is_unpinned_range("X"));
        assert!(is_unpinned_range("latest"));
        assert!(is_unpinned_range("LATEST"));
        assert!(is_unpinned_range("  *  ")); // trimmed before the checks
        assert!(is_unpinned_range("workspace:*")); // no ascii digit
        assert!(is_unpinned_range("beta"));
    }

    #[test]
    fn is_unpinned_range_false_when_a_digit_is_present() {
        assert!(!is_unpinned_range("1.0.0"));
        assert!(!is_unpinned_range("^1.2"));
        assert!(!is_unpinned_range("~0.1"));
        assert!(!is_unpinned_range(">=2"));
        assert!(!is_unpinned_range("18"));
    }

    // -- read_project_paths_with (#57, in-memory test seam) -------------------
    #[test]
    fn read_project_paths_with_honours_a_virtual_bynk_toml() {
        let root = PathBuf::from("/nonexistent-bynk-test-root-57");
        let mut overlay = HashMap::new();
        overlay.insert(
            root.join("bynk.toml"),
            "[paths]\ninclude = [\"app\"]\nexclude = [\"vendor\"]\n".to_string(),
        );
        let paths = read_project_paths_with(&root, &overlay);
        assert_eq!(paths.include, vec![PathBuf::from("app")]);
        assert_eq!(paths.exclude, vec![PathBuf::from("vendor")]);
    }

    #[test]
    fn read_project_paths_with_falls_back_to_conventional_with_no_overlay_entry() {
        // No overlay entry and no real file at this (nonexistent) root — same
        // fallback a missing on-disk `bynk.toml` gives.
        let root = PathBuf::from("/nonexistent-bynk-test-root-57-empty");
        let paths = read_project_paths_with(&root, &HashMap::new());
        let conventional = ProjectPaths::conventional(&root);
        assert_eq!(paths.include, conventional.include);
        assert_eq!(paths.exclude, conventional.exclude);
    }

    /// R3.9 (#1113): three or more `[paths] include` entries all round-trip —
    /// `try_read_project_paths_with` no longer caps the list at two.
    #[test]
    fn read_project_paths_with_honours_three_or_more_include_entries() {
        let root = PathBuf::from("/nonexistent-bynk-test-root-1113-many-includes");
        let mut overlay = HashMap::new();
        overlay.insert(
            root.join("bynk.toml"),
            "[paths]\ninclude = [\"src\", \"tests\", \"examples\"]\n".to_string(),
        );
        let paths = try_read_project_paths_with(&root, &overlay).expect("must parse");
        assert_eq!(
            paths.include,
            vec![
                PathBuf::from("src"),
                PathBuf::from("tests"),
                PathBuf::from("examples"),
            ]
        );
    }

    // -- render_package_json --------------------------------------------------
    #[test]
    fn render_package_json_declares_es_modules() {
        // #2148: with no dependencies the file is just the module-format marker.
        let bare = render_package_json(&std::collections::BTreeMap::new());
        assert_eq!(bare, "{\n  \"type\": \"module\"\n}\n");
        let mut deps = std::collections::BTreeMap::new();
        deps.insert("jose".to_string(), "^5.0.0".to_string());
        let with_deps = render_package_json(&deps);
        assert_eq!(
            with_deps,
            "{\n  \"type\": \"module\",\n  \"dependencies\": {\n    \"jose\": \"^5.0.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn render_package_json_renders_sorted_dependencies() {
        let mut deps = std::collections::BTreeMap::new();
        deps.insert("zod".to_string(), "^3.22.4".to_string());
        deps.insert("hono".to_string(), "^4.0.0".to_string());
        let out = render_package_json(&deps);
        // BTreeMap ordering keeps the file byte-stable across builds.
        assert!(
            out.find("\"hono\"").unwrap() < out.find("\"zod\"").unwrap(),
            "dependencies render in sorted order:\n{out}"
        );
        assert!(out.contains("\"hono\": \"^4.0.0\""), "{out}");
    }

    /// A package name and version range reach here from adapter declarations in
    /// Bynk source, so they are arbitrary text. This module used to escape only
    /// `"` and `\`, which let a control character through as a literal — and a
    /// literal control character inside a JSON string is a parse error, so the
    /// emitted `package.json` was invalid rather than merely odd.
    #[test]
    fn render_package_json_escapes_the_control_range() {
        let mut deps = std::collections::BTreeMap::new();
        deps.insert("pkg\nname".to_string(), "^1.0\u{1}0".to_string());
        let out = render_package_json(&deps);
        assert!(out.contains("\"pkg\\nname\""), "{out}");
        assert!(out.contains("\"^1.0\\u00010\""), "{out}");
        // No raw control character survives into the rendered document (the
        // pretty-printer's own newlines are all that remain).
        assert!(
            !out.lines().any(|l| l.chars().any(|c| (c as u32) < 0x20)),
            "a raw control character reached the output:\n{out:?}"
        );
    }

    #[test]
    fn render_package_json_escapes_structural_characters() {
        let mut deps = std::collections::BTreeMap::new();
        deps.insert("a\"b".to_string(), "c\\d".to_string());
        let out = render_package_json(&deps);
        assert!(out.contains(r#""a\"b": "c\\d""#), "{out}");
    }

    // -- normalize_rel --------------------------------------------------------
    #[test]
    fn normalize_rel_resolves_dot_and_parent() {
        assert_eq!(
            normalize_rel(Path::new("./tokens.binding.ts")),
            PathBuf::from("tokens.binding.ts")
        );
        assert_eq!(normalize_rel(Path::new("a/./b")), PathBuf::from("a/b"));
        assert_eq!(normalize_rel(Path::new("a/../b")), PathBuf::from("b"));
        assert_eq!(normalize_rel(Path::new("a/b/../../c")), PathBuf::from("c"));
        assert_eq!(normalize_rel(Path::new("a/b")), PathBuf::from("a/b"));
    }

    #[test]
    fn normalize_rel_drops_root_and_pops_through_empty() {
        // RootDir / Prefix components are dropped.
        assert_eq!(normalize_rel(Path::new("/a/b")), PathBuf::from("a/b"));
        // A leading `..` pops an empty stack (a no-op), so it vanishes.
        assert_eq!(normalize_rel(Path::new("../a")), PathBuf::from("a"));
    }

    // -- commons_dir_for / ts_output_path -------------------------------------
    #[test]
    fn commons_dir_for_splits_dotted_name_into_dirs() {
        assert_eq!(commons_dir_for("a.b.c"), PathBuf::from("a/b/c"));
        assert_eq!(commons_dir_for("foo"), PathBuf::from("foo"));
    }

    #[test]
    fn ts_output_path_sets_ts_extension() {
        assert_eq!(
            ts_output_path(Path::new("foo.bynk")),
            PathBuf::from("foo.ts")
        );
        assert_eq!(
            ts_output_path(Path::new("a/b.bynk")),
            PathBuf::from("a/b.ts")
        );
        assert_eq!(ts_output_path(Path::new("foo")), PathBuf::from("foo.ts"));
    }

    // -- worker path helpers --------------------------------------------------
    #[test]
    fn worker_paths_dasherise_and_root_under_workers() {
        assert_eq!(worker_dir_name("commerce.payment"), "commerce-payment");
        assert_eq!(worker_dir_name("plain"), "plain");
        assert_eq!(
            worker_handlers_source_path("commerce.payment"),
            PathBuf::from("workers/commerce-payment/handlers.bynk")
        );
        assert_eq!(
            worker_handlers_output_path("commerce.payment"),
            PathBuf::from("workers/commerce-payment/handlers.ts")
        );
    }

    #[test]
    fn a_split_context_file_has_its_own_module_under_handlers() {
        assert_eq!(
            worker_file_source_path("shop.orders", Path::new("shop/orders/place.bynk")),
            PathBuf::from("workers/shop-orders/handlers/place.bynk")
        );
    }

    // -- unit_path_matches ----------------------------------------------------
    #[test]
    fn unit_path_matches_single_file_layout() {
        assert!(unit_path_matches(Path::new("a/b/c.bynk"), "a.b.c"));
        assert!(unit_path_matches(Path::new("foo.bynk"), "foo"));
    }

    #[test]
    fn unit_path_matches_multi_file_layout() {
        // `a/b/c/<any>.bynk` declaring `a.b.c` (the directory is the unit).
        assert!(unit_path_matches(Path::new("a/b/c/handlers.bynk"), "a.b.c"));
        assert!(unit_path_matches(Path::new("a/b/c/anything.bynk"), "a.b.c"));
    }

    #[test]
    fn unit_path_matches_rejects_misalignment() {
        assert!(!unit_path_matches(Path::new("a/b.bynk"), "a.b.c"));
        assert!(!unit_path_matches(Path::new("x/y/z.bynk"), "a.b.c"));
    }

    // -- is_multi_file_layout (v0.132 barrel trigger) -------------------------
    #[test]
    fn is_multi_file_layout_true_only_for_directory_layout() {
        // Directory layout: `<name>/*.bynk` — the branch with no `out/<name>.ts`.
        assert!(is_multi_file_layout(Path::new("thing/a.bynk"), "thing"));
        assert!(is_multi_file_layout(Path::new("thing/b.bynk"), "thing"));
        // Dotted commons split across `src/a/b/*.bynk`.
        assert!(is_multi_file_layout(Path::new("a/b/one.bynk"), "a.b"));
    }

    #[test]
    fn is_multi_file_layout_false_for_single_file_and_misalignment() {
        // Single file `<name>.bynk` already owns `out/<name>.ts` — no barrel.
        assert!(!is_multi_file_layout(Path::new("thing.bynk"), "thing"));
        // Dotted single file `a/b.bynk` for `a.b` — the file *is* `out/a/b.ts`.
        assert!(!is_multi_file_layout(Path::new("a/b.bynk"), "a.b"));
        // Wrong directory — not this unit's file.
        assert!(!is_multi_file_layout(Path::new("other/a.bynk"), "thing"));
    }

    // -- renamed_unit_name (#302) ----------------------------------------------
    #[test]
    fn renamed_unit_name_single_file() {
        assert_eq!(
            renamed_unit_name(
                Path::new("a/b/c.bynk"),
                "a.b.c",
                Path::new("a/b/renamed.bynk")
            ),
            Some("a.b.renamed".to_string())
        );
        assert_eq!(
            renamed_unit_name(Path::new("foo.bynk"), "foo", Path::new("bar.bynk")),
            Some("bar".to_string())
        );
    }

    #[test]
    fn renamed_unit_name_multi_file_member_rename_is_a_no_op() {
        // Renaming one member file within the same directory doesn't change
        // the unit's name — the qualified name is the directory, not the
        // filename.
        assert_eq!(
            renamed_unit_name(
                Path::new("a/b/c/old.bynk"),
                "a.b.c",
                Path::new("a/b/c/new.bynk")
            ),
            Some("a.b.c".to_string())
        );
    }

    #[test]
    fn renamed_unit_name_multi_file_directory_move() {
        assert_eq!(
            renamed_unit_name(
                Path::new("a/b/c/handlers.bynk"),
                "a.b.c",
                Path::new("a/b/renamed/handlers.bynk")
            ),
            Some("a.b.renamed".to_string())
        );
    }

    #[test]
    fn renamed_unit_name_none_on_preexisting_misalignment() {
        assert_eq!(
            renamed_unit_name(Path::new("x/y/z.bynk"), "a.b.c", Path::new("x/y/w.bynk")),
            None
        );
    }

    #[test]
    fn renamed_unit_name_tolerates_a_shared_include_root_prefix() {
        // The LSP passes project-relative paths (ADR 0198), which still carry
        // a split project's `src`/`tests` root segment — `unit_path_matches`
        // itself is only ever checked against the root-stripped `source_path`.
        // `old_name` must match as a *suffix*, and the same leading-segment
        // count is preserved onto `new_rel`.
        assert_eq!(
            renamed_unit_name(
                Path::new("src/billing/charge.bynk"),
                "billing.charge",
                Path::new("src/billing/pay.bynk")
            ),
            Some("billing.pay".to_string())
        );
        // Multi-file arrangement under the same prefix.
        assert_eq!(
            renamed_unit_name(
                Path::new("src/a/b/c/handlers.bynk"),
                "a.b.c",
                Path::new("src/a/b/renamed/handlers.bynk")
            ),
            Some("a.b.renamed".to_string())
        );
    }
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    fn err(manifest: &str) -> String {
        check_manifest_str(manifest)
            .expect_err("the manifest is refused")
            .to_string()
    }

    /// #1665: every table the docs describe, with every key, is accepted.
    #[test]
    fn the_documented_tables_are_accepted() {
        check_manifest_str(
            "[project]\nname = \"p\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\nexclude = []\n\n\
             [fmt]\nindent = \"tab\"\nmax_line_width = 100\n\n[lsp]\ndiagnostics_mode = \"live\"\n\
             diagnostics_debounce_ms = 300\n\n[workers]\ncompatibility_flags = [\"nodejs_compat\"]\n",
        )
        .expect("accepted");
        check_manifest_str("").expect("an empty manifest is accepted");
    }

    #[test]
    fn a_planned_table_names_its_issue() {
        assert!(err("[dependencies]\nx = \"1\"\n").contains("not yet supported (#843)"));
        assert!(err("[deploy]\ngroups = []\n").contains("not yet supported (#551)"));
        assert!(err("[workspace]\nmembers = []\n").contains("(#843)"));
    }

    #[test]
    fn a_typod_table_suggests_the_nearest() {
        assert!(err("[pahts]\n").contains("did you mean `[paths]`?"));
        assert!(err("[fnt]\n").contains("did you mean `[fmt]`?"));
        assert!(err("[zzzzzz]\n").contains("the tables are"));
    }

    #[test]
    fn an_unknown_key_in_project_or_lsp_suggests_the_nearest() {
        assert!(err("[project]\nnmae = \"p\"\n").contains("did you mean `name`?"));
        assert!(
            err("[lsp]\ndiagnostics_mod = \"live\"\n").contains("did you mean `diagnostics_mode`?")
        );
        assert!(err("[project]\nlicense = \"MIT\"\n").contains("its keys are `name`, `version`"));
    }

    #[test]
    fn a_paths_key_keeps_its_own_error() {
        assert!(matches!(
            check_manifest_str("[paths]\nout = \"out\"\n"),
            Err(ProjectPathsError::UnknownKey(k)) if k == "out"
        ));
    }

    #[test]
    fn a_top_level_key_points_at_its_table() {
        assert!(err("name = \"p\"\n").contains("did you mean it under `[project]`?"));
    }

    /// #1770 review: a known table's name holding a plain value is refused,
    /// not read as an absent table.
    #[test]
    fn a_table_name_with_a_plain_value_must_be_a_table() {
        assert!(err("paths = \"src\"\n").contains("must be a table — write it as `[paths]`"));
        assert!(err("lsp = 3\n").contains("`[lsp]`"));
        assert!(err("fmt = true\n").contains("`[fmt]`"));
        // An inline table is a table.
        check_manifest_str("paths = { include = [\"src\"] }\n").expect("accepted");
    }

    /// #1770 review: a planned table keeps its issue whatever shape it takes.
    #[test]
    fn a_planned_table_in_any_shape_names_its_issue() {
        assert!(err("[[dependencies]]\nname = \"acme\"\n").contains("(#843)"));
        assert!(err("dependencies = [\"acme\"]\n").contains("(#843)"));
    }

    /// `[fmt]` keys are `bynk-fmt`'s to check, not this one's.
    #[test]
    fn fmt_keys_are_left_to_the_formatter() {
        check_manifest_str("[fmt]\nanything = 1\n").expect("not this check's concern");
    }

    /// #1890: `[workers] compatibility_flags` must be a list of strings; the
    /// names themselves pass through.
    #[test]
    fn workers_compatibility_flags_must_be_a_list_of_strings() {
        let msg = "`[workers]` `compatibility_flags` must be a list of strings";
        assert!(err("[workers]\ncompatibility_flags = \"nodejs_compat\"\n").contains(msg));
        assert!(err("[workers]\ncompatibility_flags = [\"a\", 1]\n").contains(msg));
        assert!(err("[workers]\ncompatibility_flags = [[\"a\"]]\n").contains(msg));
        assert!(
            err("[workers]\ncompatibility_flag = []\n")
                .contains("did you mean `compatibility_flags`?")
        );
        check_manifest_str("[workers]\ncompatibility_flags = [\"no_such_flag_yet\"]\n")
            .expect("flag names are not validated");
        check_manifest_str("[workers]\n").expect("an empty `[workers]` is accepted");
    }

    /// #1963: `[unstable] allow` must be a list of strings; whether each names
    /// something experimental is the checker's to say.
    #[test]
    fn unstable_allow_must_be_a_list_of_strings() {
        let msg = "`[unstable]` `allow` must be a list of strings";
        assert!(err("[unstable]\nallow = \"bynk.cloudflare.D1\"\n").contains(msg));
        assert!(err("[unstable]\nallow = [\"a\", 1]\n").contains(msg));
        assert!(err("[unstable]\nallows = []\n").contains("did you mean `allow`?"));
        check_manifest_str("[unstable]\nallow = [\"bynk.anything\"]\n")
            .expect("item paths are the checker's to validate");
        check_manifest_str("[unstable]\n").expect("an empty `[unstable]` is accepted");
    }

    #[test]
    fn unstable_config_reads_the_entries_in_order() {
        let root = PathBuf::from("/nonexistent-bynk-test-root-1963");
        let read = |manifest: &str| {
            let overlay = HashMap::from([(root.join("bynk.toml"), manifest.to_string())]);
            try_read_unstable_config_with(&root, &overlay)
        };
        assert_eq!(
            read("[unstable]\nallow = [\"b.x\", \"a.y\"]\n")
                .unwrap()
                .allow,
            vec!["b.x", "a.y"]
        );
        assert_eq!(
            read("[project]\nname = \"p\"\n").unwrap(),
            UnstableConfig::default()
        );
        assert!(matches!(
            read("[unstable]\nallow = 3\n"),
            Err(ProjectPathsError::NotAStringList {
                table: "unstable",
                key: "allow"
            })
        ));
    }

    #[test]
    fn workers_config_reads_the_flags_in_order() {
        let root = PathBuf::from("/nonexistent-bynk-test-root-1890");
        let read = |manifest: &str| {
            let overlay = HashMap::from([(root.join("bynk.toml"), manifest.to_string())]);
            try_read_workers_config_with(&root, &overlay)
        };
        assert_eq!(
            read("[workers]\ncompatibility_flags = [\"b\", \"a\", \"b\"]\n")
                .unwrap()
                .compatibility_flags,
            vec!["b", "a", "b"],
            "order and duplicates are kept; deduplication is the emitter's"
        );
        assert_eq!(
            read("[project]\nname = \"p\"\n").unwrap(),
            WorkersConfig::default()
        );
        assert_eq!(
            try_read_workers_config_with(&root, &HashMap::new()).unwrap(),
            WorkersConfig::default(),
            "no manifest at all"
        );
        assert!(matches!(
            read("[workers]\ncompatibility_flags = [true]\n"),
            Err(ProjectPathsError::NotAStringList { .. })
        ));
        assert!(matches!(
            read("[workers\n"),
            Err(ProjectPathsError::Malformed)
        ));
    }

    #[test]
    fn a_malformed_manifest_is_malformed() {
        assert!(matches!(
            check_manifest_str("[project\n"),
            Err(ProjectPathsError::Malformed)
        ));
    }

    /// #2096: the shapes `bynk.toml`'s JSON Schema accepts for the hand-read
    /// tables are the shapes their readers accept, and the shapes it flags for
    /// `compatibility_flags`/`allow` are refused. The schema is written apart
    /// from these readers (`manifest_schema`), so this pins the two together.
    #[test]
    fn the_readers_accept_the_shapes_the_manifest_schema_offers() {
        let root = PathBuf::from("/nonexistent-bynk-test-root-2096");
        let overlay =
            |manifest: &str| HashMap::from([(root.join("bynk.toml"), manifest.to_string())]);

        // `[paths] include`: a bare string and a list both read as the one root.
        for manifest in [
            "[paths]\ninclude = \"src\"\n",
            "[paths]\ninclude = [\"src\"]\n",
        ] {
            check_manifest_str(manifest).expect("the manifest check accepts it");
            let paths = try_read_project_paths_with(&root, &overlay(manifest))
                .unwrap_or_else(|e| panic!("{manifest:?}: {e}"));
            assert_eq!(paths.include, vec![PathBuf::from("src")], "{manifest:?}");
        }

        // `[workers] compatibility_flags` and `[unstable] allow`: lists of strings.
        let manifest = "[workers]\ncompatibility_flags = [\"x\"]\n";
        check_manifest_str(manifest).expect("the manifest check accepts it");
        assert_eq!(
            try_read_workers_config_with(&root, &overlay(manifest))
                .unwrap()
                .compatibility_flags,
            vec!["x"]
        );
        let manifest = "[unstable]\nallow = [\"x\"]\n";
        check_manifest_str(manifest).expect("the manifest check accepts it");
        assert_eq!(
            try_read_unstable_config_with(&root, &overlay(manifest))
                .unwrap()
                .allow,
            vec!["x"]
        );

        // A bare string is a list only for `[paths]`: the schema flags it for
        // these two, and so do the readers.
        let manifest = "[workers]\ncompatibility_flags = \"x\"\n";
        assert!(matches!(
            try_read_workers_config_with(&root, &overlay(manifest)),
            Err(ProjectPathsError::NotAStringList { .. })
        ));
        assert!(check_manifest_str(manifest).is_err());
        let manifest = "[unstable]\nallow = \"x\"\n";
        assert!(matches!(
            try_read_unstable_config_with(&root, &overlay(manifest)),
            Err(ProjectPathsError::NotAStringList { .. })
        ));
        assert!(check_manifest_str(manifest).is_err());
    }
}
