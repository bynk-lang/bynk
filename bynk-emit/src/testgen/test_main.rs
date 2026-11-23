//! The suite runner (`tests_main`): TypeScript builders, the run function and the filter runtime.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
// #1759: every namespace a test module imports, from one definition.
use crate::ts_build::{
    and_expr, await_expr, binary, block, call, cond_expr, const_, expr_stmt, ident, if_, member,
    method_call, num_lit, or_expr, str_lit, strict_eq,
};
use bynk_check::d1_migrations::MigrationFile;
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam,
    TsProgram, TsStmt, TsType, TsTypeMember,
};

/// P4b (#2608): `__makeIntegrationD1`'s argument for a database's
/// migrations, `[[name, sql], …]` in order, or none for a database without.
pub(super) fn d1_migrations_args(files: Option<&Vec<MigrationFile>>) -> Vec<TsExpr> {
    let Some(files) = files.filter(|f| !f.is_empty()) else {
        return Vec::new();
    };
    vec![TsExpr::Array {
        items: files
            .iter()
            .map(|f| TsExpr::Array {
                items: vec![str_lit(f.name.clone()), str_lit(f.sql.clone())],
                multiline: false,
            })
            .collect(),
        multiline: true,
    }]
}

/// [`d1_migrations_args`] as text, for a double whose body is written as
/// text: empty for a database without migrations.
pub(super) fn d1_migrations_text(files: &[MigrationFile]) -> String {
    let pairs: Vec<String> = files
        .iter()
        .map(|f| {
            format!(
                "[{}, {}]",
                bynk_project::json_string(&f.name),
                bynk_project::json_string(&f.sql)
            )
        })
        .collect();
    if pairs.is_empty() {
        String::new()
    } else {
        format!("[{}]", pairs.join(", "))
    }
}

/// One `async function __sysdrive_*` driver: an optional lead statement
/// (`const __body = JSON.stringify(...)`, typed/no-auth drivers only), `const
/// __h = __makeHarness();`, `const __req = new Request(<url>, <options>);`,
/// `const __res = await __h.env.<binding>.fetch(__req);`, then `return
/// <decode_fn>(__res, <payload>);`. Slice F (#1407): the one real shape
/// shared by all four per-route `__sysdrive_{,raw_,noauth_,rawnoauth_}*`
/// drivers in `emit_system_http_support`, differing only in name (built here
/// from `kind`/`sname`/`key`, one shared spot rather than one
/// `format!` call per driver kind), params, the optional body statement, the
/// options object (Decision A, see its own call sites), and the decode
/// function — every real difference is a parameter here, not a reason to
/// hand-write four near-identical bodies.
#[allow(clippy::too_many_arguments)]
pub(super) fn sysdrive_driver(
    kind: SysdriveKind<'_>,
    sname: &str,
    key: &str,
    mut params: Vec<TsParam>,
    body_stmt: Option<TsStmt>,
    url: TsExpr,
    options: TsExpr,
    binding: &str,
    decode_fn: &str,
    payload: &str,
    signs_claims: bool,
    // #2135: a `Signature` route's signing statements (`const __sig = …`,
    // or #2155 a sum's per-call `__auth`), after the body they sign is
    // serialised and before the request is built.
    sign_stmts: Vec<TsStmt>,
    // #2155: whether the driver takes the actor the call acts as (`__as`), on
    // an actor-sum route.
    takes_as: bool,
) -> TsStmt {
    params.push(TsParam::new(
        "__sub".to_string(),
        Some(TsType::named("string")),
    ));
    // #2019: a refinement-guarded route signs the claims the acting actor
    // presents (`{}` for a non-refinement actor) next to `sub`.
    if signs_claims {
        params.push(TsParam::new(
            "__claims".to_string(),
            Some(TsType::named_with_args(
                "globalThis.Record",
                vec![TsType::named("string"), TsType::named("unknown")],
            )),
        ));
    }
    if takes_as {
        params.push(TsParam::new(
            "__as".to_string(),
            Some(TsType::named("string")),
        ));
    }
    let mut body = Vec::new();
    if let Some(stmt) = body_stmt {
        body.push(stmt);
    }
    body.extend(sign_stmts);
    body.push(TsStmt::const_stmt(
        TsBindingName::Ident("__h".to_string()),
        None,
        call(ident("__makeHarness"), vec![]),
        None,
    ));
    body.push(TsStmt::const_stmt(
        TsBindingName::Ident("__req".to_string()),
        None,
        TsExpr::New {
            callee: Box::new(ident("globalThis.Request")),
            args: vec![url, options],
        },
        None,
    ));
    body.push(TsStmt::const_stmt(
        TsBindingName::Ident("__res".to_string()),
        None,
        await_expr(method_call(
            member(member(ident("__h"), "env"), binding),
            "fetch",
            vec![ident("__req")],
        )),
        None,
    ));
    body.push(TsStmt::return_stmt(
        Some(call(ident(decode_fn), vec![ident("__res"), ident(payload)])),
        None,
    ));
    TsStmt::decl(
        TsDecl::Function {
            name: sysdrive_name(kind, sname, key),
            generics: Vec::new(),
            params,
            return_type: None,
            body,
            is_async: true,
            inline: false,
        },
        None,
    )
}

