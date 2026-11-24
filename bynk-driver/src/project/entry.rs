use super::diagnostics::Mode;
use super::diagnostics::ProjectFailure;
use super::options::CompileOptions;
use super::output::compatibility_flag_warnings;
use super::output::finish_build;
use super::pipeline::RunChecks;
use super::pipeline::run_checks;
use bynk_emit::emit_model::ProjectOutput;
use bynk_project::AttributedError;
use bynk_project::Roots;
use bynk_project::discovery::ParsedFile;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bynk_check::checker::{ExprId, Types};

/// #57: turn `options.sources` into the `(overlay, discovered)` pair
/// `run_checks` expects — the caller-supplied file list, partitioned across
/// `trees` the same way a real walk would (single-tree: every file lands in
/// the first tree's list, matching `compile_in_memory`'s own convention).
/// Shared by [`compile_project`] and [`check_project`] so the two can't drift
/// on this.
///
/// #1077/#1081 review: sorts every partition. `sources`'s own key order is a
/// `HashMap`'s — unspecified, randomised per process — but `phase_parse`
/// walks each tree's file list in order to assign sequential `FileId`s and
/// `ExprId`s (embedded in emitted spans/source maps) and `run_checks` pushes
/// diagnostics in file order, both of which a real disk walk already
/// guaranteed via `discover_bynk_files`'s own `out.sort()`. Without this, a
/// `sources`-driven compile (the CLI's own path as of #1081) would silently
/// vary its diagnostic order and `FileId` assignment run to run.
///
/// R3.9 (#1113): partitions across every `trees` entry, not a hardcoded
/// primary/secondary pair — a path that doesn't start with any tree's root
/// (shouldn't happen for a well-formed `sources` map) falls back to the
/// *last* tree, matching the pre-R3.9 two-tree `partition`'s fallback (an
/// unmatched key landed in `tests_files`, the second/last half of the pair).
/// Falling back to the *first* tree instead — silently tried during this
/// slice's initial cut — would move an unmatched file into whichever tree
/// `check_file_directory_conflicts` and `identity_path` treat as primary,
/// changing its attribution with no diagnostic raised either way; matching
/// the old convention at least keeps this rare path's behaviour unchanged by
/// the crate move.
type Overlay = HashMap<PathBuf, String>;

/// #3956: the project's own parsed units (its source trees, not its
/// dependencies'), for a command that reads declarations without checking them
/// (`bynk lock accept-order`). A syntax error is refused: the order of a type
/// that does not parse is not a fact to record.
pub fn parse_own_units(options: &CompileOptions) -> Result<Vec<ParsedFile>, String> {
    use bynk_check::project_model::{ErrorSink, phase_discovery, phase_parse};
    let trees = options.roots.trees();
    let excludes = options.roots.excludes();
    let (overlay, discovered) = match &options.sources {
        Some(sources) => sources_to_discovered(sources, &trees),
        None => (HashMap::new(), None),
    };
    let mut errors = ErrorSink::new();
    let mut snapshots = Vec::new();
    let refused = |errors: ErrorSink| {
        let first = errors
            .into_all()
            .into_iter()
            .next()
            .map(|e| format!(": {}", e.error.message))
            .unwrap_or_default();
        format!("the project does not parse{first}; fix it first (`bynk check`)")
    };
    let file_lists = match discovered {
        Some(files) => files,
        None => match phase_discovery(&trees, &excludes, &mut errors) {
            Ok(files) => files,
            Err(()) => return Err(refused(errors)),
        },
    };
    let Ok((parsed, _, _)) =
        phase_parse(&trees, &file_lists, &overlay, &mut errors, &mut snapshots)
    else {
        return Err(refused(errors));
    };
    if !errors.is_empty() {
        return Err(refused(errors));
    }
    Ok(parsed)
}

/// One file list per `trees` entry — the same shape `phase_discovery` and
/// `run_checks`'s own `discovered` parameter already use.
type Discovered = Vec<Vec<PathBuf>>;

/// #2223: the trees a manifest-backed project compiles — its own
/// ([`Roots::trees`]) and then each path dependency's
/// ([`bynk_project::dependency_trees`]) — so the source map partitions a
/// dependency's files into their own tree rather than the last one.
fn project_trees(options: &CompileOptions) -> Vec<(PathBuf, PathBuf)> {
    let mut trees = options.roots.trees();
    if let Roots::Split { project_root, .. } = &options.roots {
        let empty = HashMap::new();
        let overlay = options.sources.as_ref().unwrap_or(&empty);
        trees.extend(bynk_project::dependency_trees(project_root, overlay));
    }
    trees
}

