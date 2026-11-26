import type { Result } from "./result.ts";
import type { Option } from "./result.ts";
import { Ok, Err, Some, None } from "./result.ts";
import type { ValidationBound, ValidationError, ValidationRule, WireViolation } from "./errors.ts";
import { __bynkDecimalToString } from "./decimal.ts";
import { W3C_TRACEPARENT_HEADER, admitTrace, formatTraceparent, isTraceId, outgoingTraceparent, withTrace } from "./traceparent.ts";

// v0.8: cross-Worker boundary protocol — JSON wire format and error types.

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [k: string]: JsonValue };

export type BoundaryError =
  | { readonly kind: "MalformedJson"; readonly details: string }
  | {
      readonly kind: "StructuralMismatch";
      readonly path: string;
      readonly expected: string;
      readonly actual: string;
    }
  | {
      readonly kind: "RefinementViolation";
      readonly path: string;
      readonly violation: ValidationError | WireViolation;
    }
  | { readonly kind: "Transport"; readonly status: number; readonly details: string };

// #3952 (Decision D): a refinement violation as it goes on the wire in a `400`: the field,
// the message and the rule, never the offending value. A rejected password failing
// `MinLength` must not be echoed back, so `value` stays runtime-only. Applied to the body of
// every `RefinementViolation` answer; any other body passes through untouched.
//
// The rule is written as every sum is on the wire, with `kind` in place of the in-memory
// `tag`; a `Decimal` bound is its canonical text and a `Duration` its milliseconds.
function wireBound(b: ValidationBound): Record<string, unknown> {
  return b.tag === "DecimalBound"
    ? { kind: b.tag, value: __bynkDecimalToString(b.value) }
    : { kind: b.tag, value: b.value };
}

function wireRule(r: ValidationRule): Record<string, unknown> {
  switch (r.tag) {
    case "InRange":
      return { kind: r.tag, lo: wireBound(r.lo), hi: wireBound(r.hi) };
    case "AtLeast":
    case "AtMost":
      return { kind: r.tag, bound: wireBound(r.bound) };
    default: {
      const { tag, ...rest } = r;
      return { kind: tag, ...rest };
    }
  }
}

export function wireRefusal(body: Record<string, unknown>): Record<string, unknown> {
  const violation = body.violation as ValidationError | undefined;
  if (body.kind !== "RefinementViolation" || violation === undefined) return body;
  return {
    ...body,
    violation: { field: violation.field, message: violation.message, rule: wireRule(violation.rule) },
  };
}

// v0.177 (#643): the deployed callee's compiled contract is not the one this
// caller was compiled against — a deploy skew, not a bad payload. The callee
// reports it as a 409 *before* reading the body, since once the contracts
// disagree the body's interpretation is exactly what is in doubt.
//
// Deliberately **not** a `BoundaryError` variant. `BoundaryError` is the
// *codec's* error domain — what deserialising a payload can conclude — and a
// codec can never produce this: it is decided from a header before any codec
// runs. Widening `BoundaryError` would oblige every consumer of a codec result
// (`Json.decode`'s error mapping among them) to narrow a case it can never
// observe. The call surface is wider than the codec surface, so it gets its own
// type.
export interface ContractMismatch {
  readonly kind: "ContractMismatch";
  readonly service: string;
  readonly expected: string;
  readonly actual: string | null;
}

/// #2193: the callee's handler faulted. It answered the bare `500` every fault
/// answers, marked with [`FAULT_HEADER`] naming its context, so the caller
/// reports which Worker's log holds the cause instead of a `Transport`
/// failure that reads like an unreachable callee. Like `ContractMismatch`, a
/// call-surface error, never a codec result. The error itself stays in the
/// callee's log: the response carries only the callee's name.
export interface CalleeFaulted {
  readonly kind: "CalleeFaulted";
  readonly context: string;
  readonly service: string;
  readonly status: number;
}

// #3894: the shape of the `bynk` adapter's `CallOutcome[R]`, which the generated
// code declares: the runtime builds one structurally.
export type CallOutcome<R> =
  | { readonly tag: "Handled"; readonly result: R }
  | { readonly tag: "Rejected"; readonly reason: { readonly tag: string } }
  | { readonly tag: "Faulted" }
  | { readonly tag: "Unreachable"; readonly status: Option<number> };

/// #2193: the header a callee's fault response names its context in.
export const FAULT_HEADER = "X-Bynk-Fault";

/// Everything a cross-context call can fail with: the codec's domain, plus the
/// skew check that precedes it and a callee's fault.
export type CallError = BoundaryError | ContractMismatch | CalleeFaulted;

/// #2193: the `500` a Worker entry answers when a handler faults. A fault in an
/// `on call` service (`route` is `call <service>`) is marked with
/// [`FAULT_HEADER`], so the calling Worker's `callService` can name it; any
/// other route's `500` is unmarked, as before.
export function faultResponse(context: string, route: string): Response {
  const headers: Record<string, string> = route.startsWith("call ") ? { [FAULT_HEADER]: context } : {};
  return new Response("Internal Server Error", { status: 500, headers });
}