/// The module's own runner, built from its real per-case/per-property/per-attack
/// `(name, runner)` pairs, in dispatch order:
///
/// ```ts
/// export const __bynkCases: readonly string[] = ["a", "b"];
/// export async function run(only?: string, onCase?: (a0: CaseResult) => void) {
///   const results: CaseResult[] = [];
///   const want = (n: string): boolean => only === undefined || only === n;
///   if (want("a")) {
///     const started = __bynkCaseNow();
///     const r = { name: "a", ...(await test_a()), durationMs: __bynkCaseSince(started) };
///     results.push(r);
///     if (onCase) onCase(r);
///   }
///   return results;
/// }
/// ```
///
/// `onCase` is called as each case finishes, so a consumer sees a result before the
/// next case runs (#2267 stage 2), and `__bynkCases` lists the case names so the
/// entry can count what a filter selects before anything runs. The case result carries
/// `durationMs`, measured around the runner. Slice G (#1409): carries no opaque lowered
/// content and no source-map splice sensitivity (unlike the per-case bodies each
/// dispatch calls out to), so it converts fully, shared by both
/// `emit_integration_module` and `emit_test_module`.
///
/// `name` is the raw, decoded case name: `str_lit` routes it through `TsLit::Str`'s own
/// printer escaping, which alone owns escaping (review of #1410, finding 1: an
/// already-escaped name would be escaped twice). The `...(await …())` spread wraps its
/// `Await` in an explicit `TsExpr::Paren`, since `TsObjectEntry::Spread` renders its
/// inner expression without operand parenthesisation.
pub(super) fn build_run_function(entries: &[(String, String)]) -> Vec<TsStmt> {
    let dispatch = |name: &str, runner_name: &str| {
        TsStmt::if_stmt(
            call(ident("want"), vec![str_lit(name.to_string())]),
            block(vec![
                const_("started", call(ident("__bynkCaseNow"), vec![])),
                const_(
                    "r",
                    TsExpr::object_entries(vec![
                        TsObjectEntry::prop("name".to_string(), str_lit(name.to_string())),
                        TsObjectEntry::Spread(TsExpr::Paren(Box::new(await_expr(call(
                            ident(runner_name),
                            vec![],
                        ))))),
                        TsObjectEntry::prop(
                            "durationMs".to_string(),
                            call(ident("__bynkCaseSince"), vec![ident("started")]),
                        ),
                    ]),
                ),
                expr_stmt(method_call(ident("results"), "push", vec![ident("r")])),
                if_(
                    ident("onCase"),
                    expr_stmt(call(ident("onCase"), vec![ident("r")])),
                ),
            ]),
            None,
        )
    };
    let mut body = vec![
        // #3639: typed, so `tsc` need not infer an evolving array's type by
        // control-flow analysis over the whole function, which passes its
        // flow-node limit (TS2563) at 500 cases.
        TsStmt::const_stmt(
            TsBindingName::Ident("results".to_string()),
            Some(TsType::array(TsType::named("CaseResult"))),
            TsExpr::array(vec![]),
            None,
        ),
        const_(
            "want",
            TsExpr::Arrow {
                params: vec![TsParam::new("n".to_string(), Some(TsType::named("string")))],
                is_async: false,
                generics: Vec::new(),
                return_type: Some(TsType::named("boolean")),
                body: Box::new(TsArrowBody::Expr(Box::new(or_expr(
                    strict_eq(ident("only"), ident("undefined")),
                    strict_eq(ident("only"), ident("n")),
                )))),
            },
        ),
    ];
    body.extend(entries.iter().map(|(name, runner)| dispatch(name, runner)));
    body.push(TsStmt::return_stmt(Some(ident("results")), None));
    vec![
        TestRuntimeModule::CaseTiming.splice(),
        TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::ConstDecl {
                name: "__bynkCases".to_string(),
                ty: Some(TsType::named("readonly string[]")),
                init: TsExpr::array(
                    entries
                        .iter()
                        .map(|(name, _)| str_lit(name.clone()))
                        .collect(),
                ),
            })),
            None,
        ),
        TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::Function {
                name: "run".to_string(),
                generics: Vec::new(),
                params: vec![
                    TsParam::optional("only".to_string(), Some(TsType::named("string"))),
                    TsParam::optional(
                        "onCase".to_string(),
                        Some(TsType::Fn {
                            rest: false,
                            names: vec![],
                            params: vec![TsType::named("CaseResult")],
                            ret: Box::new(TsType::named("void")),
                        }),
                    ),
                ],
                return_type: None,
                body,
                is_async: true,
                inline: false,
            })),
            None,
        ),
    ]
}

pub(super) fn console_log(arg: TsExpr) -> TsStmt {
    expr_stmt(method_call(ident("globalThis.console"), "log", vec![arg]))
}

