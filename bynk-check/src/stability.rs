//! Experimental standard-library items and the project's opt-in to them
//! (standard-library S3, #1963; track D2).
//!
//! A first-party item (a unit, an exported item, or a capability operation, by
//! dotted path) is stable or experimental. Using an experimental one needs an
//! entry covering it under `bynk.toml`'s `[unstable] allow`. One
//! [`StabilityPolicy`] per project carries that list and the experimental items,
//! and both the build and the editor build it the same way
//! ([`StabilityPolicy::for_project`]), so they never disagree about a file.

use crate::firstparty;
use bynk_syntax::CompileError;
use bynk_syntax::span::Span;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The manifest the `[unstable]` diagnostics are attributed to.
pub const MANIFEST: &str = "bynk.toml";
/// The diagnostic for using an experimental item without opting in.
pub const EXPERIMENTAL: &str = "bynk.stability.experimental";
/// The warning for an `[unstable] allow` entry that reaches nothing experimental.
pub const NOT_EXPERIMENTAL: &str = "bynk.project.unstable_not_experimental";
/// The error for an `[unstable] allow` entry that names no first-party item.
pub const UNKNOWN_ITEM: &str = "bynk.project.unstable_unknown_item";

/// The stability facts one unit's checks need: the project's policy, for the
/// paths a check computes itself (a capability operation), and the unit's
/// imports that are blocked, by name, decided where provenance is known.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitStability {
    pub policy: Arc<StabilityPolicy>,
    /// Imported name → item path, for each import [`StabilityPolicy::blocks`].
    pub blocked_imports: HashMap<String, String>,
}

/// A project's stability policy: what is experimental, and what it allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StabilityPolicy {
    /// The `[unstable] allow` entries, in manifest order.
    pub allow: Vec<String>,
    /// The experimental item paths; an item under one is experimental too.
    /// [`firstparty::experimental_items`] unless a test supplies its own.
    pub experimental: Vec<String>,
}

impl Default for StabilityPolicy {
    /// Blocks nothing. A check input built without
    /// [`crate::resolver::ResolvedCommons::with_stability`] (a suite case body,
    /// `resolve_recovering`) is under neither rule, so the default must fail
    /// open: a default that knew the experimental items but no `allow` would
    /// refuse them while ignoring the project's opt-in. The real policy is
    /// built only by [`StabilityPolicy::for_project`].
    fn default() -> Self {
        Self::permissive()
    }
}

impl StabilityPolicy {
    /// The policy for the project at `project_root`, reading `[unstable]` from
    /// the overlay's `bynk.toml` or the one on disk. No project root (a
    /// single-file or in-memory build) allows nothing.
    pub fn for_project(project_root: Option<&Path>, overlay: &HashMap<PathBuf, String>) -> Self {
        let allow = project_root
            .map(|root| bynk_project::read_unstable_config(root, overlay).allow)
            .unwrap_or_default();
        Self {
            allow,
            experimental: firstparty::experimental_items(),
        }
    }

    /// A policy under which nothing is experimental: for a first-party unit's
    /// own bodies, whose dependencies are the library's business.
    pub fn permissive() -> Self {
        Self {
            allow: Vec::new(),
            experimental: Vec::new(),
        }
    }

    /// Whether `path` is experimental.
    pub fn is_experimental(&self, path: &str) -> bool {
        self.experimental
            .iter()
            .any(|item| firstparty::covers(item, path))
    }

    /// Whether using `path` here is refused: it is experimental and no `allow`
    /// entry covers it.
    pub fn blocks(&self, path: &str) -> bool {
        self.is_experimental(path) && !firstparty::allows(&self.allow, path)
    }

    /// The error for a use of `path` at `span`, when [`Self::blocks`] refuses it.
    pub fn check(&self, path: &str, span: Span) -> Option<CompileError> {
        self.blocks(path).then(|| {
            CompileError::new(
                EXPERIMENTAL,
                span,
                format!("`{path}` is experimental: it may change or disappear in any release"),
            )
            .with_note(format!(
                "to use it anyway, add \"{path}\" to `[unstable] allow` in `bynk.toml`"
            ))
        })
    }