export function boundaryError(error: CallError): Error {
  const e = new Error(`BoundaryError: ${error.kind}`) as Error & {
    boundaryError?: CallError;
  };
  // #1825 review: non-enumerable, so a logged error (`console.error(..., e)`
  // in a Worker's fault catch) doesn't print the payload, which can carry a
  // value: a callee's response body, a refinement's offending value. Reading
  // `e.boundaryError` is unchanged.
  globalThis.Object.defineProperty(e, "boundaryError", {
    value: error,
    enumerable: false,
    writable: true,
    configurable: true,
  });
  return e;
}

// v0.96 (ADR 0124): an agent's persisted state failed validation on rehydration —
// a refined field, key, or entry no longer satisfies the current type definition
// (schema corruption, or a refinement that tightened across a deploy, orphaning
// previously-valid data). The load-time twin of InvariantViolation: a dedicated
// internal fault, NOT a caller-facing BoundaryError, because the supplier is
// trusted past-self, not the untrusted caller (Q6). It reuses the boundary
// validator's *detection* (the BoundaryError detail) but disposes of it as a
// fault. Logged with the agent type and field path only — never the key or the
// offending value (ADR 0107 logging discipline).
export interface RehydrationViolation {
  readonly kind: "RehydrationViolation";
  readonly agent: string;
  readonly path: string;
  readonly detail: BoundaryError | SchemaVersionMismatch | MigrationFailed | LayoutMismatch;
}

// State-migrations S2 (#2224 Decision D): a `migrate from N` step threw while
// running (for example, an `Int` division by zero). Only the step's version is
// recorded — never the thrown value, which can carry stored data.
export interface MigrationFailed {
  readonly kind: "MigrationFailed";
  readonly from: number;
}

// State-migrations S1 (#2211): a stored record's `__v` schema-version marker
// does not let this build read it. `NewerSchema`: a newer build wrote it (the
// deploy was rolled back). `NoMigrationPath`: it is malformed, or older than
// the oldest version this build reads forward without a migration step.
// `stored` is null when the marker is not a positive integer. Version numbers
// are not user data, so they are logged.
export interface SchemaVersionMismatch {
  readonly kind: "NewerSchema" | "NoMigrationPath";
  readonly stored: number | null;
  readonly current: number;
  readonly oldestReadable: number;
}

// State-migrations S1 (#2211, track §3.2): check a raw stored record's `__v`
// marker before anything is decoded or merged (Decision D). An absent marker
// is version 1, the lock's baseline. A record at a version from
// `oldestReadable` to `current` reads by zero-merge (ADR 0124 D4); anything
// newer is `NewerSchema`, anything else `NoMigrationPath` — both thrown as a
// `RehydrationViolation`, so a faulting record is never decoded or written
// back.
//
// State-migrations S2 (#2224 Decision C): `steps` lists each `migrate from N`
// step's covered range as `[lo, N]`; a version inside one is readable too.
export function checkStoredSchemaVersion(
  agent: string,
  stored: object,
  current: number,
  oldestReadable: number,
  steps: readonly (readonly [number, number])[] = [],
): void {
  const raw = (stored as { readonly __v?: unknown }).__v;
  const v = raw === undefined ? 1 : raw;
  const valid = typeof v === "number" && globalThis.Number.isInteger(v) && v >= 1;
  if (valid && v <= current && v >= oldestReadable) return;
  if (valid && steps.some(([lo, n]) => v >= lo && v <= n)) return;
  throw rehydrationViolation(agent, {
    kind: valid && v > current ? "NewerSchema" : "NoMigrationPath",
    stored: valid ? v : null,
    current,
    oldestReadable,
  });
}

// Per-entry storage slice 0 (#3189, track `per-entry-storage.md` §6): a stored
// record's `__layout` marker names a storage layout this build cannot read. No
// layout marker is defined yet: an absent marker is today's wholesale layout,
// so ANY `__layout` was written by a later build (the deploy was rolled back).
// Reading it would zero-merge the collection fields it no longer holds inline
// and write them back wholesale, orphaning every per-entry key, so it faults
// before anything is decoded, merged or written back. `stored` is the marker
// when it is a positive integer, else null (not user data, so it is logged).
export interface LayoutMismatch {
  readonly kind: "NewerLayout";
  readonly stored: number | null;
}

// Per-entry storage slice 0 (#3189): check a raw stored record's `__layout`
// marker before anything else reads it (before the `__v` check, which a
// lockless agent does not run). Absent is the wholesale layout this build
// reads; a marker this build knows (the wholesale layout 1, which is never
// written, and the per-entry layout 2) passes; any other present marker is
// `NewerLayout`, thrown as a `RehydrationViolation` so a faulting record is
// never decoded or written back. A slice that teaches a build a layout widens
// the set this accepts.
const KNOWN_LAYOUTS: readonly number[] = [1, 2];
export function checkStoredLayout(agent: string, stored: object): void {
  const raw = (stored as { readonly __layout?: unknown }).__layout;
  if (raw === undefined || (typeof raw === "number" && KNOWN_LAYOUTS.includes(raw))) return;
  throw rehydrationViolation(agent, {
    kind: "NewerLayout",
    stored: typeof raw === "number" && globalThis.Number.isInteger(raw) && raw >= 1 ? raw : null,
  });
}

