//! #3797: the entries of the refusals `deploy` makes before it reads the account, built in one
//! place so each is a plain function a test can call. The codes are in [`super::code`].

use std::path::Path;

use serde_json::Value;

use super::{Entry, code};

/// `--accept-compatible-skew` could not write the older contracts it records.
pub(crate) fn lock_unwritable(path: &Path, cause: &dyn std::fmt::Display) -> Entry {
    Entry::new(
        code::CONTRACTS_LOCK_UNWRITABLE,
        format!(
            "could not record the older contracts in {}: {cause}",
            path.display()
        ),
    )
    .with("path", path.display().to_string())
}

/// The rebuild that should carry the recorded contracts failed.
pub(crate) fn rebuild_failed(path: &Path) -> Entry {
    Entry::new(
        code::REBUILD_FAILED,
        format!(
            "the older contracts were recorded in {}, but the rebuild failed; nothing was deployed. The entries are safe to keep (every build re-checks them), or revert the file.",
            path.display()
        ),
    )
    .with("path", path.display().to_string())
}

/// What each built Worker declares could not be read.
pub(crate) fn workers_unreadable(cause: &dyn std::fmt::Display) -> Entry {
    Entry::new(code::WORKERS_UNREADABLE, cause.to_string())
}

/// `[deploy.email]` could not be applied to the named environment.
pub(crate) fn email_claim_refused(environment: &str, cause: &dyn std::fmt::Display) -> Entry {
    Entry::new(code::EMAIL_CLAIM_REFUSED, cause.to_string()).with("environment", environment)
}

/// A Worker's contracts manifest could not be read.
pub(crate) fn manifest_unreadable(worker: &str, cause: &dyn std::fmt::Display) -> Entry {
    Entry::new(
        code::CONTRACTS_MANIFEST_UNREADABLE,
        format!(
            "could not read `{worker}`'s {}: {cause}",
            bynk_emit::emitter::contracts::CONTRACTS_MANIFEST
        ),
    )
    .with("worker", worker)
}

/// `bynk.foreign.toml` did not read.
pub(crate) fn pins_unreadable(path: &str, cause: &dyn std::fmt::Display) -> Entry {
    Entry::new(code::PINS_UNREADABLE, cause.to_string()).with("path", path)
}

/// Two instances would share a name in the account.
pub(crate) fn account_name_conflict(message: String) -> Entry {
    Entry::new(code::ACCOUNT_NAME_CONFLICT, message)
}

/// A passthrough `--secrets-file` would be a second one on a first deploy.
pub(crate) fn secrets_file_passthrough(printed: &str) -> Entry {
    Entry::from_printed(code::SECRETS_FILE_PASSTHROUGH, printed)
}

/// `--strict-environments`: the environment shares values with `default`.
pub(crate) fn strict_environments_shared(
    printed: &str,
    environment: &str,
    shared: &[String],
) -> Entry {
    Entry::from_printed(code::STRICT_ENVIRONMENTS_SHARED, printed)
        .with("environment", environment)
        .with("shared", shared)
}

/// `--strict-environments`: `default` could not be built, so nothing was compared.
pub(crate) fn strict_environments_unchecked(printed: &str, environment: &str) -> Entry {
    Entry::from_printed(code::STRICT_ENVIRONMENTS_UNCHECKED, printed)
        .with("environment", environment)
}

/// `[deploy.hyperdrive]` in `bynk.toml` did not read.
pub(crate) fn hyperdrive_settings_unreadable(cause: &dyn std::fmt::Display) -> Entry {
    Entry::new(code::HYPERDRIVE_SETTINGS_UNREADABLE, cause.to_string())
}

/// A Hyperdrive binding's connection string or settings did not resolve.
pub(crate) fn hyperdrive_unresolved(problem: &dyn std::fmt::Display) -> Entry {
    Entry::new(code::HYPERDRIVE_UNRESOLVED, problem.to_string())
}

/// A changed `@jurisdiction` would make a new R2 bucket without `--yes`.
pub(crate) fn recreate_needs_yes(message: &str) -> Entry {
    Entry::new(code::RECREATE_NEEDS_YES, message)
}

