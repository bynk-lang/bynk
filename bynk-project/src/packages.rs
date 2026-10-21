//! Packaging slice 2a (#2223, track `design/tracks/packaging.md` §3.3, §6.3):
//! the dependency graph of a project's path dependencies.
//!
//! [`resolve_graph`] reads the project's `[dependencies]`, then each path
//! dependency's own `bynk.toml`, and returns the packages the build loads:
//! where each lives, the source trees it contributes, what it `exports`, and
//! which package each of its aliases names. It resolves nothing over a
//! network and reads only manifests — the overlay first, then the disk, the
//! way [`crate::read_package_config`] already does — so the CLI, `check` and
//! the editor build the same graph from the same project root.
//!
//! A dependency's units get a **canonical name** that source can't spell
//! ([`PackageId::encoded`]): the unit `currency` of `acme/money` is
//! `$acme_money.currency` inside the compiler. `$` is never part of a Bynk
//! identifier, so the name can't collide with any unit a project declares; it
//! is a valid TypeScript identifier once `.` becomes `_`, so the emitter's
//! identifier sites need no change; and [`crate::commons_dir_for`] maps it to
//! `deps/<org>/<package>/…`, the output layout (decision G). Diagnostics show
//! it as `acme/money:currency` ([`display_names`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use crate::discovery::read_source;
use crate::manifest::{DependencySource, DependencySpec, dependencies_from, package_config_from};

/// A package's identity: `org/package`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId {
    pub org: String,
    pub package: String,
}

impl PackageId {
    /// `org/package`, as `bynk.toml` writes it.
    pub fn name(&self) -> String {
        format!("{}/{}", self.org, self.package)
    }

    /// The first segment of every canonical unit name in this package:
    /// `$` + the org (each `-` written `$`) + `_` + the package. An org is
    /// `[a-z][a-z0-9-]*`, so it never contains `_`, and the first `_` always
    /// ends it: the encoding is injective, and [`PackageId::decode`] inverts it.
    pub fn encoded(&self) -> String {
        format!("${}_{}", self.org.replace('-', "$"), self.package)
    }

    /// Where this package's units are emitted, relative to `out/`.
    pub fn out_dir(&self) -> PathBuf {
        PathBuf::from("deps").join(&self.org).join(&self.package)
    }

    /// The canonical name of `unit` in this package.
    pub fn canonical(&self, unit: &str) -> String {
        format!("{}.{unit}", self.encoded())
    }

    /// The package an encoded first segment names, or `None` when `segment`
    /// isn't one.
    pub fn decode(segment: &str) -> Option<PackageId> {
        let rest = segment.strip_prefix('$')?;
        let (org, package) = rest.split_once('_')?;
        if org.is_empty() || package.is_empty() {
            return None;
        }
        Some(PackageId {
            org: org.replace('$', "-"),
            package: package.to_owned(),
        })
    }
}

/// Split a canonical dependency unit name into its package and the unit name
/// the package itself declares. `None` for a project's own or first-party unit.
pub fn split_canonical(name: &str) -> Option<(PackageId, &str)> {
    let (first, rest) = name.split_once('.')?;
    Some((PackageId::decode(first)?, rest))
}

/// `name` as a person reads it: a canonical dependency unit name becomes
/// `org/package:unit`; any other name is returned unchanged.
pub fn display_unit_name(name: &str) -> String {
    match split_canonical(name) {
        Some((id, unit)) => format!("{}:{unit}", id.name()),
        None => name.to_owned(),
    }
}

