//! The runtime import of a generated module, derived from the module's tree
//! (1A.36b-1, #3737 Decision A).
//!
//! Every module the emitter or the test harness builds derives its header this way: a unit
//! module since 1A.36b-1, the Worker `index.ts` and `compose.ts` since 1A.36b-2, and the
//! generated test modules (`tests/*.test.ts`, in [`TEST_ORDER`]) since 1B.5e.
//!
//! A module's header used to be guessed before its body existed, by predicates
//! over the source and flags the producers noted as a side effect, and each
//! guess had to agree exactly with what the body later spelled. The body is
//! built first now, and the header is what it uses: a runtime name is imported
//! iff the tree uses it and does not declare it ([`bynk_ts::unbound_names_in`]),
//! so an unused one is dropped and a used one cannot be missed.
//!
//! This module also builds the header itself, the banner and the runtime import
//! of every module the emitter builds: [`write_header`] for a project-mode unit,
//! [`header_single`] for single-file mode, and [`runtime_import_for`], the
//! specifier they print.
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
//! [`split_runtime_import`]: super::runtime_files::split_runtime_import

use std::path::Path;

use crate::emit_model::{EmitProjectCtx, ImportExt};
use bynk_check::checker::TypedCommons;
use bynk_project::UnitKind;
use bynk_ts::TsStmt;

use super::runtime_files::{is_runtime_export, is_type_only_export};

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
    "__callServiceOutcome",
    "__callOutcome",
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
    "__bynkIsCron",
    "__bynkCodePointLength",
    "__bynkRegExp",
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

/// The order runtime names have always had in a Worker's `index.ts` and `compose.ts` import
/// lists (1A.36b-2). It differs from [`ORDER`] (`__ServiceBinding` precedes `__JsonValue`
/// here, `Option` precedes `Err`), so a Worker header ranks by this list. It was read off the
/// import lines both files had before their headers were derived, merged into one order that
/// holds every pair those lines ever showed. A name it does not list ranks by [`ORDER`] and
/// then alphabetically, after the listed ones.
const WORKER_ORDER: &[&str] = &[
    "HttpResult",
    "Ok",
    "Option",
    "__AiSearchInstance",
    "__AnalyticsDataset",
    "__D1Database",
    "__Hyperdrive",
    "__KVNamespace",
    "__QueueProducer",
    "__R2Bucket",
    "__RateLimiter",
    "__SecretsStoreSecret",
    "__SendEmail",
    "__SenderIdentity",
    "__ServiceBinding",
    "__JsonValue",
    "__VectorizeIndex",
    "__WorkerAiBinding",
    "__WorkerImagesBinding",
    "__WorkerVersionMetadata",
    "__bynkBytesEqual",
    "__bynkDecimalParseWire",
    "__bynkDecimalToString",
    "__handleEmail",
    "__matchPath",
    "__mysqlExecute",
    "__pgExecute",
    "__serialiseAgentClass",
    "__serialiseAgentKey",
    "__dispatchToEventsFanout",
    "__sessionCookieOf",
    "__verifyBearerJwtHs256",
    "__verifyOidcJwt",
    "__verifySignatureHmacSha256",
    "__verifySignedCookie",
    "Err",
    "Result",
    "__AnalyticsSqlBinding",
    "__BoundaryError",
    "__InboundMessage",
    "__bynkBytesToBase64",
    "__csrfAllowed",
    "__httpResultToResponse",
    "__tokenClaims",
    "__boundaryError",
    "__bynkBytesFromBase64",
    "__oidcTrust",
    "__withPlainJson",
    "__headResponse",
    "__bynkBytesDecodeUtf8",
    "__notModifiedIfMatch",
    "__verifyCall",
    "__applyCache",
    "__CorsPolicy",
    "__applyChallenge",
    "__applyCors",
    "__applyEdgeCache",
    "__corsPreflightResponse",
    "__faultResponse",
    "__readBodyCapped",
    "__deserialiseEventEnvelope",
    "__noStore",
    "__edgeCacheGuard",
    "__varyAcceptLanguage",
    "__SecurityPolicy",
    "__applySecurityHeaders",
    "__requestBodyStream",
    "__untilBodyTooLarge",
    "__mediaTypeMatches",
    "__jsonMediaTypeMatches",
    "__allowUnion",
    "__requestHeaders",
    "__queryObject",
    "__cookieObject",
    "__multipartMediaTypeMatches",
    "__multipartObject",
    "__formMediaTypeMatches",
    "__formObject",
    "__requestCookies",
    "__cfObject",
    "__renderRejection",
    "__requestQuery",
];

