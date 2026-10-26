export interface DurableObjectStorage {
  get<T>(key: string): Promise<T | undefined>;
  put(key: string, value: unknown): Promise<void>;
  delete(key: string): Promise<boolean>;
  list<T>(options?: { prefix?: string }): Promise<Map<string, T>>;
}

export interface DurableObjectState {
  readonly storage: DurableObjectStorage;
  readonly id: { readonly name: string };
}

/**
 * Copy a stored value the way a Durable Object's storage round-trip does, for
 * every value that is **data**. #1660 (runtime-semantics track §3.5).
 *
 * - **Copied deeply:** plain objects, arrays, `Map`, `Uint8Array`, `Date`.
 * - **Passed through by reference:** an instance of any other class.
 *
 * The only such instances agent state holds are live handles. A held
 * `Connection` on the bundle target is a `TestConnection` stored directly in
 * state. On workers, a held connection persists its `connId` instead, and
 * `resolveConnection` re-presents the **same** live socket on load, so sharing
 * the handle here is the faithful analogue. `structuredClone` would instead
 * strip the handle's methods, and `send` would vanish.
 */
function cloneStored<T>(value: T): T {
  if (value === null || typeof value !== "object") return value;
  // `new Uint8Array(v)` copies; `slice()` on a Node `Buffer` (a Uint8Array) is a view.
  if (value instanceof Uint8Array) return new Uint8Array(value) as T;
  if (value instanceof Date) return new Date(value.getTime()) as T;
  if (value instanceof Map) {
    return new Map([...value].map(([k, v]) => [cloneStored(k), cloneStored(v)])) as T;
  }
  if (Array.isArray(value)) return value.map(cloneStored) as T;
  const proto = Object.getPrototypeOf(value);
  if (proto !== Object.prototype && proto !== null) return value;
  // `Object.fromEntries` *defines* each key, where `out[k] = v` would *set* it.
  // For an own `__proto__` key (which `JSON.parse` creates, so a decoded record
  // stored as a `Map[String, _]` can carry one), a set would reassign the copy's
  // prototype, or silently drop a primitive, instead of copying the entry.
  return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, cloneStored(v)])) as T;
}

/**
 * The bundle-target (and `bynkc test`) stand-in for a Durable Object's storage.
 *
 * #1660 (runtime-semantics track §3.5): every value is copied (`cloneStored`) on
 * the way in **and** on the way out, as workerd's storage copies it. A value read
 * back is never the object that was written. Two things depend on this.
 *
 * - **Fidelity.** Before #1660 this class handed back the stored reference. That
 *   let `bynkc test` pass programs that cannot work under workerd. A loaded enum
 *   compared `===` against its singleton constant (so reference `==`, #1652, was
 *   masked), and a handler's write through a shallow copy of loaded state was
 *   visible to the next load.
 * - **Atomicity (ADR 0109).** A handler begins from
 *   `{ ...(await this.loadState()) }`, which is a shallow copy. With a shared
 *   reference, a store write such as `__state.items[k] = v` mutated the record
 *   still held here, so a commit later *refused* by an invariant had already
 *   happened. Cloning on `get` gives each handler its own state, so a refused
 *   commit leaves storage untouched. The regression is pinned by
 *   `bynkc/tests/store_behaviour.rs`'s
 *   `refused_commit_does_not_leak_an_in_place_store_write`.
 */
export class InMemoryStorage implements DurableObjectStorage {
  private data = new Map<string, unknown>();

  async get<T>(key: string): Promise<T | undefined> {
    const value = this.data.get(key);
    return (value === undefined ? undefined : cloneStored(value)) as T | undefined;
  }

  async put(key: string, value: unknown): Promise<void> {
    this.data.set(key, cloneStored(value));
  }

  async delete(key: string): Promise<boolean> {
    return this.data.delete(key);
  }

  async list<T>(options?: { prefix?: string }): Promise<Map<string, T>> {
    const prefix = options?.prefix ?? "";
    const out = new Map<string, T>();
    for (const [k, v] of this.data) {
      if (k.startsWith(prefix)) out.set(k, cloneStored(v) as T);
    }
    return out;
  }
}

export function makeTestState(name: string): DurableObjectState {
  return {
    storage: new InMemoryStorage(),
    id: { name },
  };
}

export interface KVNamespace {
  get(key: string): Promise<string | null>;
  put(key: string, value: string, options?: { expirationTtl?: number }): Promise<void>;
  delete(key: string): Promise<void>;
  // v0.23: the page shape WorkersKv's drain consumes (0050).
  list(options?: { prefix?: string; cursor?: string }): Promise<{
    keys: { name: string }[];
    list_complete: boolean;
    cursor?: string;
  }>;
}

/**
 * #1959: an in-memory KV namespace for multi-Worker integration tests. The
 * harness makes a fresh one per participant per test case and binds it as
 * that participant's `env.KV`, as `bynk deploy` provisions one namespace per
 * Worker. Keys list in order, on one page; `expirationTtl` is accepted and
 * ignored, since a test case does not outlive it.
 */
export function makeIntegrationKv(): KVNamespace {
  const values = new Map<string, string>();
  return {
    async get(key: string): Promise<string | null> {
      return values.get(key) ?? null;
    },
    async put(key: string, value: string): Promise<void> {
      values.set(key, value);
    },
    async delete(key: string): Promise<void> {
      values.delete(key);
    },
    async list(options?: { prefix?: string; cursor?: string }) {
      const prefix = options?.prefix ?? "";
      const names = [...values.keys()].filter((k) => k.startsWith(prefix)).sort();
      return { keys: names.map((name) => ({ name })), list_complete: true };
    },
  };
}

