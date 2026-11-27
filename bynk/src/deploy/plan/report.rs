//! #3797: the entries of a plan's `refusals` and `warnings`.
//!
//! `bynk deploy --format json` always prints a document on stdout, a refused run included, so a
//! client reads why a plan stopped from a field instead of matching the stderr line. An entry
//! carries a stable `code`, the human `message`, the facts of its subject (`context`, `from`, …)
//! and, where one flag overrides it, `override`. The stderr lines are as before.

use std::process::ExitCode;

use bynk_driver::emsg;
use serde::Serialize;
use serde_json::{Map, Value};

/// One refusal or warning of a plan.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Entry {
    pub(crate) code: String,
    pub(crate) message: String,
    /// The facts this code carries (the context, the scripts, …), flat beside `code`.
    #[serde(flatten)]
    pub(crate) subject: Map<String, Value>,
    /// The flag that overrides a refusal, when one does.
    #[serde(rename = "override", skip_serializing_if = "Option::is_none")]
    pub(crate) override_flag: Option<String>,
}

impl Entry {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            subject: Map::new(),
            override_flag: None,
        }
    }

    /// An entry for a refusal the run prints itself, as it always did: the text starts
    /// `bynk: error: ` and may run over several lines, and the entry carries it without that prefix.
    pub(crate) fn from_printed(code: &str, printed: &str) -> Self {
        let message = printed
            .strip_prefix("bynk: error: ")
            .or_else(|| printed.strip_prefix("bynk: warning: "))
            .or_else(|| printed.strip_prefix("bynk: note: "))
            .unwrap_or(printed)
            .trim_end();
        Self::new(code, message)
    }

    /// Add one fact of the subject.
    pub(crate) fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.subject.insert(key.to_string(), value.into());
        self
    }

    /// Name the flag that overrides this refusal.
    pub(crate) fn overridden_by(mut self, flag: impl Into<String>) -> Self {
        self.override_flag = Some(flag.into());
        self
    }

    /// `code: message`, the text the stderr line carries.
    pub(crate) fn line(&self) -> String {
        format!("{}: {}", self.code, self.message)
    }
}

/// The document of a run that stopped before its plan was derived: what is known (the
/// environment and the upload order) beside the refusals and warnings, with the plan's own keys
/// left out.
#[derive(Debug, Serialize)]
struct Refused<'a> {
    environment: &'a str,
    refusals: &'a [Entry],
    warnings: &'a [Entry],
    order: &'a [String],
}

/// [`Refused`], rendered as `--format json` prints a plan.
pub(crate) fn refused_report(
    environment: &str,
    order: &[String],
    refusals: &[Entry],
    warnings: &[Entry],
) -> String {
    let doc = Refused {
        environment,
        refusals,
        warnings,
        order,
    };
    format!(
        "{}\n",
        serde_json::to_string_pretty(&doc).expect("a refusal report serialises")
    )
}

