//! #3943: what a named environment shares with `default`.
//!
//! A staging deploy whose `Service @worker("legacy-billing")` or
//! `Email @allowedRecipients([…])` resolves to the value `default` has calls
//! production's Worker and reaches production's people, and nothing in the build
//! says so. Only the plan sees both environments' values side by side, so it
//! lists each one as an advisory (`shared with default: …`), in the text plan and
//! as `shared_with_default` entries in `--format json`. Advisory, never blocking:
//! sharing a read-only partner Worker can be deliberate, and `--strict-environments`
//! is the opt-in refusal. Setting the value in `[env.<name>.vars]` (even to the
//! same value) records the sharing as deliberate, and the line goes.

use std::collections::{BTreeMap, BTreeSet};

use bynk_check::worker_bindings::{BindingSource, WorkerBinding};
use bynk_project::vars::{ResolvedConst, ValueSource};
use serde::Serialize;

/// One value a named environment takes from `default`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SharedWithDefault {
    /// `env_constant`, `service`, `email_to` or `email_allowed_recipients`.
    pub(crate) kind: &'static str,
    /// The constant's name, or the binding's (`SERVICE_BILLING`).
    pub(crate) name: String,
    /// The shared value, as written (`"legacy-billing"`).
    pub(crate) value: String,
    /// For a constant, the `bynk.toml` variable the environment leaves unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) variable: Option<String>,
}

impl SharedWithDefault {
    /// The plan's line for it.
    pub(crate) fn line(&self, environment: &str) -> String {
        match &self.variable {
            Some(variable) => format!(
                "shared with default: {} → {} ({variable} unset for {environment})",
                self.name, self.value
            ),
            None => format!("shared with default: {} → {}", self.name, self.value),
        }
    }
}

/// The constants of `facts` a named `environment` leaves to `[vars]` or the
/// constant's default, so it takes the value `default` has. Empty for `default`.
pub(crate) fn shared_constants(
    facts: &[ResolvedConst],
    environment: &str,
) -> Vec<SharedWithDefault> {
    if environment == bynk_project::vars::DEFAULT_ENVIRONMENT {
        return Vec::new();
    }
    let own = ValueSource::EnvVars(environment.to_string());
    facts
        .iter()
        .filter(|f| f.source != own)
        .map(|f| SharedWithDefault {
            kind: "env_constant",
            name: f.constant.clone(),
            value: f.value.clone(),
            variable: Some(f.variable.clone()),
        })
        .collect()
}

/// Whether `bindings` hold anything [`shared_bindings`] compares: a Service to a
/// Worker outside the build, or an `Email`. When they do not, `default` need not
/// be built at all.
pub(crate) fn has_comparable_bindings(
    bindings: &BTreeMap<String, Vec<WorkerBinding>>,
    own_scripts: &BTreeSet<String>,
) -> bool {
    bindings.values().flatten().any(|b| match &b.source {
        BindingSource::ForeignService { worker, .. } => !own_scripts.contains(worker),
        BindingSource::Email { .. } => true,
        _ => false,
    })
}

/// What to say, and whether to refuse, when `default` could not be built so its
/// `Service` and `Email` values were not compared: a literal worker name is the
/// same in every build, so `--strict-environments` cannot vouch for isolation.
pub(crate) fn comparison_skipped(environment: &str, strict: bool) -> (String, bool) {
    if strict {
        (
            format!(
                "bynk: error: `default` could not be built, so whether `{environment}` shares a `Service` worker or an `Email` address with it was not checked (`--strict-environments`)"
            ),
            true,
        )
    } else {
        (
            format!(
                "bynk: note: `default` could not be built, so `{environment}`'s `Service` workers and `Email` addresses were not compared with it; fix `default` (often a required variable only an environment sets) to see what is shared"
            ),
            false,
        )
    }
}

/// The `Service @worker` and `Email @to` / `@allowedRecipients` bindings whose
/// value in `environment`'s build equals `default`'s build, matched by context
/// and binding name. `own_scripts` is the build's own Workers' script names: a
/// Service that reaches one is a project edge, which an environment names apart
/// from `default`'s, so only a Service to a Worker outside the build is shared.
pub(crate) fn shared_bindings(
    environment: &BTreeMap<String, Vec<WorkerBinding>>,
    default: &BTreeMap<String, Vec<WorkerBinding>>,
    own_scripts: &BTreeSet<String>,
) -> Vec<SharedWithDefault> {
    let mut out = Vec::new();
    for (context, bindings) in environment {
        let Some(defaults) = default.get(context) else {
            continue;
        };
        for binding in bindings {
            let Some(other) = defaults.iter().find(|d| d.name == binding.name) else {
                continue;
            };
            match (&binding.source, &other.source) {
                (
                    BindingSource::ForeignService { worker, .. },
                    BindingSource::ForeignService { worker: same, .. },
                ) if worker == same && !own_scripts.contains(worker) => {
                    out.push(shared("service", &binding.name, format!("{worker:?}")));
                }
                (
                    BindingSource::Email {
                        to,
                        allowed_recipients,
                        ..
                    },
                    BindingSource::Email {
                        to: same_to,
                        allowed_recipients: same_recipients,
                        ..
                    },
                ) => {
                    if let Some(to) = to
                        && Some(to) == same_to.as_ref()
                    {
                        out.push(shared("email_to", &binding.name, format!("{to:?}")));
                    }
                    if !allowed_recipients.is_empty() && allowed_recipients == same_recipients {
                        out.push(shared(
                            "email_allowed_recipients",
                            &binding.name,
                            format!("{allowed_recipients:?}"),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// #3943 decision C: a binding whose value is one the environment sets in its
/// own `[env.<name>.vars]` records the sharing as deliberate, so it is not
/// listed. The binding model holds only the resolved value, so the match is on
/// the value: a variable set to the same text as `default`'s is the author
/// saying so.
pub(crate) fn without_deliberate(
    shared: Vec<SharedWithDefault>,
    facts: &[ResolvedConst],
    environment: &str,
) -> Vec<SharedWithDefault> {
    let own = ValueSource::EnvVars(environment.to_string());
    let set_here: BTreeSet<&str> = facts
        .iter()
        .filter(|f| f.source == own)
        .map(|f| f.value.as_str())
        .collect();
    shared
        .into_iter()
        .filter(|s| s.kind == "env_constant" || !set_here.contains(s.value.as_str()))
        .collect()
}

fn shared(kind: &'static str, name: &str, value: String) -> SharedWithDefault {
    SharedWithDefault {
        kind,
        name: name.to_string(),
        value,
        variable: None,
    }
}

/// The refusal `--strict-environments` makes of a non-empty set.
pub(crate) fn strict_refusal(shared: &[SharedWithDefault], environment: &str) -> Option<String> {
    if shared.is_empty() {
        return None;
    }
    // Each name once, in order: an `Email` sharing both `@to` and
    // `@allowedRecipients` is two entries of one binding.
    let mut names: Vec<&str> = Vec::new();
    for s in shared {
        if !names.contains(&s.name.as_str()) {
            names.push(s.name.as_str());
        }
    }
    Some(format!(
        "bynk: error: `{environment}` shares {} with `default`; give each its own value under `[env.{environment}.vars]` (`--strict-environments`)",
        names.join(", ")
    ))
}
