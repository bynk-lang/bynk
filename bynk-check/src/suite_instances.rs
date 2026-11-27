//! #3959: a commons suite's suite-local capability instances (capability-parameters
//! track §9). A function with `From: Kv, To: Kv` needs two distinct doubles to be
//! tested honestly, so a commons suite may declare `capability Archive = Kv`: a
//! fresh in-memory double of a kind its commons selects, with no binding and no
//! deploy effect. This lifts `bynk.capability.derived_outside_context` for a
//! commons suite only; a context's suite, and a commons itself, still cannot
//! declare one.
//!
//! The instances join a copy of the target's table that only the test phase and
//! the test emitter see, as the target's own derived capabilities would: its
//! cases then hold them, and its `deps` build each one's double.

use crate::stability::StabilityPolicy;
use crate::symbols::{DerivedInstance, UnitTable};
use bynk_project::UnitKind;
use bynk_project::discovery::ParsedFile;
use bynk_syntax::CompileError;
use bynk_syntax::ast::CapabilityDecl;
use std::collections::{BTreeMap, HashMap};

/// The unit tables the test phase checks and emits against: `None` when no
/// suite declares an instance (the project's own tables serve), else a copy in
/// which each commons target's table holds its suites' instances. The errors
/// are the refusals of the instances that could not be added.
pub fn suite_instance_tables(
    test_groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    policy: &StabilityPolicy,
) -> (Option<HashMap<String, UnitTable>>, Vec<CompileError>) {
    let mut errors = Vec::new();
    let mut tables: Option<HashMap<String, UnitTable>> = None;
    for (target, indices) in test_groups {
        for &i in indices {
            let Some(suite) = parsed[i].test() else {
                continue;
            };
            for c in &suite.capabilities {
                if let Some(err) = policy.check(crate::cap_params::GATE, c.span) {
                    errors.push(err);
                }
                if kinds.get(target) != Some(&UnitKind::Commons) {
                    errors.push(outside_commons_suite(c));
                    continue;
                }
                let tables = tables.get_or_insert_with(|| unit_tables.clone());
                let Some(table) = tables.get_mut(target) else {
                    continue;
                };
                match instance(c, table, unit_tables) {
                    Ok((decl, inst)) => {
                        table.capabilities.insert(c.name.name.clone(), decl);
                        table.derived.insert(c.name.name.clone(), inst);
                    }
                    Err(err) => errors.push(err),
                }
            }
        }
    }
    (tables, errors)
}

/// The capability declaration and derived instance one suite-local instance
/// adds to its commons' table, or why it cannot be added.
fn instance(
    c: &CapabilityDecl,
    table: &UnitTable,
    unit_tables: &HashMap<String, UnitTable>,
) -> Result<(CapabilityDecl, DerivedInstance), CompileError> {
    let name = &c.name.name;
    let Some(d) = &c.derived else {
        return Err(CompileError::new(
            "bynk.capability.derived_outside_context",
            c.span,
            format!("a suite declares only a capability instance, and `{name}` declares operations"),
        )
        .with_note("write a suite-local instance of a kind the commons selects, as in `capability Archive = Kv`"));
    };
    if table.flattened_caps.contains_key(name) || table.capabilities.contains_key(name) {
        return Err(CompileError::new(
            "bynk.resolve.duplicate_capability",
            c.name.span,
            format!("`{name}` is already a capability of this suite's commons"),
        )
        .with_note("name the suite-local instance differently: it is a second double, beside the kind's own"));
    }
    let Some(owner) = table.flattened_caps.get(&d.base.name) else {
        return Err(CompileError::new(
            "bynk.capability.unknown_base",
            d.base.span,
            format!("`{}` is not a capability this suite's commons selects", d.base.name),
        )
        .with_note(format!(
            "a suite-local instance is a double of a kind its commons selects: add `{}` to the commons' braced `consumes`",
            d.base.name
        )));
    };
    if crate::firstparty::instantiable(owner, &d.base.name).is_none() {
        return Err(CompileError::new(
            "bynk.capability.base_not_instantiable",
            d.base.span,
            format!(
                "`{owner}.{}` has no named instances, so a suite cannot declare a second double of it",
                d.base.name
            ),
        )
        .with_note(format!("a case holds the one `{}` double", d.base.name)));
    }
    let ops = unit_tables
        .get(owner)
        .and_then(|t| t.capabilities.get(&d.base.name))
        .map(|base| base.ops.clone())
        .unwrap_or_default();
    let decl = CapabilityDecl { ops, ..c.clone() };
    let inst = DerivedInstance {
        base_unit: owner.clone(),
        base_cap: d.base.name.clone(),
        binding: String::new(),
        options: Vec::new(),
        span: c.span,
    };
    Ok((decl, inst))
}

/// `bynk.capability.derived_outside_context` for an instance in a suite whose
/// target is not a commons.
fn outside_commons_suite(c: &CapabilityDecl) -> CompileError {
    CompileError::new(
        "bynk.capability.derived_outside_context",
        c.span,
        format!(
            "suite-local capability `{}` is declared in a suite whose target is not a commons",
            c.name.name
        ),
    )
    .with_note("only a commons suite declares a suite-local instance, as a second double for a function with two capability parameters of one kind; a context's suite stubs and observes the context's own instances")
}