// State-migrations S1 (#2211, track §3.4): warn, once per isolate per
// (agent, key), about a stored key the current definition does not declare — a
// field renamed or removed before the schema lock recorded it. The key is kept
// and written back, never deleted. `zero` is the agent's fresh record, so every
// key the toolchain itself keeps in state counts as declared. Logs the key's
// name only, never its value (ADR 0107).
const reportedUnknownStoredKeys = new globalThis.Set<string>();
export function reportUnknownStoredKeys(agent: string, stored: object, zero: object): void {
  for (const key of globalThis.Object.keys(stored)) {
    if (key === "__v" || globalThis.Object.hasOwn(zero, key)) continue;
    // An `@indexed` posting record of a legacy wholesale state: the map's handle rebuilds its postings.
    const idx = key.indexOf("__idx_");
    if (idx > 0 && globalThis.Object.hasOwn(zero, key.slice(0, idx))) continue;
    // A held `Map`'s insertion order, stored beside its record (`<field>:order`: no identifier has a colon).
    if (key.endsWith(":order") && globalThis.Object.hasOwn(zero, key.slice(0, -":order".length))) continue;
    const id = `${agent}\u0000${key}`;
    if (reportedUnknownStoredKeys.has(id)) continue;
    reportedUnknownStoredKeys.add(id);
    globalThis.console.warn(
      `bynk: agent ${agent} has stored field "${key}" the current definition does not declare; it is kept, not read`,
    );
  }
}

// #3793: `store new: Cell[T] replaces old` reads a record stored under `old` as `new`. Each
// `[old, new]` pair moves `old`'s value to `new` unless the record already holds `new`, so a value
// the new build wrote is never replaced by a stale one; `old` is dropped, so the next commit writes
// only `new`. Returns `stored` itself when no pair applies, and a copy otherwise (the record read
// from storage is never mutated). Pairs are most-recent-name first, so a field renamed twice is
// read from whichever of its names the record holds.
export function renameStoredFields(
  stored: object,
  renames: readonly (readonly [string, string])[],
): object {
  const has = (key: string): boolean => globalThis.Object.hasOwn(stored, key);
  if (!renames.some(([old, name]) => has(old) && old !== name)) return stored;
  const out: { [key: string]: unknown } = { ...(stored as { [key: string]: unknown }) };
  for (const [old, name] of renames) {
    if (!globalThis.Object.hasOwn(out, old) || old === name) continue;
    if (!globalThis.Object.hasOwn(out, name)) out[name] = out[old];
    delete out[old];
  }
  return out;
}

// State-migrations S2 (#2224 Decision D): the fault a step that throws
// becomes. The thrown value is discarded, not logged: it can carry stored data.
export function migrationFailed(agent: string, from: number): Error {
  return rehydrationViolation(agent, { kind: "MigrationFailed", from });
}

export function rehydrationViolation(
  agent: string,
  detail: BoundaryError | SchemaVersionMismatch | MigrationFailed | LayoutMismatch,
): Error {
  const path = "path" in detail ? detail.path : "<root>";
  // #1827: logged here, where every load-time gate builds it, as the
  // `InvariantViolation` line is logged before its throw: the agent, the field
  // path and the kind of failure. Never `detail` itself, which can carry the
  // offending value (a `RefinementViolation`'s `violation.value`). A schema
  // version mismatch carries only version numbers, which are logged.
  globalThis.console.error(
    `RehydrationViolation ${agent}`,
    "current" in detail
      ? { agent, path, kind: detail.kind, stored: detail.stored, current: detail.current }
      : detail.kind === "NewerLayout"
        ? { agent, path, kind: detail.kind, stored: detail.stored }
        : "from" in detail
        ? { agent, path, kind: detail.kind, from: detail.from }
        : { agent, path, kind: detail.kind },
  );
  const e = new Error(`RehydrationViolation: ${agent} ${detail.kind} at ${path}`);
  // #1825 review: non-enumerable, as `boundaryError`'s, so a logged error
  // doesn't print `detail` (it can carry the offending value).
  globalThis.Object.defineProperty(e, "rehydrationViolation", {
    value: {
      kind: "RehydrationViolation",
      agent,
      path,
      detail,
    } satisfies RehydrationViolation,
    enumerable: false,
    writable: true,
    configurable: true,
  });
  return e;
}

export interface ServiceBinding {
  fetch(request: Request): Promise<Response>;
}

// I2 (#1971, ADR `per-edge-call-signature`, amending ADR 0092): every
// internal request (`/_bynk/call/`, `/_bynk/event/`) is signed with
// HMAC-SHA256 under a key one caller shares with one callee, and the callee
// refuses a request whose signature does not verify before it reads anything
// else. The path prefixes are reachable from the public internet on any route
// the Worker serves, so the prefix alone proves nothing about the sender, and
// one key per edge means a key's holder can claim only its own name. Bynk's
// emitter holds the same values in `bynk_check::wire_format`, and
// `bynkc/tests/wire_reference.rs` fails if the two disagree.
export const CALLER_HEADER = "X-Bynk-Caller";
export const TIMESTAMP_HEADER = "X-Bynk-Timestamp";
export const SIGNATURE_HEADER = "X-Bynk-Signature";
export const SIGNATURE_WINDOW_SECONDS = 300;
export const MAX_BODY_BYTES = 1048576;
export const SIGNING_SCHEME = "BYNK-CALL-1";
export const EDGE_FROM_PREFIX = "BYNK_CALL_FROM_";
export const EDGE_TO_PREFIX = "BYNK_CALL_TO_";