    /// The diagnostics for the `[unstable] allow` entries themselves, which
    /// callers attribute to the project's `bynk.toml` ([`MANIFEST`]): an error
    /// for one that names no first-party
    /// item, and a warning for one that reaches nothing experimental (stable
    /// now, so it can go; never an error, so stabilising an item breaks no
    /// build).
    pub fn manifest_diagnostics(&self) -> Vec<CompileError> {
        let mut out = Vec::new();
        for entry in &self.allow {
            if !firstparty::names_first_party_item(entry) {
                out.push(CompileError::new(
                        UNKNOWN_ITEM,
                        Span::default(),
                        format!(
                            "`bynk.toml`'s `[unstable] allow` lists `{entry}`, which names no standard-library item"
                        ),
                    )
                    .with_note(
                        "an entry is a unit (`bynk.cloudflare`), an item it exports (`bynk.cloudflare.Kv`) or an operation (`bynk.cloudflare.Kv.list`)",
                    ));
            } else if !firstparty::allow_entry_is_live(&self.experimental, entry) {
                out.push(CompileError::new(
                        NOT_EXPERIMENTAL,
                        Span::default(),
                        format!(
                            "`bynk.toml`'s `[unstable] allow` lists `{entry}`, but nothing it covers is experimental"
                        ),
                    )
                    .with_note("it is stable now; remove it from `[unstable] allow`"));
            }
        }
        out
    }

