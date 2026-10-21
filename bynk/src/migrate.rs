//! `bynk migrate`: where each agent class's stored state stands, and the sweep
//! that migrates every stored instance and raises the class's floor
//! (state-migrations S4, #2259; `design/tracks/state-migrations.md` §3.3,
//! §3.6).
//!
//! `status` is offline. It reads `bynk.schema.lock`, checks the source against
//! it (read-only, as `bynk check` does), and reads `bynk.deploy.lock` only for
//! the environments each Worker is pushed in.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use bynk_emit::project::AgentVersions;
use bynk_project::schema_registry::SchemaRegistry;
use serde::Serialize;

/// Output choices for `bynk migrate status`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StatusFormat {
    Short,
    Json,
}

/// One agent class's migration state.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ClassStatus {
    /// `<context>.<Agent>`, the lock's key.
    pub class: String,
    pub version: i64,
    pub floor: i64,
    /// The oldest stored version that reads forward to `version` without a
    /// step (an all-additive path).
    pub oldest_readable: i64,
    pub steps: Vec<StepStatus>,
    /// Versions from the floor up that no step covers and that have no
    /// all-additive path: a stored record there faults `NoMigrationPath`.
    pub without_path: Vec<i64>,
    /// The environments `bynk.deploy.lock` records this class's Worker in. A
    /// floor-raising sweep must cover every one.
    pub deployed_to: Vec<String>,
}

/// One valid `migrate from N` step and the stored versions it covers.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct StepStatus {
    pub from: i64,
    /// The oldest version the step covers (`lo..=from`).
    pub covers_from: i64,
}

/// The Worker directory holding agent class `class` (`<unit>.<Agent>`): the
/// unit's qualified name with `.` replaced by `-`.
pub(crate) fn worker_of(class: &str) -> String {
    let unit = class.rsplit_once('.').map_or(class, |(unit, _)| unit);
    unit.replace('.', "-")
}

/// Every recorded agent class's status, in key order. `versions` is the
/// check's reconciled view (steps that hold, by class); `deployed` maps each
/// Worker directory to its recorded environments.
pub fn class_statuses(
    lock: &SchemaRegistry,
    versions: &std::collections::HashMap<String, AgentVersions>,
    deployed: &BTreeMap<String, Vec<String>>,
) -> Vec<ClassStatus> {
    lock.agents()
        .map(|(class, entry)| {
            // A class the source no longer declares has no reconciled view:
            // its recorded entry alone says where it stands.
            let (oldest_readable, steps) = match versions.get(class) {
                Some(v) => (
                    v.oldest_readable,
                    v.steps
                        .iter()
                        .map(|s| StepStatus {
                            from: s.from,
                            covers_from: s.lo,
                        })
                        .collect(),
                ),
                None => (entry.oldest_readable(), Vec::new()),
            };
            let without_path = (entry.floor..entry.version)
                .filter(|v| {
                    *v < oldest_readable
                        && !steps
                            .iter()
                            .any(|s: &StepStatus| s.covers_from <= *v && *v <= s.from)
                })
                .collect();
            ClassStatus {
                class: class.clone(),
                version: entry.version,
                floor: entry.floor,
                oldest_readable,
                steps,
                without_path,
                deployed_to: deployed.get(&worker_of(class)).cloned().unwrap_or_default(),
            }
        })
        .collect()
}

/// `status`'s short form: one block per class.
pub fn render_short(classes: &[ClassStatus]) -> String {
    if classes.is_empty() {
        return "no agent classes recorded in bynk.schema.lock\n".to_string();
    }
    let mut out = String::new();
    for c in classes {
        out.push_str(&format!(
            "{}  version {}, floor {}\n",
            c.class, c.version, c.floor
        ));
        for s in &c.steps {
            let range = if s.covers_from == s.from {
                s.from.to_string()
            } else {
                format!("{}..={}", s.covers_from, s.from)
            };
            out.push_str(&format!(
                "  migrate from {}  covers {range}; deletable once the floor passes {}\n",
                s.from, s.from
            ));
        }
        if c.oldest_readable < c.version {
            out.push_str(&format!(
                "  versions {}..={} read forward without a step\n",
                c.oldest_readable,
                c.version - 1
            ));
        }
        for v in &c.without_path {
            out.push_str(&format!(
                "  version {v} has no path: a stored record there faults `NoMigrationPath`\n"
            ));
        }
        if c.deployed_to.is_empty() {
            out.push_str("  deployed to: nothing recorded in bynk.deploy.lock\n");
        } else {
            out.push_str(&format!(
                "  deployed to: {} (a sweep of {} can raise the floor)\n",
                c.deployed_to.join(", "),
                if c.deployed_to.len() == 1 {
                    "it"
                } else if c.deployed_to.len() == 2 {
                    "both"
                } else {
                    "all of them"
                }
            ));
        }
    }
    out
}