/// #3437: a build whose modules carry the execution-context double drains the
/// failures a detached effect reported (`globalThis.__bynkDetachedReports`)
/// after each case, and reports them beside the case: as `detached_failures`
/// and `detached` on its event, and as a line under it in the human output.
/// A report does not fail the case, as on Workers (ADR 0478). Gated, so a build
/// with no execution context keeps its runner unchanged.
struct DetachReports {
    enabled: bool,
}

impl DetachReports {
    fn drain(&self) -> Option<TsStmt> {
        self.enabled.then(|| {
            // `(globalThis as unknown as { __bynkDetachedReports?: … }).__bynkDetachedReports?.splice(0) ?? []`
            let reports = crate::ts_build::as_expr(
                crate::ts_build::as_expr(ident("globalThis"), TsType::named("unknown")),
                TsType::named(
                    "{ __bynkDetachedReports?: { kind: string; site: string; type: string | null; variant: string | null }[] }",
                ),
            );
            let splice = crate::ts_build::call(
                TsExpr::OptionalMember {
                    object: Box::new(member(crate::ts_build::paren(reports), "__bynkDetachedReports")),
                    property: "splice".to_string(),
                },
                vec![crate::ts_build::num_lit(0)],
            );
            crate::ts_build::const_(
                "detached",
                crate::ts_build::binary(
                    bynk_ts::TsBinaryOp::NullishCoalescing,
                    splice,
                    TsExpr::array(Vec::new()),
                ),
            )
        })
    }

    fn entries(&self) -> Vec<TsObjectEntry> {
        if !self.enabled {
            return Vec::new();
        }
        vec![
            TsObjectEntry::prop(
                "detached_failures".to_string(),
                detached_or_undefined(member(ident("detached"), "length")),
            ),
            TsObjectEntry::prop(
                "detached".to_string(),
                detached_or_undefined(ident("detached")),
            ),
        ]
    }

    /// The statements of an NDJSON `report`, after the drain.
    fn wrap_ndjson(&self, stmts: Vec<TsStmt>) -> Vec<TsStmt> {
        self.drain().into_iter().chain(stmts).collect()
    }

    /// The statements of the human `report`: drain, the case's line, then the reports.
    fn wrap_human(&self, stmts: Vec<TsStmt>) -> Vec<TsStmt> {
        self.wrap_ndjson(stmts)
            .into_iter()
            .chain(self.print())
            .collect()
    }

    fn print(&self) -> Option<TsStmt> {
        self.enabled.then(|| {
            use crate::ts_build::{cond_expr, null_lit, str_lit, strict_eq};
            let field = |name: &str| member(ident("d"), name);
            // `${d.variant === null ? "" : `.${d.variant}`}`
            let variant = cond_expr(
                strict_eq(field("variant"), null_lit()),
                str_lit(""),
                TsExpr::template_lit(vec![".".to_string(), String::new()], vec![field("variant")]),
            );
            // `returned Err (${d.type}${variant})`, or `"faulted"`.
            let outcome = cond_expr(
                strict_eq(field("kind"), str_lit("err")),
                TsExpr::template_lit(
                    vec!["returned Err (".to_string(), String::new(), ")".to_string()],
                    vec![field("type"), variant],
                ),
                str_lit("faulted"),
            );
            crate::ts_build::for_of(
                "d",
                ident("detached"),
                expr_stmt(method_call(
                    ident("console"),
                    "log",
                    vec![TsExpr::template_lit(
                        vec![
                            "    ! a detached effect ".to_string(),
                            " at ".to_string(),
                            String::new(),
                        ],
                        vec![outcome, field("site")],
                    )],
                )),
            )
        })
    }
}

