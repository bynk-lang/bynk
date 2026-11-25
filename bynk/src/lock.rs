//! `bynk lock`: changes to `bynk.schema.lock` that are a person's decision.
//!
//! `accept-order` (#3956) records the order a `derives Ord` type now declares,
//! after a reorder that `bynk.types.ord_order_changed` refused. Moving a field or
//! a variant changes how every value of the type sorts, so the step is
//! deliberate and shows up in the diff of the lock.

use std::path::Path;
use std::process::ExitCode;

use bynk_driver::emsg;

/// Run `bynk lock accept-order`. `select` is a type name, or `None` for every
/// type whose order differs from the lock.
pub fn accept_order(root: &Path, select: Option<&str>) -> ExitCode {
    match run(root, select) {
        Ok(accepted) => {
            for line in &accepted {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            emsg!("bynk lock accept-order: error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The lines to print for what was accepted; the lock is rewritten when it changes.
fn run(root: &Path, select: Option<&str>) -> Result<Vec<String>, String> {
    let existing = bynk_driver::schema_lock::read(root).map_err(|e| {
        format!(
            "could not read {}: {e}",
            bynk_driver::schema_lock::lock_path(root).display()
        )
    })?;
    let lock = bynk_project::schema_registry::parse(existing.as_deref(), root)?;
    let options = bynk_driver::try_project_options(root).map_err(|e| e.to_string())?;
    let parsed = bynk_driver::project::parse_own_units(&options)?;
    let (updated, accepted) = bynk_check::ord_order::accept_orders(&lock, &parsed, select)?;
    if accepted.is_empty() {
        return Ok(vec![match select {
            Some(name) => format!("{name}: the recorded order is already the declared one"),
            None => "every recorded order is already the declared one".to_string(),
        }]);
    }
    bynk_driver::schema_lock::write(root, &bynk_project::schema_registry::serialize(&updated))
        .map_err(|e| format!("could not write bynk.schema.lock: {e}"))?;
    let mut lines: Vec<String> = accepted
        .iter()
        .map(|key| format!("accepted the order of {key}"))
        .collect();
    lines.push("bynk.schema.lock updated; commit it with the reorder.".to_string());
    Ok(lines)
}