/// Run `bynk migrate status` on the project at `root`.
pub fn status(root: &Path, format: StatusFormat) -> ExitCode {
    match status_report(root) {
        Ok(classes) => {
            match format {
                StatusFormat::Short => print!("{}", render_short(&classes)),
                StatusFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "classes": classes }))
                        .expect("a status report always serializes")
                ),
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("bynk migrate status: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The lock, the source checked against it, and the ledger, as statuses. A
/// source with errors is refused: a step that fails its check covers nothing,
/// so the report would understate what the next build does.
fn status_report(root: &Path) -> Result<Vec<ClassStatus>, String> {
    let existing = bynk_driver::schema_lock::read(root).map_err(|e| {
        format!(
            "could not read {}: {e}",
            bynk_driver::schema_lock::lock_path(root).display()
        )
    })?;
    let lock = bynk_project::schema_registry::parse(existing.as_deref(), root)?;
    let options = bynk_driver::try_project_options(root).map_err(|e| e.to_string())?;
    let options = options.schema_registry(bynk_emit::project::SchemaLock::On { existing });
    let check = bynk_emit::project::check_project(&options);
    if check.has_errors() {
        bynk_driver::print_project_check_short(&check);
        return Err("the project has errors; fix them first (`bynk check`)".to_string());
    }
    let workers: Vec<String> = lock.agents().map(|(class, _)| worker_of(class)).collect();
    let deployed = crate::deploy::deployed_environments(root, &workers)?;
    Ok(class_statuses(&lock, &check.agent_versions, &deployed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_emit::project::MigrationStep;
    use bynk_project::schema_registry::{AgentEntry, AgentShape};

    fn entry(version: i64, floor: i64, additive: &[bool]) -> AgentEntry {
        AgentEntry {
            version,
            floor,
            shapes: (floor..=version)
                .zip(additive.iter())
                .map(|(v, a)| AgentShape {
                    version: v,
                    additive: *a,
                    fields: Vec::new(),
                })
                .collect(),
            renamed_from: Vec::new(),
        }
    }

    fn step(lo: i64, from: i64) -> MigrationStep {
        MigrationStep {
            lo,
            from,
            carried: Vec::new(),
            known: Vec::new(),
            store_maps: Vec::new(),
        }
    }

    #[test]
    fn worker_of_dasherises_the_unit() {
        assert_eq!(worker_of("shop.Cart"), "shop");
        assert_eq!(worker_of("acme.shop.Cart"), "acme-shop");
    }

    #[test]
    fn a_class_reports_its_steps_gaps_and_environments() {
        let mut lock = SchemaRegistry::new();
        // 1 -> 2 additive, 2 -> 3 not, 3 -> 4 not: a step from 3 only.
        lock.insert_agent(
            "shop.Cart".to_string(),
            entry(4, 1, &[false, true, false, false]),
        );
        let versions = std::collections::HashMap::from([(
            "shop.Cart".to_string(),
            AgentVersions {
                version: 4,
                oldest_readable: 4,
                steps: vec![step(3, 3)],
            },
        )]);
        let deployed = BTreeMap::from([(
            "shop".to_string(),
            vec!["default".to_string(), "staging".to_string()],
        )]);
        let classes = class_statuses(&lock, &versions, &deployed);
        assert_eq!(
            classes,
            vec![ClassStatus {
                class: "shop.Cart".to_string(),
                version: 4,
                floor: 1,
                oldest_readable: 4,
                steps: vec![StepStatus {
                    from: 3,
                    covers_from: 3
                }],
                without_path: vec![1, 2],
                deployed_to: vec!["default".to_string(), "staging".to_string()],
            }]
        );
        let text = render_short(&classes);
        assert!(text.contains("shop.Cart  version 4, floor 1"), "{text}");
        assert!(text.contains("migrate from 3  covers 3;"), "{text}");
        assert!(text.contains("version 1 has no path"), "{text}");
        assert!(
            text.contains("deployed to: default, staging (a sweep of both"),
            "{text}"
        );
    }

    #[test]
    fn a_class_gone_from_source_reads_from_its_entry() {
        let mut lock = SchemaRegistry::new();
        lock.insert_agent("shop.Old".to_string(), entry(2, 1, &[false, true]));
        let classes = class_statuses(
            &lock,
            &std::collections::HashMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(classes[0].oldest_readable, 1);
        assert!(classes[0].without_path.is_empty());
        assert!(render_short(&classes).contains("nothing recorded in bynk.deploy.lock"));
    }
}
