// `bynk dev --trace-events` (#3580, ADR 0640): per-request step recording.
//
// This module is reached only by a dev build that asked for traces, so a deploy
// build never emits it (ADR 0615: a runtime module is emitted only when emitted
// code imports it). The existing runtime modules are not edited: the emitter
// wraps the three sites it traces (a cross-context call, an agent call, a
// capability op) in `traceStep`, and wraps the generated `fetch` in
// `traceFetch`.
//
// Recording only observes. It never changes what a wrapped call returns or
// throws, and it carries no argument values (kinds, targets, spans, timings and
// outcomes only).
//
// The request scope is `AsyncLocalStorage`, found through
// `process.getBuiltinModule("node:async_hooks")` as `storage.ts` finds
// `node:sqlite`, because a generated project has no `@types/node`. The dev build
// adds the `nodejs_als` compatibility flag. Where the module is unavailable the
// request still answers and is recorded with no steps.

/** The `.bynk` call site that made a step. */
export interface TraceSpan {
  file: string;
  line: number;
  col: number;
}

export type TraceStepKind = "capability" | "call" | "agent";

export interface TraceStep {
  kind: TraceStepKind;
  target: string;
  span: TraceSpan | null;
  ms: number;
  outcome: "ok" | "fault" | "pending";
  /** A cross-context call: the id its callee's own request record will carry. */
  request?: string;
}

interface TraceRecord {
  id: string;
  /** The caller's request id, when an internal call carried one. */
  parent: string | null;
  /** The step begun most recently, which `traceBinding` reads its child id from. */
  last: TraceStep | undefined;
  steps: TraceStep[];
  truncated: boolean;
  fault: string | null;
}

/** The request id a caller gives its callee, so the callee's record carries it. */
export const TRACE_REQUEST_HEADER = "x-bynk-trace-request";
/** The caller's own request id, which the callee's record reports as its `parent`. */
export const TRACE_PARENT_HEADER = "x-bynk-trace-parent";

/** The prefix `bynk dev` recognises on a Worker's stdout (`pump_output`). */
export const TRACE_PREFIX = "__bynk_trace:";

/** Steps kept per request; a request that makes more sets `truncated`. */
export const TRACE_STEP_LIMIT = 200;

interface ScopeStore {
  getStore(): TraceRecord | undefined;
  run<R>(store: TraceRecord, fn: () => R): R;
}

let scope: ScopeStore | undefined | null = null;

function requestScope(): ScopeStore | undefined {
  if (scope === null) {
    const host = (globalThis as {
      process?: { getBuiltinModule?(id: string): unknown };
    }).process;
    const hooks = host?.getBuiltinModule?.("node:async_hooks") as
      | { AsyncLocalStorage?: new () => ScopeStore }
      | undefined;
    scope = hooks?.AsyncLocalStorage === undefined ? undefined : new hooks.AsyncLocalStorage();
  }
  return scope;
}

function now(): number {
  return globalThis.performance.now();
}

/**
 * Begin one step on the current request and return the function that settles
 * it: `traceStep(kind, target, span)(call)`. The emitter wraps a call
 * expression as the argument, so its arguments evaluate (and may contain
 * `await`) exactly where they did. The step begins, and its clock starts, when
 * the wrapped expression starts evaluating, so steps are in the order their
 * calls begin. An effectful call is bound with `<-` and so is hoisted out of
 * argument position; an argument that were itself a traced call would list after
 * the call it is an argument to, and be inside its timing.
 *
 * A result that is a promise is timed to its settlement and returned as a
 * derived promise, so a wrapped site must be awaited as it is (an RPC stub's
 * pipelining and disposal are not carried over). The value or error passes
 * through untouched. A step still unsettled when the response is ready, or whose
 * call or arguments threw synchronously, is reported as `pending`. Outside a
 * traced request this returns the identity.
 */