pub fn emit_test_main(tests: &[RunnableTest], import_ext: ImportExt, pg: bool) -> TsProgram {
    let ext = import_ext.as_str();
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment("top-level test runner", None));
    // Node's `process` global isn't declared without @types/node. The runner
    // uses `process.exit` and reads `process.env.BYNK_TEST_FORMAT` (v0.59: set
    // to `ndjson` by `bynk test --format json`), so we narrow the global with a
    // minimal ambient declaration rather than pulling in a dependency.
    program.push(TsStmt::decl(
        TsDecl::DeclareConst {
            name: "process".to_string(),
            ty: TsType::Object(vec![
                TsTypeMember::method(
                    "exit",
                    vec![TsParam::new(
                        "code".to_string(),
                        Some(TsType::named("number")),
                    )],
                    TsType::named("never"),
                ),
                TsTypeMember::prop(
                    "env",
                    TsType::Object(vec![TsTypeMember::index(
                        "k",
                        TsType::named("string"),
                        TsType::union(vec![TsType::named("string"), TsType::named("undefined")]),
                    )]),
                ),
            ]),
        },
        None,
    ));
    let mut sorted: Vec<&RunnableTest> = tests.iter().collect();
    sorted.sort_by(|a, b| a.target_name.cmp(&b.target_name));
    for (i, t) in sorted.iter().enumerate() {
        let module_stem = t
            .module_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("test");
        program.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: case_alias(i),
                from: sibling_specifier(module_stem, ext),
            },
            None,
        ));
    }

    // v0.127 (editor-currency slice 6), #2273: `bynk test --case <name>` and
    // `--suite <name>`, each repeatable, set BYNK_TEST_FILTER (see
    // `TEST_FILTER_RUNTIME`). `active` is the modules whose suite is selected;
    // each runs through `runSelected`, which calls its own `run(name)` once per
    // selected case name and concatenates the results, so a module is still one
    // suite record. Unset runs the whole project unchanged.
    let mut main_body: Vec<TsStmt> = Vec::new();
    let modules_items: Vec<TsExpr> = sorted
        .iter()
        .enumerate()
        .map(|(i, t)| {
            TsExpr::object(vec![
                ("name".to_string(), str_lit(t.target_name.clone())),
                (
                    "names".to_string(),
                    member(ident(case_alias(i)), "__bynkCases"),
                ),
                ("run".to_string(), member(ident(case_alias(i)), "run")),
            ])
        })
        .collect();
    main_body.push(const_("modules", TsExpr::multiline_array(modules_items)));
    main_body.push(TsStmt::let_stmt(
        TsBindingName::Ident("passed".to_string()),
        None,
        Some(num_lit("0")),
        None,
    ));
    main_body.push(TsStmt::let_stmt(
        TsBindingName::Ident("failed".to_string()),
        None,
        Some(num_lit("0")),
        None,
    ));
    // v0.59: `--format json` sets BYNK_TEST_FORMAT=ndjson and captures stdout;
    // the runner then emits one JSON event per line (an internal protocol the
    // driver re-renders into the pinned document). Otherwise the human ✓ / ✗
    // output is byte-for-byte unchanged.
    //
    // `TsLit::Raw`, not `TsLit::Str` — see its own doc: the literal already
    // spells the JS unicode escape as six ASCII characters, which `Str`'s own
    // escaper would double the backslash of.
    main_body.push(const_(
        "PREFIX",
        TsExpr::Lit(TsLit::Raw("\"system \\u00b7 \"".to_string())),
    ));
    main_body.push(const_(
        "active",
        method_call(
            ident("modules"),
            "filter",
            vec![TsExpr::Arrow {
                params: vec![TsParam::new("m".to_string(), None)],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(call(
                    ident("selected"),
                    vec![cond_expr(
                        method_call(
                            member(ident("m"), "name"),
                            "startsWith",
                            vec![ident("PREFIX")],
                        ),
                        method_call(
                            member(ident("m"), "name"),
                            "slice",
                            vec![member(ident("PREFIX"), "length")],
                        ),
                        member(ident("m"), "name"),
                    )],
                )))),
            }],
        ),
    ));

    let emit_call = |entries: Vec<TsObjectEntry>| {
        expr_stmt(call(ident("emit"), vec![TsExpr::object_entries(entries)]))
    };
    // #2138 (track §3.6): a remote run's cases report how often `--settle`
    // retried them.
    let remote = tests.iter().any(|t| t.remote);
    let retries_entry = || {
        remote.then(|| TsObjectEntry::prop("retries".to_string(), member(ident("r"), "retries")))
    };
    // Track C (#2266): a failed generative case reports how to reproduce it as
    // data — `kind`, `seed` and, when shrinking produced one, `counterexample` —
    // after the fields above. Gated on the build having a generative module, as
    // `retries` is on a remote one, so every other runner is unchanged;
    // `JSON.stringify` drops one a case leaves undefined.
    let generative = tests.iter().any(|t| t.generative);
    // #3437: failures a detached effect reported, drained after each case.
    let detach = DetachReports {
        enabled: tests.iter().any(|t| t.detach_reports),
    };
    let generative_entries = || -> Vec<TsObjectEntry> {
        if !generative {
            return Vec::new();
        }
        // Every module's results are typed as the entry's `Result`, whose fields
        // these are optional, so a build mixing generative and plain modules
        // type-checks.
        ["kind", "seed", "counterexample"]
            .into_iter()
            .map(|k| TsObjectEntry::prop(k.to_string(), member(ident("r"), k)))
            .collect()
    };

    // #2267 stage 2: how long the case ran, in milliseconds, measured by its module's
    // own `run`. Only the stream carries it; `--format json` keeps its document
    // deterministic and drops it.
    let duration_entry =
        || TsObjectEntry::prop("durationMs".to_string(), member(ident("r"), "durationMs"));

    // #2842: the modules that take a snapshot export `__bynkSnapshotReport`; the
    // run hands the driver every selected one's report — as an event in the stream,
    // and as one marked line in the human output when the driver asks for it.
    let snapshot_report = snapshot_report_decl(&sorted);

    let mut ndjson_stmts = vec![
        const_(
            "emit",
            TsExpr::Arrow {
                params: vec![TsParam::new(
                    "o".to_string(),
                    Some(TsType::named("unknown")),
                )],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(method_call(
                    ident("globalThis.console"),
                    "log",
                    vec![method_call(
                        ident("globalThis.JSON"),
                        "stringify",
                        vec![ident("o")],
                    )],
                )))),
            },
        ),
        emit_call(vec![
            TsObjectEntry::prop("type".to_string(), str_lit("run-begin")),
            TsObjectEntry::prop("suites".to_string(), member(ident("active"), "length")),
        ]),
        TsStmt::for_of(
            "m",
            ident("active"),
            block(vec![
                const_(
                    "integration",
                    method_call(
                        member(ident("m"), "name"),
                        "startsWith",
                        vec![ident("PREFIX")],
                    ),
                ),
                const_(
                    "suite",
                    cond_expr(
                        ident("integration"),
                        method_call(
                            member(ident("m"), "name"),
                            "slice",
                            vec![member(ident("PREFIX"), "length")],
                        ),
                        member(ident("m"), "name"),
                    ),
                ),
                const_(
                    "kind",
                    cond_expr(
                        ident("integration"),
                        str_lit("integration"),
                        str_lit("unit"),
                    ),
                ),
                emit_call(vec![
                    TsObjectEntry::prop("type".to_string(), str_lit("suite-begin")),
                    TsObjectEntry::prop("name".to_string(), ident("suite")),
                    TsObjectEntry::Shorthand("kind".to_string()),
                    TsObjectEntry::prop(
                        "tests".to_string(),
                        call(ident("planned"), vec![member(ident("m"), "names")]),
                    ),
                ]),
                TsStmt::decl(
                    TsDecl::Function {
                        name: "report".to_string(),
                        generics: Vec::new(),
                        params: vec![TsParam::new("r".to_string(), Some(TsType::named("Result")))],
                        return_type: None,
                        body: detach.wrap_ndjson(vec![TsStmt::if_else_same_line_stmt(
                            member(ident("r"), "pass"),
                            block(vec![
                                TsStmt::increment(ident("passed"), None),
                                emit_call(
                                    vec![
                                        TsObjectEntry::prop("type".to_string(), str_lit("case")),
                                        TsObjectEntry::Shorthand("suite".to_string()),
                                        TsObjectEntry::prop(
                                            "name".to_string(),
                                            member(ident("r"), "name"),
                                        ),
                                        TsObjectEntry::prop("outcome".to_string(), str_lit("pass")),
                                    ]
                                    .into_iter()
                                    .chain(retries_entry())
                                    .chain(detach.entries())
                                    .chain(std::iter::once(duration_entry()))
                                    .collect(),
                                ),
                            ]),
                            block(vec![
                                TsStmt::increment(ident("failed"), None),
                                emit_call(
                                    vec![
                                        TsObjectEntry::prop("type".to_string(), str_lit("case")),
                                        TsObjectEntry::Shorthand("suite".to_string()),
                                        TsObjectEntry::prop(
                                            "name".to_string(),
                                            member(ident("r"), "name"),
                                        ),
                                        TsObjectEntry::prop("outcome".to_string(), str_lit("fail")),
                                        TsObjectEntry::prop(
                                            "message".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "message"),
                                            ),
                                        ),
                                        TsObjectEntry::prop(
                                            "location".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "location"),
                                            ),
                                        ),
                                        // #2272: the declaration a violated invariant or transition
                                        // names; absent (`JSON.stringify` drops it) otherwise.
                                        TsObjectEntry::prop(
                                            "violation".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "violation"),
                                            ),
                                        ),
                                        // #2272: the steps of a failed history that an agent refused.
                                        TsObjectEntry::prop(
                                            "refusals".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "refusals"),
                                            ),
                                        ),
                                    ]
                                    .into_iter()
                                    .chain(retries_entry())
                                    .chain(detach.entries())
                                    .chain(generative_entries())
                                    .chain(std::iter::once(duration_entry()))
                                    .collect(),
                                ),
                            ]),
                            None,
                        )]),
                        is_async: false,
                        inline: false,
                    },
                    None,
                ),
                expr_stmt(await_expr(call(
                    ident("runSelected"),
                    vec![member(ident("m"), "run"), ident("report")],
                ))),
                emit_call(vec![
                    TsObjectEntry::prop("type".to_string(), str_lit("suite-end")),
                    TsObjectEntry::prop("name".to_string(), ident("suite")),
                ]),
            ]),
            None,
        ),
        emit_call(vec![
            TsObjectEntry::prop("type".to_string(), str_lit("run-end")),
            TsObjectEntry::Shorthand("passed".to_string()),
            TsObjectEntry::Shorthand("failed".to_string()),
            // #2273: the `--suite`/`--case` values that selected nothing; absent
            // (`JSON.stringify` drops an undefined) unless a filter went unmatched.
            TsObjectEntry::prop("unmatched".to_string(), call(ident("unmatched"), vec![])),
        ]),
    ];
    // #2842: the snapshot report goes ahead of `run-end`.
    let run_end = ndjson_stmts.pop();
    if let Some(decl) = &snapshot_report {
        ndjson_stmts.push(decl.clone());
        ndjson_stmts.push(emit_call(vec![
            TsObjectEntry::prop("type".to_string(), str_lit("snapshots")),
            TsObjectEntry::prop("files".to_string(), ident("__snapshotFiles")),
        ]));
    }
    ndjson_stmts.extend(run_end);
    let ndjson_branch = block(ndjson_stmts);

    let human_branch = block(vec![
        console_log(str_lit("Running tests...\n")),
        TsStmt::for_of(
            "m",
            ident("active"),
            block(vec![
                console_log(TsExpr::template_lit(
                    vec![String::new(), ":".to_string()],
                    vec![member(ident("m"), "name")],
                )),
                TsStmt::decl(
                    TsDecl::Function {
                        name: "report".to_string(),
                        generics: Vec::new(),
                        params: vec![TsParam::new("r".to_string(), Some(TsType::named("Result")))],
                        return_type: None,
                        body: detach.wrap_human(vec![TsStmt::if_else_same_line_stmt(
                            member(ident("r"), "pass"),
                            TsStmt::inline_block(
                                vec![
                                    TsStmt::increment(ident("passed"), None),
                                    console_log(TsExpr::template_lit(
                                        vec!["  \\u2713 ".to_string(), String::new()],
                                        vec![member(ident("r"), "name")],
                                    )),
                                ],
                                None,
                            ),
                            TsStmt::inline_block(
                                vec![
                                    TsStmt::increment(ident("failed"), None),
                                    console_log(TsExpr::template_lit(
                                        vec!["  \\u2717 ".to_string(), String::new()],
                                        vec![member(ident("r"), "name")],
                                    )),
                                    if_(
                                        member(ident("r"), "error"),
                                        console_log(TsExpr::template_lit(
                                            vec!["    ".to_string(), String::new()],
                                            vec![member(member(ident("r"), "error"), "message")],
                                        )),
                                    ),
                                ],
                                None,
                            ),
                            None,
                        )]),
                        is_async: false,
                        inline: false,
                    },
                    None,
                ),
                expr_stmt(await_expr(call(
                    ident("runSelected"),
                    vec![member(ident("m"), "run"), ident("report")],
                ))),
                console_log(str_lit("")),
            ]),
            None,
        ),
        console_log(TsExpr::template_lit(
            vec![
                String::new(),
                " passed, ".to_string(),
                " failed.".to_string(),
            ],
            vec![ident("passed"), ident("failed")],
        )),
        expr_stmt(call(ident("reportUnmatched"), vec![])),
    ]);
    // #2842: under `BYNK_SNAPSHOT_REPORT=1` the driver reads the report from one
    // marked line at the end of the human output and strips it.
    let human_branch = with_snapshot_report(human_branch, &snapshot_report);

    main_body.push(TsStmt::if_else_same_line_stmt(
        strict_eq(
            member(member(ident("process"), "env"), "BYNK_TEST_FORMAT"),
            str_lit("ndjson"),
        ),
        ndjson_branch,
        human_branch,
        None,
    ));
    main_body.extend(pg.then(pglite_teardown));
    // A filter value that selected nothing fails the run, as a failed case does.
    main_body.push(if_(
        or_expr(
            binary(TsBinaryOp::GreaterThan, ident("failed"), num_lit("0")),
            TsExpr::Binary {
                op: TsBinaryOp::StrictNotEq,
                left: Box::new(call(ident("unmatched"), vec![])),
                right: Box::new(ident("undefined")),
            },
        ),
        expr_stmt(call(member(ident("process"), "exit"), vec![num_lit("1")])),
    ));

    program.push(TestRuntimeModule::TestFilter.splice());
    program.push(TsStmt::decl(
        TsDecl::Function {
            name: "main".to_string(),
            generics: Vec::new(),
            params: vec![],
            return_type: None,
            body: main_body,
            is_async: true,
            inline: false,
        },
        None,
    ));
    program.push(expr_stmt(call(ident("main"), vec![])));
    program
}

