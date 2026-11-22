//! The test harness's generated names, one spelling each (back-half paydown 1B.2c, #4028,
//! Decision G; the 1A.39b precedent). Each helper is the only place its shape is written, so
//! the definer and every reference agree. The names the emitter's case lowering also spells
//! are in `emitter::harness_names`.

use bynk_syntax::ast::{TypeDecl, TypeRef};
use std::collections::HashMap;
use std::sync::Arc;

/// A participant's `env_<ns>` binding in the integration harness: the one spelling its
/// declaration and every reference use (1A.39b, Decision C).
pub(super) fn env_binding(ns: &str) -> String {
    format!("env_{ns}")
}

/// #1959: the harness's import alias for participant `ns`'s events fan-out
/// module, shared by the import and the binding that constructs from it.
pub(super) fn fanout_alias(ns: &str) -> String {
    format!("fanout_{ns}")
}

/// The qualifier a participant's types carry in a test module: `<ns>.`.
pub(super) fn ns_qualifier(ns: &str) -> String {
    format!("{ns}.")
}

/// The binding a participant's Worker is imported and forwarded as: `worker_<ns>`.
pub(super) fn worker_binding_name(ns: &str) -> String {
    format!("worker_{ns}")
}

/// The specifier of a file of a participant's Worker directory, from `tests/`:
/// `../workers/<dir>/<file>.js`.
pub(super) fn worker_specifier(dir: &str, file: &str) -> String {
    format!("../workers/{dir}/{file}.js")
}

/// A module beside the importing test module: `./<path>.<ext>`.
pub(super) fn sibling_specifier(path: &str, ext: &str) -> String {
    format!("./{path}.{ext}")
}

/// A module in the parent of `tests/`: `../<module>.<ext>`.
pub(super) fn parent_specifier(module: &str, ext: &str) -> String {
    format!("../{module}.{ext}")
}

/// The discovered case name of a contract's property test: `contract <fn>`.
pub(super) fn contract_case_name(fname: &str) -> String {
    format!("contract {fname}")
}

/// The import alias of the `i`th test module in the entry: `test_<i>`.
pub(super) fn case_alias(i: usize) -> String {
    format!("test_{i}")
}

/// The field holding a stub clause's call counter: `__seq_<clause>`.
pub(super) fn seq_field(clause_idx: usize) -> String {
    format!("__seq_{clause_idx}")
}

/// The factory of a capability's test double: `__bynkTest_<cap>`.
pub(super) fn test_cap_factory(cap: &str) -> String {
    format!("__bynkTest_{cap}")
}

/// A driver's `body: <param>, ` request-init field.
pub(super) fn body_field(param: &str) -> String {
    format!("body: {param}, ")
}

/// A property test's runner function: `__prop_<case>`.
pub(super) fn prop_runner_name(case: &str) -> String {
    format!("__prop_{case}")
}

/// The extra argument carrying a record type's renamed-field specs: `, <specs>`, empty when no
/// field is renamed.
pub(super) fn renamed_specs_arg(
    type_ref: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> String {
    crate::emitter::renamed_specs_source(type_ref, types)
        .map(|t| format!(", {t}"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each name against the literal it replaced.
    #[test]
    fn each_name_has_its_historic_spelling() {
        assert_eq!(env_binding("shop"), "env_shop");
        assert_eq!(fanout_alias("shop"), "fanout_shop");
        assert_eq!(ns_qualifier("shop"), "shop.");
        assert_eq!(worker_binding_name("shop"), "worker_shop");
        assert_eq!(
            worker_specifier("shop", "events_fanout"),
            "../workers/shop/events_fanout.js"
        );
        assert_eq!(sibling_specifier("a/b", "ts"), "./a/b.ts");
        assert_eq!(parent_specifier("m", "js"), "../m.js");
        assert_eq!(contract_case_name("f"), "contract f");
        assert_eq!(case_alias(3), "test_3");
        assert_eq!(seq_field(2), "__seq_2");
        assert_eq!(test_cap_factory("Clock"), "__bynkTest_Clock");
        assert_eq!(body_field("b"), "body: b, ");
        assert_eq!(prop_runner_name("p_1"), "__prop_p_1");
    }
}
