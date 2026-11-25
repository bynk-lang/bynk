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

// The scope's store is a box, so an entry point can start a trace and the
// admission of a signed call can then replace it with the caller's.
interface TraceBox {
  trace: TraceId;
}

interface TraceScope {
  getStore(): TraceBox | undefined;
  run<R>(store: TraceBox, fn: () => R): R;
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
  return s === undefined ? fn() : s.run({ trace }, fn);
}

/** The current request's trace id, or `undefined` outside a trace or where there is no scope. */
export function currentTraceId(): TraceId | undefined {
  try {
    return traceScope()?.getStore()?.trace;
  } catch {
    return undefined;
  }
}

// The trace id Cloudflare gives the current invocation, when it gives one
// (`tracing.getActiveSpan()?.spanContext().traceId`, which matches the one on
// the invocation's logs). It is read through a dynamic import of a computed
// specifier, so a Node, browser or test build never resolves
// `cloudflare:workers`. Only the id is used: Cloudflare's `traceFlags` is not
// a sampling decision, so it is never copied into a `traceparent`.
let platformModule: Promise<unknown> | undefined;

async function platformTraceId(): Promise<TraceId | undefined> {
  try {
    const specifier = "cloudflare:workers";
    platformModule ??= import(specifier).catch(() => undefined);
    const mod = (await platformModule) as
      | { tracing?: { getActiveSpan?(): { spanContext?(): { traceId?: unknown } } | undefined } }
      | undefined;
    const id = mod?.tracing?.getActiveSpan?.()?.spanContext?.().traceId;
    return isTraceId(id) ? id : undefined;
  } catch {
    return undefined;
  }
}

/**
 * Run an entry point's work inside a new trace (decision B of #3895): the
 * platform's own trace id when it gives one, otherwise a random one. A public
 * HTTP request, a cron run, a queue batch and an email each start here; a signed
 * call or event delivery starts here too and `verifyCall` then replaces the
 * trace with the one its caller carried. Where there is no request scope this
 * runs `fn` as it is.
 */
export async function enterTrace<R>(fn: () => Promise<R>): Promise<R> {
  const s = traceScope();
  if (s === undefined) return fn();
  return s.run({ trace: (await platformTraceId()) ?? mintTraceId() }, fn);
}

/** `message`, followed by ` trace=<id>` inside a trace: how the runtime's own log lines carry the trace. */
export function withTrace(message: string): string {
  const trace = currentTraceId();
  return trace === undefined ? message : `${message} trace=${trace}`;
}

/** `fields` with the current trace under `trace`, or as they are outside a trace. The trace wins over a field of the same name. */
export function withTraceField<F extends Record<string, unknown>>(fields: F): F | (F & { trace: TraceId }) {
  const trace = currentTraceId();
  return trace === undefined ? fields : { ...fields, trace };
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
  if (trace === undefined) return;
  admitted.set(request, trace);
  // The request is now in its caller's trace.
  try {
    const box = traceScope()?.getStore();
    if (box !== undefined) box.trace = trace;
  } catch {
    // A trace never fails a request.
  }
}

/** The trace an admitted request carried, or `undefined`. */
export function admittedTrace(request: Request): TraceId | undefined {
  return admitted.get(request);
}