/// #3321: closes the PGlite databases a run started. PGlite leaves a ten-second
/// timer behind, which would otherwise hold the process open that long; the
/// runtime registers the closer on `globalThis`, so no import is needed.
fn pglite_teardown() -> TsStmt {
    // `await (globalThis as unknown as { __bynkPgliteTeardown?: () => Promise<void> }).__bynkPgliteTeardown?.();`
    let closer = TsExpr::OptionalCall {
        callee: Box::new(member(
            crate::ts_build::paren(crate::ts_build::as_expr(
                crate::ts_build::as_expr(ident("globalThis"), TsType::named("unknown")),
                TsType::named("{ __bynkPgliteTeardown?: () => Promise<void> }"),
            )),
            "__bynkPgliteTeardown",
        )),
        args: Vec::new(),
    };
    expr_stmt(await_expr(closer))
}

/// #2842: under `BYNK_SNAPSHOT_REPORT=1` the driver reads the report from one
/// marked line at the end of the human output and strips it.
fn with_snapshot_report(human_branch: TsStmt, snapshot_report: &Option<TsStmt>) -> TsStmt {
    match snapshot_report {
        Some(decl) => {
            // `if (process.env.BYNK_SNAPSHOT_REPORT === "1") { globalThis.console.log("\u0001bynk-snapshots " + globalThis.JSON.stringify(__snapshotFiles)); }`
            let marked = crate::ts_build::binary(
                TsBinaryOp::Add,
                crate::ts_build::raw_lit("\"\\u0001bynk-snapshots \""),
                method_call(
                    ident("globalThis.JSON"),
                    "stringify",
                    vec![ident("__snapshotFiles")],
                ),
            );
            block(vec![
                human_branch,
                decl.clone(),
                crate::ts_build::if_(
                    strict_eq(
                        member(member(ident("process"), "env"), "BYNK_SNAPSHOT_REPORT"),
                        str_lit("1"),
                    ),
                    TsStmt::inline_block(vec![console_log(marked)], None),
                ),
            ])
        }
        None => human_branch,
    }
}

