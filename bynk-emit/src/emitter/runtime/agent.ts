import type { DurableObjectState } from "./storage.ts";
import { makeTestState } from "./storage.ts";
import type { BoundaryError, JsonValue } from "./boundary.ts";
import { boundaryError } from "./boundary.ts";
import type { Result } from "./result.ts";
import { Ok } from "./result.ts";

// v0.9.2: agent instantiation + per-key state lifecycle.
//
// An agent keyed by some value lowers to a *lookup-or-create* of the durable
// state for that key. In bundle mode the per-agent `StateRegistry` holds an
// in-memory `DurableObjectState` per serialised key (same key → same state
// within a session; reset per test). In workers mode the agent is a Durable
// Object: `makeWorkersAgent` returns a typed proxy over the DO stub whose
// method calls route through `callDurableObjectMethod`. `makeAgent` picks the
// path: a present binding means workers, an absent one means bundle.

// A minimal structural view of the Cloudflare Durable Object namespace/stub
// surface. The real runtime is richer but structurally compatible.
export interface DurableObjectStub {
  fetch(input: string, init?: unknown): Promise<Response>;
}

export interface DurableObjectNamespace {
  idFromName(name: string): unknown;
  get(id: unknown): DurableObjectStub;
}

// Serialise an agent key to a stable string. Two semantically-equal keys must
// serialise identically: a string is itself, a primitive is its JSON form, a
// record is canonical JSON with sorted fields.
export function serialiseAgentKey(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean" || value === null) {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return JSON.stringify(value.map((v) => serialiseAgentKey(v)));
  }
  if (typeof value === "object") {
    const obj = value as { [k: string]: unknown };
    const keys = Object.keys(obj).sort();
    return JSON.stringify(keys.map((k) => [k, serialiseAgentKey(obj[k])]));
  }
  return JSON.stringify(value);
}

// Per-agent registry: serialised key → in-memory durable state. Used in bundle
// mode (and in `bynkc test`). `reset()` clears every state so a fresh test
// sees a clean slate.
export class StateRegistry<K> {
  private states = new Map<string, DurableObjectState>();

  getOrCreate(key: K): DurableObjectState {
    const sk = serialiseAgentKey(key);
    let state = this.states.get(sk);
    if (state === undefined) {
      state = makeTestState(sk);
      this.states.set(sk, state);
    }
    return state;
  }

  reset(): void {
    this.states.clear();
  }
}

// #1678 (runtime-semantics track S11): on the workers target an agent call
// crosses a Durable Object `fetch`, so its arguments and result are on a wire.
// They go through the boundary codec, the same `serialise_*`/`deserialise_*`
// helpers a cross-context call uses, so a value whose in-memory shape is not
// JSON-faithful (`Bytes`, a value `Map`, a non-finite `Float`) survives the
// trip. The emitter generates one `AgentWire` table per agent: for each handler,
// a codec per parameter and one for the result. Both ends read the same table:
// the caller's proxy encodes arguments and decodes the result, and the DO's
// `fetch` decodes arguments and encodes the result.

// One wire position's codec. `enc` takes the position's in-memory value; it is
// typed `never` so any concrete serialiser (`(value: Blob) => JsonValue`) fits.
export interface WireCodec {
  readonly enc: (value: never) => JsonValue;
  readonly dec: (json: JsonValue, path?: string) => Result<unknown, BoundaryError>;
}

export interface AgentWireMethod {
  readonly args: readonly WireCodec[];
  readonly result: WireCodec;
}

export type AgentWire = Readonly<Record<string, AgentWireMethod>>;

// A position whose type has no wire form (a held `Connection`, a function, a
// `Stream`, a `Query`) passes through as it did before #1678.
export const AGENT_WIRE_PASS: WireCodec = {
  enc: (value: never) => value as JsonValue,
  dec: (json: JsonValue) => Ok(json),
};

function agentWireMethod(wire: AgentWire, method: string): AgentWireMethod {
  // `hasOwn`: a method name comes off the request path.
  if (!Object.hasOwn(wire, method)) {
    throw new Error(`agent method \`${method}\` has no wire codec`);
  }
  return wire[method];
}

