//! The Worker entry's responses and faults: the JSON and text responses, the fault
//! helpers `fetch` catches into, and the member guard that marks the router's own answers.

use crate::ts_build::{
    and_expr, binary, block, call, cond_expr, const_, expr_stmt, ident, if_, member, method_call,
    new, null_lit, num_lit, paren, return_, str_lit,
};
use bynk_check::wire_format::CallCheck;
use bynk_ts::{TsExpr, TsStmt, TsSwitchCase};

/// #1825: `__route = "<what>";`, naming the dispatch `fetch`'s fault log
/// reports.
pub(super) fn set_fault_route(what: String) -> TsStmt {
    TsStmt::assign(ident("__route"), str_lit(what), None)
}

/// #2053: what a fault handler logs for a caught `e`, the expression
/// `e instanceof globalThis.Error ? (e.stack || globalThis.String(e)) : e`.
///
/// Logging the error as a value reaches `wrangler tail` as its string,
/// `Error: <message>`, with no frames at all. A V8 stack's own first line is
/// that same string, so nothing is lost; a thrown non-`Error` is logged as
/// itself, as before. `||`, not `??`, so an empty `stack` (assignable on any
/// `Error`) falls back to the string too, rather than logging nothing.
///
/// #2192: the frames stay in the bundle's coordinates (`index.js:L:C`).
/// Cloudflare applies an uploaded source map only to an exception it records,
/// never to a logged string, so the handler also reports the fault
/// (`__bynkReportFault`), and that exception's stack is the mapped one.
pub(crate) fn fault_detail(e: &str) -> TsExpr {
    cond_expr(
        binary(
            bynk_ts::TsBinaryOp::InstanceOf,
            ident(e),
            ident("globalThis.Error"),
        ),
        paren(binary(
            bynk_ts::TsBinaryOp::Or,
            member(ident(e), "stack"),
            call(ident("globalThis.String"), vec![ident(e)]),
        )),
        ident(e),
    )
}

/// #2192: reports a fault the entry caught to the platform, so Cloudflare
/// records it as an exception and applies an uploaded source map to its
/// stack (the logged `fault_detail` string is never mapped). The rejected
/// promise goes to `waitUntil`, which reports it without replacing the
/// handler's own answer: `fetch` still returns its 500 and keeps the outcome
/// `ok`, and a queue message's `ack()`/`retry()` still decides its delivery.
/// It is handled first, so a host that never awaits `waitUntil` (or awaits it
/// late, as the test doubles do) sees no unhandled rejection. `ctx` is absent
/// when the entry is called in-process, e.g. by the test runner.
pub(super) const REPORT_FAULT_HELPER: &str = "function __bynkReportFault(ctx: { waitUntil(promise: globalThis.Promise<unknown>): void } | undefined, e: unknown): void {
  if (ctx === undefined) return;
  const fault = globalThis.Promise.reject(e);
  fault.catch(() => {});
  ctx.waitUntil(fault);
}
";