/// `const __snapshotFiles = ([[i, test_i], …] as const).filter(([i]) => active.includes(modules[i]!)).flatMap(([, t]) => t.__bynkSnapshotReport());`:
/// the report of every selected module that takes a snapshot.
fn snapshot_files_decl(modules: Vec<TsExpr>) -> TsStmt {
    use crate::ts_build::{arrow, index, paren};
    let selected = method_call(
        paren(TsExpr::AsConst(Box::new(TsExpr::array(modules)))),
        "filter",
        vec![arrow(
            vec![TsParam::array_pattern(vec!["i".to_string()], None)],
            method_call(
                ident("active"),
                "includes",
                vec![TsExpr::NonNull(Box::new(index(
                    ident("modules"),
                    ident("i"),
                )))],
            ),
        )],
    );
    const_(
        "__snapshotFiles",
        method_call(
            selected,
            "flatMap",
            vec![arrow(
                vec![TsParam::array_pattern(
                    vec![String::new(), "t".to_string()],
                    None,
                )],
                method_call(ident("t"), "__bynkSnapshotReport", Vec::new()),
            )],
        ),
    )
}

/// #2842: the report declaration, when some selected module takes a snapshot.
fn snapshot_report_decl(sorted: &[&RunnableTest]) -> Option<TsStmt> {
    let modules: Vec<TsExpr> = sorted
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.snapshot_files.is_empty())
        .map(|(i, _)| {
            TsExpr::array(vec![
                crate::ts_build::num_lit(i),
                ident(format!("test_{i}")),
            ])
        })
        .collect();
    (!modules.is_empty()).then(|| snapshot_files_decl(modules))
}