// A context name an edge key can be derived from: dot-separated segments of
// lowercase letters and digits, starting with a letter, with single
// underscores between runs. On these names `edgeSuffix` is injective, so one
// caller's key never verifies another caller's name.
const EDGE_NAME = /^[a-z][a-z0-9]*(?:_[a-z0-9]+)*(?:\.[a-z][a-z0-9]*(?:_[a-z0-9]+)*)*$/;

// `shop.orders` → `SHOP__ORDERS`, or `undefined` outside the domain above.
export function edgeSuffix(context: string): string | undefined {
  return EDGE_NAME.test(context) ? context.toUpperCase().split(".").join("__") : undefined;
}

// The edge key under `prefix` + the context's suffix in a Worker's `env`, or
// `undefined` when the name is outside the domain, or the key unset or not a
// non-empty string. `env` is `unknown` because the keys are secrets, not
// declared bindings, so no generated `Env` interface names them (the actor
// auth secrets read theirs the same way).
export function edgeKeyOf(env: unknown, prefix: string, context: string): string | undefined {
  const suffix = edgeSuffix(context);
  if (suffix === undefined || typeof env !== "object" || env === null) return undefined;
  const value = (env as Record<string, unknown>)[prefix + suffix];
  return typeof value === "string" && value !== "" ? value : undefined;
}

// I3 (#1971): a foreign caller's secret, by its declared name, from `env`.
function foreignKeyOf(env: unknown, name: string): string | undefined {
  if (typeof env !== "object" || env === null) return undefined;
  const value = (env as Record<string, unknown>)[name];
  return typeof value === "string" && value !== "" ? value : undefined;
}

// The fields a signature covers, one per line (`signing_input` in
// `bynk_check::wire_format`). Every header the callee acts on is here; an
// absent contract is signed as empty and an absent wire version as `1`.
export interface SignedFields {
  caller: string;
  callee: string;
  method: string;
  path: string;
  contract: string;
  wire: string;
  timestamp: string;
  bodyDigest: string;
}

export function signingInput(f: SignedFields): string {
  return [SIGNING_SCHEME, f.caller, f.callee, f.method, f.path, f.contract, f.wire, f.timestamp, f.bodyDigest].join("\n");
}

function hex(bytes: ArrayBuffer): string {
  return Array.from(new Uint8Array(bytes), (b) => b.toString(16).padStart(2, "0")).join("");
}

async function sha256Hex(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  return hex(await crypto.subtle.digest("SHA-256", bytes));
}

function hmacKey(secret: string, usage: "sign" | "verify"): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [usage]);
}

// The signature a caller holding `key` sends over `fields`.
export async function signature(key: string, fields: SignedFields): Promise<string> {
  return hex(await crypto.subtle.sign("HMAC", await hmacKey(key, "sign"), new TextEncoder().encode(signingInput(fields))));
}

// #3895: the `traceparent` header a call sends from inside a trace, and none
// outside one. It is not part of the signing input: it is observability
// metadata, and a callee reads it only from a request that has already proved
// its caller.
function traceHeaders(): Record<string, string> {
  const traceparent = outgoingTraceparent();
  return traceparent === undefined ? {} : { [W3C_TRACEPARENT_HEADER]: traceparent };
}

// #3895: the header an event delivery sends, from the envelope's `trace` alone
// and never from the request scope. The fan-out Durable Object delivers outside
// the emitting request, and the handler reads the same trace as `env.trace`, so
// the header and the envelope cannot disagree.
function eventTraceHeaders(envelope: unknown): Record<string, string> {
  const trace = (envelope as { trace?: unknown } | null)?.trace;
  return isTraceId(trace) ? { [W3C_TRACEPARENT_HEADER]: formatTraceparent(trace) } : {};
}

// The sending side: the headers that prove `caller` sent `body` to `callee`.
// Throws, naming the unset key, when the Worker holds none for the edge:
// sending without it would only come back as an opaque 401.
export async function signedHeaders(
  env: unknown,
  caller: string,
  callee: string,
  path: string,
  contract: string,
  // The wire version as sent; `"1"` for a request that sends no header.
  wire: string,
  body: string,
  what: string,
): Promise<Record<string, string>> {
  const key = edgeKeyOf(env, EDGE_TO_PREFIX, callee);
  if (key === undefined) {
    throw new Error(
      `${EDGE_TO_PREFIX}${edgeSuffix(callee) ?? callee} is not set, so ${what} cannot be sent: \`bynk deploy\` sets one key per calling edge, and \`bynk dev\` passes a local one.`,
    );
  }
  const timestamp = String(Math.floor(Date.now() / 1000));
  const bodyDigest = await sha256Hex(new TextEncoder().encode(body));
  const sig = await signature(key, { caller, callee, method: "POST", path, contract, wire, timestamp, bodyDigest });
  return { [CALLER_HEADER]: caller, [TIMESTAMP_HEADER]: timestamp, [SIGNATURE_HEADER]: sig };
}