export function traceStep(
  kind: TraceStepKind,
  target: string,
  span: TraceSpan | null,
): <T>(result: T) => T {
  const record = scope === null || scope === undefined ? undefined : scope.getStore();
  if (record === undefined) return (result) => result;
  const started = now();
  const step: TraceStep = { kind, target, span, ms: 0, outcome: "pending" };
  if (record.steps.length >= TRACE_STEP_LIMIT) {
    record.truncated = true;
    record.last = undefined;
    return (result) => result;
  }
  if (kind === "call") step.request = globalThis.crypto.randomUUID();
  record.steps.push(step);
  record.last = step;
  const settle = (outcome: "ok" | "fault"): void => {
    step.ms = now() - started;
    step.outcome = outcome;
  };
  return <T>(result: T): T => {
    const then = (result as { then?: unknown } | null | undefined)?.then;
    if (typeof then !== "function") {
      settle("ok");
      return result;
    }
    return (result as unknown as PromiseLike<unknown>).then(
      (value) => {
        settle("ok");
        return value;
      },
      (e: unknown) => {
        settle("fault");
        throw e;
      },
    ) as unknown as T;
  };
}

/**
 * A Service Binding whose requests carry this request's id and the id its call
 * step gave the callee, so the callee's record names its parent. The emitter
 * wraps the binding it hands `callService` in a traced build, so `boundary.ts` is
 * untouched; the signature `callService` adds does not cover these headers. Outside
 * a traced request the binding is used as it is.
 */
export function traceBinding<B extends { fetch(request: Request): Promise<Response> }>(binding: B): B {
  const record = scope === null || scope === undefined ? undefined : scope.getStore();
  if (record === undefined) return binding;
  const child = record.last?.kind === "call" ? record.last.request : undefined;
  const parent = record.id;
  return {
    fetch(request: Request): Promise<Response> {
      const headers = new globalThis.Headers(request.headers);
      headers.set(TRACE_PARENT_HEADER, parent);
      if (child !== undefined) headers.set(TRACE_REQUEST_HEADER, child);
      return binding.fetch(new globalThis.Request(request, { headers }));
    },
  } as B;
}

/** Record the fault the generated `fetch` caught, so the request's line carries it. */
export function traceFault(error: unknown): void {
  const record = scope === null || scope === undefined ? undefined : scope.getStore();
  if (record !== undefined) traceFaultOn(record, error);
}

/**
 * Serve one request under a record and print it as a `__bynk_trace:` line when
 * the response is ready. A rejection from `inner` is recorded and rethrown.
 */
export async function traceFetch<E, C>(
  context: string,
  inner: (request: Request, env: E, ctx: C) => Promise<Response>,
  request: Request,
  env: E,
  ctx: C,
): Promise<Response> {
  const als = requestScope();
  const record: TraceRecord = {
    id: request.headers.get(TRACE_REQUEST_HEADER) ?? globalThis.crypto.randomUUID(),
    parent: request.headers.get(TRACE_PARENT_HEADER),
    last: undefined,
    steps: [],
    truncated: false,
    fault: null,
  };
  const started = now();
  const url = new globalThis.URL(request.url);
  const flush = (status: number): void => {
    globalThis.console.log(
      TRACE_PREFIX +
        JSON.stringify({
          context,
          id: record.id,
          parent: record.parent,
          route: `${request.method} ${url.pathname}`,
          status,
          duration_ms: now() - started,
          steps: record.steps,
          truncated: record.truncated,
          fault: record.fault,
        }),
    );
  };
  try {
    const response = await (als === undefined ? inner(request, env, ctx) : als.run(record, () => inner(request, env, ctx)));
    flush(response.status);
    return response;
  } catch (e) {
    traceFaultOn(record, e);
    flush(500);
    throw e;
  }
}

function traceFaultOn(record: TraceRecord, error: unknown): void {
  record.fault =
    error instanceof globalThis.Error ? (error.stack ?? globalThis.String(error)) : globalThis.String(error);
}