/// `(detached.length > 0 ? <when_some> : undefined)`: a report field only a run with detached
/// failures carries.
fn detached_or_undefined(when_some: TsExpr) -> TsExpr {
    crate::ts_build::paren(crate::ts_build::cond_expr(
        crate::ts_build::binary(
            bynk_ts::TsBinaryOp::GreaterThan,
            member(ident("detached"), "length"),
            crate::ts_build::num_lit(0),
        ),
        when_some,
        ident("undefined"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review of #1410, finding 1: `run_dispatch_stmt` must escape a raw
    /// case/property name exactly once. Pins a name containing both a
    /// double quote and a backslash — the exact shape a double-escape
    /// (passing an already-`emitter::escape_ts_string`-escaped name into
    /// `str_lit`, which escapes again) would corrupt, and which no `.bynk`
    /// fixture exercises today.
    #[test]
    fn the_run_function_escapes_a_quoted_backslashed_name_exactly_once() {
        let stmts = build_run_function(&[(r#"say "hi" \ ok"#.to_string(), "test_a".to_string())]);
        let printed: String = stmts.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect();
        // The name is escaped once in the case list and once in the dispatch.
        assert!(
            printed
                .contains(r#"export const __bynkCases: readonly string[] = ["say \"hi\" \\ ok"];"#),
            "{printed}"
        );
        assert!(
            printed.contains(r#"if (want("say \"hi\" \\ ok")) {"#),
            "{printed}"
        );
        assert!(printed.contains(r#"const r = { name: "say \"hi\" \\ ok", ...(await test_a()), durationMs: __bynkCaseSince(started) };"#), "{printed}");
    }

    /// #2267 stage 2: each case is timed, handed to `onCase` as it finishes and
    /// pushed to the results, in that order, and the module lists its case names.
    #[test]
    fn the_run_function_times_each_case_and_reports_it_as_it_finishes() {
        let stmts = build_run_function(&[
            ("one".to_string(), "test_one".to_string()),
            ("two".to_string(), "test_two".to_string()),
        ]);
        let printed: String = stmts.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect();
        assert!(
            printed.contains(r#"export const __bynkCases: readonly string[] = ["one", "two"];"#)
        );
        assert!(printed.contains(
            "export async function run(only?: string, onCase?: (a0: CaseResult) => void)"
        ));
        let started = printed
            .find("const started = __bynkCaseNow();")
            .expect("timed");
        let ran = printed.find("...(await test_one())").expect("ran");
        let pushed = printed.find("results.push(r);").expect("pushed");
        let reported = printed.find("if (onCase) onCase(r);").expect("reported");
        assert!(
            started < ran && ran < pushed && pushed < reported,
            "{printed}"
        );
        assert_eq!(printed.matches("const started = ").count(), 2);
    }

    // v0.127 (editor-currency slice 6), #2273: the top-level runner reads
    // `BYNK_TEST_CASE` and threads it into every suite's `run(only)`, so a
    // `bynk test --case <name>` filters which cases execute.
    #[test]
    fn emit_test_main_threads_the_case_and_suite_filter() {
        let tests = vec![RunnableTest {
            snapshot_files: Vec::new(),
            target_name: "commerce.money".to_string(),
            module_path: std::path::PathBuf::from("tests/commerce_money.test.ts"),
            kind: "unit",
            suite_name: "commerce.money".to_string(),
            cases: vec![DiscoveredCase {
                name: "rounds".to_string(),
                location: None,
                tier: None,
                generative: None,
                doc: false,
            }],
            skipped: Vec::new(),
            envs: Vec::new(),
            remote: false,
            generative: false,
            detach_reports: false,
        }];
        let program = emit_test_main(&tests, ImportExt::Js, false);
        let out = bynk_ts::print(&program, "", "", "").text;
        assert!(
            out.contains("process.env.BYNK_TEST_FILTER"),
            "the runner must read the filter from the environment, got:\n{out}"
        );
        // Both the NDJSON and the human loop walk the selected modules and run
        // each through the filter, so a module stays one suite record.
        assert_eq!(
            out.matches("await runSelected(m.run, report)").count(),
            2,
            "both dispatch branches must run each suite through the filter, got:\n{out}"
        );
        assert_eq!(
            out.matches("for (const m of active)").count(),
            2,
            "got:\n{out}"
        );
        assert!(
            out.contains("unmatched: unmatched()"),
            "run-end reports the values that selected nothing, got:\n{out}"
        );
    }

    /// #1325's own direct fixture test — `emit_test_main`'s signature
    /// (`&[RunnableTest]`/`ImportExt`) is simple enough to hand-construct
    /// directly, unlike `emit_worker_compose`'s/`emit_worker_entry`'s own
    /// whole-project `UnitTable`-derived data (#1321/#1323's own reason for
    /// relying on the full fixture corpus instead). Calls the real,
    /// converted function with real-shaped data and asserts the printed
    /// output is byte-identical to a real fixture's own `tests/main.ts`, not
    /// a hand-rebuilt approximation of it.
    #[test]
    fn matches_the_real_fixtures_own_tests_main_ts_byte_for_byte() {
        let expected = include_str!(
            "../../../bynk-tests/tests/fixtures/positive/109_test_with_opaque_construction/\
             expected/tests/main.ts"
        );
        let tests = vec![RunnableTest {
            snapshot_files: Vec::new(),
            target_name: "commerce.payment".to_string(),
            module_path: std::path::PathBuf::from("commerce_payment.test.js"),
            kind: "unit",
            suite_name: "commerce.payment".to_string(),
            cases: vec![DiscoveredCase {
                name: "opaque".to_string(),
                location: None,
                tier: None,
                generative: None,
                doc: false,
            }],
            skipped: Vec::new(),
            envs: Vec::new(),
            remote: false,
            generative: false,
            detach_reports: false,
        }];
        let program = emit_test_main(&tests, ImportExt::Js, false);
        let printed = bynk_ts::print(&program, "", "", "");
        assert_eq!(printed.text, expected);
    }

    /// No stored golden holds a snapshot report, so the statements are pinned: the declaration of
    /// `__snapshotFiles` over the modules that take a snapshot, and the marked human line.
    #[test]
    fn the_snapshot_report_prints_its_declaration_and_marked_line() {
        let decl = snapshot_files_decl(vec![
            TsExpr::array(vec![crate::ts_build::num_lit(0), ident("test_0")]),
            TsExpr::array(vec![crate::ts_build::num_lit(2), ident("test_2")]),
        ]);
        assert_eq!(
            bynk_ts::print_stmt(&decl, 0),
            "const __snapshotFiles = ([[0, test_0], [2, test_2]] as const).filter(([i]) => active.includes(modules[i]!)).flatMap(([, t]) => t.__bynkSnapshotReport());\n"
        );
        let human = with_snapshot_report(block(Vec::new()), &Some(decl));
        let printed = bynk_ts::print_stmt(&human, 0);
        assert!(
            printed.contains(
                "if (process.env.BYNK_SNAPSHOT_REPORT === \"1\") { globalThis.console.log(\"\\u0001bynk-snapshots \" + globalThis.JSON.stringify(__snapshotFiles)); }"
            ),
            "{printed}"
        );
    }
}
