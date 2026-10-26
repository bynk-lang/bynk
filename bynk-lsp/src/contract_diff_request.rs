//! #2467: `bynk/contractDiff` — the contract-diff custom LSP request.
//!
//! Project-scoped, like `bynk/architectureModel`: the params carry a
//! `textDocument` only to resolve which project's committed round to read.
//! Two lists come back:
//!
//! - **`changes`**: every event and agent class whose shape differs from
//!   `bynk.schema.lock`, classified `compatible` or `breaking`. The
//!   classification is `bynk_check::schema_registry::diff_against_lock`,
//!   which reports the verdicts of the reconciliation `bynk check` runs; this
//!   module never decides what "breaking" means.
//! - **`services`**: each context's `on call` contract hash that differs from
//!   what `bynk.deploy.lock` records as live, per environment. A hash says
//!   *whether* a contract moved, never whether the move is breaking (ADR 0491
//!   compares hashes too), so these rows are `changed`/`added`/`removed`,
//!   never `breaking`.
//!
//! The ledger is read here, above the driver boundary, like the schema lock
//! (`schema_lock.rs`); read-only, on request, nothing cached.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use bynk_check::analysis::ContractDiff;
#[cfg(test)]
use bynk_check::analysis::ProvidedContracts;
use bynk_check::index::{ProjectIndex, SymbolKey, SymbolKind};
use bynk_check::schema_registry::{BoundaryChange, ChangeClass, ChangeKind};
use serde::{Deserialize, Serialize};

pub use crate::architecture_request::WireLoc as CdLoc;

/// The deploy ledger's file name, at the project root.
pub(crate) const LEDGER_FILE: &str = "bynk.deploy.lock";

/// The `bynk/contractDiff` request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractDiffParams {
    pub text_document: tower_lsp::lsp_types::TextDocumentIdentifier,
}