pub(super) fn sources_to_discovered(
    sources: &HashMap<PathBuf, String>,
    trees: &[(PathBuf, PathBuf)],
) -> (Overlay, Option<Discovered>) {
    let mut buckets: Vec<Vec<PathBuf>> = vec![Vec::new(); trees.len()];
    let mut keys: Vec<PathBuf> = sources.keys().cloned().collect();
    keys.sort();
    for p in keys {
        let idx = trees
            .iter()
            .position(|(root, _)| p.starts_with(root))
            .unwrap_or(trees.len().saturating_sub(1));
        buckets[idx].push(p);
    }
    (sources.clone(), Some(buckets))
}

/// Compile a Bynk project, keeping error attribution + snapshots on failure
/// (so the CLI can render project errors with source context, ADR 0052). Use
/// `.map_err(ProjectFailure::flatten)` for the flattened `Vec<CompileError>`
/// shape.
pub fn compile_project(options: &CompileOptions) -> Result<ProjectOutput, ProjectFailure> {
    // T3.6b (R4.1): one table per build, shared across every unit compiled.
    let tys = &Arc::new(Types::new());
    let trees = project_trees(options);
    let excludes = options.roots.excludes();
    let (overlay, discovered) = match &options.sources {
        Some(sources) => sources_to_discovered(sources, &trees),
        None => (HashMap::new(), None),
    };
    let run = run_checks(
        &trees,
        options.target,
        options.platform,
        options.import_ext,
        Mode::Build,
        &overlay,
        &excludes,
        discovered,
        options.contracts,
        options.trace_requests,
        options.inspect_agents,
        options.tests,
        &options.schema_registry,
        options.remote_tests.as_ref(),
        options.roots.project_root(),
        options.asset_files.as_ref(),
        options.embed_files.as_ref(),
        options.fixture_files.as_ref(),
        options.migration_files.as_ref(),
        options.snapshot_files.as_ref(),
        matches!(options.roots, Roots::Split { .. }),
        tys,
        options.progress.as_ref(),
    );
    // #1078: `bynk-emit` no longer writes `bynk.schema.lock` itself — the
    // reconciled content comes back on `ProjectOutput::schema_lock`
    // (`finish_build` populates it from `RunChecks::Checked`, only ever
    // constructed on the `Ok` path, i.e. only on a fully clean build — a
    // build that fails for any reason, including a schema mismatch
    // reconciliation itself just reported, produces `Err(ProjectFailure)`
    // instead and has no revised content for a caller to persist). The
    // caller (today, `bynk-driver`'s two wiring points) does the atomic
    // write.
    finish_build(
        run,
        options.import_ext,
        options.roots.project_root(),
        &options.compatibility_flags,
        options.workers_dev,
        options.out_dir.as_deref(),
        options.contracts_lock.as_deref(),
    )
}

/// Result of [`check_project`]: every diagnostic from a non-bailing project
/// analysis — errors *and* warnings together (ADR 0117), unconditionally,
/// unlike [`ProjectOutput`]/[`ProjectFailure`] where `errors`/`warnings` is
/// picked by which variant the caller got. `bynk check`'s exit code is
/// decided by [`Self::has_errors`], not by whether this was reached at all.
pub struct ProjectCheck {
    pub errors: Vec<AttributedError>,
    pub snapshots: Vec<(PathBuf, String)>,
    /// #1772: the project or tree root the build was given, as the caller
    /// spelled it. `snapshots` and each error's `source_path` are keyed by
    /// identity path, relative to this root; a renderer shows
    /// `display_root.join(identity)`, the path as typed from the working
    /// directory. Empty for an in-memory build.
    pub display_root: PathBuf,
    /// State-migrations S4 (#2259): each agent class's lock version, oldest
    /// readable version and valid `migrate` steps, keyed `<unit>.<Agent>`.
    /// Empty without a lock, or when the check stopped before reconciling.
    pub agent_versions: HashMap<String, bynk_check::schema_registry::AgentVersions>,
    /// #3142: the SQL arguments the checker tied to a D1 call (`all`, `first`,
    /// `run`, `select`, `firstAs`, or an inline `batch` statement). Read by
    /// `bynk test --mutate` to pick SQL mutants. Empty when the check stopped
    /// before the units ran.
    pub d1_sql_sites: Vec<D1SqlRef>,
    /// #3355: every `Forward(...)` verdict a `from email` handler returns, as
    /// the checker resolved it. Read by `bynk deploy` to warn about a
    /// destination Cloudflare will refuse. Empty when the check stopped before
    /// the units ran.
    pub email_forwards: Vec<EmailForward>,
}