/// The doctor found the environment not ready for a deploy. The entry carries the doctor's own
/// pinned JSON (`capabilities`, each with its `rows`: `label`, `level`, `detail`, `remedy`), so a
/// client reads which check failed without parsing the table in the message.
pub(crate) fn preflight_failed(printed: &str, report: &crate::doctor::Report) -> Entry {
    let message = printed
        .strip_prefix("bynk: error: ")
        .unwrap_or(printed)
        .trim_end();
    let mut entry = Entry::new(code::PREFLIGHT_FAILED, message);
    if let Ok(Value::Object(doc)) =
        serde_json::from_str(&crate::report::render(report, crate::report::Format::Json))
        && let Some(capabilities) = doc.get("capabilities")
    {
        entry = entry.with("capabilities", capabilities.clone());
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::{Capability, CapabilityReport, Level, Report, Row};

    #[test]
    fn an_unwritable_contracts_lock_names_its_path() {
        let e = lock_unwritable(Path::new("bynk.contracts.lock"), &"read-only");
        assert_eq!(e.code, "bynk.deploy.contracts_lock_unwritable");
        assert_eq!(e.subject["path"], "bynk.contracts.lock");
        assert!(e.message.contains("read-only"), "{}", e.message);
    }

    #[test]
    fn a_failed_rebuild_names_the_lock_it_recorded_in() {
        let e = rebuild_failed(Path::new("bynk.contracts.lock"));
        assert_eq!(e.code, "bynk.deploy.rebuild_failed");
        assert_eq!(e.subject["path"], "bynk.contracts.lock");
        assert!(e.message.contains("nothing was deployed"), "{}", e.message);
    }

    #[test]
    fn unreadable_worker_declarations_carry_the_cause() {
        let e = workers_unreadable(&"no wrangler.toml in shop-orders");
        assert_eq!(e.code, "bynk.deploy.workers_unreadable");
        assert_eq!(e.message, "no wrangler.toml in shop-orders");
    }

    #[test]
    fn a_refused_email_claim_names_its_environment() {
        let e = email_claim_refused("staging", &"no mapping");
        assert_eq!(e.code, "bynk.deploy.email_claim_refused");
        assert_eq!(e.subject["environment"], "staging");
    }

    #[test]
    fn an_unreadable_manifest_names_its_worker() {
        let e = manifest_unreadable("shop-orders", &"no such file");
        assert_eq!(e.code, "bynk.deploy.contracts_manifest_unreadable");
        assert_eq!(e.subject["worker"], "shop-orders");
        assert!(
            e.message.starts_with("could not read `shop-orders`'s "),
            "{}",
            e.message
        );
    }

    #[test]
    fn the_refusals_after_the_build_have_their_codes_and_subjects() {
        let e = pins_unreadable("bynk.foreign.toml", &"bad toml");
        assert_eq!(e.code, "bynk.deploy.pins_unreadable");
        assert_eq!(e.subject["path"], "bynk.foreign.toml");
        assert_eq!(
            account_name_conflict("two instances".into()).code,
            "bynk.deploy.account_name_conflict"
        );
        let e = secrets_file_passthrough("bynk: error: a passthrough file\n");
        assert_eq!(e.code, "bynk.deploy.secrets_file_passthrough");
        assert_eq!(e.message, "a passthrough file");
        let e = strict_environments_shared(
            "bynk: error: `staging` shares X",
            "staging",
            &["X".to_string()],
        );
        assert_eq!(e.code, "bynk.deploy.strict_environments_shared");
        assert_eq!(e.subject["environment"], "staging");
        assert_eq!(e.subject["shared"], serde_json::json!(["X"]));
        let e =
            strict_environments_unchecked("bynk: error: `default` could not be built", "staging");
        assert_eq!(e.code, "bynk.deploy.strict_environments_unchecked");
        assert_eq!(
            hyperdrive_settings_unreadable(&"bad").code,
            "bynk.deploy.hyperdrive_settings_unreadable"
        );
        assert_eq!(
            hyperdrive_unresolved(&"no variable").code,
            "bynk.deploy.hyperdrive_unresolved"
        );
        let e = recreate_needs_yes("move the data, then re-run with --yes");
        assert_eq!(e.code, "bynk.deploy.recreate_needs_yes");
        assert!(e.subject.is_empty());
    }

    #[test]
    fn a_failed_preflight_carries_the_doctors_failing_rows() {
        let report = Report {
            driver_version: "0.0.0".into(),
            capabilities: vec![CapabilityReport {
                capability: Capability::Deploy,
                optional: false,
                level: Level::Fail,
                rows: vec![Row {
                    label: "wrangler".into(),
                    level: Level::Fail,
                    detail: "not found".into(),
                    remedy: Some("npm install -g wrangler".into()),
                }],
            }],
        };
        let e = preflight_failed("bynk: error: environment not ready for `deploy`\n", &report);
        assert_eq!(e.code, "bynk.deploy.preflight_failed");
        assert_eq!(e.message, "environment not ready for `deploy`");
        let caps = e.subject["capabilities"].as_array().expect("capabilities");
        assert_eq!(caps[0]["capability"], "deploy");
        assert_eq!(caps[0]["rows"][0]["label"], "wrangler");
        assert_eq!(caps[0]["rows"][0]["level"], "fail");
        assert_eq!(caps[0]["rows"][0]["remedy"], "npm install -g wrangler");
    }
}