/// What the ledger read gave: the diff against it is only as good as this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LedgerState {
    /// No `bynk.deploy.lock`: nothing has been deployed from this checkout.
    Absent,
    /// Present but not parseable here; the services list is empty.
    Unreadable,
    Read,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CdChange {
    /// `event` or `agent`.
    pub kind: &'static str,
    pub unit: String,
    pub name: String,
    /// `compatible` or `breaking`.
    pub class: &'static str,
    pub locked_version: i64,
    pub detail: String,
    pub loc: Option<CdLoc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CdService {
    pub environment: String,
    /// The providing context, qualified.
    pub context: String,
    pub service: String,
    /// `changed`, `added` (not in the live record) or `removed` (in the live
    /// record, no longer provided).
    pub status: &'static str,
    /// The hash the ledger records as live; `None` for `added`.
    pub live: Option<String>,
    /// The hash this source provides; `None` for `removed`.
    pub current: Option<String>,
    pub loc: Option<CdLoc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CdModel {
    /// Whether a `bynk.schema.lock` exists to diff events and agents against.
    pub lock_present: bool,
    pub ledger: LedgerState,
    pub changes: Vec<CdChange>,
    pub services: Vec<CdService>,
}

/// The slice of `bynk.deploy.lock` this view reads. No `deny_unknown_fields`:
/// the `bynk` CLI owns the format and adds to it.
#[derive(Debug, Default, Deserialize)]
struct Ledger {
    #[serde(default)]
    environments: BTreeMap<String, LedgerEnvironment>,
}

#[derive(Debug, Default, Deserialize)]
struct LedgerEnvironment {
    #[serde(default)]
    workers: BTreeMap<String, LedgerWorker>,
}

#[derive(Debug, Default, Deserialize)]
struct LedgerWorker {
    /// `None`: a push that recorded no contracts. Silence is not a match
    /// (ADR 0491), so such a Worker yields no row.
    #[serde(default)]
    contracts: Option<BTreeMap<String, String>>,
}

/// Read `bynk.deploy.lock` for the project, read-only.
pub(crate) fn read_ledger(project_root: &Path) -> (LedgerState, Option<String>) {
    match std::fs::read_to_string(project_root.join(LEDGER_FILE)) {
        Ok(text) => (LedgerState::Read, Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (LedgerState::Absent, None),
        Err(e) => {
            tracing::warn!("could not read {LEDGER_FILE}: {e}");
            (LedgerState::Unreadable, None)
        }
    }
}

/// Lower the round's [`ContractDiff`] (and the ledger text, if any) to the
/// wire model.
pub fn to_wire(
    diff: &ContractDiff,
    ledger: (LedgerState, Option<&str>),
    project_root: &Path,
    index: &ProjectIndex,
    snapshots: &HashMap<PathBuf, String>,
) -> CdModel {
    let loc_of = |unit: &str, kind: SymbolKind, name: &str| {
        let key = SymbolKey {
            unit: unit.to_string(),
            kind,
            name: name.to_string(),
        };
        let def = index.symbols.get(&key)?.def.as_ref()?;
        let text = snapshots.get(&def.path)?;
        let uri = tower_lsp::lsp_types::Url::from_file_path(project_root.join(&def.path)).ok()?;
        Some(CdLoc {
            uri,
            range: crate::position::span_to_range(text, def.span),
        })
    };

    let changes = diff
        .changes
        .iter()
        .map(|c: &BoundaryChange| CdChange {
            kind: match c.kind {
                ChangeKind::Event => "event",
                ChangeKind::Agent => "agent",
            },
            unit: c.unit.clone(),
            name: c.name.clone(),
            class: match c.class {
                ChangeClass::Compatible => "compatible",
                ChangeClass::Breaking => "breaking",
            },
            locked_version: c.locked_version,
            detail: c.detail.clone(),
            loc: loc_of(
                &c.unit,
                match c.kind {
                    ChangeKind::Event => SymbolKind::Type,
                    ChangeKind::Agent => SymbolKind::Agent,
                },
                &c.name,
            ),
        })
        .collect();

    let (state, text) = ledger;
    let mut state = state;
    let mut services = Vec::new();
    if let Some(text) = text {
        match toml::from_str::<Ledger>(text) {
            Ok(ledger) => services = service_rows(diff, &ledger, &loc_of),
            Err(e) => {
                tracing::warn!("could not parse {LEDGER_FILE}: {e}");
                state = LedgerState::Unreadable;
            }
        }
    }

    CdModel {
        lock_present: diff.lock_present,
        ledger: state,
        changes,
        services,
    }
}

/// Compare each context's provided hashes with each live Worker record. The
/// Worker for a context is named as the deploy gate names it.
fn service_rows(
    diff: &ContractDiff,
    ledger: &Ledger,
    loc_of: &dyn Fn(&str, SymbolKind, &str) -> Option<CdLoc>,
) -> Vec<CdService> {
    let mut rows = Vec::new();
    for (environment, env) in &ledger.environments {
        for (context, current) in &diff.services {
            let Some(live) = env
                .workers
                .get(&current.worker)
                .and_then(|w| w.contracts.as_ref())
            else {
                continue;
            };
            for (service, hash) in &current.hashes {
                let (status, recorded) = match live.get(service) {
                    Some(l) if l == hash => continue,
                    Some(l) => ("changed", Some(l.clone())),
                    None => ("added", None),
                };
                rows.push(CdService {
                    environment: environment.clone(),
                    context: context.clone(),
                    service: service.clone(),
                    status,
                    live: recorded,
                    current: Some(hash.clone()),
                    loc: loc_of(context, SymbolKind::Service, service),
                });
            }
            for (service, l) in live {
                if !current.hashes.contains_key(service) {
                    rows.push(CdService {
                        environment: environment.clone(),
                        context: context.clone(),
                        service: service.clone(),
                        status: "removed",
                        live: Some(l.clone()),
                        current: None,
                        loc: None,
                    });
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff_with(context: &str, services: &[(&str, &str)]) -> ContractDiff {
        ContractDiff {
            lock_present: true,
            changes: Vec::new(),
            services: BTreeMap::from([(
                context.to_string(),
                ProvidedContracts {
                    worker: context.replace('.', "-"),
                    hashes: services
                        .iter()
                        .map(|(s, h)| (s.to_string(), h.to_string()))
                        .collect(),
                },
            )]),
        }
    }

    fn model(diff: &ContractDiff, ledger: &str) -> CdModel {
        to_wire(
            diff,
            (LedgerState::Read, Some(ledger)),
            Path::new("/p"),
            &ProjectIndex::default(),
            &HashMap::new(),
        )
    }

    const LEDGER: &str = r#"
version = 1
[environments.default.workers.shop-billing]
deployed = true
[environments.default.workers.shop-billing.contracts]
charge = "aaaa"
refund = "bbbb"
"#;

    #[test]
    fn an_unchanged_hash_yields_no_row() {
        let m = model(
            &diff_with("shop.billing", &[("charge", "aaaa"), ("refund", "bbbb")]),
            LEDGER,
        );
        assert!(m.services.is_empty());
        assert_eq!(m.ledger, LedgerState::Read);
    }

    #[test]
    fn a_moved_hash_is_changed_never_breaking() {
        let m = model(
            &diff_with("shop.billing", &[("charge", "cccc"), ("refund", "bbbb")]),
            LEDGER,
        );
        assert_eq!(m.services.len(), 1);
        let row = &m.services[0];
        assert_eq!((row.status, row.service.as_str()), ("changed", "charge"));
        assert_eq!(row.live.as_deref(), Some("aaaa"));
        assert_eq!(row.current.as_deref(), Some("cccc"));
    }

    #[test]
    fn added_and_removed_services_are_reported() {
        let m = model(
            &diff_with("shop.billing", &[("charge", "aaaa"), ("void", "dddd")]),
            LEDGER,
        );
        let by: Vec<_> = m
            .services
            .iter()
            .map(|r| (r.status, r.service.as_str()))
            .collect();
        assert_eq!(by, vec![("added", "void"), ("removed", "refund")]);
    }

    #[test]
    fn a_context_with_no_live_record_yields_nothing() {
        let m = model(&diff_with("shop.orders", &[("place", "eeee")]), LEDGER);
        assert!(m.services.is_empty(), "silence is not a match");
    }

    #[test]
    fn an_unparseable_ledger_is_reported_not_guessed() {
        let m = model(
            &diff_with("shop.billing", &[("charge", "cccc")]),
            "not toml [",
        );
        assert_eq!(m.ledger, LedgerState::Unreadable);
        assert!(m.services.is_empty());
    }
}