/// #3355: one `Forward(to)` verdict in a `from email` handler's body.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EmailForward {
    /// The context holding the handler, as written (`shop.support`).
    pub context: String,
    /// The `from email` service.
    pub service: String,
    /// The address the service claims (`support@shop.example`, or `*@domain`).
    pub address: String,
    /// The literal destination, or `None` when the argument is computed and so
    /// cannot be known at deploy time.
    pub to: Option<String>,
}

/// #3142: one D1 call's SQL argument, as the checker recorded it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct D1SqlRef {
    /// The unit the call is in.
    pub unit: String,
    /// The absolute path of the file holding the argument.
    pub path: PathBuf,
    /// The argument's byte span: the string literal itself, or, when `konst` is
    /// set, the reference to the constant.
    pub start: usize,
    pub end: usize,
    /// The name of the `String` constant the argument refers to, if it is one.
    pub konst: Option<String>,
}

impl ProjectCheck {
    /// Finding #64: `bynk check`'s exit-code gate is "does any error-severity
    /// diagnostic exist in the project" — not "did the pipeline reach the end
    /// without bailing", which is what `compile_project`'s `Result` encoded
    /// and why a `bynk.toml`-wide structural error anywhere silently hid every
    /// later diagnostic (including a test body's own type errors) from
    /// `bynk check`, even though the editor (via
    /// `bynk_check::analysis::analyse_project`, which shares this same
    /// non-bailing shape) still reported them.
    pub fn has_errors(&self) -> bool {
        self.errors
            .iter()
            .any(|ae| bynk_syntax::Severity::for_error(&ae.error) == bynk_syntax::Severity::Error)
    }
}

/// Check a project without building (finding #64) — never bails after
/// discovery (`Mode::Analyse`, the same mode `bynk_check::analysis::analyse_project`
/// runs for the editor), so a diagnostic anywhere in the project does not
/// suppress diagnostics elsewhere. `compile_project`'s `Mode::Build` bails at
/// the first structural error and returns only what it collected up to that
/// point — correct for `build`/`test`, which must not emit past a real
/// error, but wrong for `check`, whose only job is to report everything.
/// `bynk check`'s directory path calls this instead of `compile_project`.
pub fn check_project(options: &CompileOptions) -> ProjectCheck {
    // T3.6b (R4.1): one table per check, shared across every unit.
    let tys = &Arc::new(Types::new());
    let trees = project_trees(options);
    let excludes = options.roots.excludes();
    let (overlay, discovered) = match &options.sources {
        Some(sources) => sources_to_discovered(sources, &trees),
        None => (HashMap::new(), None),
    };
    let run = run_checks(
        &trees,
        options.target,
        options.platform,
        options.import_ext,
        Mode::Analyse,
        &overlay,
        &excludes,
        discovered,
        options.contracts,
        options.trace_requests,
        options.inspect_agents,
        options.tests,
        // #2078: `bynk check` reconciles against the committed
        // `bynk.schema.lock` whenever the caller pre-read it
        // (`SchemaLock::On`), so a non-additive event change fails the check
        // the way it fails a build. Read-only by construction: a
        // `ProjectCheck` carries no reconciled content for anyone to write.
        &options.schema_registry,
        options.remote_tests.as_ref(),
        options.roots.project_root(),
        options.asset_files.as_ref(),
        options.embed_files.as_ref(),
        options.fixture_files.as_ref(),
        options.migration_files.as_ref(),
        options.snapshot_files.as_ref(),
        matches!(options.roots, Roots::Split { .. }),
        tys,
        None,
    );
    let (errors, snapshots, agent_versions, d1_sql_sites, email_forwards) = match run {
        RunChecks::Bailed {
            errors, snapshots, ..
        } => (errors, snapshots, HashMap::new(), Vec::new(), Vec::new()),
        RunChecks::Checked {
            errors,
            snapshots,
            agent_versions,
            unit_callees,
            parsed,
            ..
        } => {
            // #3142: a site's span carries the durable `FileId` its file's
            // absolute path interns to.
            let paths: HashMap<_, _> = parsed
                .iter()
                .filter_map(|pf| {
                    let abs = pf.abs_path()?;
                    Some((bynk_project::parse_cache::file_id_for(&abs), abs))
                })
                .collect();
            let mut lits: Vec<D1SqlRef> = unit_callees
                .iter()
                .flat_map(|(unit, m)| m.values().map(move |c| (unit, c)))
                .filter_map(|(unit, c)| {
                    let (site, konst) = match c {
                        bynk_check::checker::Callee::D1Sql(site) => (site, None),
                        bynk_check::checker::Callee::Const(name, Some(site)) => {
                            (&**site, Some(name.clone()))
                        }
                        _ => return None,
                    };
                    let path = paths.get(&site.span.file)?;
                    Some(D1SqlRef {
                        unit: unit.clone(),
                        path: path.clone(),
                        start: site.span.start,
                        end: site.span.end,
                        konst,
                    })
                })
                .collect();
            lits.sort();
            lits.dedup();
            let forwards = email_forwards_of(&parsed, &unit_callees);
            (errors, snapshots, agent_versions, lits, forwards)
        }
    };
    ProjectCheck {
        // #1890: the manifest's duplicate-flag warnings, as a build reports.
        errors: errors
            .into_all()
            .into_iter()
            .chain(compatibility_flag_warnings(&options.compatibility_flags))
            .collect(),
        snapshots,
        display_root: options.roots.project_root().to_path_buf(),
        agent_versions,
        d1_sql_sites,
        email_forwards,
    }
}

