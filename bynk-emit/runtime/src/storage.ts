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
  // P1 (#2590): a value as bytes.
  get(key: string, type: "arrayBuffer"): Promise<ArrayBuffer | null>;
  // P1 (#2590): a text value with its metadata, optionally edge-cached.
  getWithMetadata(
    key: string,
    options: { type: "text"; cacheTtl?: number },
  ): Promise<{ value: string | null; metadata: unknown }>;
  put(
    key: string,
    value: string | ArrayBuffer | ArrayBufferView,
    options?: { expirationTtl?: number; expiration?: number; metadata?: unknown },
  ): Promise<void>;
  delete(key: string): Promise<void>;
  // v0.23: the page shape WorkersKv's drain consumes (0050); P1 (#2590) adds
  // `limit` and each key's expiry and metadata, for `listPage`.
  list(options?: { prefix?: string; cursor?: string; limit?: number }): Promise<{
    keys: { name: string; expiration?: number; metadata?: unknown }[];
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
  // P1 (#2590): values are bytes, as on the platform (text is its UTF-8), with
  // metadata and an expiry beside them. Expiry is kept, not enforced.
  type Entry = { bytes: Uint8Array; metadata: unknown; expiration?: number };
  const entries = new Map<string, Entry>();
  const encode = (value: string | ArrayBuffer | ArrayBufferView): Uint8Array =>
    typeof value === "string"
      ? new TextEncoder().encode(value)
      : value instanceof ArrayBuffer
        ? new Uint8Array(value.slice(0))
        : new Uint8Array(value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength));
  function get(key: string): Promise<string | null>;
  function get(key: string, type: "arrayBuffer"): Promise<ArrayBuffer | null>;
  async function get(key: string, type?: "arrayBuffer"): Promise<string | ArrayBuffer | null> {
    const e = entries.get(key);
    if (e === undefined) return null;
    return type === "arrayBuffer" ? e.bytes.slice().buffer : new TextDecoder().decode(e.bytes);
  }
  return {
    get,
    async getWithMetadata(key: string) {
      const e = entries.get(key);
      return e === undefined
        ? { value: null, metadata: null }
        : { value: new TextDecoder().decode(e.bytes), metadata: e.metadata ?? null };
    },
    async put(
      key: string,
      value: string | ArrayBuffer | ArrayBufferView,
      options?: { expirationTtl?: number; expiration?: number; metadata?: unknown },
    ): Promise<void> {
      const expiration =
        options?.expiration ??
        (options?.expirationTtl === undefined ? undefined : Math.floor(Date.now() / 1000) + options.expirationTtl);
      entries.set(key, { bytes: encode(value), metadata: options?.metadata, expiration });
    },
    async delete(key: string): Promise<void> {
      entries.delete(key);
    },
    // Pages in key order; the cursor is the last key of the page.
    async list(options?: { prefix?: string; cursor?: string; limit?: number }) {
      const prefix = options?.prefix ?? "";
      const after = options?.cursor;
      const limit = options?.limit ?? 1000;
      const names = [...entries.keys()]
        .filter((k) => k.startsWith(prefix) && (after === undefined || k > after))
        .sort();
      const page = names.slice(0, limit);
      const complete = page.length === names.length;
      return {
        keys: page.map((name) => {
          const e = entries.get(name)!;
          return {
            name,
            ...(e.expiration === undefined ? {} : { expiration: e.expiration }),
            ...(e.metadata === undefined ? {} : { metadata: e.metadata }),
          };
        }),
        list_complete: complete,
        ...(complete ? {} : { cursor: page[page.length - 1] }),
      };
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
  readonly body: ReadableStream<Uint8Array>;
  arrayBuffer(): Promise<ArrayBuffer>;
}

export interface R2PutOptions {
  httpMetadata?: { contentType?: string };
  customMetadata?: Record<string, string>;
  onlyIf?: { etagMatches?: string; etagDoesNotMatch?: string };
}

/**
 * P5b (#2626): the part of Cloudflare's `R2MultipartUpload` that `WorkersR2`
 * uses. `resumeMultipartUpload` rebuilds one from its key and id without
 * checking either; an unknown upload fails at its first operation.
 */
export interface R2MultipartUpload {
  readonly key: string;
  readonly uploadId: string;
  uploadPart(partNumber: number, value: Uint8Array): Promise<{ partNumber: number; etag: string }>;
  complete(parts: { partNumber: number; etag: string }[]): Promise<R2ObjectMeta>;
  abort(): Promise<void>;
}

export interface R2Bucket {
  get(key: string): Promise<R2ObjectBody | null>;
  head(key: string): Promise<R2ObjectMeta | null>;
  // P5b (#2626): a stream is written whole; on Workers it must have a known
  // length (the readable half of a `FixedLengthStream`).
  put(key: string, value: Uint8Array | ReadableStream<Uint8Array>, options?: R2PutOptions): Promise<R2ObjectMeta | null>;
  delete(key: string): Promise<void>;
  list(options?: {
    prefix?: string;
    cursor?: string;
    limit?: number;
    include?: ("httpMetadata" | "customMetadata")[];
  }): Promise<{ objects: R2ObjectMeta[]; truncated: boolean; cursor?: string }>;
  createMultipartUpload(
    key: string,
    options?: { httpMetadata?: { contentType?: string }; customMetadata?: Record<string, string> },
  ): Promise<R2MultipartUpload>;
  resumeMultipartUpload(key: string, uploadId: string): R2MultipartUpload;
}

/**
 * An in-memory R2 bucket for multi-Worker integration tests, bound as that
 * participant's `env.R2_<NAME>` and fresh per test case, as `makeIntegrationKv`
 * is. It honours the preconditions `WorkersR2` sends and pages `list` in key
 * order at R2's own page size (`limit`, default 1000), the cursor being the
 * last key returned.
 *
 * P5b (#2626): `get`'s `body` streams the object in 64 KiB chunks, `put`
 * takes a stream as well as bytes, and multipart uploads complete as R2's do:
 * every listed part uploaded with that etag, part numbers 1 to 10,000, and
 * every part but the last the same size. R2's 5 MiB minimum part is not
 * enforced, so a test needn't build 5 MiB of bytes.
 */
export function makeIntegrationR2(): R2Bucket {
  type Stored = { meta: R2ObjectMeta; body: Uint8Array };
  type Upload = {
    options: { httpMetadata?: { contentType?: string }; customMetadata?: Record<string, string> } | undefined;
    parts: Map<number, { etag: string; body: Uint8Array }>;
  };
  const objects = new Map<string, Stored>();
  const uploads = new Map<string, Upload>();
  let version = 0;
  const store = (key: string, body: Uint8Array, options?: R2PutOptions): R2ObjectMeta => {
    version += 1;
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
  };
  const upload = (key: string, uploadId: string): R2MultipartUpload => {
    const open = (): Upload => {
      const u = uploads.get(`${key}\u0000${uploadId}`);
      if (!u) throw new Error(`R2: no multipart upload ${uploadId} of ${key}`);
      return u;
    };
    return {
      key,
      uploadId,
      async uploadPart(partNumber, value) {
        const u = open();
        if (!Number.isInteger(partNumber) || partNumber < 1 || partNumber > 10_000) {
          throw new Error(`R2: part number ${partNumber} is not between 1 and 10,000`);
        }
        version += 1;
        const etag = `part-${version}`;
        u.parts.set(partNumber, { etag, body: value.slice() });
        return { partNumber, etag };
      },
      async complete(parts) {
        const u = open();
        const listed = [...parts].sort((a, b) => a.partNumber - b.partNumber);
        const bodies = listed.map((p, i) => {
          const part = u.parts.get(p.partNumber);
          if (!part || part.etag !== p.etag) throw new Error(`R2: part ${p.partNumber} was not uploaded with etag ${p.etag}`);
          if (i > 0 && listed[i - 1].partNumber === p.partNumber) throw new Error(`R2: part ${p.partNumber} is listed twice`);
          return part.body;
        });
        if (bodies.length === 0) throw new Error("R2: a multipart upload needs at least one part");
        if (bodies.slice(0, -1).some((b) => b.length !== bodies[0].length)) {
          throw new Error("R2: every part but the last must be the same size");
        }
        if (bodies.length > 1 && bodies[bodies.length - 1].length > bodies[0].length) {
          throw new Error("R2: the last part must be no larger than the others");
        }
        const body = new Uint8Array(bodies.reduce((n, b) => n + b.length, 0));
        let at = 0;
        for (const b of bodies) {
          body.set(b, at);
          at += b.length;
        }
        uploads.delete(`${key}\u0000${uploadId}`);
        return store(key, body, u.options);
      },
      async abort() {
        uploads.delete(`${key}\u0000${uploadId}`);
      },
    };
  };
  return {
    async get(key: string): Promise<R2ObjectBody | null> {
      const s = objects.get(key);
      if (!s) return null;
      const bytes = s.body;
      let at = 0;
      const body = new ReadableStream<Uint8Array>({
        pull(controller) {
          if (at >= bytes.length) {
            controller.close();
            return;
          }
          controller.enqueue(bytes.slice(at, at + 65_536));
          at += 65_536;
        },
      });
      return {
        ...s.meta,
        body,
        async arrayBuffer(): Promise<ArrayBuffer> {
          return bytes.slice().buffer as ArrayBuffer;
        },
      };
    },
    async head(key: string): Promise<R2ObjectMeta | null> {
      return objects.get(key)?.meta ?? null;
    },
    async put(key: string, value: Uint8Array | ReadableStream<Uint8Array>, options?: R2PutOptions): Promise<R2ObjectMeta | null> {
      const body = value instanceof Uint8Array ? value.slice() : new Uint8Array(await new Response(value).arrayBuffer());
      const current = objects.get(key);
      const cond = options?.onlyIf;
      if (cond?.etagMatches !== undefined && current?.meta.etag !== cond.etagMatches) return null;
      if (cond?.etagDoesNotMatch !== undefined) {
        const clash = cond.etagDoesNotMatch === "*" ? current !== undefined : current?.meta.etag === cond.etagDoesNotMatch;
        if (clash) return null;
      }
      return store(key, body, options);
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
    async createMultipartUpload(key, options) {
      version += 1;
      const uploadId = `upload-${version}`;
      uploads.set(`${key}\u0000${uploadId}`, { options, parts: new Map() });
      return upload(key, uploadId);
    },
    resumeMultipartUpload(key, uploadId) {
      return upload(key, uploadId);
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

/**
 * Standard-library P2 (#2578): the part of Cloudflare's queue producer binding
 * that `WorkersQueue` uses. `delaySeconds` is whole seconds; every body is sent
 * as `contentType: "json"`, so the consumer's `msg.body` is the parsed value.
 */
export interface QueueProducer {
  send(body: unknown, options?: { contentType?: "json"; delaySeconds?: number }): Promise<void>;
  sendBatch(
    messages: Iterable<{ body: unknown; contentType?: "json"; delaySeconds?: number }>,
  ): Promise<void>;
}

/**
 * An in-memory queue producer for multi-Worker integration tests, bound as that
 * participant's `env.QUEUE_<NAME>` and fresh per test case, as
 * `makeIntegrationKv` is. It accepts every send and keeps the bodies it was
 * given in `sent`, in order; nothing is delivered to a consumer.
 */
export function makeIntegrationQueue(): QueueProducer & { sent: unknown[] } {
  const sent: unknown[] = [];
  return {
    sent,
    async send(body: unknown): Promise<void> {
      sent.push(body);
    },
    async sendBatch(messages: Iterable<{ body: unknown }>): Promise<void> {
      for (const m of messages) sent.push(m.body);
    },
  };
}

/**
 * Standard-library P3b (#2582): the part of Cloudflare's Analytics Engine
 * binding that `WorkersAnalytics` uses. The write is synchronous: the platform
 * batches it and sends it after the response.
 */
export interface AnalyticsDataset {
  writeDataPoint(point: { indexes?: string[]; blobs?: string[]; doubles?: number[] }): void;
}

/**
 * An in-memory Analytics Engine dataset for multi-Worker integration tests,
 * bound as that participant's `env.ANALYTICS_<NAME>` and fresh per test case,
 * as `makeIntegrationKv` is. It keeps every data point it is given in
 * `written`, in order.
 */
export function makeIntegrationAnalytics(): AnalyticsDataset & {
  written: { indexes?: string[]; blobs?: string[]; doubles?: number[] }[];
} {
  const written: { indexes?: string[]; blobs?: string[]; doubles?: number[] }[] = [];
  return {
    written,
    writeDataPoint(point): void {
      written.push(point);
    },
  };
}

/**
 * Standard-library P4a (#2602): the part of Cloudflare's D1 binding that
 * `WorkersD1` uses. Parameters are positional; a statement's `meta` reports
 * what a write changed.
 */
export interface D1Result {
  results: Record<string, unknown>[];
  meta: { changes?: number; last_row_id?: number };
}

export interface D1PreparedStatement {
  bind(...values: unknown[]): D1PreparedStatement;
  all(): Promise<D1Result>;
  first(): Promise<Record<string, unknown> | null>;
  run(): Promise<D1Result>;
}

export interface D1Database {
  prepare(sql: string): D1PreparedStatement;
  batch(statements: D1PreparedStatement[]): Promise<D1Result[]>;
}

// The part of Node's built-in `node:sqlite` the D1 double uses.
interface SqliteStatement {
  all(...params: unknown[]): Record<string, unknown>[];
  get(...params: unknown[]): Record<string, unknown> | undefined;
  run(...params: unknown[]): { changes: number | bigint; lastInsertRowid: number | bigint };
}
interface SqliteDatabase {
  prepare(sql: string): SqliteStatement;
  exec(sql: string): void;
}

/**
 * An in-memory D1 database for tests, bound as `env.D1_<NAME>` and fresh per
 * test case, as `makeIntegrationKv` is. It is real SQLite, through Node's
 * built-in `node:sqlite` (Node 22.13 or later), so statements run as they do
 * on D1: foreign keys enforced, integral numbers bound as `INTEGER`, and a
 * failure reported as D1 words it (`D1_ERROR: …: SQLITE_CONSTRAINT`). A
 * `batch` is one transaction; its results carry `meta`, not rows.
 *
 * P4b (#2608): `migrations` are the database's `[name, sql]` files in order,
 * each run in its own transaction before the database is handed out, as
 * `wrangler d1 migrations apply` runs them. One that fails faults, naming the
 * file, so every test using the database fails the same way.
 */
export function makeIntegrationD1(migrations: readonly (readonly [string, string])[] = []): D1Database {
  const node = (globalThis as { process?: { getBuiltinModule?(id: string): unknown } }).process;
  const sqlite = node?.getBuiltinModule?.("node:sqlite") as
    | { DatabaseSync: new (path: string) => SqliteDatabase }
    | undefined;
  if (sqlite === undefined) {
    throw new Error("bynk: the D1 test double needs Node's built-in `node:sqlite` (Node 22.13 or later)");
  }
  const db = new sqlite.DatabaseSync(":memory:");
  db.exec("PRAGMA foreign_keys = ON");
  for (const [name, sql] of migrations) {
    db.exec("BEGIN");
    try {
      db.exec(sql);
      db.exec("COMMIT");
    } catch (e) {
      db.exec("ROLLBACK");
      throw new Error(`bynk: D1 migration \`${name}\` failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }
  const d1Error = (e: unknown): Error => {
    const message = e instanceof Error ? e.message : String(e);
    const code = (e as { errcode?: number }).errcode;
    return new Error(
      code !== undefined && (code & 0xff) === 19
        ? `D1_ERROR: ${message}: SQLITE_CONSTRAINT`
        : `D1_ERROR: ${message}`,
    );
  };
  const attempt = <T>(f: () => T): T => {
    try {
      return f();
    } catch (e) {
      throw d1Error(e);
    }
  };
  const meta = (r: { changes: number | bigint; lastInsertRowid: number | bigint }) => ({
    changes: Number(r.changes),
    last_row_id: Number(r.lastInsertRowid),
  });
  const statement = (sql: string, params: unknown[]): D1PreparedStatement & { exec(): D1Result } => {
    const values = params.map((v) => (typeof v === "number" && Number.isInteger(v) ? BigInt(v) : v));
    // Node before 22.18 refuses positional values for numbered parameters
    // (`?1`), so those bind by name: `{ "?1": … }`, one key per number used.
    const numbered = [...new Set(sql.match(/\?\d+/g) ?? [])];
    const bound: unknown[] =
      numbered.length === 0
        ? values
        : [Object.fromEntries(numbered.map((name) => [name, values[Number(name.slice(1)) - 1] ?? null]))];
    const prepared = () => attempt(() => db.prepare(sql));
    return {
      bind: (...values: unknown[]) => statement(sql, values),
      all: async () => ({ results: attempt(() => prepared().all(...bound)), meta: {} }),
      first: async () => attempt(() => prepared().get(...bound)) ?? null,
      run: async () => ({ results: [], meta: meta(attempt(() => prepared().run(...bound))) }),
      exec: () => ({ results: [], meta: meta(attempt(() => prepared().run(...bound))) }),
    };
  };
  return {
    prepare: (sql: string) => statement(sql, []),
    async batch(statements: D1PreparedStatement[]): Promise<D1Result[]> {
      db.exec("BEGIN");
      try {
        const results = statements.map((s) => (s as D1PreparedStatement & { exec(): D1Result }).exec());
        db.exec("COMMIT");
        return results;
      } catch (e) {
        db.exec("ROLLBACK");
        throw e;
      }
    },
  };
}