/// #3872: runs one cron handler and makes a failed run visible to the platform.
/// An `Err` is one structured `console.error` record (the site, the error's type
/// and, for a sum error, its variant tag, never the payload, which can hold user
/// data: #3437, Decision D; only a tag in `variants`, the sum's declared variant names,
/// is taken, so a record error's own `tag` field never reaches the log) and is reported as a fault, so Cloudflare's Cron
/// Past Events shows the run as failed. A throw is logged with its stack,
/// reported and rethrown, so it still fails as an uncaught exception does. The
/// result is returned either way for the test build's `__bynkResult` store.
pub(super) const CRON_RUN_HELPER: &str = "async function __bynkCronRun<T extends { readonly tag: string; readonly error?: unknown }>(ctx: { waitUntil(promise: globalThis.Promise<unknown>): void } | undefined, site: string, errType: string, variants: readonly string[], run: () => globalThis.Promise<T>): globalThis.Promise<T> {
  try {
    const result = await run();
    if (result.tag === \"Err\") {
      const error = result.error as { readonly tag?: unknown } | null;
      const variant = typeof error === \"object\" && error !== null && typeof error.tag === \"string\" && variants.includes(error.tag) ? error.tag : \"\";
      globalThis.console.error({ message: \"a cron run returned Err\", level: \"error\", fields: { site, type: errType, variant } });
      __bynkReportFault(ctx, new globalThis.Error(`${site} returned ${errType}${variant === \"\" ? \"\" : `.${variant}`}`));
    }
    return result;
  } catch (thrown) {
    globalThis.console.error(`${site} faulted`, thrown instanceof globalThis.Error ? (thrown.stack || globalThis.String(thrown)) : thrown);
    __bynkReportFault(ctx, thrown);
    throw thrown;
  }
}
";

/// #3878: settles one queue message whose body does not decode as the consumer's message type.
/// No retry can change the bytes, so it is never retried. With a dead-letter queue (`binding`
/// names its producer binding on `env`) the body is sent there unchanged and the message acked;
/// a failed send, or a configured binding missing from `env`, retries it, so nothing is lost while
/// the dead-letter queue is unavailable. A body a non-Bynk producer sent as raw bytes keeps its content type.
/// Without one it is acked and reported as a fault. One log record names the queue, the message
/// type and the decode error's path and kind, never the body (#3437: it can hold user data).
pub(super) const UNDECODABLE_HELPER: &str = "async function __bynkUndecodable(ctx: { waitUntil(promise: globalThis.Promise<unknown>): void } | undefined, env: unknown, binding: string | undefined, queue: string, type: string, error: unknown, msg: { readonly body: unknown; ack(): void; retry(): void }): globalThis.Promise<void> {
  const cause = error as { readonly kind?: unknown; readonly path?: unknown } | null;
  const path = typeof cause === \"object\" && cause !== null && typeof cause.path === \"string\" ? cause.path : \"$\";
  const kind = typeof cause === \"object\" && cause !== null && typeof cause.kind === \"string\" ? cause.kind : \"\";
  const dlq = binding === undefined ? undefined : (env as globalThis.Record<string, { send(body: unknown, options?: { contentType?: string }): globalThis.Promise<void> } | undefined>)[binding];
  if (binding !== undefined && dlq === undefined) {
    globalThis.console.error(`queue ${queue} dead-letter failed`, `the producer binding ${binding} is missing from env`);
    msg.retry();
    return;
  }
  if (dlq === undefined) {
    globalThis.console.error(`queue ${queue} undecodable`, { type, path, kind, outcome: \"dropped\" });
    __bynkReportFault(ctx, new globalThis.Error(`queue ${queue} received a message that is not a ${type} (${kind} at ${path})`));
    msg.ack();
    return;
  }
  try {
    const raw = msg.body instanceof globalThis.ArrayBuffer || globalThis.ArrayBuffer.isView(msg.body);
    await dlq.send(msg.body, { contentType: raw ? \"bytes\" : \"json\" });
  } catch (thrown) {
    globalThis.console.error(`queue ${queue} dead-letter failed`, thrown instanceof globalThis.Error ? (thrown.stack || globalThis.String(thrown)) : thrown);
    msg.retry();
    return;
  }
  globalThis.console.error(`queue ${queue} undecodable`, { type, path, kind, outcome: \"dead-lettered\" });
  msg.ack();
}
";

/// #3881: retries one queue message for a `Retry` or a `RetryAfter` verdict. A `Retry` retries with no
/// options, so the consumer's `retryDelay` (#3877) applies; a `RetryAfter` overrides it for this message
/// with `delaySeconds`, its `Duration` rounded up to whole seconds and clamped to Cloudflare's range of
/// 1 second to 24 hours. A clamp is logged (a fault would retry with no delay, the opposite of the ask).
pub(super) const RETRY_MESSAGE_HELPER: &str = "function __bynkRetryMessage(queue: string, msg: { retry(options?: { delaySeconds: number }): void }, result: { readonly tag: string; readonly delay?: number }): void {
  if (result.tag !== \"RetryAfter\" || typeof result.delay !== \"number\") {
    msg.retry();
    return;
  }
  const wanted = globalThis.Math.ceil(result.delay / 1000);
  const seconds = globalThis.Number.isNaN(wanted) ? 1 : globalThis.Math.min(86400, globalThis.Math.max(1, wanted));
  if (seconds !== wanted) globalThis.console.error(`queue ${queue} retry delay clamped`, { requestedMs: result.delay, delaySeconds: seconds });
  msg.retry({ delaySeconds: seconds });
}
";

/// The fault helpers a Worker entry carries: `__bynkReportFault`, with cron handlers (#3872)
/// `__bynkCronRun`, which reports through it, and with queue consumers (#3878) `__bynkUndecodable`.
/// One raw snippet, so the entry's text writes do not grow.
pub(super) fn fault_helpers(has_cron: bool, has_queue: bool) -> String {
    let mut all: Vec<&str> = vec![REPORT_FAULT_HELPER];
    if has_cron {
        all.push(CRON_RUN_HELPER);
    }
    if has_queue {
        all.push(UNDECODABLE_HELPER);
        all.push(RETRY_MESSAGE_HELPER);
    }
    all.join("\n")
}

/// #3562 (G2b, 2c-4a): the marker of a member's own misses. A Worker group's
/// dispatcher tries its members in turn and must tell a member that does not own
/// a request (its router's `404`, its synthesised `405`) from one that answered
/// it, including with a `404` of its own. The router's own misses are added to a
/// module-level `WeakSet`, and `isMiss` reads it; a response a handler returned
/// is never added. It is never a header, so it cannot reach a client.
///
/// The dispatcher's contract, which the group `index.ts` implements: members are
/// tried in declaration order; a marked response is a miss and the next member
/// is tried; the first unmarked response is the answer. If every member misses,
/// the answer is the first marked response that is not a `404` (the synthesised
/// `405`/`204`) with the `Allow` of every such response merged, else the
/// single-context `Not Found`. The synthesised
/// `405` is a miss on purpose: `GET /x` in one member and `POST /x` in another is
/// allowed (a route is keyed on method and path), so the `POST` must reach the
/// second member rather than be answered by the first member's `405`.
pub(super) const MISS_HELPER: &str =
    "const __misses = new globalThis.WeakSet<globalThis.Response>();
function __miss(response: globalThis.Response): globalThis.Response {
  __misses.add(response);
  return response;
}
export function isMiss(response: globalThis.Response): boolean {
  return __misses.has(response);
}
";

/// `__miss(<response>)` in a Worker group's member, `<response>` otherwise: the
/// router's own "not mine" answers are marked there ([`MISS_HELPER`]).
pub(super) fn miss(member_mode: bool, response: TsExpr) -> TsExpr {
    if member_mode {
        call(ident("__miss"), vec![response])
    } else {
        response
    }
}

/// The condition of an internal dispatch block: `path.startsWith(<prefix>)`, and,
/// in a Worker group's member, that the rest of the path is one of `names`. A
/// member answers only what it owns, and decides that before it reads the body
/// or verifies a signature, so a request meant for another member reaches that
/// member untouched.
pub(super) fn own_path_guard(member_mode: bool, prefix: &str, names: &[String]) -> TsExpr {
    let starts = method_call(ident("path"), "startsWith", vec![str_lit(prefix)]);
    if !member_mode {
        return starts;
    }
    let rest = method_call(
        ident("path"),
        "slice",
        vec![member(str_lit(prefix), "length")],
    );
    and_expr(
        starts,
        method_call(str_array(names), "includes", vec![rest]),
    )
}

/// #3932: the head of `fetch`'s fault handler, `const __busy = __agentBusyResponse(e, __route);
/// if (__busy !== null) return __busy;`: a caught `AgentBusy` answers `503` with `Retry-After`
/// and goes no further, so it is not logged or reported as a fault.
pub(super) fn agent_busy_answer() -> Vec<TsStmt> {
    vec![
        const_(
            "__busy",
            call(
                ident("__agentBusyResponse"),
                vec![ident("e"), ident("__route")],
            ),
        ),
        if_(
            binary(
                bynk_ts::TsBinaryOp::StrictNotEq,
                ident("__busy"),
                null_lit(),
            ),
            return_(Some(ident("__busy"))),
        ),
    ]
}

/// #2192: `__bynkReportFault(ctx, e);`, in an entry-point catch clause.
pub(super) fn report_fault() -> TsStmt {
    expr_stmt(call(
        ident("__bynkReportFault"),
        vec![ident("ctx"), ident("e")],
    ))
}

pub(super) fn try_catch_stmt(
    try_stmts: Vec<TsStmt>,
    catch_param: Option<&str>,
    catch_stmts: Vec<TsStmt>,
) -> TsStmt {
    TsStmt::try_catch(block(try_stmts), catch_param, block(catch_stmts), None)
}

pub(super) fn case_(test: TsExpr, body: Vec<TsStmt>) -> TsSwitchCase {
    TsSwitchCase {
        labels: vec![test],
        body,
        default_braced: false,
        // Arc E slice 6 (#1445): every real non-`default` case in this module
        // is `{ }`-blocked, same as before `case_braced` existed.
        case_braced: true,
        span: None,
    }
}

pub(super) fn default_case(body: Vec<TsStmt>) -> TsSwitchCase {
    TsSwitchCase {
        labels: vec![],
        body,
        default_braced: false,
        case_braced: true,
        span: None,
    }
}

pub(super) fn new_response(args: Vec<TsExpr>) -> TsExpr {
    new(ident("globalThis.Response"), args)
}

pub(super) fn json_stringify(expr: TsExpr) -> TsExpr {
    method_call(ident("globalThis.JSON"), "stringify", vec![expr])
}

fn status_obj(status: i64) -> TsExpr {
    TsExpr::object(vec![("status".to_string(), num_lit(status.to_string()))])
}

fn json_content_type_headers() -> (String, TsExpr) {
    // `TsObjectEntry::Prop`'s own key prints verbatim (no quoting logic in
    // the printer), so a hyphenated key must be pre-quoted by the caller —
    // the same precedent `workers.rs`'s own conversion (#1321) already set
    // for this exact header key.
    (
        "headers".to_string(),
        TsExpr::object(vec![(
            "\"content-type\"".to_string(),
            str_lit("application/json"),
        )]),
    )
}

pub(super) fn status_json_headers_obj(status: i64) -> TsExpr {
    TsExpr::object(vec![
        ("status".to_string(), num_lit(status.to_string())),
        json_content_type_headers(),
    ])
}

/// `new Response(JSON.stringify(<body>), { status: <status>, headers: {
/// "content-type": "application/json" } })` — the dominant JSON-error-
/// response shape across this module's own real content.
pub(super) fn json_response(body: TsExpr, status: i64) -> TsExpr {
    new_response(vec![json_stringify(body), status_json_headers_obj(status)])
}

pub(super) fn text_response(text: &str, status: i64) -> TsExpr {
    new_response(vec![str_lit(text), status_obj(status)])
}

/// #1969: a [`CallCheck`]'s published status, as the emitter's `i64`.
pub(super) fn check_status(check: CallCheck) -> i64 {
    i64::from(check.status())
}

/// #1969: a [`CallCheck`]'s published error `kind`. Only called for the checks
/// that answer with a JSON error body.
pub(super) fn check_kind(check: CallCheck) -> &'static str {
    check
        .kind()
        .expect("only a check with a JSON error body names a kind")
}

pub(super) fn null_response(status: i64) -> TsExpr {
    new_response(vec![null_lit(), status_obj(status)])
}

/// `{ kind: "<kind>", ...fields }` — the shared shape of every synthesised
/// boundary-error body this module builds.
pub(super) fn json_error_kind(kind: &str, fields: Vec<(String, TsExpr)>) -> TsExpr {
    let mut entries = vec![("kind".to_string(), str_lit(kind))];
    entries.extend(fields);
    TsExpr::object(entries)
}

/// `[<items>]` of string literals.
pub(super) fn str_array(items: &[String]) -> TsExpr {
    TsExpr::array(items.iter().map(|s| str_lit(s.clone())).collect())
}

/// `(why) => surface.<m>(why)`: the service's `on rejected` handler, which `__renderRejection`
/// calls with the refusal.
pub(super) fn rejection_renderer(m: &str) -> TsExpr {
    crate::ts_build::arrow(
        vec![bynk_ts::TsParam::new("why", None)],
        method_call(ident("surface"), m, vec![ident("why")]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A member tests that the service is its own before anything reads the body
    /// or verifies a signature, so a request for another member is left untouched.
    #[test]
    fn a_member_tests_ownership_in_the_guard_before_any_gate() {
        let names = vec!["a".to_string(), "b".to_string()];
        let guard = bynk_ts::print_expr(&own_path_guard(true, "/_bynk/call/", &names));
        assert_eq!(
            guard,
            "path.startsWith(\"/_bynk/call/\") && [\"a\", \"b\"].includes(path.slice(\"/_bynk/call/\".length))"
        );
    }

    /// The router's own answers are marked in a member; the marker is a call on a
    /// module-level set, never a header.
    #[test]
    fn a_member_marks_its_routers_own_answers() {
        let marked = bynk_ts::print_expr(&miss(true, text_response("Not Found", 404)));
        assert!(
            marked.starts_with("__miss(new globalThis.Response(\"Not Found\""),
            "{marked}"
        );
        assert!(MISS_HELPER.contains("new globalThis.WeakSet"));
        assert!(
            !MISS_HELPER.contains("headers"),
            "the mark is never a header"
        );
    }
}