/**
 * P5 (#2323): the part of Cloudflare's `R2Bucket` that `WorkersR2` uses,
 * declared here so the binding and the in-memory double share one shape.
 * `put` resolves `null` when its `onlyIf` precondition failed;
 * `etagDoesNotMatch: "*"` is the create-only precondition.
 */
export interface R2ObjectMeta {
  readonly key: string;
  readonly size: number;
  readonly etag: string;
  readonly uploaded: Date;
  readonly httpMetadata?: { contentType?: string };
  readonly customMetadata?: Record<string, string>;
}

export interface R2ObjectBody extends R2ObjectMeta {
  arrayBuffer(): Promise<ArrayBuffer>;
}

export interface R2PutOptions {
  httpMetadata?: { contentType?: string };
  customMetadata?: Record<string, string>;
  onlyIf?: { etagMatches?: string; etagDoesNotMatch?: string };
}

export interface R2Bucket {
  get(key: string): Promise<R2ObjectBody | null>;
  head(key: string): Promise<R2ObjectMeta | null>;
  put(key: string, value: Uint8Array, options?: R2PutOptions): Promise<R2ObjectMeta | null>;
  delete(key: string): Promise<void>;
  list(options?: {
    prefix?: string;
    cursor?: string;
    limit?: number;
    include?: ("httpMetadata" | "customMetadata")[];
  }): Promise<{ objects: R2ObjectMeta[]; truncated: boolean; cursor?: string }>;
}

/**
 * An in-memory R2 bucket for multi-Worker integration tests, bound as that
 * participant's `env.R2_<NAME>` and fresh per test case, as `makeIntegrationKv`
 * is. It honours the preconditions `WorkersR2` sends and pages `list` in key
 * order at R2's own page size (`limit`, default 1000), the cursor being the
 * last key returned.
 */
export function makeIntegrationR2(): R2Bucket {
  type Stored = { meta: R2ObjectMeta; body: Uint8Array };
  const objects = new Map<string, Stored>();
  let version = 0;
  return {
    async get(key: string): Promise<R2ObjectBody | null> {
      const s = objects.get(key);
      if (!s) return null;
      const body = s.body;
      return {
        ...s.meta,
        async arrayBuffer(): Promise<ArrayBuffer> {
          return body.slice().buffer as ArrayBuffer;
        },
      };
    },
    async head(key: string): Promise<R2ObjectMeta | null> {
      return objects.get(key)?.meta ?? null;
    },
    async put(key: string, value: Uint8Array, options?: R2PutOptions): Promise<R2ObjectMeta | null> {
      const current = objects.get(key);
      const cond = options?.onlyIf;
      if (cond?.etagMatches !== undefined && current?.meta.etag !== cond.etagMatches) return null;
      if (cond?.etagDoesNotMatch !== undefined) {
        const clash = cond.etagDoesNotMatch === "*" ? current !== undefined : current?.meta.etag === cond.etagDoesNotMatch;
        if (clash) return null;
      }
      version += 1;
      const body = value.slice();
      const contentType = options?.httpMetadata?.contentType;
      const meta: R2ObjectMeta = {
        key,
        size: body.length,
        etag: `etag-${version}`,
        uploaded: new Date(),
        httpMetadata: contentType === undefined ? {} : { contentType },
        customMetadata: { ...(options?.customMetadata ?? {}) },
      };
      objects.set(key, { meta, body });
      return meta;
    },
    async delete(key: string): Promise<void> {
      objects.delete(key);
    },
    async list(options) {
      const prefix = options?.prefix ?? "";
      const limit = options?.limit ?? 1000;
      const after = options?.cursor;
      const keys = [...objects.keys()]
        .filter((k) => k.startsWith(prefix) && (after === undefined || k > after))
        .sort();
      const page = keys.slice(0, limit);
      const truncated = keys.length > page.length;
      return {
        objects: page.map((k) => objects.get(k)!.meta),
        truncated,
        cursor: truncated ? page[page.length - 1] : undefined,
      };
    },
  };
}

/**
 * Standard-library P3 (#2342): the part of Cloudflare's rate limiting binding
 * that `WorkersRateLimit` uses. The limit and the period are the binding's
 * configuration, not arguments.
 */
export interface RateLimiter {
  limit(options: { key: string }): Promise<{ success: boolean }>;
}

/**
 * An in-memory rate limiter for multi-Worker integration tests, bound as that
 * participant's `env.RATELIMIT_<NAME>` and fresh per test case, as
 * `makeIntegrationKv` is. It counts calls per key over fixed windows of
 * `periodSecs` (aligned to multiples of the period, as the platform's are)
 * and allows `limit` of them in each, on the harness clock (`now`). Unlike
 * the platform it is exact; the
 * platform's per-location counting only ever allows more.
 */
export function makeIntegrationRateLimit(
  limit: number,
  periodSecs: number,
  // Milliseconds; the harness passes its clock. Standing still at 0 when
  // none is given keeps every run's answers the same.
  now: () => number = () => 0,
): RateLimiter {
  const counts = new Map<string, { window: number; count: number }>();
  return {
    async limit({ key }: { key: string }): Promise<{ success: boolean }> {
      const window = Math.floor(now() / (periodSecs * 1000));
      const entry = counts.get(key);
      const current = entry !== undefined && entry.window === window ? entry : { window, count: 0 };
      current.count += 1;
      counts.set(key, current);
      return { success: current.count <= limit };
    },
  };
}

// #1653: the `__` aliases emitted code imports.
export { makeIntegrationKv as __makeIntegrationKv, makeIntegrationR2 as __makeIntegrationR2, makeIntegrationRateLimit as __makeIntegrationRateLimit, makeTestState as __makeTestState };
export type { DurableObjectState as __DurableObjectState, KVNamespace as __KVNamespace, R2Bucket as __R2Bucket, RateLimiter as __RateLimiter };
