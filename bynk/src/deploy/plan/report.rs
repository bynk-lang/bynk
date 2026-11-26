//! #3797: the entries of a plan's `refusals` and `warnings`.
//!
//! `bynk deploy --format json` always prints a document on stdout, a refused run included, so a
//! client reads why a plan stopped from a field instead of matching the stderr line. An entry
//! carries a stable `code`, the human `message`, the facts of its subject (`context`, `from`, …)
//! and, where one flag overrides it, `override`. The stderr lines are as before.

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