/// #3355: the `Forward(to)` verdicts of every `from email` handler, found on
/// the parsed declarations by the checker's own classification of each call
/// (`Callee::Ctor` of `EmailVerdict`'s `Forward`), never by text: a comment,
/// a string that spells `Forward("x@y")`, or a user type with a `Forward`
/// variant of its own is not one. The destination is the first argument when it
/// is a string literal, and `None` otherwise (an interpolated or computed one).
///
/// A handler may delegate to a helper `fn` of its context, so a `Forward` in
/// any `fn` of a context that has a `from email` service is attributed to each
/// of that context's email handlers. That can over-report (a helper no handler
/// calls), which is cheap: the result only ever warns.
fn email_forwards_of(
    parsed: &[ParsedFile],
    unit_callees: &HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
) -> Vec<EmailForward> {
    use bynk_syntax::ast::{CommonsItem, ServiceProtocol, SourceUnit};
    let mut out = Vec::new();
    for pf in parsed {
        let SourceUnit::Context(ctx) = pf.unit() else {
            continue;
        };
        let helpers: Vec<Option<String>> = ctx
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Fn(f) => Some(forwards_in(&f.body, unit_callees)),
                _ => None,
            })
            .flatten()
            .collect();
        for item in &ctx.items {
            let CommonsItem::Service(service) = item else {
                continue;
            };
            let ServiceProtocol::Email { address } = &service.protocol else {
                continue;
            };
            let own = service
                .handlers
                .iter()
                .flat_map(|h| forwards_in(&h.body, unit_callees));
            out.extend(own.chain(helpers.iter().cloned()).map(|to| EmailForward {
                context: ctx.name.joined(),
                service: service.name.name.clone(),
                address: address.clone(),
                to,
            }));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The destination of each `EmailVerdict`'s `Forward(...)` call under `body`:
/// its string literal, or `None` when computed.
fn forwards_in(
    body: &bynk_syntax::ast::Block,
    unit_callees: &HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
) -> Vec<Option<String>> {
    use bynk_syntax::ast::ExprKind;
    let is_forward = |id: &ExprId| {
        unit_callees.values().any(|m| {
            matches!(
                m.get(id),
                Some(bynk_check::checker::Callee::Ctor { sum, tag })
                    if tag == "Forward" && sum.name.name == "EmailVerdict"
            )
        })
    };
    let mut found = Vec::new();
    let mut stack: Vec<&bynk_syntax::ast::Expr> = Vec::new();
    for s in &body.statements {
        bynk_syntax::ast::statement_exprs(s, &mut stack);
    }
    stack.push(&body.tail);
    while let Some(e) = stack.pop() {
        if let ExprKind::Call { args, .. } = &e.kind
            && is_forward(&e.id)
        {
            found.push(match args.first().map(|a| &a.kind) {
                Some(ExprKind::StrLit(s)) => Some(s.clone()),
                _ => None,
            });
        }
        stack.extend(bynk_syntax::ast::expr_children(e));
    }
    found
}
