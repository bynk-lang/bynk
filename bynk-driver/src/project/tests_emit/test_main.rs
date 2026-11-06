//! The suite runner (`tests_main`): TypeScript builders, the run function and the filter runtime.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
// #1759: every namespace a test module imports, from one definition.
use bynk_check::d1_migrations::MigrationFile;
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam,
    TsProgram, TsStmt, TsType, TsTypeMember,
};

pub(super) fn ident(s: impl Into<String>) -> TsExpr {
    TsExpr::Ident(s.into())
}

pub(super) fn str_lit(s: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Str(s.into()))
}

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

pub(super) fn num_lit(n: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Num(n.into()))
}

pub(super) fn member(object: TsExpr, property: impl Into<String>) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.into(),
    }
}

pub(super) fn call(callee: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(callee),
        args,
    }
}

pub(super) fn method_call(object: TsExpr, method: &str, args: Vec<TsExpr>) -> TsExpr {
    call(member(object, method), args)
}

pub(super) fn await_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Await(Box::new(expr))
}

/// A zero-substitution template literal — `` `<text>` `` with `text` already
/// fully assembled (any `${...}` it contains is baked-in literal text, not a
/// real substitution `bynk_ts` tracks). Slice F (#1407): `emit_system_http_
/// support`'s own `concrete_path` (every `:name` path segment already
/// resolved to `${paramIdent}` text at Rust-`format!` time, per its own doc)
/// is the real site. Routes through the validated `TsExpr::template_lit`
/// constructor rather than the bare `TsExpr::TemplateLit { .. }` variant, the
/// same guard this file's own not-yet-refactored call sites already use
/// (review of #1408, finding 1).
pub(super) fn template(text: impl Into<String>) -> TsExpr {
    TsExpr::template_lit(vec![text.into()], vec![])
}

/// One `async function __sysdrive_*` driver: an optional lead statement
/// (`const __body = JSON.stringify(...)`, typed/no-auth drivers only), `const
/// __h = __makeHarness();`, `const __req = new Request(<url>, <options>);`,
/// `const __res = await __h.env.<binding>.fetch(__req);`, then `return
/// <decode_fn>(__res, <payload>);`. Slice F (#1407): the one real shape
/// shared by all four per-route `__sysdrive_{,raw_,noauth_,rawnoauth_}*`
/// drivers in `emit_system_http_support`, differing only in name (built here
/// from `kind_prefix`/`sname`/`key`, one shared spot rather than one
/// `format!` call per driver kind), params, the optional body statement, the
/// options object (Decision A, see its own call sites), and the decode
/// function — every real difference is a parameter here, not a reason to
/// hand-write four near-identical bodies.
#[allow(clippy::too_many_arguments)]
pub(super) fn sysdrive_driver(
    kind_prefix: &str,
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
    params.push(TsParam {
        name: "__sub".to_string(),
        ty: Some(TsType::named("string")),
        optional: false,
    });
    // #2019: a refinement-guarded route signs the claims the acting actor
    // presents (`{}` for a non-refinement actor) next to `sub`.
    if signs_claims {
        params.push(TsParam {
            name: "__claims".to_string(),
            ty: Some(TsType::named_with_args(
                "globalThis.Record",
                vec![TsType::named("string"), TsType::named("unknown")],
            )),
            optional: false,
        });
    }
    if takes_as {
        params.push(TsParam {
            name: "__as".to_string(),
            ty: Some(TsType::named("string")),
            optional: false,
        });
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
            name: format!("__sysdrive_{kind_prefix}{sname}_{key}"),
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

pub(super) fn and_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::And,
        left: Box::new(left),
        right: Box::new(right),
    }
}

pub(super) fn gt(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::GreaterThan,
        left: Box::new(left),
        right: Box::new(right),
    }
}

pub(super) fn strict_eq(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::StrictEq,
        left: Box::new(left),
        right: Box::new(right),
    }
}