/// Rewrite every canonical dependency unit name in `text` (a diagnostic's
/// message or note) to its [`display_unit_name`] form, and a bare encoded
/// package segment to `org/package`.
pub fn display_names(text: &str) -> String {
    if !text.contains('$') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        // The encoded segment runs over `$`, `_` and `[a-z0-9]`.
        let end = tail[1..]
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$'))
            .map_or(tail.len(), |i| i + 1);
        let segment = &tail[..end];
        match PackageId::decode(segment) {
            Some(id) if tail[end..].starts_with('.') => {
                out.push_str(&id.name());
                out.push(':');
                rest = &tail[end + 1..];
            }
            Some(id) => {
                out.push_str(&id.name());
                rest = &tail[end..];
            }
            None => {
                out.push('$');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One dependency package in a resolved graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub id: PackageId,
    /// The directory holding its `bynk.toml`, as the graph reached it.
    pub dir: PathBuf,
    /// Its source trees as `(root, identity prefix)` pairs, in the shape
    /// [`crate::Roots::trees`] returns. The prefix is the tree's path relative
    /// to the consuming project's root, so a diagnostic in a dependency names
    /// a real file.
    pub trees: Vec<(PathBuf, PathBuf)>,
    /// `[package] exports`, when listed; `None` shares every commons and
    /// adapter (ADR 0542).
    pub exports: Option<Vec<String>>,
    /// Each of its own aliases, and the package (an index into
    /// [`PackageGraph::packages`]) it names.
    pub aliases: BTreeMap<String, usize>,
    /// #2240: whether it came from the store (a git dependency), so its own
    /// path dependencies are never honoured.
    pub fetched: bool,
}

/// A project's resolved dependency graph (single version: one entry per
/// package, however many aliases reach it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageGraph {
    /// The consuming project's aliases.
    pub root_aliases: BTreeMap<String, usize>,
    /// #2240: the aliases of `root_aliases` that come only from
    /// `[dev-dependencies]`, so only a suite may reach them.
    pub dev_aliases: BTreeSet<String>,
    /// #2240: the project's aliases whose package couldn't be resolved (a
    /// problem already reported, such as `not_fetched`), so a reference
    /// through one is dropped rather than reported again as unknown.
    pub unresolved_aliases: BTreeSet<String>,
    pub packages: Vec<Package>,
}

impl PackageGraph {
    /// Every dependency source tree, in package order.
    pub fn trees(&self) -> Vec<(PathBuf, PathBuf)> {
        self.packages
            .iter()
            .flat_map(|p| p.trees.iter().cloned())
            .collect()
    }

    /// The package whose source tree contains `path` — a parsed file's
    /// `abs_path`, which is `std::path::absolute` of the path discovery found,
    /// so each tree root is compared in that same form.
    ///
    /// Both sides are resolved lexically too, so `shop/../packages/money/…`
    /// (a project root joined with a dependency file's identity path) matches.
    pub fn package_of(&self, path: &Path) -> Option<usize> {
        let path = lexical(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
        self.packages.iter().position(|p| {
            p.trees.iter().any(|(root, _)| {
                let root = lexical(&std::path::absolute(root).unwrap_or_else(|_| root.clone()));
                path.starts_with(&root)
            })
        })
    }

    /// The package with this identity, if the graph has it.
    pub fn by_id(&self, id: &PackageId) -> Option<usize> {
        self.packages.iter().position(|p| &p.id == id)
    }
}

/// What is wrong with a graph, as a coded diagnostic's parts. `bynk-check`
/// turns each into a `CompileError` attributed to `bynk.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphProblem {
    pub code: &'static str,
    pub message: String,
    pub note: Option<String>,
}

impl GraphProblem {
    fn new(code: &'static str, message: String) -> Self {
        GraphProblem {
            code,
            message,
            note: None,
        }
    }

    fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// A dependency's `bynk.toml` isn't readable, or isn't a well-formed package.
pub const DEPENDENCY_NOT_FOUND: &str = "bynk.package.dependency_not_found";
/// A path dependency's `[package] name` isn't the package its entry declares.
pub const NAME_MISMATCH: &str = "bynk.package.name_mismatch";
/// One package reached from two different directories.
pub const DUPLICATE_SOURCE: &str = "bynk.package.duplicate_source";
/// The package graph has a cycle.
pub const DEPENDENCY_CYCLE: &str = "bynk.package.dependency_cycle";
/// A source kind a later slice resolves: git (2b) or a registry version (4).
pub const UNSUPPORTED_SOURCE: &str = "bynk.package.unsupported_source";
/// A path dependency declared by a dependency, outside the workspace.
pub const PATH_OUTSIDE_ROOT: &str = "bynk.dependency.path_outside_root";
/// #2240: a git dependency with no lock entry, or whose store entry is
/// missing: `bynk fetch` fetches it.
pub const NOT_FETCHED: &str = "bynk.package.not_fetched";
/// #2240: one package reached at two different git commits.
pub const VERSION_CONFLICT: &str = "bynk.package.version_conflict";
/// #2240: `bynk.lock` can't be read.
pub const LOCK_INVALID: &str = "bynk.package.lock_invalid";

/// Resolve the path-dependency graph of the project at `project_root`.
/// `None` with no problems when the project declares no dependencies.
///
/// A path dependency in the project's own manifest is the author's own file
/// and is honoured wherever it points. One declared by a *dependency* is
/// honoured only inside the workspace the project belongs to (track §6.3,
/// decision F): outside a workspace it is refused, so no dependency can make
/// the build read a directory the author never named.
pub fn resolve_graph(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> (Option<PackageGraph>, Vec<GraphProblem>) {
    let mut problems = Vec::new();
    let Some(root_doc) = read_manifest(project_root, overlay) else {
        return (None, problems);
    };
    let deps = specs_of(&root_doc);
    let dev = dev_specs_of(&root_doc);
    if deps.is_empty() && dev.is_empty() {
        return (None, problems);
    }
    let normal: BTreeSet<String> = deps.iter().map(|s| s.alias.clone()).collect();
    let root_specs: Vec<(DependencySpec, bool)> = deps
        .into_iter()
        .map(|s| (s, false))
        .chain(
            dev.into_iter()
                .filter(|s| !normal.contains(&s.alias))
                .map(|s| (s, true)),
        )
        .collect();
    // #2240: git dependencies resolve through the lock to the store.
    let lock = match crate::lock::read_lock(&crate::lock::lock_root(project_root), overlay) {
        Ok(lock) => lock,
        Err(e) => {
            problems.push(GraphProblem::new(LOCK_INVALID, e.0));
            None
        }
    };
    let home = crate::lock::bynk_home();
    let workspace = crate::workspace::find_parent_workspace(project_root).or_else(|| {
        crate::workspace::is_workspace_root(project_root).then(|| project_root.to_path_buf())
    });
    let workspace = workspace.map(|w| real(&w));
    let root_real = real(project_root);

    let mut graph = PackageGraph::default();
    // Directory (real) of each package, for duplicate and cycle detection.
    let mut dirs: Vec<PathBuf> = Vec::new();
    // (declaring package or `None` for the root, its directory, spec, dev-only)
    let mut queue: Vec<(Option<usize>, PathBuf, DependencySpec, bool)> = root_specs
        .into_iter()
        .map(|(s, dev)| (None, project_root.to_path_buf(), s, dev))
        .collect();
    let mut cursor = 0;
    while cursor < queue.len() {
        let (declarer, declarer_dir, spec, dev) = queue[cursor].clone();
        cursor += 1;
        let declarer_fetched = declarer.is_some_and(|i| graph.packages[i].fetched);
        let who = match declarer {
            None => "this project".to_string(),
            Some(i) => format!("dependency `{}`", graph.packages[i].id.name()),
        };
        let mut fetched = false;
        let (dir, rel) = match &spec.source {
            DependencySource::Path(p) => {
                if declarer_fetched {
                    problems.push(GraphProblem::new(
                        PATH_OUTSIDE_ROOT,
                        format!(
                            "{who} declares the path dependency `{}` (`{p}`), and a fetched package's path dependencies are never honoured",
                            spec.alias
                        ),
                    ));
                    continue;
                }
                (lexical_join(&declarer_dir, Path::new(p)), p.clone())
            }
            DependencySource::Git { url, rev } => {
                if declarer_fetched && url.starts_with("file:") {
                    problems.push(GraphProblem::new(
                        PATH_OUTSIDE_ROOT,
                        format!(
                            "{who} declares the git dependency `{}` at the local URL `{url}`, and a fetched package can't reach a local repository",
                            spec.alias
                        ),
                    ));
                    continue;
                }
                let store = lock
                    .as_ref()
                    .and_then(|l| l.git(&spec.package, url, rev))
                    .and_then(|(entry, _)| {
                        crate::lock::store_dir(home.as_deref()?, entry.checksum.as_deref()?)
                    })
                    .filter(|d| d.is_dir());
                let Some(store) = store else {
                    problems.push(GraphProblem::new(
                        NOT_FETCHED,
                        format!(
                            "{who}'s git dependency `{}` (`{url}` at `{rev}`) hasn't been fetched",
                            spec.alias
                        ),
                    ).with_note("run `bynk fetch` (or any `bynk` command without `--offline`) to fetch it and update `bynk.lock`"));
                    continue;
                };
                fetched = true;
                (store, format!("{url} at {rev}"))
            }
            DependencySource::Version(_) => {
                problems.push(GraphProblem::new(
                    UNSUPPORTED_SOURCE,
                    format!(
                        "{who}'s dependency `{}` is a registry dependency, which can't be resolved until packaging slice 4 (#843)",
                        spec.alias
                    ),
                ).with_note("for now, use a `path = \"…\"` dependency"));
                continue;
            }
        };
        let dir_real = real(&dir);
        if declarer.is_some() && !fetched {
            let inside = workspace.as_ref().is_some_and(|w| dir_real.starts_with(w));
            if !inside {
                problems.push(GraphProblem::new(
                    PATH_OUTSIDE_ROOT,
                    format!(
                        "{who} declares the path dependency `{}` (`{rel}`), which is outside {}",
                        spec.alias,
                        if workspace.is_some() { "the workspace" } else { "any workspace" }
                    ),
                ).with_note("a dependency's own path dependencies are honoured only between members of one workspace"));
                continue;
            }
        }
        if dir_real == root_real {
            problems.push(GraphProblem::new(
                DEPENDENCY_CYCLE,
                format!(
                    "{who}'s dependency `{}` is this project itself, so the package graph has a cycle",
                    spec.alias
                ),
            ));
            continue;
        }
        let index = if let Some(existing) = dirs.iter().position(|d| *d == dir_real) {
            let id = &graph.packages[existing].id;
            if id.name() != spec.package {
                problems.push(GraphProblem::new(
                    NAME_MISMATCH,
                    format!(
                        "{who}'s dependency `{}` declares `package = \"{}\"`, but `{rel}` is the package `{}`",
                        spec.alias,
                        spec.package,
                        id.name()
                    ),
                ));
                continue;
            }
            existing
        } else {
            let Some(doc) = read_manifest(&dir, overlay) else {
                problems.push(GraphProblem::new(
                    DEPENDENCY_NOT_FOUND,
                    format!(
                        "{who}'s dependency `{}` points at `{rel}`, which has no readable `bynk.toml`",
                        spec.alias
                    ),
                ));
                continue;
            };
            if let Err(e) = crate::check_manifest_str(&doc.1) {
                problems.push(GraphProblem::new(
                    DEPENDENCY_NOT_FOUND,
                    format!(
                        "dependency `{}`'s `bynk.toml` is invalid: {e}",
                        spec.package
                    ),
                ));
                continue;
            }
            let Some(config) = doc
                .0
                .get("package")
                .and_then(|v| v.as_table())
                .and_then(|t| package_config_from(t).ok())
            else {
                problems.push(GraphProblem::new(
                    DEPENDENCY_NOT_FOUND,
                    format!(
                        "{who}'s dependency `{}` points at `{rel}`, which isn't a package: its `bynk.toml` has no `[package]`",
                        spec.alias
                    ),
                ));
                continue;
            };
            let id = PackageId {
                org: config.org.clone(),
                package: config.package.clone(),
            };
            if id.name() != spec.package {
                problems.push(GraphProblem::new(
                    NAME_MISMATCH,
                    format!(
                        "{who}'s dependency `{}` declares `package = \"{}\"`, but `{rel}` is the package `{}`",
                        spec.alias,
                        spec.package,
                        id.name()
                    ),
                ));
                continue;
            }
            if let Some(other) = graph.by_id(&id)
                && (fetched || graph.packages[other].fetched)
            {
                problems.push(GraphProblem::new(
                    VERSION_CONFLICT,
                    format!(
                        "the package `{}` is reached at two versions: `{}` and `{}`",
                        id.name(),
                        graph.packages[other].dir.display(),
                        dir.display()
                    ),
                ).with_note("a build holds one copy of each package; point every dependency on it at the same revision"));
                continue;
            }
            if let Some(other) = graph.by_id(&id) {
                problems.push(GraphProblem::new(
                    DUPLICATE_SOURCE,
                    format!(
                        "the package `{}` is reached from two directories: `{}` and `{}`",
                        id.name(),
                        graph.packages[other].dir.display(),
                        dir.display()
                    ),
                ).with_note("a build holds one copy of each package; point every dependency on it at the same directory"));
                continue;
            }
            let trees = source_trees(project_root, &dir, &doc.0, fetched);
            graph.packages.push(Package {
                id,
                dir: dir.clone(),
                trees,
                exports: config.exports,
                aliases: BTreeMap::new(),
                fetched,
            });
            dirs.push(dir_real.clone());
            let index = graph.packages.len() - 1;
            for s in specs_of(&doc) {
                queue.push((Some(index), dir.clone(), s, false));
            }
            index
        };
        match declarer {
            None => {
                graph.root_aliases.insert(spec.alias.clone(), index);
                if dev {
                    graph.dev_aliases.insert(spec.alias.clone());
                }
            }
            Some(i) => {
                graph.packages[i].aliases.insert(spec.alias.clone(), index);
            }
        }
    }
    for (_, _, spec, _) in queue.iter().filter(|(d, ..)| d.is_none()) {
        if !graph.root_aliases.contains_key(&spec.alias) {
            graph.unresolved_aliases.insert(spec.alias.clone());
        }
    }
    if let Some(cycle) = find_cycle(&graph) {
        problems.push(GraphProblem::new(
            DEPENDENCY_CYCLE,
            format!(
                "the package graph has a cycle: {}",
                cycle
                    .iter()
                    .map(|&i| format!("`{}`", graph.packages[i].id.name()))
                    .collect::<Vec<_>>()
                    .join(" → ")
            ),
        ));
    }
    if graph.packages.is_empty()
        && graph.root_aliases.is_empty()
        && graph.unresolved_aliases.is_empty()
    {
        return (None, problems);
    }
    (Some(graph), problems)
}

/// Every dependency source tree of the project at `project_root` — what
/// discovery walks and what the CLI's source map partitions into. Problems are
/// [`resolve_graph`]'s to report.
pub fn dependency_trees(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Vec<(PathBuf, PathBuf)> {
    resolve_graph(project_root, overlay)
        .0
        .map(|g| g.trees())
        .unwrap_or_default()
}

/// The parsed manifest at `dir` and its text: the overlay first, then the disk,
/// as [`crate::read_package_config`] reads it.
fn read_manifest(dir: &Path, overlay: &HashMap<PathBuf, String>) -> Option<(toml::Table, String)> {
    let path = dir.join("bynk.toml");
    let text = match read_source(&path, overlay) {
        Ok(text) => text,
        Err(_) => std::fs::read_to_string(&path).ok()?,
    };
    let doc = text.parse::<toml::Table>().ok()?;
    Some((doc, text))
}

/// A manifest's `[dependencies]` entries. A malformed table is
/// `check_manifest`'s to report, so it contributes nothing here.
fn specs_of(doc: &(toml::Table, String)) -> Vec<DependencySpec> {
    doc.0
        .get("dependencies")
        .and_then(|v| v.as_table())
        .and_then(|t| dependencies_from("dependencies", t).ok())
        .unwrap_or_default()
}

/// #2240: a manifest's `[dev-dependencies]` entries — read from the project
/// itself only; a dependency's dev-dependencies are its own business.
fn dev_specs_of(doc: &(toml::Table, String)) -> Vec<DependencySpec> {
    doc.0
        .get("dev-dependencies")
        .and_then(|v| v.as_table())
        .and_then(|t| dependencies_from("dev-dependencies", t).ok())
        .unwrap_or_default()
}

/// A dependency's source trees: its `[paths] include`, or `src`, without any
/// `tests` tree — a dependency's suites are its own to run.
fn source_trees(
    project_root: &Path,
    dir: &Path,
    doc: &toml::Table,
    fetched: bool,
) -> Vec<(PathBuf, PathBuf)> {
    let include: Vec<String> = match doc
        .get("paths")
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("include"))
    {
        Some(toml::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Some(toml::Value::String(s)) => vec![s.clone()],
        _ => vec!["src".to_string()],
    };
    let mut out: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in include {
        if Path::new(&entry).file_name().is_some_and(|n| n == "tests") {
            continue;
        }
        let root = lexical_join(dir, Path::new(&entry));
        if out.iter().any(|(r, _)| *r == root) {
            continue;
        }
        // A store tree is named by its own (absolute) path: a path relative
        // to the project would climb out through the user's home directory.
        let prefix = if fetched {
            root.clone()
        } else {
            relative_to(project_root, &root)
        };
        out.push((root, prefix));
    }
    out
}

/// A cycle among dependency packages, as the packages on it in order.
fn find_cycle(graph: &PackageGraph) -> Option<Vec<usize>> {
    fn visit(
        graph: &PackageGraph,
        node: usize,
        state: &mut Vec<u8>,
        stack: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        state[node] = 1;
        stack.push(node);
        for &next in graph.packages[node].aliases.values() {
            if state[next] == 1 {
                let start = stack.iter().position(|&n| n == next).unwrap_or(0);
                let mut cycle = stack[start..].to_vec();
                cycle.push(next);
                return Some(cycle);
            }
            if state[next] == 0
                && let Some(c) = visit(graph, next, state, stack)
            {
                return Some(c);
            }
        }
        stack.pop();
        state[node] = 2;
        None
    }
    let mut state = vec![0u8; graph.packages.len()];
    for start in 0..graph.packages.len() {
        if state[start] == 0 {
            let mut stack = Vec::new();
            if let Some(c) = visit(graph, start, &mut state, &mut stack) {
                return Some(c);
            }
        }
    }
    None
}

/// `base.join(rel)` with `.` and `..` resolved lexically, keeping `base`'s
/// own form (relative or absolute), so the trees the CLI walks and the trees
/// `compile_project` partitions by agree on every path.
fn lexical_join(base: &Path, rel: &Path) -> PathBuf {
    if rel.is_absolute() {
        return lexical(rel);
    }
    lexical(&base.join(rel))
}

/// `p` with `.` and `..` resolved lexically. A leading `..` that can't pop is
/// kept, so `../money` stays `../money`.
fn lexical(p: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(c),
            },
            other => out.push(other),
        }
    }
    if out.is_empty() {
        return PathBuf::from(".");
    }
    out.iter().collect()
}

/// `target` relative to `base`, lexically: the identity prefix of a dependency
/// tree, so a diagnostic in it names a real path from the project root. When
/// the two can't be related (one absolute, one not), `target` itself.
fn relative_to(base: &Path, target: &Path) -> PathBuf {
    let base = lexical(base);
    let target = lexical(target);
    if base.is_absolute() != target.is_absolute() {
        return target;
    }
    let b: Vec<Component> = base
        .components()
        .filter(|c| *c != Component::CurDir)
        .collect();
    let t: Vec<Component> = target
        .components()
        .filter(|c| *c != Component::CurDir)
        .collect();
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c.as_os_str());
    }
    out
}