/// The order runtime names have in a generated test module's import lines (1B.5e, #4054
/// Decision E). It was read off the import lines the 371 `tests/*.test.ts` goldens had before
/// their headers were derived, merged into one order that holds every pair those lines showed
/// but one: `__JsonValue` came before `__BoundaryError` on an integration module's value line
/// and after it on a unit module's `import type` line, and the integration order is kept (the
/// ruled reordering of the unit modules). A name it does not list follows, alphabetically.
const TEST_ORDER: &[&str] = &[
    "HttpResult",
    "Ok",
    "Err",
    "Some",
    "None",
    "Result",
    "Option",
    "TestConnection",
    "ValidationError",
    "DecodeError",
    "__BynkKeyMap",
    "__BynkKeySet",
    "__DurableObjectNamespace",
    "__DurableObjectState",
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
    "__bynkCmpBytes",
    "__bynkCmpDecimal",
    "__bynkCmpFloat",
    "__bynkCmpInt",
    "__bynkCmpList",
    "__bynkCmpOption",
    "__bynkCmpRecord",
    "__bynkCmpString",
    "__bynkCmpSum",
    "__bynkDecimalAbs",
    "__bynkDecimalAdd",
    "__bynkDecimalDiv",
    "__bynkDecimalEqual",
    "__bynkDecimalFromInt",
    "__bynkDecimalMin",
    "__bynkDecimalMul",
    "__bynkDecimalNeg",
    "__bynkDecimalParse",
    "__bynkDecimalParseWire",
    "__bynkDecimalRound",
    "__bynkDecimalShow",
    "__bynkDecimalSub",
    "__bynkDecimalToFloat",
    "__bynkDecimalToInt",
    "__bynkDecimalToString",
    "__bynkDistinctBy",
    "__bynkEq",
    "__bynkGroupBy",
    "__bynkInstantToIso",
    "__bynkInstantFromIso",
    "__bynkJoinOn",
    "__bynkMapInsert",
    "__bynkSetInsert",
    "__bynkSetOf",
    "__bynkSetRemove",
    "__bynkSettleAll",
    "__callService",
    "__callServiceOutcome",
    "__callOutcome",
    "__JsonValue",
    "__BoundaryError",
    "__ServiceBinding",
    "__makeIntegrationAi",
    "__makeIntegrationAnalytics",
    "__makeIntegrationD1",
    "__makeIntegrationEmail",
    "__makeIntegrationImages",
    "__makeIntegrationKv",
    "__makeIntegrationDoNamespace",
    "__makeIntegrationQueue",
    "__makeIntegrationR2",
    "__makeIntegrationRateLimit",
    "__makeIntegrationSecretsStoreSecret",
    "__makeIntegrationVectorize",
    "__makeIntegrationVersionMetadata",
    "__makeTestState",
    "__responseToHttpResult",
    "__responseToHttpOutcome",
    "__responseToUnauthOutcome",
    "__requestHeaders",
    "__requestCookies",
    "__requestCookiesHeader",
    "__requestQuery",
    "__queryString",
    "__multipartForm",
    "__cookieString",
    "__requestHeaderPairs",
    "__requestQuerySearch",
    "__signSessionCookie",
    "__newSessionKey",
];

/// The runtime names `stmts` (a module's imports, if it has any besides the
/// runtime's, and its body) use and do not bind, spelled as an import names
/// them (`type X` for a type-only export) and in their historical order.
///
/// A name that the runtime exports and the tree uses bare is a use of the
/// runtime's even where it shares a global's name, so no global hides it.
pub(crate) fn derived_runtime_names(stmts: &[TsStmt]) -> Vec<String> {
    names_in_order(stmts, |n| ORDER.iter().position(|o| *o == n))
}