/// The codes of the refusals a run stops on before it reads the account (#3797, part 2).
pub(crate) mod code {
    pub(crate) const ENV_PASSTHROUGH_CONFLICT: &str = "bynk.deploy.env_passthrough_conflict";
    pub(crate) const PROJECT_UNDEPLOYABLE: &str = "bynk.deploy.project_undeployable";
    pub(crate) const PREFLIGHT_FAILED: &str = "bynk.deploy.preflight_failed";
    pub(crate) const CONTEXT_NOT_FOUND: &str = "bynk.deploy.context_not_found";
    pub(crate) const NO_WORKERS_BUILT: &str = "bynk.deploy.no_workers_built";
    pub(crate) const WORKERS_UNREADABLE: &str = "bynk.deploy.workers_unreadable";
    pub(crate) const EMAIL_CLAIM_REFUSED: &str = "bynk.deploy.email_claim_refused";
    pub(crate) const SECRETS_SOURCE_UNREADABLE: &str = "bynk.deploy.secrets_source_unreadable";
    pub(crate) const LEDGER_UNREADABLE: &str = "bynk.deploy.ledger_unreadable";
    pub(crate) const DEPENDENCY_NOT_DEPLOYED: &str = "bynk.deploy.dependency_not_deployed";
    pub(crate) const CONTRACTS_MANIFEST_UNREADABLE: &str =
        "bynk.deploy.contracts_manifest_unreadable";
    pub(crate) const CONTRACTS_LOCK_UNWRITABLE: &str = "bynk.deploy.contracts_lock_unwritable";
    pub(crate) const REBUILD_FAILED: &str = "bynk.deploy.rebuild_failed";
    pub(crate) const UPLOAD_ORDER_CYCLE: &str = "bynk.deploy.upload_order_cycle";
    pub(crate) const RECREATE_NEEDS_YES: &str = "bynk.deploy.recreate_needs_yes";
    pub(crate) const PINS_UNREADABLE: &str = "bynk.deploy.pins_unreadable";
    pub(crate) const ADOPTION_UNREADABLE: &str = "bynk.deploy.adoption_unreadable";
    pub(crate) const ADOPTION_UNMATCHED: &str = "bynk.deploy.adoption_unmatched";
    pub(crate) const ADOPTION_PARTIAL: &str = "bynk.deploy.adoption_partial";
    pub(crate) const ACCOUNT_NOTE: &str = "bynk.deploy.account_note";
    pub(crate) const EMAIL_FORWARD_DESTINATION: &str = "bynk.deploy.email_forward_destination";
    pub(crate) const ACCOUNT_NAME_CONFLICT: &str = "bynk.deploy.account_name_conflict";
    pub(crate) const SECRETS_FILE_PASSTHROUGH: &str = "bynk.deploy.secrets_file_passthrough";
    pub(crate) const STRICT_ENVIRONMENTS_SHARED: &str = "bynk.deploy.strict_environments_shared";
    pub(crate) const STRICT_ENVIRONMENTS_UNCHECKED: &str =
        "bynk.deploy.strict_environments_unchecked";
    pub(crate) const HYPERDRIVE_SETTINGS_UNREADABLE: &str =
        "bynk.deploy.hyperdrive_settings_unreadable";
    pub(crate) const HYPERDRIVE_UNRESOLVED: &str = "bynk.deploy.hyperdrive_unresolved";
}

/// Why `deploy` stopped (#3797): the exit code, and what a client asked for JSON reads of it. A
/// stop with no entries is one the run explained some other way (a build's own diagnostics, a
/// declined confirmation, a failed push); a stop with `refusals` prints them as the plan document.
#[derive(Debug)]
pub(crate) struct Stop {
    pub(crate) code: ExitCode,
    pub(crate) refusals: Vec<Entry>,
    pub(crate) warnings: Vec<Entry>,
    /// The upload order, when the run had one.
    pub(crate) order: Vec<String>,
}

impl From<ExitCode> for Stop {
    fn from(code: ExitCode) -> Self {
        Self {
            code,
            refusals: Vec::new(),
            warnings: Vec::new(),
            order: Vec::new(),
        }
    }
}

impl Stop {
    /// A stop that says why, as one refusal.
    pub(crate) fn refused(entry: Entry) -> Self {
        Self {
            refusals: vec![entry],
            ..Self::from(ExitCode::FAILURE)
        }
    }

    /// The `bynk: error: <message>` line a run has always printed, and the refusal behind it.
    pub(crate) fn error(entry: Entry) -> Self {
        emsg!("bynk: error: {}", entry.message);
        Self::refused(entry)
    }

    /// What the run knew when it stopped: the upload order and the warnings it had gathered
    /// (stderr has printed them), for a stop that did not carry them.
    pub(crate) fn with_run(mut self, order: &[String], warnings: &[Entry]) -> Self {
        if self.order.is_empty() {
            self.order = order.to_vec();
        }
        if self.warnings.is_empty() {
            self.warnings = warnings.to_vec();
        }
        self
    }

    /// A refusal whose text the run prints itself, as it always did (it starts `bynk: error: ` and
    /// may run over several lines): the entry carries the text without that prefix.
    pub(crate) fn printed(code: &str, printed: &str, subject: &[(&str, Value)]) -> Self {
        bynk_driver::colour::eprint(printed);
        let mut entry = Entry::from_printed(code, printed);
        for (key, value) in subject {
            entry = entry.with(key, value.clone());
        }
        Self::refused(entry)
    }
}