/// `p` with symlinks followed, when it exists; otherwise lexically resolved.
/// Confinement and duplicate detection compare these.
fn real(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|cwd| lexical_join(&cwd, p))
            .unwrap_or_else(|_| lexical(p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_round_trips_and_is_injective() {
        let ids = [
            ("acme", "money"),
            ("acme-labs", "ledger_kit"),
            ("a--b", "c"),
            ("a-", "b_c"),
            ("a", "b_c"),
        ];
        let mut seen = std::collections::HashSet::new();
        for (org, package) in ids {
            let id = PackageId {
                org: org.into(),
                package: package.into(),
            };
            let enc = id.encoded();
            assert!(seen.insert(enc.clone()), "{enc} collides");
            assert_eq!(PackageId::decode(&enc), Some(id.clone()));
            // A valid TypeScript identifier once `.` becomes `_`.
            let ident = id.canonical("currency.rates").replace('.', "_");
            assert!(
                ident
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
            );
        }
    }

    #[test]
    fn display_rewrites_canonical_names_only() {
        assert_eq!(
            display_names("unknown commons `$acme_money.currency` here"),
            "unknown commons `acme/money:currency` here"
        );
        assert_eq!(
            display_names("`$acme$labs_ledger_kit.a.b`"),
            "`acme-labs/ledger_kit:a.b`"
        );
        assert_eq!(display_names("costs $5"), "costs $5");
        assert_eq!(display_names("no dollars"), "no dollars");
    }

    #[test]
    fn relative_and_lexical_paths() {
        assert_eq!(lexical(Path::new("a/./b/../c")), PathBuf::from("a/c"));
        assert_eq!(lexical(Path::new("../money")), PathBuf::from("../money"));
        assert_eq!(
            relative_to(
                Path::new("/w/apps/shop"),
                Path::new("/w/packages/money/src")
            ),
            PathBuf::from("../../packages/money/src")
        );
        assert_eq!(
            relative_to(Path::new("."), Path::new("vendor/money/src")),
            PathBuf::from("vendor/money/src")
        );
    }
}