pub(super) fn or_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::Or,
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// The module's own runner, built from its real per-case/per-property/per-attack
/// `(name, runner)` pairs, in dispatch order:
///
/// ```ts
/// export const __bynkCases: readonly string[] = ["a", "b"];
/// export async function run(only?: string, onCase?: (a0: CaseResult) => void) {
///   const results = [];
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
                        TsObjectEntry::Prop("name".to_string(), str_lit(name.to_string())),
                        TsObjectEntry::Spread(TsExpr::Paren(Box::new(await_expr(call(
                            ident(runner_name),
                            vec![],
                        ))))),
                        TsObjectEntry::Prop(
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
        const_("results", TsExpr::array(vec![])),
        const_(
            "want",
            TsExpr::Arrow {
                params: vec![TsParam {
                    name: "n".to_string(),
                    ty: Some(TsType::named("string")),
                    optional: false,
                }],
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
    let names = entries
        .iter()
        .map(|(name, _)| format!("\"{}\"", bynk_emit::emitter::escape_ts_string(name)))
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        TsStmt::raw(CASE_TIMING_RUNTIME, None),
        TsStmt::raw(
            format!("export const __bynkCases: readonly string[] = [{names}];"),
            None,
        ),
        TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::Function {
                name: "run".to_string(),
                generics: Vec::new(),
                params: vec![
                    TsParam {
                        name: "only".to_string(),
                        ty: Some(TsType::named("string")),
                        optional: true,
                    },
                    TsParam {
                        name: "onCase".to_string(),
                        ty: Some(TsType::Fn {
                            params: vec![TsType::named("CaseResult")],
                            ret: Box::new(TsType::named("void")),
                        }),
                        optional: true,
                    },
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

/// What every module's `run` shares (#2267 stage 2): the shape of a finished case
/// handed to `onCase`, and the clock that measures it. `performance.now()` where the
/// host has one (sub-millisecond), else `Date.now()`. It is read through
/// `globalThis`, so no DOM or Node typings are assumed, and is the host's own clock,
/// not the harness-owned test clock (`advance Clock`).
pub(super) const CASE_TIMING_RUNTIME: &str = r#"type CaseResult = {
  name: string;
  pass: boolean;
  error?: { message: string; location: string; violation?: { kind: string; agent: string; name: string; location: string }; refusals?: { index: number; call: string; kind: string; agent: string; name: string; location: string }[] };
  durationMs: number;
  retries?: number;
  kind?: string;
  seed?: string;
  counterexample?: string;
};
const __bynkCasePerformance = (globalThis as unknown as { performance?: { now(): number } }).performance;
const __bynkCaseNow = (): number => (__bynkCasePerformance ? __bynkCasePerformance.now() : Date.now());
const __bynkCaseSince = (started: number): number => Math.round((__bynkCaseNow() - started) * 100) / 100;"#;

/// `test ? consequent : alternate`.
pub(super) fn cond_expr(test: TsExpr, consequent: TsExpr, alternate: TsExpr) -> TsExpr {
    TsExpr::Conditional {
        test: Box::new(test),
        consequent: Box::new(consequent),
        alternate: Box::new(alternate),
    }
}

pub(super) fn console_log(arg: TsExpr) -> TsStmt {
    expr_stmt(method_call(ident("globalThis.console"), "log", vec![arg]))
}

pub(super) fn const_(name: impl Into<String>, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.into()), None, init, None)
}

pub(super) fn expr_stmt(expr: TsExpr) -> TsStmt {
    TsStmt::expr_stmt(expr, None)
}

pub(super) fn block(stmts: Vec<TsStmt>) -> TsStmt {
    TsStmt::block(stmts, None)
}

pub(super) fn if_(cond: TsExpr, then_branch: TsStmt) -> TsStmt {
    TsStmt::if_stmt(cond, then_branch, None)
}

pub(crate) fn emit_test_main(tests: &[RunnableTest], import_ext: ImportExt) -> TsProgram {
    let ext = import_ext.as_str();
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynkc — do not edit by hand.",
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
                    vec![TsParam {
                        name: "code".to_string(),
                        ty: Some(TsType::named("number")),
                        optional: false,
                    }],
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
                alias: format!("test_{i}"),
                from: format!("./{module_stem}.{ext}"),
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
                    member(ident(format!("test_{i}")), "__bynkCases"),
                ),
                ("run".to_string(), member(ident(format!("test_{i}")), "run")),
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
                params: vec![TsParam {
                    name: "m".to_string(),
                    ty: None,
                    optional: false,
                }],
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
        remote.then(|| TsObjectEntry::Prop("retries".to_string(), member(ident("r"), "retries")))
    };
    // Track C (#2266): a failed generative case reports how to reproduce it as
    // data — `kind`, `seed` and, when shrinking produced one, `counterexample` —
    // after the fields above. Gated on the build having a generative module, as
    // `retries` is on a remote one, so every other runner is unchanged;
    // `JSON.stringify` drops one a case leaves undefined.
    let generative = tests.iter().any(|t| t.generative);
    let generative_entries = || -> Vec<TsObjectEntry> {
        if !generative {
            return Vec::new();
        }
        // Every module's results are typed as the entry's `Result`, whose fields
        // these are optional, so a build mixing generative and plain modules
        // type-checks.
        ["kind", "seed", "counterexample"]
            .into_iter()
            .map(|k| TsObjectEntry::Prop(k.to_string(), member(ident("r"), k)))
            .collect()
    };

    // #2267 stage 2: how long the case ran, in milliseconds, measured by its module's
    // own `run`. Only the stream carries it; `--format json` keeps its document
    // deterministic and drops it.
    let duration_entry =
        || TsObjectEntry::Prop("durationMs".to_string(), member(ident("r"), "durationMs"));

    // #2842: the modules that take a snapshot export `__bynkSnapshotReport`; the
    // run hands the driver every selected one's report — as an event in the stream,
    // and as one marked line in the human output when the driver asks for it.
    let snapshot_modules: Vec<String> = sorted
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.snapshot_files.is_empty())
        .map(|(i, _)| format!("[{i}, test_{i}]"))
        .collect();
    let snapshot_report = (!snapshot_modules.is_empty()).then(|| {
        format!(
            "const __snapshotFiles = ([{}] as const).filter(([i]) => active.includes(modules[i]!)).flatMap(([, t]) => t.__bynkSnapshotReport());",
            snapshot_modules.join(", ")
        )
    });

    let mut ndjson_stmts = vec![
        const_(
            "emit",
            TsExpr::Arrow {
                params: vec![TsParam {
                    name: "o".to_string(),
                    ty: Some(TsType::named("unknown")),
                    optional: false,
                }],
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
            TsObjectEntry::Prop("type".to_string(), str_lit("run-begin")),
            TsObjectEntry::Prop("suites".to_string(), member(ident("active"), "length")),
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
                    TsObjectEntry::Prop("type".to_string(), str_lit("suite-begin")),
                    TsObjectEntry::Prop("name".to_string(), ident("suite")),
                    TsObjectEntry::Shorthand("kind".to_string()),
                    TsObjectEntry::Prop(
                        "tests".to_string(),
                        call(ident("planned"), vec![member(ident("m"), "names")]),
                    ),
                ]),
                TsStmt::decl(
                    TsDecl::Function {
                        name: "report".to_string(),
                        generics: Vec::new(),
                        params: vec![TsParam {
                            name: "r".to_string(),
                            ty: Some(TsType::named("Result")),
                            optional: false,
                        }],
                        return_type: None,
                        body: vec![TsStmt::if_else_same_line_stmt(
                            member(ident("r"), "pass"),
                            block(vec![
                                TsStmt::increment(ident("passed"), None),
                                emit_call(
                                    vec![
                                        TsObjectEntry::Prop("type".to_string(), str_lit("case")),
                                        TsObjectEntry::Shorthand("suite".to_string()),
                                        TsObjectEntry::Prop(
                                            "name".to_string(),
                                            member(ident("r"), "name"),
                                        ),
                                        TsObjectEntry::Prop("outcome".to_string(), str_lit("pass")),
                                    ]
                                    .into_iter()
                                    .chain(retries_entry())
                                    .chain(std::iter::once(duration_entry()))
                                    .collect(),
                                ),
                            ]),
                            block(vec![
                                TsStmt::increment(ident("failed"), None),
                                emit_call(
                                    vec![
                                        TsObjectEntry::Prop("type".to_string(), str_lit("case")),
                                        TsObjectEntry::Shorthand("suite".to_string()),
                                        TsObjectEntry::Prop(
                                            "name".to_string(),
                                            member(ident("r"), "name"),
                                        ),
                                        TsObjectEntry::Prop("outcome".to_string(), str_lit("fail")),
                                        TsObjectEntry::Prop(
                                            "message".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "message"),
                                            ),
                                        ),
                                        TsObjectEntry::Prop(
                                            "location".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "location"),
                                            ),
                                        ),
                                        // #2272: the declaration a violated invariant or transition
                                        // names; absent (`JSON.stringify` drops it) otherwise.
                                        TsObjectEntry::Prop(
                                            "violation".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "violation"),
                                            ),
                                        ),
                                        // #2272: the steps of a failed history that an agent refused.
                                        TsObjectEntry::Prop(
                                            "refusals".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "refusals"),
                                            ),
                                        ),
                                    ]
                                    .into_iter()
                                    .chain(retries_entry())
                                    .chain(generative_entries())
                                    .chain(std::iter::once(duration_entry()))
                                    .collect(),
                                ),
                            ]),
                            None,
                        )],
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
                    TsObjectEntry::Prop("type".to_string(), str_lit("suite-end")),
                    TsObjectEntry::Prop("name".to_string(), ident("suite")),
                ]),
            ]),
            None,
        ),
        emit_call(vec![
            TsObjectEntry::Prop("type".to_string(), str_lit("run-end")),
            TsObjectEntry::Shorthand("passed".to_string()),
            TsObjectEntry::Shorthand("failed".to_string()),
            // #2273: the `--suite`/`--case` values that selected nothing; absent
            // (`JSON.stringify` drops an undefined) unless a filter went unmatched.
            TsObjectEntry::Prop("unmatched".to_string(), call(ident("unmatched"), vec![])),
        ]),
    ];
    // #2842: the snapshot report goes ahead of `run-end`.
    let run_end = ndjson_stmts.pop();
    if let Some(decl) = &snapshot_report {
        ndjson_stmts.push(TsStmt::raw(decl.clone(), None));
        ndjson_stmts.push(TsStmt::raw(
            "emit({ type: \"snapshots\", files: __snapshotFiles });".to_string(),
            None,
        ));
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
                        params: vec![TsParam {
                            name: "r".to_string(),
                            ty: Some(TsType::named("Result")),
                            optional: false,
                        }],
                        return_type: None,
                        body: vec![TsStmt::if_else_same_line_stmt(
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
                        )],
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
    let human_branch = match &snapshot_report {
        Some(decl) => block(vec![
            human_branch,
            TsStmt::raw(
                format!(
                    "{decl}\nif (process.env.BYNK_SNAPSHOT_REPORT === \"1\") {{ globalThis.console.log(\"\\u0001bynk-snapshots \" + globalThis.JSON.stringify(__snapshotFiles)); }}"
                ),
                None,
            ),
        ]),
        None => human_branch,
    };

    main_body.push(TsStmt::if_else_same_line_stmt(
        strict_eq(
            member(member(ident("process"), "env"), "BYNK_TEST_FORMAT"),
            str_lit("ndjson"),
        ),
        ndjson_branch,
        human_branch,
        None,
    ));
    // A filter value that selected nothing fails the run, as a failed case does.
    main_body.push(if_(
        or_expr(
            gt(ident("failed"), num_lit("0")),
            TsExpr::Binary {
                op: TsBinaryOp::StrictNotEq,
                left: Box::new(call(ident("unmatched"), vec![])),
                right: Box::new(ident("undefined")),
            },
        ),
        expr_stmt(call(member(ident("process"), "exit"), vec![num_lit("1")])),
    ));

    program.push(TsStmt::raw(TEST_FILTER_RUNTIME, None));
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

/// #2273: the entry's `--suite`/`--case` machinery. `BYNK_TEST_FILTER` is one
/// JSON value (`bynk` builds it): `suites` and `cases` each combine their values
/// as "or", the two as "and". `selected` keeps a module whose suite is chosen,
/// `runSelected` runs one module's selected cases through its own `run(name)`
/// (the module is unchanged) and concatenates them, and `unmatched` names each
/// value that selected nothing, which is an error unless the run sees only part of
/// the selection (`--against dev` runs one process per `env` group).
pub(super) const TEST_FILTER_RUNTIME: &str = r#"type Result = {
  name: string;
  pass: boolean;
  error?: { message: string; location: string; violation?: { kind: string; agent: string; name: string; location: string }; refusals?: { index: number; call: string; kind: string; agent: string; name: string; location: string }[] };
  durationMs?: number;
  retries?: number;
  kind?: string;
  seed?: string;
  counterexample?: string;
};
type Filter = {
  program?: string;
  suites?: string[];
  cases?: string[];
  partial?: boolean;
  skipped?: { suite: string; case: string }[];
};
const filter: Filter | undefined =
  process.env.BYNK_TEST_FILTER === undefined ? undefined : globalThis.JSON.parse(process.env.BYNK_TEST_FILTER);
