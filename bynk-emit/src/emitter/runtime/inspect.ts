// `bynk dev` agent inspection (#3581, the dev agent-inspection track): list the
// instances of an agent class by key, and read one instance's stored state.
//
// This module is reached only by a dev build that asked for inspection, so a
// deploy build never emits it (ADR 0615: a runtime module is emitted only when
// emitted code imports it). The existing runtime modules are not edited: the
// emitter wraps an agent factory in `noteAgentInstance`, adds a branch to each
// agent's `fetch` and a route to each Worker's entry, and re-exports
// `BynkInspectRegistry` as a Durable Object class.
//
// A read never writes. It does not run the class's `loadState`, which upgrades
// an older layout by writing; it applies the same layout and schema-version
// gates to the raw stored record, and returns what is stored, which is the wire
// shape. A record at an older version than the build is reported, not migrated.
// The history ring buffer (slice 1) is stored under `HISTORY_KEY`, a reserved
// key outside layout 2's namespaces, and is not read here yet.

import type { DurableObjectNamespace, WireCodec } from "./agent.ts";
import { serialiseAgentKey } from "./agent.ts";
import { CALLER_HEADER, checkStoredLayout, checkStoredSchemaVersion, verifyCall } from "./boundary.ts";
import type { JsonValue } from "./boundary.ts";
import { decodeKeyPart, logPrefix, mapPrefix, metaKey, setPrefix, type WholesaleField } from "./entries.ts";
import type { DurableObjectState } from "./storage.ts";

/** The Worker binding of the registry Durable Object (dev builds only). */
export const INSPECT_REGISTRY_BINDING = "__BYNK_INSPECT_REGISTRY";

/** The reserved caller name `bynk dev inspect` signs its requests as. */
export const INSPECT_CALLER = "bynk.dev";

/** Entries per page when a request names no `limit`, and the most one may ask for. */
export const INSPECT_PAGE_DEFAULT = 50;
export const INSPECT_PAGE_MAX = 500;

/** The reserved storage key of an instance's transition ring buffer (slice 1). */
export const HISTORY_KEY = "__bynk_dev/history";

/** What the registry knows of one instance. `key` is the key in wire form. */
export interface InspectInstance {
  class: string;
  key: JsonValue;
  id: string;
}

interface InspectNamespace extends DurableObjectNamespace {
  idFromString(id: string): unknown;
}

const noted = new globalThis.Set<string>();

// Record that `key` addressed an instance of `agent`, and hand the agent back
// unchanged. Fire and forget: a registry write that fails or never lands must
// not change the call, so it is neither awaited nor allowed to reject. A key
// that cannot encode is not recorded.
export function noteAgentInstance<C>(
  registry: DurableObjectNamespace | undefined,
  binding: DurableObjectNamespace | undefined,
  className: string,
  key: unknown,
  keyCodec: WireCodec,
  agent: C,
): C {
  if (registry === undefined || binding === undefined) return agent;
  try {
    const name = serialiseAgentKey(key);
    const seen = `${className}\u0000${name}`;
    if (noted.has(seen)) return agent;
    noted.add(seen);
    const instance: InspectInstance = {
      class: className,
      key: keyCodec.enc(key as never),
      id: String(binding.idFromName(name)),
    };
    const stub = registry.get(registry.idFromName("registry"));
    void stub
      .fetch("https://_bynk/_bynk/inspect/register", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(instance),
      })
      .then((r) => {
        if (!r.ok) noted.delete(seen);
      })
      .catch(() => noted.delete(seen));
  } catch {
    // Recording is best effort.
  }
  return agent;
}

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
}

// The registry Durable Object: one per Worker, addressed by a fixed name. It
// stores one record per instance, keyed `<class>/<id>`, and never holds state.
export class BynkInspectRegistry {
  private readonly state: DurableObjectState;

  constructor(state: DurableObjectState) {
    this.state = state;
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (request.method === "POST" && url.pathname === "/_bynk/inspect/register") {
      const instance = (await request.json()) as InspectInstance;
      await this.state.storage.put(`${instance.class}/${instance.id}`, instance);
      return json({ ok: true });
    }
    if (request.method === "GET" && url.pathname === "/_bynk/inspect/list") {
      const only = url.searchParams.get("class");
      const all = await this.state.storage.list<InspectInstance>(
        only === null ? {} : { prefix: `${only}/` },
      );
      return json({ instances: [...all.values()] });
    }
    return new Response("Not Found", { status: 404 });
  }
}