    /// The names a unit imports that are experimental and not allowed, mapped
    /// to their item paths: `imported_from` is name → providing unit, so the
    /// path is `unit.name`. Decided once per unit, where provenance is known,
    /// and carried to every check built from it.
    pub fn blocked_imports(
        &self,
        imported_from: &HashMap<String, String>,
    ) -> HashMap<String, String> {
        imported_from
            .iter()
            .filter(|(_, unit)| firstparty::unit(unit).is_some())
            .map(|(name, unit)| (name.clone(), format!("{unit}.{name}")))
            .filter(|(_, path)| self.blocks(path))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(allow: &[&str], experimental: &[&str]) -> StabilityPolicy {
        StabilityPolicy {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            experimental: experimental.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn an_experimental_item_is_blocked_until_allowed() {
        let p = policy(&[], &["bynk.cloudflare.Kv.putTtl"]);
        assert!(p.blocks("bynk.cloudflare.Kv.putTtl"));
        assert!(!p.blocks("bynk.cloudflare.Kv.put"));
        assert!(
            !p.blocks("bynk.cloudflare.Kv"),
            "its capability stays stable"
        );
        let err = p
            .check("bynk.cloudflare.Kv.putTtl", Span::default())
            .unwrap();
        assert_eq!(err.category, "bynk.stability.experimental");
        assert!(err.notes[0].contains("\"bynk.cloudflare.Kv.putTtl\""));

        for allow in [
            "bynk.cloudflare.Kv.putTtl",
            "bynk.cloudflare.Kv",
            "bynk.cloudflare",
        ] {
            assert!(
                !policy(&[allow], &["bynk.cloudflare.Kv.putTtl"])
                    .blocks("bynk.cloudflare.Kv.putTtl"),
                "{allow}"
            );
        }
    }

    #[test]
    fn manifest_entries_are_checked_for_existence_and_staleness() {
        let p = policy(
            &[
                "bynk.cloudflare.Kv.putTtl",
                "bynk.cloudflare",
                "bynk.list",
                "bynk.cloudflare.Kvv",
                "user.thing",
            ],
            &["bynk.cloudflare.Kv.putTtl"],
        );
        let found: Vec<(String, String)> = p
            .manifest_diagnostics()
            .into_iter()
            .map(|e| (e.category.to_string(), e.message.clone()))
            .collect();
        let codes: Vec<&str> = found.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(codes, [NOT_EXPERIMENTAL, UNKNOWN_ITEM, UNKNOWN_ITEM]);
        assert!(found[0].1.contains("`bynk.list`"), "stale: {found:?}");
        assert!(found[1].1.contains("`bynk.cloudflare.Kvv`"));
        assert!(found[2].1.contains("`user.thing`"));
    }

    #[test]
    fn blocked_imports_are_first_party_experimental_and_unallowed() {
        let imported = HashMap::from([
            ("join".to_string(), "bynk.string".to_string()),
            ("reverse".to_string(), "bynk.list".to_string()),
            ("Money".to_string(), "app.money".to_string()),
        ]);
        let p = policy(&[], &["bynk.string.join", "app.money"]);
        assert_eq!(
            p.blocked_imports(&imported),
            HashMap::from([("join".to_string(), "bynk.string.join".to_string())]),
            "a user unit is never experimental, whatever the list says"
        );
        assert!(
            policy(&["bynk.string"], &["bynk.string.join"])
                .blocked_imports(&imported)
                .is_empty()
        );
    }

    /// The project the end-to-end tests analyse: one reference of each kind to
    /// a shipped first-party item, the function call inside a handler body
    /// (where `imported_from` is not available, so the per-unit blocked imports
    /// must have been carried there).
    const PROJECT: &[(&str, &str)] = &[
        (
            "app/probe.test.bynk",
            "suite app.probe\n\ncase \"a case calls an operation\" {\n\tlet _ <- Kv.putTtl(\"k\", \"v\", 120)\n\texpect Kv.putTtl called once\n}\n",
        ),
        (
            "app/qual.bynk",
            "context app.qual\n\nconsumes bynk\n\nservice ping {\n\ton call() -> Effect[Int] given bynk.Clock {\n\t\tlet _ <- bynk.Clock.now()\n\t\tEffect.pure(1)\n\t}\n}\n",
        ),
        (
            "app/probe.bynk",
            "context app.probe\n\nuses bynk.string\nuses bynk.locale.types\nconsumes bynk { Random }\nconsumes bynk.cloudflare { Kv }\n\nservice probe {\n\ton call(xs: List[String]) -> Effect[String] given Kv {\n\t\tlet _ <- Kv.putTtl(\"k\", \"v\", 120)\n\t\tlet joiner: (List[String], String) -> String = join\n\t\tEffect.pure(join(xs, \",\"))\n\t}\n}\n",
        ),
    ];

    /// Every `bynk.stability.experimental` the analysis reports, as
    /// `(item path, reported text)`.
    fn experimental_uses(policy: StabilityPolicy) -> Vec<(String, String)> {
        let analysed = crate::testkit::analyse_with(PROJECT, Some(policy));
        let mut out: Vec<(String, String)> = analysed
            .analysis
            .errors
            .iter()
            .filter(|a| a.error.category == "bynk.stability.experimental")
            .map(|a| {
                let path = a.error.message.split('`').nth(1).unwrap_or("").to_string();
                let (start, end) = (a.error.span.start, a.error.span.end);
                let file = a.source_path.as_deref().expect("a use is in a file");
                let (_, text) = PROJECT
                    .iter()
                    .find(|(rel, _)| file.ends_with(rel))
                    .expect("a project file");
                (path, text[start..end].to_string())
            })
            .collect();
        out.sort();
        out
    }

    /// End to end, through the analysis the editor runs: each kind of
    /// reference to an experimental item is refused at its own span, and an
    /// `allow` entry covering it lifts the refusal.
    #[test]
    fn every_reference_point_refuses_an_experimental_item() {
        let experimental = &[
            "bynk.Clock.now",
            "bynk.Random",
            "bynk.locale.types",
            "bynk.string.join",
            "bynk.cloudflare.Kv.putTtl",
        ];
        assert_eq!(
            experimental_uses(policy(&[], experimental)),
            [
                ("bynk.Clock.now".to_string(), "now".to_string()),
                ("bynk.Random".to_string(), "Random".to_string()),
                (
                    "bynk.cloudflare.Kv.putTtl".to_string(),
                    "putTtl".to_string()
                ),
                (
                    "bynk.locale.types".to_string(),
                    "bynk.locale.types".to_string()
                ),
                ("bynk.string.join".to_string(), "join".to_string()),
                ("bynk.string.join".to_string(), "join".to_string()),
            ],
            "a consumes selection, a uses, a capability operation (flattened and qualified), and a function called and passed as a value in a handler; never in a suite case"
        );
        assert!(
            experimental_uses(policy(
                &[
                    "bynk.Clock",
                    "bynk.Random",
                    "bynk.locale",
                    "bynk.string",
                    "bynk.cloudflare.Kv"
                ],
                experimental
            ))
            .is_empty(),
            "allow entries cover their path and everything under it"
        );
        assert!(
            experimental_uses(StabilityPolicy::default()).is_empty(),
            "the default fails open"
        );
    }

    /// The codes a one-file project's analysis reports.
    fn codes(source: &str) -> Vec<&'static str> {
        crate::testkit::analyse(&[("app/probe.bynk", source)])
            .analysis
            .errors
            .iter()
            .map(|a| a.error.category)
            .collect()
    }

    const FETCH_MATCH: &str = "context app.probe\n\nconsumes bynk { Fetch }\n\nservice probe {\n\ton call(url: String) -> Effect[String] given Fetch {\n\t\tlet res <- Fetch.send(Request { method: Get, url: url, contentType: None, authorization: None, headers: Map.empty(), body: None })\n\t\tmatch res {\n\t\t\tOk(r) => Effect.pure(\"ok\")\n\t\t\tErr(Network) => Effect.pure(\"network\")\n\t\t\tErr(Timeout) => Effect.pure(\"timeout\")\n\t\t\tErr(InvalidHeader) => Effect.pure(\"header\")\nWILDCARD\t\t}\n\t}\n}\n";

    /// An extensible enum (S3, #1963): listing every variant it has today is
    /// not enough; a wildcard is, and it is then not reported unreachable (the
    /// future variant is still uncovered when it is reached).
    #[test]
    fn an_extensible_enum_needs_a_wildcard_which_is_then_reachable() {
        assert_eq!(
            codes(&FETCH_MATCH.replace("WILDCARD", "")),
            ["bynk.types.extensible_match_needs_wildcard"]
        );
        assert_eq!(
            codes(&FETCH_MATCH.replace("WILDCARD", "\t\t\tErr(_) => Effect.pure(\"later\")\n")),
            Vec::<&str>::new(),
            "a wildcard satisfies it, and is not `bynk.types.unreachable_arm`"
        );
        assert_eq!(
            codes(&FETCH_MATCH.replace("WILDCARD", "\t\t\tother => Effect.pure(\"later\")\n")),
            Vec::<&str>::new(),
            "so does a binding"
        );
    }

    /// A user's own enum of an extensible enum's name is not extensible: the
    /// rule follows the first-party unit's enum, not the name.
    #[test]
    fn a_users_own_enum_of_the_same_name_is_not_extensible() {
        assert_eq!(
            codes(
                "context app.probe\n\ntype FetchError = enum { Network, Timeout, InvalidHeader }\n\nfn label(e: FetchError) -> String {\n\tmatch e {\n\t\tNetwork => \"n\"\n\t\tTimeout => \"t\"\n\t\tInvalidHeader => \"h\"\n\t}\n}\n\nservice probe {\n\ton call() -> Effect[String] {\n\t\tEffect.pure(label(Network))\n\t}\n}\n"
            ),
            Vec::<&str>::new()
        );
    }

    /// An input built without `with_stability` (a suite case body,
    /// `resolve_recovering`) must fail open: the default is the permissive
    /// policy, not the shipped experimental list with an empty `allow`.
    #[test]
    fn the_default_policy_fails_open() {
        assert_eq!(StabilityPolicy::default(), StabilityPolicy::permissive());
        assert_eq!(
            UnitStability::default().policy.experimental,
            Vec::<String>::new()
        );
    }

    /// The shipped policy blocks only the experimental items: today
    /// `bynk.cloudflare`'s `Service` (interop I5, #1972) and `R2` (P5,
    /// #2323), and their operations with them, and the whole of `bynk.crypto`
    /// (K4).
    #[test]
    fn the_shipped_policy_blocks_only_the_experimental_items() {
        let p = StabilityPolicy::for_project(None, &HashMap::new());
        assert_eq!(
            p.experimental,
            [
                "bynk.Fetch.sendWith",
                "bynk.Logger.log",
                "bynk.cloudflare.Service",
                "bynk.cloudflare.R2",
                "bynk.cloudflare.RateLimit",
                "bynk.cloudflare.EdgeCache",
                "bynk.cloudflare.Queue",
                "bynk.crypto"
            ]
        );
        assert!(!p.blocks("bynk.cloudflare.Kv.list"));
        // K5a (#2586): new operations of stable capabilities, alone.
        assert!(p.blocks("bynk.Fetch.sendWith"));
        assert!(!p.blocks("bynk.Fetch.send"));
        assert!(!p.blocks("bynk.FetchOptions"));
        assert!(p.blocks("bynk.Logger.log"));
        assert!(!p.blocks("bynk.Logger.warn"));
        assert!(p.blocks("bynk.cloudflare.Service"));
        assert!(p.blocks("bynk.cloudflare.Service.send"));
        assert!(p.blocks("bynk.cloudflare.R2"));
        assert!(p.blocks("bynk.cloudflare.R2.putWith"));
        assert!(!p.blocks("bynk.cloudflare.R2Put"));
        assert!(p.blocks("bynk.cloudflare.RateLimit.allow"));
        assert!(p.blocks("bynk.cloudflare.Queue.sendBatch"));
        assert!(!p.blocks("bynk.cloudflare.QueueSend"));
        assert!(p.blocks("bynk.cloudflare.EdgeCache.purge"));
        assert!(p.manifest_diagnostics().is_empty());
    }
}