// The body's bytes, read under `cap` (`MAX_BODY_BYTES` for the internal
// dispatch; a route's effective `maxBody` for `from http`), or `undefined` once
// it is past the cap: a declared length is refused before reading, and a
// streamed body is cancelled as soon as it crosses the cap, so an
// unauthenticated sender cannot make the callee buffer an unbounded body. The
// count is of the bytes actually read, so a chunked body, or one whose
// `Content-Length` understates it, is cut off all the same.
export async function readCapped(
  request: Request,
  cap: number = MAX_BODY_BYTES,
): Promise<Uint8Array<ArrayBuffer> | undefined> {
  const declared = request.headers.get("content-length");
  if (declared !== null && Number(declared) > cap) return undefined;
  if (request.body === null) return new Uint8Array(0);
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > cap) {
      await reader.cancel();
      return undefined;
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) {
    bytes.set(c, at);
    at += c.byteLength;
  }
  return bytes;
}

const SIGNATURE_SHAPE = /^[0-9a-f]{64}$/;
const TIMESTAMP_SHAPE = /^[0-9]{1,15}$/;

// The receiving side, the first thing every internal dispatch does: the body
// as text when `request` is signed for `callee` by a caller this Worker holds
// a key for, within the window; otherwise the refusal to return, a bare 413
// or 401. Fails closed: a caller with no key here (an undeclared or forged
// one, or any caller when no key is set) is refused. `crypto.subtle.verify`
// compares in constant time.
//
// I3 (#1971): `foreign` maps each foreign caller the callee admits (a
// `Foreign` actor's name, which it sends as its caller) to the secret it signs
// under. Such a name is never a context's (an actor name starts with a capital
// letter, which `edgeSuffix` refuses), so a foreign caller is checked under
// its own secret and a Bynk caller under its edge key, and neither can pass
// for the other. Looked up with `Object.hasOwn`, never plain indexing, so a
// caller named after an `Object.prototype` member finds nothing.
export async function verifyCall(
  request: Request,
  env: unknown,
  callee: string,
  foreign: Record<string, string> = {},
): Promise<string | Response> {
  const bytes = await readCapped(request);
  if (bytes === undefined) return new Response("Payload Too Large", { status: 413 });
  const refused = new Response("Unauthorized", { status: 401 });
  const caller = request.headers.get(CALLER_HEADER) ?? "";
  const key = Object.hasOwn(foreign, caller) ? foreignKeyOf(env, foreign[caller]) : edgeKeyOf(env, EDGE_FROM_PREFIX, caller);
  if (key === undefined) return refused;
  const timestamp = request.headers.get(TIMESTAMP_HEADER) ?? "";
  if (!TIMESTAMP_SHAPE.test(timestamp)) return refused;
  if (Math.abs(Date.now() / 1000 - Number(timestamp)) > SIGNATURE_WINDOW_SECONDS) return refused;
  const given = request.headers.get(SIGNATURE_HEADER) ?? "";
  if (!SIGNATURE_SHAPE.test(given)) return refused;
  const sig = new Uint8Array(32);
  for (let i = 0; i < 32; i++) sig[i] = parseInt(given.slice(2 * i, 2 * i + 2), 16);
  const fields: SignedFields = {
    caller,
    callee,
    method: request.method,
    path: new URL(request.url).pathname,
    contract: request.headers.get("X-Bynk-Contract") ?? "",
    wire: request.headers.get(WIRE_VERSION_HEADER) ?? "1",
    timestamp,
    bodyDigest: await sha256Hex(bytes),
  };
  const ok = await crypto.subtle.verify(
    "HMAC",
    await hmacKey(key, "verify"),
    sig,
    new TextEncoder().encode(signingInput(fields)),
  );
  if (!ok) return refused;
  // #3895: only a request that proved its caller is read for a trace.
  admitTrace(request);
  return new TextDecoder().decode(bytes);
}

// #1969: the published cross-context wire format's version
// (`reference/wire-format.md`). A caller states the version it speaks; a callee
// accepts a range of them and treats an absent header as version 1. Bynk's
// emitter holds the same values in `bynk_check::wire_format`, and
// `bynkc/tests/wire_reference.rs` fails if the two disagree.
export const WIRE_VERSION_HEADER = "X-Bynk-Wire";
export const WIRE_VERSION = "1";

// What one call came to before it is a return value or a thrown fault: shared
// by `callService` (which throws) and `callServiceOutcome` (which reports it as
// a `CallOutcome`).
type CallRaw<R> =
  | { readonly kind: "ok"; readonly value: R }
  | { readonly kind: "error"; readonly error: CallError }
  | { readonly kind: "thrown"; readonly thrown: unknown };

