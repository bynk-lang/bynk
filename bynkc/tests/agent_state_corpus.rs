//! #2220 (transition-diagnostics track §3.8): pin what the agent state
//! model's `transition` overlay finds across the corpus — every `.bynk` file
//! under `examples/` and this crate's `tests/fixtures/positive` — so a change
//! to the model that adds or removes a finding shows up in review. The model
//! is the one a checker warning will read (#2221), and the corpus is too
//! small to measure a false-positive rate (§1.1), so the pinned set is the
//! record.
//!
//! Types are resolved per directory: each file's agents see the sum types
//! declared in any file beside it, which covers every fixture whose state
//! type is declared with its agent or in a sibling commons.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bynk_check::agent_state::{Certainty, Target, agent_state_model};
use bynk_syntax::ast::{CommonsItem, SourceUnit, TypeDecl};

fn bynk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            bynk_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "bynk") {
            out.push(p);
        }
    }
}

#[test]
fn the_overlays_findings_across_the_corpus_are_pinned() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = crate_dir.parent().unwrap();
    let mut files = Vec::new();
    bynk_files(&repo.join("examples"), &mut files);
    bynk_files(&crate_dir.join("tests/fixtures/positive"), &mut files);

    let mut by_dir: HashMap<PathBuf, Vec<(PathBuf, SourceUnit)>> = HashMap::new();
    let mut skipped = Vec::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        let parsed = bynk_syntax::lexer::tokenize(&src).ok().and_then(|tokens| {
            match bynk_syntax::parser::parse_unit_with_recovery(&tokens, &src) {
                // A warning (`308`'s orphan doc block) doesn't stop the unit.
                (Some(u), errs)
                    if errs.iter().all(|e| {
                        bynk_syntax::Severity::for_error(e) == bynk_syntax::Severity::Warning
                    }) =>
                {
                    Some(u)
                }
                _ => None,
            }
        });
        match parsed {
            Some(u) => by_dir
                .entry(f.parent().unwrap().to_path_buf())
                .or_default()
                .push((f, u)),
            None => skipped.push(f),
        }
    }
    // Every corpus file parses, so no finding can be lost to a file silently
    // dropped.
    assert!(
        skipped.is_empty(),
        "corpus files that did not parse: {skipped:?}"
    );

    let items = |u: &SourceUnit| match u {
        SourceUnit::Context(c) => Some(c.items.clone()),
        SourceUnit::Commons(c) => Some(c.items.clone()),
        _ => None,
    };
    let mut agents = 0;
    let mut machines = 0;
    let mut findings = Vec::new();
    let mut not_evaluated = Vec::new();
    for units in by_dir.values() {
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        for (_, u) in units {
            for i in items(u).unwrap_or_default() {
                if let CommonsItem::Type(t) = i {
                    types.insert(t.name.name.clone(), Arc::new(t));
                }
            }
        }
        for (f, u) in units {
            // `/`-separated on every platform, so the pinned list holds on
            // Windows too.
            let rel = f
                .strip_prefix(repo)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            for i in items(u).unwrap_or_default() {
                let CommonsItem::Agent(a) = i else { continue };
                agents += 1;
                let m = agent_state_model(&a, &types);
                for sm in &m.machines {
                    machines += 1;
                    for n in &sm.not_evaluated {
                        not_evaluated.push(format!(
                            "{rel} {}.{} {}",
                            a.name.name, sm.field, n.transition
                        ));
                    }
                    for e in &sm.edges {
                        for v in &e.violations {
                            let to = match &e.to {
                                Target::State(t) => t.clone(),
                                Target::Unknown => "?".into(),
                                Target::Unchanged => "(unchanged)".into(),
                            };
                            let certainty = match v.certainty {
                                Certainty::Known => "Known",
                                Certainty::Possible => "Possible",
                            };
                            findings.push(format!(
                                "{rel} {}.{}: {} -> {to} [{}] {certainty}",
                                a.name.name,
                                e.handler,
                                v.from.join("|"),
                                v.transition
                            ));
                        }
                    }
                }
            }
        }
    }
    findings.sort();
    not_evaluated.sort();

    // Guard against a vacuous pass (a moved corpus, a parse regression).
    // 143 agents and 14 state fields when this was written.
    assert!(agents >= 140, "only {agents} agents found");
    assert!(machines >= 14, "only {machines} state fields found");
    let fixture = |f: &str| format!("bynkc/tests/fixtures/positive/{f}");
    assert_eq!(
        findings,
        [
            fixture(
                "2221_transition_may_violate_quiet/src/desk/plants.bynk \
                 Dial.turn: Low -> ? [low_stays_low] Possible"
            ),
            fixture(
                "2221_transition_may_violate_warns/src/desk/tickets.bynk \
                 Gate.ping: Idle -> (unchanged) [idle_must_start] Known"
            ),
            fixture(
                "2221_transition_may_violate_warns/src/desk/tickets.bynk \
                 Ticket.close: Archived -> Closed [archived_is_final] Known"
            ),
            fixture(
                "2221_transition_may_violate_warns/src/desk/tickets.bynk \
                 Ticket.reopen: Archived -> Open [archived_is_final] Known"
            ),
            fixture(
                "245_transition_step_invariant/src/commerce/order.bynk \
                 Order.place: Paid -> Placed [paid_is_terminal] Known"
            ),
        ],
        "the overlay's findings changed; update this list if the change is intended"
    );
    // #2221's quiet fixture adds `Counter.grows_count`, which reads another
    // field; 1374's is a hoisting test.
    assert_eq!(not_evaluated.len(), 2, "{not_evaluated:?}");
    assert!(
        not_evaluated.iter().any(|n| n.contains("1374_")),
        "{not_evaluated:?}"
    );
    assert!(
        not_evaluated
            .iter()
            .any(|n| n.contains("2221_transition_may_violate_quiet")),
        "{not_evaluated:?}"
    );
}