/** What the emitter knows of one agent class, for the read's gates. */
export interface InspectInfo {
  agent: string;
  current: number | null;
  oldestReadable: number;
  steps: readonly (readonly [number, number])[];
}

interface Page {
  kind: WholesaleField["kind"];
  size: number | null;
  entries: unknown[];
  next: string | null;
}

function limitOf(url: URL): number {
  const raw = Number(url.searchParams.get("limit") ?? INSPECT_PAGE_DEFAULT);
  if (!globalThis.Number.isInteger(raw) || raw < 1) return INSPECT_PAGE_DEFAULT;
  return Math.min(raw, INSPECT_PAGE_MAX);
}

// One page of a layout-2 collection: the entries under its prefix after the
// cursor (a raw storage key), in key order, and the cursor of the next page.
async function entryPage(
  state: DurableObjectState,
  field: WholesaleField,
  after: string | null,
  limit: number,
): Promise<Page> {
  const prefix = { map: mapPrefix, cache: mapPrefix, set: setPrefix, log: logPrefix }[field.kind](field.name);
  const meta = await state.storage.get<{ size?: number }>(metaKey(field.name));
  const listed = await state.storage.list<unknown>({
    prefix,
    limit: limit + 1,
    ...(after === null ? {} : { startAfter: after }),
  });
  const rows = [...listed.entries()];
  const more = rows.length > limit;
  const kept = rows.slice(0, limit);
  return {
    kind: field.kind,
    size: typeof meta?.size === "number" ? meta.size : null,
    entries: kept.map(([k, v]) =>
      field.kind === "log" ? v : { key: decodeKeyPart(k.slice(prefix.length)), value: v },
    ),
    next: more ? kept[kept.length - 1]![0] : null,
  };
}

// A layout-1 collection is inline in the record; page it by position.
function inlinePage(field: WholesaleField, inline: unknown, after: string | null, limit: number): Page {
  const rows: unknown[] =
    inline === undefined || inline === null
      ? []
      : Array.isArray(inline)
        ? inline
        : globalThis.Object.entries(inline as Record<string, unknown>).map(([key, value]) => ({ key, value }));
  const parsed = after === null ? 0 : Number(after);
  const from = globalThis.Number.isInteger(parsed) && parsed >= 0 ? parsed : 0;
  const kept = rows.slice(from, from + limit);
  return {
    kind: field.kind,
    size: rows.length,
    entries: kept,
    next: from + limit < rows.length ? String(from + limit) : null,
  };
}

function faultKind(e: unknown): string {
  const raised = (e as { rehydrationViolation?: { detail?: { kind?: unknown } } } | null)?.rehydrationViolation;
  const kind = raised?.detail?.kind;
  return typeof kind === "string" ? kind : "Fault";
}

// The agent side: read this instance's stored state, without writing. `fields`
// describes its collections. `?field=<f>&after=<cursor>&limit=<n>` pages one
// collection; otherwise each collection's first page comes back with its cursor.
export async function inspectAgent(
  state: DurableObjectState,
  fields: readonly WholesaleField[],
  info: InspectInfo,
  url: URL,
): Promise<Response> {
  const stored = await state.storage.get<Record<string, unknown>>("state");
  if (stored === undefined || stored === null) return json({ class: info.agent, exists: false });
  const rawV = stored.__v;
  const version = typeof rawV === "number" ? rawV : rawV === undefined ? 1 : null;
  const layout = typeof stored.__layout === "number" ? stored.__layout : 1;
  const head = { class: info.agent, exists: true, version, layout, current: info.current };
  try {
    checkStoredLayout(info.agent, stored);
    if (info.current !== null) {
      checkStoredSchemaVersion(info.agent, stored, info.current, info.oldestReadable, info.steps);
    }
  } catch (e) {
    return json({ ...head, readable: false, fault: faultKind(e) });
  }
  // A record behind the build that a migration step covers: shown as stored, not migrated.
  const behind = version !== null && info.current !== null && version < info.current && info.steps.some(([lo, n]) => version >= lo && version <= n);
  const cells: Record<string, unknown> = {};
  for (const [k, v] of globalThis.Object.entries(stored)) {
    if (k === "__v" || k === "__layout") continue;
    if (fields.some((f) => f.name === k || (f.indexes ?? []).some((ix) => ix.recordField === k))) continue;
    cells[k] = v;
  }
  const limit = limitOf(url);
  const only = url.searchParams.get("field");
  const after = url.searchParams.get("after");
  const collections: Record<string, Page> = {};
  for (const f of fields) {
    if (only !== null && f.name !== only) continue;
    const cursor = only === null ? null : after;
    collections[f.name] = layout >= 2 ? await entryPage(state, f, cursor, limit) : inlinePage(f, stored[f.name], cursor, limit);
  }
  return json({ ...head, readable: true, migrationPending: behind, cells, collections });
}