async function callServiceCore<R>(
  binding: ServiceBinding,
  servicePath: string,
  argsJson: JsonValue,
  // #988: generic over the whole return type `R`, not a `Result`'s two
  // halves — `R = Result<T, E>` recovers the old behaviour exactly, but a
  // plain (non-`Result`) `R` now has something to bind `deserialiseResult`'s
  // inference to, instead of degrading to `unknown`. The `path` parameter is
  // optional here because every generated `deserialise_*` declares it with a
  // default, which callers of this function are not required to match.
  deserialiseResult: (json: JsonValue, path?: string) => Result<R, BoundaryError>,
  // v0.54: the calling context's qualified name, stamped beside the args so the
  // callee's `by c: Caller` handler can present a live `CallerId` (Q7). A
  // compile-time constant; the args body itself is unchanged. The name is only a
  // claim until the request's per-edge signature (ADR 0485) proves it: the
  // callee checks the signature, then admits the proven caller.
  callerContext: string = "",
  // v0.177 (#643): this caller's compiled hash of the callee's contract. A
  // compile-time constant, stamped beside the caller identity as metadata — the
  // args body is untouched. The callee compares it against its own constant and
  // fails closed on mismatch (409), which is what makes a `deploy --context`
  // skew a loud, nameable failure instead of a silent misinterpretation.
  contractHash: string = "",
  // I2 (#1971): the calling Worker's `env`, which holds the key for the edge
  // to `calleeContext` ([`EDGE_TO_PREFIX`]), and the callee's qualified name,
  // which the signature binds the request to.
  env: unknown = undefined,
  calleeContext: string = "",
): Promise<CallRaw<R>> {
  const body = JSON.stringify(argsJson);
  const path = `/_bynk/call/${servicePath}`;
  const signed = await signedHeaders(env, callerContext, calleeContext, path, contractHash, WIRE_VERSION, body, `the call to ${servicePath}`);
  const request = new Request(`http://internal/_bynk/call/${servicePath}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "X-Bynk-Caller": callerContext,
      "X-Bynk-Contract": contractHash,
      [WIRE_VERSION_HEADER]: WIRE_VERSION,
      ...traceHeaders(),
      ...signed,
    },
    body,
  });
  let response: Response;
  try {
    response = await binding.fetch(request);
  } catch (thrown) {
    return { kind: "thrown", thrown };
  }
  if (!response.ok) {
    // Read the body **once**: the stream is consumed on first read, so a second
    // read throws `TypeError: Body is unusable` — which would replace the very
    // diagnosis this increment exists to give with an opaque failure from inside
    // the runtime.
    const raw = await response.text();
    // v0.177 (#643): a 409 from the internal boundary is the callee refusing a
    // skewed contract. Surface it as the named `ContractMismatch` rather than a
    // generic transport failure — the whole point of failing closed is that the
    // operator learns *what* is wrong, and "409 with an opaque body" would bury
    // it. Anything else — including a 409 that is not ours, or one whose body
    // does not parse — stays a `Transport`.
    if (response.status === 409) {
      let detail: ContractMismatch | null = null;
      try {
        detail = JSON.parse(raw) as ContractMismatch;
      } catch {
        // Not a Bynk 409; fall through to `Transport`.
      }
      if (detail && detail.kind === "ContractMismatch") {
        // #1826: logged on the caller's side too, with both hashes, before the
        // fault: the caller's catch reports only `BoundaryError: ContractMismatch`.
        globalThis.console.error(withTrace(`ContractMismatch ${callerContext} -> ${servicePath}`), detail);
        return { kind: "error", error: detail };
      }
    }
    // #2193: a marked 500 is the callee's own fault. Logged here too, naming
    // the callee, so the caller's tail says which Worker's log to read. The
    // header is bounded in the log, as #1826 bounds the contract hash.
    const faulted = response.status === 500 ? response.headers.get(FAULT_HEADER) : null;
    if (faulted !== null) {
      const detail: CalleeFaulted = {
        kind: "CalleeFaulted",
        context: faulted.slice(0, 128),
        service: servicePath,
        status: response.status,
      };
      globalThis.console.error(withTrace(`CalleeFaulted ${callerContext} -> ${detail.context} call ${servicePath}`));
      return { kind: "error", error: detail };
    }
    return { kind: "error", error: { kind: "Transport", status: response.status, details: raw } };
  }
  let responseJson: JsonValue;
  try {
    responseJson = (await response.json()) as JsonValue;
  } catch (e) {
    return { kind: "error", error: { kind: "MalformedJson", details: String(e) } };
  }
  const result = deserialiseResult(responseJson);
  if (result.tag === "Err") {
    return { kind: "error", error: result.error };
  }
  return { kind: "ok", value: result.value };
}

export async function callService<R>(
  binding: ServiceBinding,
  servicePath: string,
  argsJson: JsonValue,
  deserialiseResult: (json: JsonValue, path?: string) => Result<R, BoundaryError>,
  callerContext: string = "",
  contractHash: string = "",
  env: unknown = undefined,
  calleeContext: string = "",
): Promise<R> {
  const raw = await callServiceCore(binding, servicePath, argsJson, deserialiseResult, callerContext, contractHash, env, calleeContext);
  if (raw.kind === "ok") return raw.value;
  if (raw.kind === "thrown") throw raw.thrown;
  throw boundaryError(raw.error);
}

// #3894: `<context>.<service>.outcome(args)`. The same call as `callService`,
// but a failure to reach the callee's handler is a value, not a thrown fault:
// each of the callee's refusals (`reference/wire-format.md`) is a `Rejected`,
// its marked `500` is `Faulted`, and any other status or a thrown `fetch` is
// `Unreachable`. A failure to read a `200` stays a fault: the contract hashes
// make it a toolchain bug, not an outcome. The failure is logged once, as
// `callService` logs it (`ContractMismatch`, `CalleeFaulted`), and by this
// function for the rest.
export async function callServiceOutcome<R>(
  binding: ServiceBinding,
  servicePath: string,
  argsJson: JsonValue,
  deserialiseResult: (json: JsonValue, path?: string) => Result<R, BoundaryError>,
  callerContext: string = "",
  contractHash: string = "",
  env: unknown = undefined,
  calleeContext: string = "",
): Promise<CallOutcome<R>> {
  const raw = await callServiceCore(binding, servicePath, argsJson, deserialiseResult, callerContext, contractHash, env, calleeContext);
  if (raw.kind === "ok") return { tag: "Handled", result: raw.value };
  if (raw.kind === "thrown") {
    globalThis.console.error(withTrace(`CallUnreachable ${callerContext} -> ${servicePath}`));
    return { tag: "Unreachable", status: None };
  }
  const outcome = outcomeOfCallError<R>(raw.error);
  if (outcome === null) throw boundaryError(raw.error);
  if (raw.error.kind === "Transport") {
    globalThis.console.error(withTrace(`Call${outcome.tag} ${callerContext} -> ${servicePath} status ${raw.error.status}`));
  }
  return outcome;
}

// #3894: the callee's answer to a refused or failed call, as the caller's
// `CallOutcome`; `null` when it is not a refusal or a failure to reach (an
// unreadable `200`).
function outcomeOfCallError<R>(error: CallError): CallOutcome<R> | null {
  const refused = (reason: string): CallOutcome<R> => ({ tag: "Rejected", reason: { tag: reason } });
  switch (error.kind) {
    case "ContractMismatch":
      return refused("StaleContract");
    case "CalleeFaulted":
      return { tag: "Faulted" };
    case "Transport": {
      switch (error.status) {
        case 413:
          return refused("TooLarge");
        case 401:
          return refused("NotAuthenticated");
        case 404:
          return refused("NoSuchService");
        case 403:
          return refused("NotAdmitted");
        case 400: {
          let kind: unknown = null;
          try {
            kind = (JSON.parse(error.details) as { kind?: unknown }).kind;
          } catch {
            // Not a JSON body: an undecodable one.
          }
          return refused(kind === "UnsupportedWireVersion" ? "UnsupportedWire" : "Undecodable");
        }
        default:
          return { tag: "Unreachable", status: Some(error.status) };
      }
    }
    default:
      return null;
  }
}

// #3894: the `bundle` target has no wire, so `.outcome(…)` awaits the in-process
// call: a result is `Handled`, a fault is `Faulted`. A test stub that is
// `rejected` or `unreachable` throws a marked error (`__bynkCallStub`), which
// reads back as that outcome.
export async function callOutcome<R>(call: Promise<R>): Promise<CallOutcome<R>> {
  try {
    return { tag: "Handled", result: await call };
  } catch (e) {
    const stub = (e as { bynkCallStub?: { tag: string; reason?: string; status?: number } } | null)?.bynkCallStub;
    if (stub?.tag === "Rejected") return { tag: "Rejected", reason: { tag: stub.reason ?? "" } };
    if (stub?.tag === "Unreachable") {
      return { tag: "Unreachable", status: stub.status === undefined ? None : Some(stub.status) };
    }
    globalThis.console.error(withTrace("CalleeFaulted bundle call"));
    return { tag: "Faulted" };
  }
}

// Events track, slice 0: the fan-out DO's own delivery to one subscriber,
// over that subscriber's Service Binding. Subscriber failure isolation (ADR
// 0284) is the caller's responsibility — the fan-out DO catches per-subscriber
// so one subscriber's rejection does not stop delivery to its siblings; this
// helper itself just throws on a non-`ok` response.
//
// Slice 2 (spine #936): the POST body is `{ payload, envelope }`, not the
// bare payload — the envelope is always sent; whether the receiving
// subscriber's handler actually declared the optional `env: EventEnvelope`
// parameter is decided entirely on the receiving side (the `/_bynk/event/`
// route in the emitted `index.ts`), which forwards it to the handler only
// when that handler's own generated wrapper expects it.
export async function deliverEvent(
  binding: ServiceBinding,
  servicePath: string,
  payload: unknown,
  envelope: unknown,
  // I2 (#1971): the fan-out Durable Object's `env`, which holds the key for
  // the edge from the publishing context to the subscriber, and the two
  // contexts' qualified names. The subscriber verifies before it reads.
  env: unknown = undefined,
  publisher: string = "",
  subscriber: string = "",
): Promise<void> {
  const body = JSON.stringify({ payload, envelope });
  const path = `/_bynk/event/${servicePath}`;
  const signed = await signedHeaders(env, publisher, subscriber, path, "", "1", body, `the event delivery to ${servicePath}`);
  const request = new Request(`http://internal/_bynk/event/${servicePath}`, {
    method: "POST",
    headers: { "content-type": "application/json", ...eventTraceHeaders(envelope), ...signed },
    body,
  });
  const response = await binding.fetch(request);
  if (!response.ok) {
    throw new Error(`event delivery to ${servicePath} failed: ${response.status}`);
  }
}