const matchedSuites = new Set<string>();
const matchedCases = new Set<string>();

function selected(suite: string): boolean {
  if (filter?.suites === undefined) return true;
  if (!filter.suites.includes(suite)) return false;
  matchedSuites.add(suite);
  return true;
}

// How many cases the filter selects in a module, known before any of them runs: the
// `tests` of its `suite-begin`. A name is counted once per case that bears it.
function planned(names: readonly string[]): number {
  if (filter?.cases === undefined) return names.length;
  let count = 0;
  for (const name of filter.cases) count += names.filter((n) => n === name).length;
  return count;
}

// Runs a module's selected cases, handing each result to `onCase` as it finishes.
async function runSelected(
  run: (only?: string, onCase?: (r: Result) => void) => Promise<Result[]>,
  onCase: (r: Result) => void,
): Promise<Result[]> {
  if (filter?.cases === undefined) return run(undefined, onCase);
  const results: Result[] = [];
  for (const name of filter.cases) {
    const found = await run(name, onCase);
    if (found.length > 0) matchedCases.add(name);
    results.push(...found);
  }
  return results;
}

// A case the build skipped is not in it but exists, so a value naming one is matched.
for (const s of filter?.skipped ?? []) {
  if (selected(s.suite) && filter?.cases?.includes(s.case)) matchedCases.add(s.case);
}