// The DO side: decode a call's arguments. A decode failure is an internal
// fault (the caller is the program itself, not an untrusted client), so it
// throws, and the DO's `fetch` fails with a 500 the caller rethrows.
export function decodeAgentArgs(wire: AgentWire, method: string, args: unknown[]): unknown[] {
  const m = agentWireMethod(wire, method);
  return args.map((a, i) => {
    const r = (m.args[i] ?? AGENT_WIRE_PASS).dec(a as JsonValue, `$.args[${i}]`);
    if (r.tag === "Err") throw boundaryError(r.error);
    return r.value;
  });
}

// #1818: the DO side of an agent that reads its own key (`self.<key>`). A
// Durable Object knows only its id, not the key it was addressed by, so the
// caller's proxy sends the key with each call, encoded by the key type's codec,
// and the DO decodes it here.
export function decodeAgentKey(codec: WireCodec, json: unknown): unknown {
  const r = codec.dec(json as JsonValue, "$.key");
  if (r.tag === "Err") throw boundaryError(r.error);
  return r.value;
}

// The DO side: encode a call's result.
export function encodeAgentResult(wire: AgentWire, method: string, result: unknown): JsonValue {
  return agentWireMethod(wire, method).result.enc(result as never);
}

// Workers-mode agent method call: route through the DO stub's `fetch` under
// the `/_bynk/agent/<method>` wire protocol. `args` are already encoded, and
// the result comes back as raw JSON for the caller to decode.
// #3132: the caller's `deps` crosses the wire to a Durable Object that rebuilds
// `env`, its `given` providers and its events fan-out from its own
// `this.__env`. The forwarded `env` gives the callee nothing it lacks, and its
// secrets and vars (plain strings) would ride the internal request body, where a
// tail worker or request log can read them. A `given` provider built with the
// Worker `env` also holds it as an own field (`this.deps = { env, ... }`), so the
// strip covers `env` at any depth, not only the top. The runtime cannot tell a
// provider from plain data, so one replacer drops every `env` key. Every agent
// call (handler, service, ws lifecycle body) funnels through here.
function stripRebuiltDeps(deps: unknown): unknown {
  if (typeof deps !== "object" || deps === null) return deps;
  return JSON.parse(JSON.stringify(deps, (k, v: unknown) => (k === "env" ? undefined : v)));
}