/// [`derived_runtime_names`] for a generated test module (`tests/*.test.ts`), in [`TEST_ORDER`].
pub(crate) fn derived_test_runtime_names(stmts: &[TsStmt]) -> Vec<String> {
    names_in_order(stmts, |n| TEST_ORDER.iter().position(|o| *o == n))
}

/// [`derived_runtime_names`] for a Worker's `index.ts` or `compose.ts`, in [`WORKER_ORDER`].
pub(crate) fn derived_worker_runtime_names(stmts: &[TsStmt]) -> Vec<String> {
    names_in_order(stmts, |n| {
        WORKER_ORDER.iter().position(|o| *o == n).or_else(|| {
            ORDER
                .iter()
                .position(|o| *o == n)
                .map(|i| WORKER_ORDER.len() + i)
        })
    })
}

fn names_in_order(stmts: &[TsStmt], position: impl Fn(&str) -> Option<usize>) -> Vec<String> {
    let mut used: Vec<String> = bynk_ts::unbound_names_in(stmts, is_runtime_export)
        .into_iter()
        .filter(|n| is_runtime_export(n))
        .collect();
    let rank = |n: &str| position(n).unwrap_or(usize::MAX);
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

/// Compute the runtime import specifier for a module at `from_source`. For a
/// file at `commerce/payment.ts` the runtime sits two levels up, so this
/// returns `../runtime.js`; for a top-level file it returns `./runtime.js`.
pub fn runtime_import_for(from_source: &Path, ext: ImportExt) -> String {
    let depth = from_source
        .parent()
        .map(|p| {
            p.components()
                .filter(|c| matches!(c, std::path::Component::Normal(_)))
                .count()
        })
        .unwrap_or(0);
    let ext = ext.as_str();
    if depth == 0 {
        format!("./runtime.{ext}")
    } else {
        let prefix: String = "../".repeat(depth);
        format!("{prefix}runtime.{ext}")
    }
}

/// A module's header: the two-line banner, and the runtime import its tree asks
/// for (1A.36b-1). `body` is every statement the module has besides the header,
/// built first, so the import is what the tree uses and cannot disagree with it
/// (see this module). A unit the driver builds (`Test`, `Integration`)
/// gets the banner only: its imports are the driver's.
pub(super) fn write_header(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    body: &[bynk_ts::TsStmt],
) -> Vec<bynk_ts::TsStmt> {
    // Arc C, slice 30 (#1392): the 2-line banner converts to two real
    // `TsStmt::Comment` statements, the same `events_fanout.rs`-established
    // precedent (#1317) every other Arc C header already uses.
    let kind = match ctx.unit_kind {
        UnitKind::Commons => "commons",
        UnitKind::Context => "context",
        UnitKind::Test => "test",
        UnitKind::Integration => "integration test",
        UnitKind::Adapter => "adapter",
    };
    // #1486: no explicit blank stmt after the banner — the printer's
    // automatic top-level spacing policy already supplies the single blank
    // after it (`Comment`→`Comment` is exempted, matching the banner's own
    // two adjacent lines; the automatic policy is not exempted for whatever
    // follows the banner, so the wanted blank appears there for free).
    let mut stmts = vec![
        bynk_ts::TsStmt::comment("Generated by bynk — do not edit by hand.", None),
        bynk_ts::TsStmt::comment(format!("{kind} {}", commons.commons.name.joined()), None),
    ];
    if !commons.commons.items.is_empty() {
        let names = derived_runtime_names(body);
        stmts.extend(super::runtime_files::runtime_import_stmts(
            false,
            &names,
            &runtime_import_for(&ctx.source_path, ctx.import_ext),
        ));
    }
    stmts
}

/// Variant of write_header for single-file (no project context) emission: the
/// same banner and the same derivation, over the module's built `body`. The banner and the
/// imports are statements of the one program the body joins (1A.39a), so the printer's top-level
/// spacing supplies the blank lines after each.
pub(super) fn header_single(
    commons: &TypedCommons,
    body: &[bynk_ts::TsStmt],
) -> Vec<bynk_ts::TsStmt> {
    // Arc C, slice 30 (#1392): the same 2-comment-statement conversion
    // `write_header` above already established.
    let mut stmts = vec![
        bynk_ts::TsStmt::comment("Generated by bynk — do not edit by hand.", None),
        bynk_ts::TsStmt::comment(
            format!(
                "commons {}",
                bynk_project::display_unit_name(&commons.commons.name.joined())
            ),
            None,
        ),
    ];
    if !commons.commons.items.is_empty() {
        let names = derived_runtime_names(body);
        let imports = super::runtime_files::runtime_import_stmts(false, &names, "./runtime.js");
        // A module with no runtime imports keeps the blank line its (empty) import block has
        // always left, beside the one after the banner. The printer supplies the second.
        if imports.is_empty() {
            let mut blank = bynk_ts::TsStmt::blank(None);
            blank.no_blank_before = true;
            stmts.push(blank);
        }
        stmts.extend(imports);
    }
    stmts
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
    fn the_worker_order_names_runtime_exports_once_each() {
        let mut seen = std::collections::BTreeSet::new();
        for n in WORKER_ORDER {
            assert!(is_runtime_export(n), "{n} is not a runtime export");
            assert!(seen.insert(*n), "{n} is listed twice");
        }
    }

    #[test]
    fn a_worker_header_ranks_by_the_worker_order() {
        let stmts = program_using(vec![
            call("__JsonValue"),
            call("__ServiceBinding"),
            call("Ok"),
        ]);
        assert_eq!(
            derived_worker_runtime_names(&stmts),
            ["Ok", "type __ServiceBinding", "type __JsonValue"]
        );
        assert_eq!(
            derived_runtime_names(&stmts),
            ["Ok", "type __JsonValue", "type __ServiceBinding"]
        );
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

    #[test]
    fn runtime_import_depth_resolves_correctly() {
        assert_eq!(
            runtime_import_for(Path::new("compose.ts"), ImportExt::Js),
            "./runtime.js"
        );
        assert_eq!(
            runtime_import_for(Path::new("commerce/payment.ts"), ImportExt::Js),
            "../runtime.js"
        );
        assert_eq!(
            runtime_import_for(Path::new("commerce/orders/types.ts"), ImportExt::Js),
            "../../runtime.js"
        );
        assert_eq!(
            runtime_import_for(Path::new("tests/commerce_payment.test.ts"), ImportExt::Js),
            "../runtime.js"
        );
    }
}

/// Which conditional runtime helpers a module's import line ends up carrying.
///
/// These drive the single-file `emit()` path end-to-end (parse → resolve → check
/// → emit), so they exercise the real producers rather than the accumulator in
/// isolation. Before `RuntimeUse`, the decision was `body.contains("__bynkBytes")`
/// — a scan of the generated text — and `escapes_a_marker_in_a_string_literal`
/// below is the case that got wrong.
#[cfg(test)]
mod conditional_runtime_import_tests {
    use crate::testkit::emit_source;

    /// The runtime import lines of the emitted module, joined: one `import …
    /// from "./__bynk_runtime/<module>.js"` per module it uses.
    fn runtime_import_line(ts: &str) -> String {
        ts.lines()
            .filter(|l| l.starts_with("import ") && l.contains("__bynk_runtime/"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn bytes_helpers_are_imported_when_a_bytes_value_is_built() {
        let ts = emit_source(
            "commons b\n\nfn decode(s: String) -> Option[Bytes] {\n  Bytes.fromBase64(s)\n}\n",
        );
        assert!(
            runtime_import_line(&ts).contains("__bynkBytesFromBase64"),
            "{ts}"
        );
    }

    #[test]
    fn bytes_helpers_are_imported_for_content_equality() {
        let ts = emit_source("commons b\n\nfn same(a: Bytes, b: Bytes) -> Bool {\n  a == b\n}\n");
        assert!(
            runtime_import_line(&ts).contains("__bynkBytesEqual"),
            "{ts}"
        );
    }

    /// #2016: the operation helpers are their own group. A module that only
    /// builds or compares `Bytes` keeps its import list, and one that calls an
    /// operation gets that group without the base one.
    #[test]
    fn bytes_ops_helpers_are_imported_only_when_an_operation_is_called() {
        let base = emit_source("commons b\n\nfn same(a: Bytes, b: Bytes) -> Bool {\n  a == b\n}\n");
        assert!(
            !runtime_import_line(&base).contains("__bynkBytesConcat"),
            "{base}"
        );
        let ops = emit_source("commons b\n\nfn hex(b: Bytes) -> String {\n  b.toHex()\n}\n");
        let line = runtime_import_line(&ops);
        assert!(line.contains("__bynkBytesToHex"), "{ops}");
        assert!(!line.contains("__bynkBytesEqual"), "{ops}");
    }

    /// K3 (#2592): the `Instant` ISO-8601 helpers come in only when called,
    /// and each lowers to its own helper.
    #[test]
    fn instant_iso_helpers_are_imported_only_when_called() {
        let base = emit_source("commons t\n\nfn ms(i: Instant) -> Int {\n  i.toEpochMillis()\n}\n");
        assert!(
            !runtime_import_line(&base).contains("__bynkInstant"),
            "{base}"
        );
        let ts = emit_source(
            "commons t\n\nfn iso(i: Instant) -> String {\n  i.toIso()\n}\n\nfn read(s: String) -> Option[Instant] {\n  Instant.fromIso(s)\n}\n",
        );
        let line = runtime_import_line(&ts);
        assert!(line.contains("__bynkInstantToIso"), "{ts}");
        assert!(line.contains("__bynkInstantFromIso"), "{ts}");
        assert!(ts.contains("__bynkInstantToIso(i)"), "{ts}");
        assert!(ts.contains("__bynkInstantFromIso(s)"), "{ts}");
    }

    /// K2 (#2594): the percent-encoding helpers come in only when called.
    #[test]
    fn percent_helpers_are_imported_only_when_called() {
        let base = emit_source("commons u\n\nfn n(s: String) -> Int {\n  s.length()\n}\n");
        assert!(
            !runtime_import_line(&base).contains("__bynkPercent"),
            "{base}"
        );
        let ts = emit_source(
            "commons u\n\nfn e(s: String) -> String {\n  s.percentEncode()\n}\n\nfn d(s: String) -> Option[String] {\n  s.percentDecode()\n}\n",
        );
        let line = runtime_import_line(&ts);
        assert!(
            line.contains("__bynkPercentEncode") && line.contains("__bynkPercentDecode"),
            "{ts}"
        );
        assert!(
            ts.contains("__bynkPercentEncode(s)") && ts.contains("__bynkPercentDecode(s)"),
            "{ts}"
        );
    }

    /// #2016: `slice` lowers inline, so it pulls in no helper at all.
    #[test]
    fn bytes_slice_needs_no_helper() {
        let ts = emit_source("commons b\n\nfn tag(b: Bytes) -> Bytes {\n  b.slice(0, 4)\n}\n");
        assert!(!runtime_import_line(&ts).contains("__bynkBytes"), "{ts}");
    }

    #[test]
    fn bytes_helpers_are_absent_from_a_module_that_never_uses_bytes() {
        let ts = emit_source("commons b\n\nfn double(n: Int) -> Int {\n  n * 2\n}\n");
        assert!(!runtime_import_line(&ts).contains("__bynkBytes"), "{ts}");
    }

    /// The regression this replaced a text scan for: a `Bytes` helper name
    /// appearing inside a user **string literal** is not a reference to the
    /// helper, and must not pull the import in. `body.contains("__bynkBytes")`
    /// could not tell the two apart, because the literal is emitted verbatim
    /// into the same buffer it scanned.
    #[test]
    fn escapes_a_marker_in_a_string_literal() {
        let ts = emit_source("commons b\n\nfn label() -> String {\n  \"__bynkBytesEqual\"\n}\n");
        assert!(
            ts.contains("\"__bynkBytesEqual\""),
            "the literal should survive into the body: {ts}"
        );
        assert!(
            !runtime_import_line(&ts).contains("__bynkBytes"),
            "a marker inside a string literal is not a helper reference: {ts}"
        );
    }
}