async function instancesOf(registry: DurableObjectNamespace, className: string | null): Promise<InspectInstance[]> {
  const stub = registry.get(registry.idFromName("registry"));
  const path = className === null ? "list" : `list?class=${encodeURIComponent(className)}`;
  const response = await stub.fetch(`https://_bynk/_bynk/inspect/${path}`);
  return ((await response.json()) as { instances: InspectInstance[] }).instances;
}

// The Worker side: `POST /_bynk/inspect/agents[?class=C]` lists the instances the
// registry has seen, and `POST /_bynk/inspect/agent/<Class>?id=<id>[&…]` reads
// one through its Durable Object. Both are read-only; they are `POST`s, with no
// body, because the call signature covers the method and the published signer
// signs `POST`. Both are signed as every internal call is, by
// `INSPECT_CALLER` under the local edge key, and refused otherwise before
// anything is looked up. `bindings` maps each agent class this Worker hosts to
// its Durable Object binding name.
export async function inspectRoute(
  env: unknown,
  request: Request,
  context: string,
  bindings: Readonly<Record<string, string>>,
): Promise<Response> {
  if (request.method !== "POST") return new Response("Method Not Allowed", { status: 405 });
  // Only `bynk dev inspect`: a sibling context holding an edge key to this Worker
  // signs validly as itself, and must not read an agent's state. Checked before
  // the signature, as the migrate route checks its caller.
  if (request.headers.get(CALLER_HEADER) !== INSPECT_CALLER) return new Response("Unauthorized", { status: 401 });
  const verified = await verifyCall(request, env, context);
  if (typeof verified !== "string") return verified;
  const url = new URL(request.url);
  const bag = env as Record<string, unknown>;
  const registry = bag[INSPECT_REGISTRY_BINDING] as DurableObjectNamespace | undefined;
  if (url.pathname === "/_bynk/inspect/agents") {
    if (registry === undefined) return json({ context, instances: [] });
    const only = url.searchParams.get("class");
    const instances = await instancesOf(registry, only);
    return json({ context, instances });
  }
  const prefix = "/_bynk/inspect/agent/";
  if (url.pathname.startsWith(prefix)) {
    const className = decodeURIComponent(url.pathname.slice(prefix.length));
    const id = url.searchParams.get("id");
    if (!globalThis.Object.hasOwn(bindings, className) || id === null) return json({ error: "unknown agent" }, 404);
    const binding = bag[bindings[className]!] as InspectNamespace | undefined;
    if (binding === undefined) return json({ error: "unknown agent" }, 404);
    let stub;
    try {
      stub = binding.get(binding.idFromString(id));
    } catch {
      return json({ error: "unknown instance" }, 404);
    }
    const forward = new URL("https://_bynk/_bynk/inspect");
    for (const [k, v] of url.searchParams) if (k !== "id") forward.searchParams.set(k, v);
    return stub.fetch(forward.toString());
  }
  return new Response("Not Found", { status: 404 });
}

// #1653: the `__` aliases emitted code imports.
export { inspectAgent as __inspectAgent, inspectRoute as __inspectRoute, noteAgentInstance as __noteAgentInstance };