// #3895: the envelope a subscriber's handler is handed, from one that was
// minted in this process. The minting site writes `trace` as a bare id (so it
// serialises as the wire's optional string, and is absent outside a trace); a
// handler reads it as an `Option[String]`. An envelope that already carries an
// `Option` is returned as it is.
export function eventEnvelopeOf<E extends object>(envelope: E): Omit<E, "trace"> & { trace: Option<string> } {
  const trace = (envelope as { trace?: unknown }).trace;
  if (typeof trace === "object" && trace !== null) return envelope as Omit<E, "trace"> & { trace: Option<string> };
  return { ...envelope, trace: isTraceId(trace) ? Some(trace) : None };
}

// #973: `EventEnvelope` (`{ eventId, publisherId, emittedAt, schemaVersion, trace }`)
// is declared in `adapter bynk`, not by a consumed context, so it never goes
// through the ordinary `emit_record_codec` machinery a context's own boundary
// types get (that machinery is rooted at a *consumed context's* exports;
// widening it to cover every adapter-declared type would touch every context
// that imports any `bynk` type, for one envelope). This hand-written validator
// is its sole codec, checked at the receiving `/_bynk/event/` route
// (`workers_entry.rs`) exactly once per delivery, unconditionally — the
// envelope is always on the wire regardless of whether a given subscriber's
// handler declared the optional `env: EventEnvelope` parameter. Field rules
// mirror the generated `Int`/`Instant` codec rule (`serialisation.rs`'s
// `TypeRef::Base` arm): both wire as `number`, integer-checked.
export function deserialiseEventEnvelope(
  json: JsonValue,
  path: string = "$",
): Result<
  { eventId: string; publisherId: string; emittedAt: number; schemaVersion: number; trace: Option<string> },
  BoundaryError
