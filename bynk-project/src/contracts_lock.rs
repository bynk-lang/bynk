//! #2554: `bynk.contracts.lock`, the older service contracts a callee still
//! accepts, as a document.
//!
//! A callee compares the `X-Bynk-Contract` hash a caller stamps with its own
//! (ADR 0200). Once the classifier (`bynk_check::contract_compat`, #2485) can
//! say a change is compatible, a callee can be deployed ahead of its callers
//! if it also accepts the hash they were compiled against. That hash cannot be
//! derived from the callee's source, which only knows the contract it has
//! now, so the older contracts are recorded here, committed, and visible in
//! review.
//!
//! The document holds the older contracts as the shape text
//! (`bynk_check::contract_compat::ServiceShape::to_text`), not hashes: the
//! build re-checks each against the contract the callee has now, and a hash
//! cannot be classified. This module is pure, like `schema_registry`: it
//! reads and writes no disk. `bynk-driver`'s `contracts_lock` module owns the
//! file, and `bynk-check` owns what the shapes mean.
//!
//! ```toml
//! version = 1
//!
//! [accepts."shop.payment"]
//! charge = ['Svc("charge",P("order",B("Int")),Ret(B("Bool")))']
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file at the project root.
pub const LOCK_FILE: &str = "bynk.contracts.lock";

const LOCK_VERSION: u32 = 1;

/// The older shapes each context's services accept: qualified context name
/// (`shop.payment`) to service to shape texts, oldest first.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct ContractsLock {
    // No serde `default`: a lock with no `version` is a truncated write, not
    // an empty lock, and must fail the read rather than silently forget what
    // a callee accepts (the argument `bynk.schema.lock` makes for the same
    // field).
    version: u32,
    #[serde(default)]
    accepts: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

impl ContractsLock {
    pub fn new() -> Self {
        ContractsLock {
            version: LOCK_VERSION,
            accepts: BTreeMap::new(),
        }
    }

