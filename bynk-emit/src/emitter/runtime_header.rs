//! The runtime import of a generated module, derived from the module's tree
//! (1A.36b-1, #3737 Decision A).
//!
//! A module's header used to be guessed before its body existed, by predicates
//! over the source and flags the producers noted as a side effect, and each
//! guess had to agree exactly with what the body later spelled. The body is
//! built first now, and the header is what it uses: a runtime name is imported
//! iff the tree uses it and does not declare it ([`bynk_ts::unbound_names_in`]),
//! so an unused one is dropped and a used one cannot be missed.
//!
//! The names reach the existing [`split_runtime_import`], which partitions them
//! by the runtime module that defines each.
//!
//! # What the tree cannot see
//!
//! Text the emitter still writes as a string is invisible to the closure, bar
//! the names it declares with `TsStmt::raw_using`. A producer of such text must
//! declare the runtime names it spells. `import_shadow.rs` is the net under
//! that rule: for every module whose header is derived it asserts that each
//! runtime export the printed code spells (outside strings and comments) is
//! imported, so a name only text spells fails there rather than in `tsc`.
//!
//! # The gate
//!
//! [`derives_runtime_imports`] is the one place that says which modules this
//! applies to. The driver's `tests/*.test.ts` modules are opaque test text whose
//! header the driver curates from the same [`RuntimeUse`](super::runtime_use)
//! notes the emitter once read, so they keep their predicted imports until 1B.2.
//!
//! [`split_runtime_import`]: super::runtime_files::split_runtime_import

use bynk_project::UnitKind;
use bynk_ts::TsStmt;

use super::runtime_files::{is_runtime_export, is_type_only_export};

/// Whether a unit of `kind` takes its runtime import from its tree.
///
/// Every kind is listed, so a new one has to be decided. A `Test` or
/// `Integration` unit is built by the driver (`tests_emit`), not through the
/// emitter's header at all; it keeps its predicted imports until 1B.2 and must
/// not be derived here.
pub(crate) fn derives_runtime_imports(kind: UnitKind) -> bool {
    match kind {
        UnitKind::Commons | UnitKind::Context | UnitKind::Adapter => true,
        UnitKind::Test | UnitKind::Integration => false,
    }
}

/// The order runtime names have always had in a generated import list. A
/// derived name takes its place here, so the names a module keeps stay in the
/// order they had; one this list does not know (a `Cmp`/`Decimal`/key helper
/// the lowering picks by type) follows them, alphabetically.
const ORDER: &[&str] = &[
    // The fixed prefix.
    "Ok",
    "Err",
    "Some",
    "None",
    "Result",
    "Option",
    "ValidationError",
    "DecodeError",
    "Connection",
    // Agents.
    "__DurableObjectState",
    "__DurableObjectNamespace",
    "__StateRegistry",
    "__makeAgent",
    "__noteAgentInstance",
    "__inspectAgent",
    "__historyBefore",
    "__historyRecord",
    "__invariantViolation",
    "__AgentWire",
    "__decodeAgentArgs",
    "__encodeAgentResult",
    "__decodeAgentKey",
    "__AGENT_WIRE_PASS",
    "__dispatchToEventsFanout",
    "__rehydrationViolation",
    "__migrationFailed",
    "__reportUnknownStoredKeys",
    "__checkStoredLayout",
    "__readStored",
    "__commitStored",
    "__noteClock",
    // The handles and wire helpers an agent's state is made of, sorted.
    "__StoreCache",
    "__StoreLog",
    "__StoreMap",
    "__StoreSet",
    "__attachLazy",
    "__bynkThen",
    "__fillHeld",
    "__fillLog",
    "__fillMap",
    "__fillSet",
    "__readsLazily",
    "__wireEntries",
    "__wireHeld",
    "__wireHeldOrder",
    "__wireLog",
    "__wireMap",
    "__wirePostings",
    "__wireSet",
    "__checkStoredSchemaVersion",
    "__wakeForMigration",
    // Connections.
    "__resolveConnection",
    "__connIdOf",
    "__acceptHibernatableConnection",
    "__newWebSocketPair",
    "__webSocketUpgradeResponse",
    "__WorkersConnection",
    "HttpResult",
    "QueueResult",
    // The boundary.
    "__JsonValue",
    "__BoundaryError",
    "__ServiceBinding",
    "__callService",
    "__boundaryError",
    // `Bytes`, `Instant`, predicates, `par`, effects, faults, traces.
    "__bynkBytesEqual",
    "__bynkBytesToBase64",
    "__bynkBytesFromBase64",
    "__bynkBytesDecodeUtf8",
    "__bynkBytesConcat",
    "__bynkBytesCompare",
    "__bynkBytesToHex",
    "__bynkBytesFromHex",
    "__bynkBytesToBase64Url",
    "__bynkBytesFromBase64Url",
    "__bynkInstantToIso",
    "__bynkInstantFromIso",
    "__bynkPgRow",
    "__bynkIsIsoDate",
    "__bynkYamlParse",
    "__bynkSettleAll",
    "__bynkWithin",
    "__bynkRace",
    "__bynkDetach",
    "__fault",
    "__traceStep",
    "__traceBinding",
    // Requests, cookies, queries, forms.
    "__bynkPercentEncode",
    "__bynkPercentDecode",
    "__requestHeaders",
    "__requestCookies",
    "__requestCookiesHeader",
    "__issueSessionCookie",
    "__expireSessionCookie",
    "__sessionKeysOf",
    "__cookieValue",
    "__signSessionCookie",
    "__newSessionKey",
    "__requestQuery",
    "__queryString",
    "__formDecode",
    "__formString",
    "__cookieString",
    "__multipartForm",
    // Equality, `Int`, messages.
    "__bynkEq",
    "__bynkIntDiv",
    "__bynkToInt",
    "__bynkIntDivFloor",
    "__bynkIntRem",
    "__bynkIntMod",
    "__bynkIntPow",
    "__bynkIntAnd",
    "__bynkIntOr",
    "__bynkIntXor",
    "__bynkIntAndNot",
    "__bynkIntShl",
    "__bynkIntShr",
    "__selectPluralArm",
    "__formatIcuNumber",
    "__formatIcuDate",
];