> {
  if (typeof json !== "object" || json === null || Array.isArray(json)) {
    return Err({ kind: "StructuralMismatch", path, expected: "object", actual: typeof json });
  }
  const obj = json as { [k: string]: JsonValue };
  const eventId = obj["eventId"];
  if (typeof eventId !== "string") {
    return Err({
      kind: "StructuralMismatch",
      path: `${path}.eventId`,
      expected: "string",
      actual: typeof eventId,
    });
  }
  const publisherId = obj["publisherId"];
  if (typeof publisherId !== "string") {
    return Err({
      kind: "StructuralMismatch",
      path: `${path}.publisherId`,
      expected: "string",
      actual: typeof publisherId,
    });
  }
  const emittedAt = obj["emittedAt"];
  if (typeof emittedAt !== "number" || !Number.isInteger(emittedAt)) {
    return Err({
      kind: "StructuralMismatch",
      path: `${path}.emittedAt`,
      expected: "integer",
      actual: typeof emittedAt,
    });
  }
  const schemaVersion = obj["schemaVersion"];
  if (typeof schemaVersion !== "number" || !Number.isInteger(schemaVersion)) {
    return Err({
      kind: "StructuralMismatch",
      path: `${path}.schemaVersion`,
      expected: "integer",
      actual: typeof schemaVersion,
    });
  }
  // #3895: optional and additive. Absent, `null` and a malformed id are all
  // `None`: a trace must never make an event undeliverable. On the wire the
  // trace is a bare id string, as the fan-out carries it. That is not the
  // tagged `{ kind: "Some", value }` form the std codec writes for an
  // `Option` field, which this route reads as `None`: the two shapes do not
  // round-trip, and nothing sends the std form here.
  const wireTrace = obj["trace"];
  const trace = isTraceId(wireTrace) ? Some(wireTrace) : None;
  return Ok({ eventId, publisherId, emittedAt, schemaVersion, trace });
}

// #1653: the `__` aliases emitted code imports.
export { boundaryError as __boundaryError, callOutcome as __callOutcome, callService as __callService, callServiceOutcome as __callServiceOutcome, checkStoredLayout as __checkStoredLayout, checkStoredSchemaVersion as __checkStoredSchemaVersion, deliverEvent as __deliverEvent, deserialiseEventEnvelope as __deserialiseEventEnvelope, eventEnvelopeOf as __eventEnvelopeOf, faultResponse as __faultResponse, migrationFailed as __migrationFailed, rehydrationViolation as __rehydrationViolation, readCapped as __readBodyCapped, renameStoredFields as __renameStoredFields, reportUnknownStoredKeys as __reportUnknownStoredKeys, verifyCall as __verifyCall, wireRefusal as __wireRefusal };
export type { BoundaryError as __BoundaryError, JsonValue as __JsonValue, ServiceBinding as __ServiceBinding };
