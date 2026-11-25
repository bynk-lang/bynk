// #3895: the W3C `traceparent` that correlates one request's logs across the
// contexts it crosses (ADR in design/pending/claude-l3-3895.md).
//
// A trace is one 32-hex-digit id. Every cross-context call and event delivery
// carries it as a `traceparent` header (`00-<trace id>-<span id>-01`), so the
// callee's logs carry the id of the request that caused them. The span id is
// minted afresh for each hop; the flags are always `01` (sampled), because Bynk
// never decides sampling.
//
// A trace must never fail a request. Every function here that reads the scope
// or parses a header answers `undefined` when there is nothing usable, and the
// caller carries on without a trace.
//
// The request scope is `AsyncLocalStorage`, found through
// `process.getBuiltinModule("node:async_hooks")` as `trace.ts` finds it, because
// a generated project has no `@types/node`. Where it is unavailable (a Worker
// without `nodejs_als` or `nodejs_compat`) the scope is empty and nothing is
// carried.

/** The header a caller sends a trace in, and a callee reads it from. */
export const TRACEPARENT_HEADER = "traceparent";

const TRACEPARENT_SHAPE = /^00-([0-9a-f]{32})-([0-9a-f]{16})-[0-9a-f]{2}$/;
const ALL_ZERO = /^0+$/;

/** A trace id, as 32 lowercase hex digits that are not all zero. */
export type TraceId = string;

interface TraceScope {
  getStore(): TraceId | undefined;
  run<R>(store: TraceId, fn: () => R): R;
}

let scope: TraceScope | undefined | null = null;

function traceScope(): TraceScope | undefined {
  if (scope === null) {
    try {
      const host = (globalThis as {
        process?: { getBuiltinModule?(id: string): unknown };
      }).process;
      const hooks = host?.getBuiltinModule?.("node:async_hooks") as
        | { AsyncLocalStorage?: new () => TraceScope }
        | undefined;
      scope = hooks?.AsyncLocalStorage === undefined ? undefined : new hooks.AsyncLocalStorage();
    } catch {
      scope = undefined;
    }
  }
  return scope;
}

function randomHex(bytes: number): string {
  const buffer = new Uint8Array(bytes);
  globalThis.crypto.getRandomValues(buffer);
  // An all-zero id is invalid, and astronomically unlikely; fix the last digit
  // rather than loop.
  if (buffer.every((b) => b === 0)) buffer[bytes - 1] = 1;
  let out = "";
  for (const b of buffer) out += (b < 16 ? "0" : "") + b.toString(16);
  return out;
}

/** A fresh trace id. */
export function mintTraceId(): TraceId {
  return randomHex(16);
}

/** The trace id in a `traceparent` header value, or `undefined` when it is absent or malformed. */
export function parseTraceparent(value: string | null | undefined): TraceId | undefined {
  if (typeof value !== "string") return undefined;
  const m = TRACEPARENT_SHAPE.exec(value);
  if (m === null) return undefined;
  if (ALL_ZERO.test(m[1]) || ALL_ZERO.test(m[2])) return undefined;
  return m[1];
}

/** A `traceparent` header value for `trace`, with a fresh span id. */
export function formatTraceparent(trace: TraceId): string {
  return `00-${trace}-${randomHex(8)}-01`;
}

/** Whether `value` is a trace id this module would mint. */
export function isTraceId(value: unknown): value is TraceId {
  return typeof value === "string" && /^[0-9a-f]{32}$/.test(value) && !ALL_ZERO.test(value);
}

/** Run `fn` with `trace` as the current request's trace; runs it plainly where there is no scope. */
export function runInTrace<R>(trace: TraceId, fn: () => R): R {
  const s = traceScope();
  return s === undefined ? fn() : s.run(trace, fn);
}

/** The current request's trace id, or `undefined` outside a trace or where there is no scope. */
export function currentTraceId(): TraceId | undefined {
  try {
    return traceScope()?.getStore();
  } catch {
    return undefined;
  }
}

/** The `traceparent` header a call or event delivery from here sends, or `undefined` outside a trace. */
export function outgoingTraceparent(): string | undefined {
  const trace = currentTraceId();
  return trace === undefined ? undefined : formatTraceparent(trace);
}

// The trace a verified inbound request carried. Only `verifyCall` records here,
// after the signature holds, so a `traceparent` from anyone who is not a
// signed Bynk or foreign caller is never read (decision C of #3895).
const admitted = new WeakMap<Request, TraceId>();

/** Record the trace a request carried; called by `verifyCall` once the request is proved. */
export function admitTrace(request: Request): void {
  const trace = parseTraceparent(request.headers.get(TRACEPARENT_HEADER));
  if (trace !== undefined) admitted.set(request, trace);
}

/** The trace an admitted request carried, or `undefined`. */
export function admittedTrace(request: Request): TraceId | undefined {
  return admitted.get(request);
}