export async function callDurableObjectMethod(
  stub: DurableObjectStub,
  method: string,
  args: unknown[],
  deps: unknown,
  key?: JsonValue,
): Promise<JsonValue> {
  const wireDeps = stripRebuiltDeps(deps);
  const response = await stub.fetch(`https://_bynk/_bynk/agent/${method}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    // #1818: the key rides along only for an agent that reads it.
    body: JSON.stringify(key === undefined ? { args, deps: wireDeps } : { args, deps: wireDeps, key }),
  });
  if (!response.ok) throw new Error(await response.text());
  return (await response.json()) as JsonValue;
}

// Workers-mode agent: a typed proxy over the DO stub. Each method access
// returns a function that splits its final argument off as `deps` and routes
// the rest as method args through `callDurableObjectMethod`. The `as C` cast
// gives call sites the agent's real method signatures, so user code reads
// identically to bundle mode.
export function makeWorkersAgent<C>(
  binding: DurableObjectNamespace,
  key: unknown,
  wire?: AgentWire,
  keyCodec?: WireCodec,
): C {
  const stub = binding.get(binding.idFromName(serialiseAgentKey(key)));
  // Encoded per call, not here, so a key that cannot encode fails at the call,
  // as an argument that cannot encode does.
  const sentKey = (): JsonValue | undefined =>
    keyCodec === undefined ? undefined : keyCodec.enc(key as never);
  const proxy = new Proxy(
    {},
    {
      get(_target, prop: string | symbol) {
        if (typeof prop !== "string") return undefined;
        return async (...callArgs: unknown[]) => {
          const deps = callArgs.length > 0 ? callArgs[callArgs.length - 1] : {};
          const args = callArgs.length > 0 ? callArgs.slice(0, -1) : [];
          if (wire === undefined) return callDurableObjectMethod(stub, prop, args, deps, sentKey());
          const m = agentWireMethod(wire, prop);
          const encoded = args.map((a, i) => (m.args[i] ?? AGENT_WIRE_PASS).enc(a as never));
          const json = await callDurableObjectMethod(stub, prop, encoded, deps, sentKey());
          const r = m.result.dec(json);
          if (r.tag === "Err") throw boundaryError(r.error);
          return r.value;
        };
      },
    },
  );
  return proxy as C;
}

// Single agent-construction helper. A present DO binding selects the workers
// path; otherwise the bundle registry path. Call sites are identical across
// targets.
export function makeAgent<C>(
  registry: StateRegistry<unknown>,
  binding: DurableObjectNamespace | undefined,
  key: unknown,
  constructBundle: (state: DurableObjectState) => C,
  wire?: AgentWire,
  keyCodec?: WireCodec,
): C {
  if (binding !== undefined) {
    return makeWorkersAgent<C>(binding, key, wire, keyCodec);
  }
  const state = registry.getOrCreate(key);
  return serialiseBundleAgent(state, constructBundle(state));
}

// Events track, slice 0 (spine #936, ADR 0284): a publishing context's
// `deps.__eventsDispatch` hands its release-at-commit event batch to this
// context's own fan-out Durable Object (one namespace per publishing
// context — `idFromName` is passed a fixed key since the DO instance itself,
// not the id, is what scopes the fan-out to this publisher and gives it
// single-threaded per-publisher ordering). A non-`ok` response is logged, not
// thrown: the publishing handler already committed by the time this runs
// (release-at-commit), so a fan-out transport failure must not surface as a
// failure of the handler that emitted.
export async function dispatchToEventsFanout(
  binding: DurableObjectNamespace,
  events: Array<{
    type: string;
    payload: unknown;
    envelope: { eventId: string; publisherId: string; emittedAt: number; schemaVersion: number };
  }>,
): Promise<void> {
  // The whole round trip is wrapped, not just the status check: a rejected
  // `stub.fetch` (a network error, the DO throwing) is exactly as much a
  // fan-out transport failure as a non-`ok` response, and by this point
  // `commitState` has already succeeded — letting a rejection propagate
  // would surface a transport problem as a failure of a handler that, from
  // the caller's perspective, already completed.
  try {
    const stub = binding.get(binding.idFromName("singleton"));
    const response = await stub.fetch("https://_bynk/_bynk/fanout", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ events }),
    });
    if (!response.ok) {
      console.error("EventsFanout dispatch failed", { status: response.status });
    }
  } catch (e) {
    console.error("EventsFanout dispatch failed", { error: String(e) });
  }
}

// v0.16: an in-process Durable-Object namespace for multi-Worker integration
// tests. `cls` is the emitted DO class, built from a fresh in-memory state;
// one instance is kept per key (so state accumulates within a test case). The
// returned stub bridges the stub-side `fetch(url, init)` the agent runtime
// speaks to the DO class's server-side `fetch(request)`.
//
// #1959: the class is constructed with `(state, env)` as workerd does, so an
// agent that rebuilds `given` deps or emits events from `env`, and the events
// fan-out, see the participant's bindings. A class whose constructor takes
// only `state` still fits this type: a constructor with fewer parameters is
// assignable to one with more.
export function makeIntegrationDoNamespace(
  cls: new (state: DurableObjectState, env?: unknown) => {
    fetch(request: Request): Promise<Response>;
  },
  env?: unknown,
  queueWaitMs: number = AGENT_QUEUE_WAIT_MS,
): DurableObjectNamespace {
  const instances = new Map<string, { fetch(request: Request): Promise<Response> }>();
  return {
    idFromName(name: string): unknown {
      return name;
    },
    get(id: unknown): DurableObjectStub {
      const k = String(id);
      let inst = instances.get(k);
      if (inst === undefined) {
        inst = new (serialiseAgentClass(cls, { waitMs: queueWaitMs }))(makeTestState(k), env);
        instances.set(k, inst);
      }
      const target = inst;
      return {
        fetch: (input: string, init?: unknown): Promise<Response> =>
          target.fetch(new Request(input, init as RequestInit)),
      };
    },
  };
}

// State-migrations S4 (#2259 Decision D): `bynk migrate sweep`'s wake.
//
// The Worker side. `body` is the verified request body, `{ "ids": [...] }`,
// each a Durable Object id from Cloudflare's objects listing. Every id is
// woken through the reserved `/_bynk/migrate` entry, which loads the
// instance's state (running any migration step) and commits it when it was
// older than `current`. The answer is `{ current, results }`: `current` is
// the version this deployed Worker writes, which the sweep needs even for a
// namespace with no instances, and `results` holds each id's
// `{ id, before, after }` or `{ id, fault }`. One id's fault never stops the
// others.
export interface MigrationNamespace {
  idFromString(id: string): unknown;
  get(id: unknown): DurableObjectStub;
}

export const MIGRATION_MAX_IDS = 1000;

export async function sweepAgentInstances(
  binding: MigrationNamespace,
  body: string,
  current: number,
  // The storage layout this build writes (#3189): 1 is the wholesale layout every
  // build wrote before per-entry storage. Reported to the sweep, which uses it only
  // to say how far the layout upgrade has got.
  layout: number = 1,
): Promise<Response> {
  let ids: unknown;
  try {
    ids = (JSON.parse(body) as { ids?: unknown } | null)?.ids;
  } catch {
    ids = undefined;
  }
  if (
    !Array.isArray(ids) ||
    ids.length > MIGRATION_MAX_IDS ||
    !ids.every((id) => typeof id === "string")
  ) {
    return new Response("Bad Request", { status: 400 });
  }
  const results = await Promise.all(
    (ids as string[]).map(async (id) => {
      let stub: DurableObjectStub;
      try {
        stub = binding.get(binding.idFromString(id));
      } catch {
        return { id, fault: "BadId" };
      }
      try {
        const response = await stub.fetch("https://_bynk/_bynk/migrate", { method: "POST" });
        if (!response.ok) return { id, fault: `Status${response.status}` };
        return { id, ...((await response.json()) as object) };
      } catch {
        return { id, fault: "Unreachable" };
      }
    }),
  );
  return new Response(JSON.stringify({ current, layout, results }), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

// The Durable Object side, the whole of its `/_bynk/migrate` entry. `stored`
// is the raw record under the state key (`undefined` when there is none). An
// absent `__v` is version 1, as `checkStoredSchemaVersion` reads it. `load`
// is the class's `loadState`, which checks the marker and runs any step;
// `commit` is its `commitState`, which runs the invariant gate and stamps
// `current`. A record already at `current` is read and not written. A fault
// is reported by kind only: never its detail, which can carry stored data.
export async function wakeForMigration<S>(
  stored: unknown,
  current: number,
  load: () => Promise<S>,
  commit: (state: S) => Promise<void>,
  // The storage layout this build writes. `load` upgrades an older record to it
  // (the lazy upgrade), so a record that woke without a fault is in it afterwards.
  layout: number = 1,
): Promise<Response> {
  const json = (value: object) =>
    new Response(JSON.stringify(value), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  if (stored === undefined || stored === null) return json({ before: null, after: null });
  const raw = typeof stored === "object" ? (stored as { readonly __v?: unknown }).__v : undefined;
  const marker = raw === undefined ? 1 : raw;
  const before = typeof marker === "number" ? marker : null;
  // The layout marker, read as `layoutOf` reads it (an absent one is layout 1); null
  // when it is not a positive integer.
  const rawLayout =
    typeof stored === "object" ? (stored as { readonly __layout?: unknown }).__layout : undefined;
  const layoutBefore =
    rawLayout === undefined
      ? 1
      : typeof rawLayout === "number" && Number.isInteger(rawLayout) && rawLayout >= 1
        ? rawLayout
        : null;
  try {
    const state = await load();
    // A record in an older layout is written by `load` itself; a commit is still
    // owed only when the schema version is older.
    if (before === null || before < current) await commit(state);
    return json({ before, after: current, layoutBefore, layoutAfter: layout });
  } catch (e) {
    return json({ before, layoutBefore, fault: migrationFaultKind(e) });
  }
}

function migrationFaultKind(e: unknown): string {
  if (typeof e === "object" && e !== null) {
    const rehydration = (e as { rehydrationViolation?: { detail?: { kind?: unknown } } })
      .rehydrationViolation;
    if (rehydration !== undefined) {
      const kind = rehydration.detail?.kind;
      return typeof kind === "string" ? kind : "RehydrationViolation";
    }
    if ((e as { invariantViolation?: unknown }).invariantViolation !== undefined) {
      return "InvariantViolation";
    }
  }
  return "Fault";
}

// #3202: an agent instance runs the handlers that reach it one at a time.
//
// A handler loads the instance's state, runs, and commits it. The Durable
// Object runtime closes its input gate only while a storage operation is in
// flight, so a handler that awaits anything else (a call to another agent, a
// capability) lets a second request run against the same instance, and the
// second commit overwrites the first. The Book's "atomically when the handler
// returns" needs the instance to run one handler at a time, so every entry
// point that reaches an instance goes through one first-in, first-out queue.
//
// The queue is the whole fix; the rest is what keeps it from hanging.
//
//  - Re-entrancy. A handler that reaches its own instance again (directly, or
//    through other agents) would wait on itself. Each cross-agent call carries
//    the ids of the instances whose handlers are running along the way, and an
//    instance that finds its own id in the chain faults `ReentrantAgentCall`
//    at once instead of queuing. The chain rides in a request header set only
//    by the wrapped `env` below, and is read only on the internal
//    `/_bynk/agent/` route, never on a forwarded client request. It is bounded.
//    A call that crosses a Service Binding to another Worker and back is not in
//    the chain: the queue-wait timeout is the backstop for that.
//  - Contention. A call waits at most `AGENT_QUEUE_WAIT_MS` for the handlers
//    ahead of it, then faults `AgentBusy`. The timer measures queue wait only,
//    never a handler's own run time. It is a different fault from re-entrancy,
//    because a hot instance can reach it with no cycle at all.

/** How long a call may wait behind the handlers ahead of it on one instance. */
export const AGENT_QUEUE_WAIT_MS = 30_000;

/** The request header that carries the call chain between agent instances. */
export const AGENT_CHAIN_HEADER = "x-bynk-call-chain";

/** The longest call chain honoured; a longer one is a fault, not truncated. */
export const AGENT_CHAIN_MAX = 16;

export interface AgentContentionFault {
  readonly kind: "ReentrantAgentCall" | "AgentBusy";
  readonly agent: string;
}

function contentionFault(kind: AgentContentionFault["kind"], agent: string, message: string): Error {
  // Logged by kind and agent only, never an id or an argument (ADR 0107).
  globalThis.console.error(`${kind} ${agent}`, { agent, kind });
  const e = new Error(`${kind}: ${message}`);
  globalThis.Object.defineProperty(e, "agentFault", {
    value: { kind, agent } satisfies AgentContentionFault,
    enumerable: false,
  });
  return e;
}

export function reentrantAgentCall(agent: string): Error {
  return contentionFault(
    "ReentrantAgentCall",
    agent,
    `${agent} was called from inside one of its own handlers, directly or through other agents. An agent instance runs one handler at a time, so the call would wait on itself.`,
  );
}

export function agentBusy(agent: string, waitMs: number, queued: number): Error {
  return contentionFault(
    "AgentBusy",
    agent,
    `agent ${agent} busy: waited ${waitMs / 1000} s behind ${queued} queued call${queued === 1 ? "" : "s"}`,
  );
}

// One first-in, first-out queue per instance.
export class AgentQueue {
  private tail: Promise<void> = Promise.resolve();
  private pending = 0;

  // Run `fn` once every call queued before it has finished. The wait, and only
  // the wait, is bounded by `waitMs`; a call that gives up passes its place on.
  //
  // `fn` is handed `yieldSlot`, which gives the place up early, once: a handler
  // that has committed and has only a reply left to send (the events fan-out,
  // which calls subscribers that may call this instance back) lets the next
  // call in without waiting for the reply to finish.
  run<T>(agent: string, waitMs: number, fn: (yieldSlot: () => void) => Promise<T>): Promise<T> {
    const ahead = this.pending;
    const prev = this.tail;
    let release!: () => void;
    const mine = new globalThis.Promise<void>((r) => {
      release = r;
    });
    this.tail = prev.then(() => mine);
    this.pending += 1;
    return (async () => {
      let timer: ReturnType<typeof globalThis.setTimeout> | undefined;
      try {
        await globalThis.Promise.race([
          prev,
          new globalThis.Promise<never>((_, reject) => {
            timer = globalThis.setTimeout(() => reject(agentBusy(agent, waitMs, ahead)), waitMs);
          }),
        ]);
      } catch (e) {
        this.pending -= 1;
        void prev.then(release);
        throw e;
      } finally {
        globalThis.clearTimeout(timer);
      }
      let held = true;
      const yieldSlot = (): void => {
        if (!held) return;
        held = false;
        this.pending -= 1;
        release();
      };
      try {
        return await fn(yieldSlot);
      } finally {
        yieldSlot();
      }
    })();
  }
}

// Bundle mode: an agent value is rebuilt around the instance's one state on
// every call, so the queue is keyed by that state. A call from outside goes
// through the queue; a handler calling a sibling on `this` calls the raw
// instance and does not. There is no call chain here, so a cycle is only
// caught by the queue-wait timeout.
const bundleQueues = new globalThis.WeakMap<object, AgentQueue>();

export function serialiseBundleAgent<C>(
  state: DurableObjectState,
  instance: C,
  waitMs: number = AGENT_QUEUE_WAIT_MS,
): C {
  let queue = bundleQueues.get(state);
  if (queue === undefined) {
    queue = new AgentQueue();
    bundleQueues.set(state, queue);
  }
  const q = queue;
  const agent = (instance as { constructor: { name: string } }).constructor.name;
  return new Proxy(instance as object, {
    get(target, prop, receiver) {
      const v = Reflect.get(target, prop, receiver) as unknown;
      if (typeof v !== "function" || typeof prop !== "string") return v;
      return (...args: unknown[]): Promise<unknown> =>
        q.run(agent, waitMs, async () => Reflect.apply(v, target, args));
    },
  }) as C;
}

interface InstanceGate {
  readonly id: string;
  readonly agent: string;
  readonly queue: AgentQueue;
  readonly waitMs: number;
  // The chain of the handler running now. Only one runs at a time, so one
  // field is unambiguous.
  chain: readonly string[];
  // Gives the running handler's place up early; unset between handlers.
  yieldSlot?: () => void;
}

function isNamespace(v: unknown): v is DurableObjectNamespace {
  return (
    typeof v === "object" &&
    v !== null &&
    typeof (v as { idFromName?: unknown }).idFromName === "function" &&
    typeof (v as { get?: unknown }).get === "function"
  );
}

// The `env` an instance's class is built with: each Durable Object namespace
// in it hands out stubs that add this instance's call chain to what they send.
function chainEnv(env: unknown, gate: InstanceGate): unknown {
  if (typeof env !== "object" || env === null) return env;
  return new Proxy(env, {
    get(target, prop) {
      const v = Reflect.get(target, prop, target) as unknown;
      if (!isNamespace(v)) return v;
      return new Proxy(v, {
        get(ns, nsProp) {
          const f = Reflect.get(ns, nsProp, ns) as unknown;
          if (nsProp !== "get" || typeof f !== "function") {
            return typeof f === "function" ? f.bind(ns) : f;
          }
          return (id: unknown): DurableObjectStub => {
            const stub = f.call(ns, id) as DurableObjectStub;
            // A proxy over the real stub that changes only `fetch`: everything
            // else on it (`id`, `name`, RPC methods) is the stub's own.
            return new Proxy(stub, {
              get(target, key) {
                const member = Reflect.get(target, key, target) as unknown;
                if (key !== "fetch") return typeof member === "function" ? member.bind(target) : member;
                return (input: unknown, init?: unknown): Promise<Response> => {
                  // The events fan-out runs after the handler has committed and
                  // calls subscribers that may call this instance back. Holding
                  // the place while it runs would make that call wait out the
                  // queue limit, so the place is given up here.
                  if (typeof input === "string" && input.endsWith("/_bynk/fanout")) gate.yieldSlot?.();
                  const i = (init ?? {}) as { headers?: unknown };
                  const headers = new globalThis.Headers(i.headers as HeadersInit | undefined);
                  headers.set(AGENT_CHAIN_HEADER, gate.chain.join(","));
                  // A `Request` input already carries its own headers: merge
                  // them, so none is lost, with the chain set last.
                  if (typeof input === "object" && input !== null && "headers" in input) {
                    (input as Request).headers.forEach((v, k) => {
                      if (!headers.has(k)) headers.set(k, v);
                    });
                  }
                  return (target.fetch as (a: unknown, b?: unknown) => Promise<Response>).call(
                    target,
                    input,
                    { ...(i as object), headers },
                  );
                };
              },
            });
          };
        },
      });
    },
  });
}

function parseChain(header: string | null): string[] | undefined {
  if (header === null || header === "") return [];
  const parts = header.split(",");
  return parts.length > AGENT_CHAIN_MAX ? undefined : parts;
}

function stateIdOf(state: DurableObjectState): string {
  const id = state.id as { readonly name?: string; toString(): string };
  const s = id.toString();
  return s === "[object Object]" ? String(id.name) : s;
}

const ENTRY_POINTS = ["fetch", "webSocketMessage", "webSocketClose", "webSocketError", "alarm"] as const;
const GATE = Symbol("bynk.agentGate");

// A Durable Object class whose entry points run one at a time per instance.
// Exported from the Worker entry in place of the emitted class, and used by
// the integration namespace, so the same rule holds under `bynk test`.
export function serialiseAgentClass<C extends new (...args: never[]) => object>(
  Base: C,
  options: { readonly waitMs?: number } = {},
): C {
  const Mixin = Base as unknown as new (...args: unknown[]) => object;
  class Serialised extends Mixin {
    constructor(...args: unknown[]) {
      const state = args[0] as DurableObjectState;
      const gate: InstanceGate = {
        id: `${Base.name}:${stateIdOf(state)}`,
        agent: Base.name,
        queue: new AgentQueue(),
        waitMs: options.waitMs ?? AGENT_QUEUE_WAIT_MS,
        chain: [],
      };
      super(state, chainEnv(args[1], gate), ...args.slice(2));
      (this as Record<symbol, unknown>)[GATE] = gate;
    }
  }
  const proto = Serialised.prototype as Record<string, unknown>;
  for (const name of ENTRY_POINTS) {
    const original = (Base.prototype as Record<string, unknown>)[name];
    if (typeof original !== "function") continue;
    proto[name] = function (this: object, ...args: unknown[]): Promise<unknown> {
      const gate = (this as Record<symbol, InstanceGate>)[GATE] as InstanceGate;
      let incoming: readonly string[] = [];
      if (name === "fetch") {
        const request = args[0] as Request;
        if (new globalThis.URL(request.url).pathname.startsWith("/_bynk/agent/")) {
          const chain = parseChain(request.headers.get(AGENT_CHAIN_HEADER));
          if (chain === undefined) {
            return globalThis.Promise.resolve(
              new globalThis.Response(
                `AgentCallChainTooLong: more than ${AGENT_CHAIN_MAX} agent instances deep`,
                { status: 500 },
              ),
            );
          }
          incoming = chain;
        }
      }
      if (incoming.includes(gate.id)) {
        return globalThis.Promise.resolve(
          new globalThis.Response(reentrantAgentCall(gate.agent).message, { status: 500 }),
        );
      }
      const run = gate.queue.run(gate.agent, gate.waitMs, async (yieldSlot) => {
        const chain = [...incoming, gate.id];
        gate.chain = chain;
        gate.yieldSlot = yieldSlot;
        try {
          return await Reflect.apply(original, this, args);
        } finally {
          // A handler that yielded its place must not clear what the next
          // handler set.
          if (gate.chain === chain) {
            gate.chain = [];
            gate.yieldSlot = undefined;
          }
        }
      });
      // A contention fault answers a `fetch` as the emitted dispatch answers a
      // fault, so the caller's `callDurableObjectMethod` throws its message.
      if (name !== "fetch") return run;
      return run.catch((e: unknown) => {
        if ((e as { agentFault?: unknown }).agentFault === undefined) throw e;
        return new globalThis.Response((e as Error).message, { status: 500 });
      });
    };
  }
  return Serialised as unknown as C;
}

// #1653: the `__` aliases emitted code imports.
export { AGENT_WIRE_PASS as __AGENT_WIRE_PASS, StateRegistry as __StateRegistry, decodeAgentArgs as __decodeAgentArgs, decodeAgentKey as __decodeAgentKey, dispatchToEventsFanout as __dispatchToEventsFanout, encodeAgentResult as __encodeAgentResult, makeAgent as __makeAgent, makeIntegrationDoNamespace as __makeIntegrationDoNamespace, serialiseAgentKey as __serialiseAgentKey, serialiseAgentClass as __serialiseAgentClass, sweepAgentInstances as __sweepAgentInstances, wakeForMigration as __wakeForMigration };
export type { AgentWire as __AgentWire, DurableObjectNamespace as __DurableObjectNamespace };