/// The runtime names `stmts` (a module's imports, if it has any besides the
/// runtime's, and its body) use and do not bind, spelled as an import names
/// them (`type X` for a type-only export) and in their historical order.
///
/// A name that the runtime exports and the tree uses bare is a use of the
/// runtime's even where it shares a global's name, so no global hides it.
pub(crate) fn derived_runtime_names(stmts: &[TsStmt]) -> Vec<String> {
    let mut used: Vec<String> = bynk_ts::unbound_names_in(stmts, is_runtime_export)
        .into_iter()
        .filter(|n| is_runtime_export(n))
        .collect();
    let rank = |n: &str| ORDER.iter().position(|o| *o == n).unwrap_or(ORDER.len());
    used.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    used.into_iter()
        .map(|n| {
            if is_type_only_export(&n) {
                format!("type {n}")
            } else {
                n
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::runtime_files::is_runtime_export;
    use bynk_ts::{TsDecl, TsExpr, TsParam, TsType};

    fn program_using(body: Vec<TsStmt>) -> Vec<TsStmt> {
        body
    }

    fn call(name: &str) -> TsStmt {
        TsStmt::expr_stmt(
            TsExpr::Call {
                callee: Box::new(TsExpr::Ident(name.to_string())),
                args: vec![],
            },
            None,
        )
    }

    fn function_over(ty: TsType) -> TsStmt {
        TsStmt::decl(
            TsDecl::Function {
                name: "f".to_string(),
                generics: vec![],
                params: vec![TsParam::new("x".to_string(), Some(ty))],
                return_type: None,
                body: vec![],
                is_async: false,
                inline: false,
            },
            None,
        )
    }

    #[test]
    fn the_order_names_runtime_exports_once_each() {
        let mut seen = std::collections::BTreeSet::new();
        for n in ORDER {
            assert!(is_runtime_export(n), "{n} is not a runtime export");
            assert!(seen.insert(*n), "{n} is listed twice");
        }
    }

    #[test]
    fn a_module_that_uses_nothing_imports_nothing() {
        assert!(derived_runtime_names(&program_using(vec![])).is_empty());
        // Names that are not the runtime's are not this header's to import.
        let stmts = program_using(vec![call("helper"), function_over(TsType::named("Order"))]);
        assert!(derived_runtime_names(&stmts).is_empty());
    }

    /// A global (`Response`, `Promise<Request>`) is not the runtime's: the closure is asked
    /// for the runtime's exports (`provided`) and nothing else, so a global stays unimported.
    /// (`Request` and `Response` are `bynk`'s own types, imported from a sibling module.)
    #[test]
    fn a_global_is_not_imported_from_the_runtime() {
        let stmts = program_using(vec![
            call("Response"),
            function_over(TsType::named("Promise<Request>")),
        ]);
        assert!(derived_runtime_names(&stmts).is_empty());
    }

    #[test]
    fn a_used_name_is_imported_and_a_type_gets_its_prefix() {
        let stmts = program_using(vec![
            call("Some"),
            function_over(TsType::named("Option<number>")),
            function_over(TsType::named("HttpResult<string>")),
        ]);
        assert_eq!(
            derived_runtime_names(&stmts),
            ["Some", "type Option", "HttpResult"]
        );
    }

    #[test]
    fn names_keep_their_historical_order_and_unknown_ones_follow() {
        let stmts = program_using(vec![
            call("__bynkCmpInt"),
            call("__bynkEq"),
            call("Err"),
            call("__bynkCmpFloat"),
            call("Ok"),
        ]);
        assert_eq!(
            derived_runtime_names(&stmts),
            ["Ok", "Err", "__bynkEq", "__bynkCmpFloat", "__bynkCmpInt"]
        );
    }

    #[test]
    fn a_name_the_module_binds_is_not_imported() {
        let stmts = program_using(vec![
            TsStmt::decl(
                TsDecl::Import {
                    type_only: false,
                    names: vec!["Some".to_string()],
                    from: "./elsewhere.js".to_string(),
                },
                None,
            ),
            call("Some"),
            call("None"),
        ]);
        assert_eq!(derived_runtime_names(&stmts), ["None"]);
    }

    /// 1A.36b-1: the gate. Only the units the emitter's header serves derive
    /// their imports; the driver's test units keep the imports it predicts.
    #[test]
    fn the_driver_test_units_are_gated_off() {
        assert!(derives_runtime_imports(UnitKind::Commons));
        assert!(derives_runtime_imports(UnitKind::Context));
        assert!(derives_runtime_imports(UnitKind::Adapter));
        assert!(!derives_runtime_imports(UnitKind::Test));
        assert!(!derives_runtime_imports(UnitKind::Integration));
    }
}