function unmatched(): { suites?: string[]; cases?: string[] } | undefined {
  if (filter === undefined || filter.partial === true) return undefined;
  const suites = (filter.suites ?? []).filter((s) => !matchedSuites.has(s));
  const cases = (filter.cases ?? []).filter((c) => !matchedCases.has(c));
  if (suites.length === 0 && cases.length === 0) return undefined;
  return { ...(suites.length > 0 ? { suites } : {}), ...(cases.length > 0 ? { cases } : {}) };
}

function reportUnmatched(): void {
  const left = unmatched();
  if (left === undefined) return;
  const program = filter?.program ?? "bynkc";
  for (const s of left.suites ?? []) {
    globalThis.console.error(`${program} test: no suite named --suite ${globalThis.JSON.stringify(s)}`);
  }
  for (const c of left.cases ?? []) {
    globalThis.console.error(`${program} test: no case matched --case ${globalThis.JSON.stringify(c)}`);
  }
}"#;

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
        }];
        let program = emit_test_main(&tests, ImportExt::Js);
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
            "../../../../bynk-tests/tests/fixtures/positive/109_test_with_opaque_construction/\
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
        }];
        let program = emit_test_main(&tests, ImportExt::Js);
        let printed = bynk_ts::print(&program, "", "", "");
        assert_eq!(printed.text, expected);
    }
}