    /// The older shapes `context`'s `service` accepts, oldest first.
    pub fn accepted(&self, context: &str, service: &str) -> &[String] {
        self.accepts
            .get(context)
            .and_then(|s| s.get(service))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Every context's accepted shapes.
    pub fn contexts(&self) -> impl Iterator<Item = (&String, &BTreeMap<String, Vec<String>>)> {
        self.accepts.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.accepts.is_empty()
    }

    /// Record `shape` as accepted by `context`'s `service`. Returns whether it
    /// was new; an entry already there is left where it is.
    pub fn accept(&mut self, context: &str, service: &str, shape: &str) -> bool {
        let shapes = self
            .accepts
            .entry(context.to_string())
            .or_default()
            .entry(service.to_string())
            .or_default();
        if shapes.iter().any(|s| s == shape) {
            return false;
        }
        shapes.push(shape.to_string());
        true
    }

    /// Replace what `context`'s `service` accepts, dropping the service (and
    /// the context, when it was the last) if `shapes` is empty.
    pub fn set(&mut self, context: &str, service: &str, shapes: Vec<String>) {
        if shapes.is_empty() {
            if let Some(services) = self.accepts.get_mut(context) {
                services.remove(service);
                if services.is_empty() {
                    self.accepts.remove(context);
                }
            }
            return;
        }
        self.accepts
            .entry(context.to_string())
            .or_default()
            .insert(service.to_string(), shapes);
    }
}

/// Read the lock's pre-read content. `None` is a project that has none, which
/// accepts nothing. `Some(text)` that is empty, unparseable or of another
/// version is corruption and fails: a callee silently forgetting the callers
/// it accepts would answer them `409` in production.
///
/// `project_root` only names the file in a message.
pub fn parse(existing: Option<&str>, project_root: &Path) -> Result<ContractsLock, String> {
    let Some(text) = existing else {
        return Ok(ContractsLock::new());
    };
    let path = project_root.join(LOCK_FILE);
    if text.trim().is_empty() {
        return Err(format!(
            "contracts lock `{}` is empty or truncated (corrupt); restore it from version control",
            path.display()
        ));
    }
    let lock: ContractsLock = toml::from_str(text).map_err(|e| {
        format!(
            "contracts lock `{}` is corrupt ({e}); restore it from version control",
            path.display()
        )
    })?;
    if lock.version != LOCK_VERSION {
        return Err(format!(
            "unsupported contracts lock version {} (`{}`)",
            lock.version,
            path.display()
        ));
    }
    Ok(lock)
}

/// The lock's TOML form. Cannot fail for this data shape (string keys and
/// string lists).
pub fn serialize(lock: &ContractsLock) -> String {
    toml::to_string_pretty(lock).expect("ContractsLock always serializes")
}

/// How many older contracts one service may accept before the build warns. A
/// callee that accepts a long tail has not been pruned: callers have moved on.
pub const ACCEPTED_CAP: usize = 8;

fn attributed(category: &'static str, message: String, note: &str) -> crate::AttributedError {
    let mut error =
        bynk_syntax::CompileError::new(category, bynk_syntax::span::Span::default(), message);
    error.notes.push(note.to_string());
    crate::AttributedError::new(Some(PathBuf::from(LOCK_FILE)), error)
}

/// `bynk.project.contracts_lock_corrupt`: the lock cannot be read. Like the
/// schema lock, it fails the build rather than reading as empty.
pub fn corrupt_error(message: String) -> crate::AttributedError {
    attributed(
        "bynk.project.contracts_lock_corrupt",
        message,
        "restore it from version control rather than deleting it: a callee that forgets the contracts it accepts refuses its live callers",
    )
}

/// `bynk.project.contracts_lock_stale` (warning): an entry the callee does not
/// accept, because it is unreadable or no longer compatible with the contract
/// the service has now. The build leaves it out; the lock is not rewritten.
pub fn stale_warning(context: &str, service: &str, why: &str) -> crate::AttributedError {
    attributed(
        "bynk.project.contracts_lock_stale",
        format!(
            "`{LOCK_FILE}` lists an older contract of `{context}.{service}` that the service no longer accepts: {why}"
        ),
        "the entry is ignored; callers still on that contract are refused (409 ContractMismatch). Remove the entry once they have moved on",
    )
}

/// `bynk.project.contracts_lock_oversized` (warning): a service accepts more
/// than [`ACCEPTED_CAP`] older contracts.
pub fn oversized_warning(context: &str, service: &str, count: usize) -> crate::AttributedError {
    attributed(
        "bynk.project.contracts_lock_oversized",
        format!(
            "`{LOCK_FILE}` has `{context}.{service}` accepting {count} older contracts (more than {ACCEPTED_CAP})"
        ),
        "remove the entries no live caller was compiled against",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> &'static Path {
        Path::new("/project")
    }

    #[test]
    fn no_file_accepts_nothing() {
        let lock = parse(None, root()).unwrap();
        assert!(lock.is_empty());
        assert!(lock.accepted("shop.payment", "charge").is_empty());
    }

    #[test]
    fn a_lock_round_trips() {
        let mut lock = ContractsLock::new();
        assert!(lock.accept("shop.payment", "charge", "one"));
        assert!(lock.accept("shop.payment", "charge", "two"));
        assert!(
            !lock.accept("shop.payment", "charge", "one"),
            "no duplicate"
        );
        assert!(lock.accept("shop.orders", "place", "three"));
        let text = serialize(&lock);
        let back = parse(Some(&text), root()).unwrap();
        assert_eq!(back, lock);
        assert_eq!(back.accepted("shop.payment", "charge"), ["one", "two"]);
    }

    #[test]
    fn a_shape_with_quotes_and_commas_survives() {
        let shape = r#"Svc("charge",P("o",B("Int")),Ret(Rf("T","String","Matches(\"a,b\")")))"#;
        let mut lock = ContractsLock::new();
        lock.accept("shop.payment", "charge", shape);
        let back = parse(Some(&serialize(&lock)), root()).unwrap();
        assert_eq!(back.accepted("shop.payment", "charge"), [shape]);
    }

    #[test]
    fn set_replaces_and_empties() {
        let mut lock = ContractsLock::new();
        lock.accept("shop.payment", "charge", "one");
        lock.accept("shop.payment", "refund", "x");
        lock.set("shop.payment", "charge", vec!["two".into()]);
        assert_eq!(lock.accepted("shop.payment", "charge"), ["two"]);
        lock.set("shop.payment", "charge", Vec::new());
        assert!(lock.accepted("shop.payment", "charge").is_empty());
        lock.set("shop.payment", "refund", Vec::new());
        assert!(lock.is_empty(), "an emptied context disappears");
    }

    #[test]
    fn a_corrupt_lock_fails_rather_than_forgetting() {
        for bad in ["", "   \n", "not toml [", "accepts = {}"] {
            assert!(parse(Some(bad), root()).is_err(), "{bad:?}");
        }
        assert!(
            parse(Some("version = 2\n"), root())
                .unwrap_err()
                .contains("unsupported")
        );
    }
}
