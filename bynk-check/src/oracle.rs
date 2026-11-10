//! The differential oracle's comparison (#3243, #1523 S2).
//!
//! The language server is to reuse per-file analysis work between edits (a memo
//! table, [`crate::memo`]). The one property that must never break is that a
//! reused round answers **exactly** what a fresh round answers: a stale
//! diagnostic is the failure mode of every incremental scheme. This module is
//! what the CI oracle compares: [`canonical_report`] renders everything a
//! [`ProjectAnalysis`] says into one deterministic string, so two analyses are
//! equal exactly when their reports are.
//!
//! Deterministic means independent of everything that is not meaning:
//!
//! - hash-map iteration order: maps are walked in sorted key order;
//! - `TyId`s, which are indices into one analysis's own interner and differ
//!   between two analyses of the same project: types are rendered structurally
//!   through each analysis's own table (`Types::display`), never by id.
//!
//! What it deliberately does not compare is memory identity (`Arc` pointers) or
//! timing. Fields whose `Debug` output contains a hash map (the sequence and
//! boundary tables) are compared through their sorted keys and the sorted debug
//! of each entry, one level deep; the checker's own results (diagnostics, index,
//! hints, locals, requirements, expression types, capability paths, contract
//! diff) are compared in full.

use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Write};

use crate::analysis::ProjectAnalysis;
use crate::resolver::CrossContextInfo;

/// One deterministic rendering of everything `analysis` says. See the module doc.
pub fn canonical_report(analysis: &ProjectAnalysis) -> String {
    let mut out = String::new();
    let a = analysis;

    section(&mut out, "snapshots", {
        let mut v: Vec<String> = a.snapshots.iter().map(|s| format!("{s:?}")).collect();
        v.sort();
        v
    });
    // The error list's order is part of the contract: it is what the user reads.
    section(
        &mut out,
        "errors",
        a.errors
            .iter()
            .map(|e| format!("{:?} | {:?} | {:?}", e.source_path, e.error, e.label_files))
            .collect(),
    );

    section(&mut out, "index.symbols", sorted_map(&a.index.symbols));
    section(
        &mut out,
        "index.foreign_refs",
        debug_all(&a.index.foreign_refs),
    );
    section(&mut out, "index.calls", debug_all(&a.index.calls));
    section(&mut out, "index.impls", debug_all(&a.index.impls));
    section(
        &mut out,
        "index.refinements",
        debug_all(&a.index.refinements),
    );

    section(&mut out, "hints", sorted_map(&a.hints));
    section(&mut out, "locals", sorted_map(&a.locals));
    section(&mut out, "requirements", sorted_map(&a.requirements));
    section(
        &mut out,
        "capability_paths",
        sorted_map(&a.capability_paths),
    );

    // Expression types are `(Span, TyId)`; the id means nothing outside this
    // analysis's own table, so each is rendered through it.
    let tys = &a.ty_intern;
    let expr: BTreeMap<String, Vec<String>> = a
        .expr_types
        .iter()
        .map(|(path, entries)| {
            (
                format!("{path:?}"),
                entries
                    .iter()
                    .map(|(span, id)| format!("{span:?} : {}", tys.display(*id)))
                    .collect(),
            )
        })
        .collect();
    section(
        &mut out,
        "expr_types",
        expr.into_iter()
            .map(|(path, entries)| format!("{path} => {entries:?}"))
            .collect(),
    );

    section(&mut out, "unit_sources", sorted_map(&a.unit_sources));
    section(&mut out, "doc_scope", sorted_map(&a.doc_scope));
    section(
        &mut out,
        "contract_diff",
        vec![format!("{:?}", a.contract_diff)],
    );
    section(
        &mut out,
        "sequence_info",
        sorted_lines(a.sequence_info.iter().map(|(name, info)| {
            format!(
                "{name:?} => cross_context[{}] agents[{}]",
                cross_context(&info.cross_context),
                sorted_map(&info.agents).join("; ")
            )
        })),
    );
    section(
        &mut out,
        "boundary_info",
        sorted_lines(a.boundary_info.iter().map(|(name, info)| {
            format!(
                "{name:?} => types[{}] services[{}] agents[{}]",
                sorted_map(&info.types).join("; "),
                sorted_map(&info.services).join("; "),
                sorted_map(&info.agents).join("; ")
            )
        })),
    );
    out
}

/// A [`CrossContextInfo`] with every map and set in sorted order. Its fields are
/// maps of maps, so `Debug` on the whole would print hash order.
fn cross_context(c: &CrossContextInfo) -> String {
    fn two_level<V: Debug>(map: &HashMap<String, HashMap<String, V>>) -> String {
        sorted_lines(
            map.iter()
                .map(|(k, inner)| format!("{k:?} => {{{}}}", sorted_map(inner).join("; "))),
        )
        .join("; ")
    }
    let events = sorted_lines(c.consumed_event_names.iter().map(|(k, set)| {
        let mut names: Vec<&String> = set.iter().collect();
        names.sort();
        format!("{k:?} => {names:?}")
    }))
    .join("; ");
    [
        format!("self_context={:?}", c.self_context),
        format!("consumed_contexts={:?}", c.consumed_contexts),
        format!("aliases={{{}}}", sorted_map(&c.aliases).join("; ")),
        format!("consumed_services={{{}}}", two_level(&c.consumed_services)),
        format!("consumed_types={{{}}}", two_level(&c.consumed_types)),
        format!(
            "consumed_type_renames={{{}}}",
            two_level(&c.consumed_type_renames)
        ),
        format!(
            "consumed_capabilities={{{}}}",
            two_level(&c.consumed_capabilities)
        ),
        format!(
            "flattened_caps={{{}}}",
            sorted_map(&c.flattened_caps).join("; ")
        ),
        format!("consumed_event_names={{{events}}}"),
        format!(
            "consumed_type_owners={{{}}}",
            two_level(&c.consumed_type_owners)
        ),
        format!(
            "consumed_type_aliases={{{}}}",
            two_level(&c.consumed_type_aliases)
        ),
        format!(
            "callee_type_aliases={{{}}}",
            two_level(&c.callee_type_aliases)
        ),
        format!("callee_alias_keys={{{}}}", two_level(&c.callee_alias_keys)),
    ]
    .join(" ")
}

fn sorted_lines(lines: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = lines.into_iter().collect();
    v.sort();
    v
}

fn section(out: &mut String, name: &str, lines: Vec<String>) {
    let _ = writeln!(out, "== {name} ({})", lines.len());
    for line in lines {
        let _ = writeln!(out, "{line}");
    }
}

fn debug_all<T: Debug>(items: &[T]) -> Vec<String> {
    items.iter().map(|i| format!("{i:?}")).collect()
}

/// A map as sorted `key => value` lines, so its hash order cannot show.
fn sorted_map<'a, K, V>(map: impl IntoIterator<Item = (&'a K, &'a V)>) -> Vec<String>
where
    K: Debug + 'a,
    V: Debug + 'a,
{
    let mut v: Vec<String> = map
        .into_iter()
        .map(|(k, val)| format!("{k:?} => {val:?}"))
        .collect();
    v.sort();
    v
}
