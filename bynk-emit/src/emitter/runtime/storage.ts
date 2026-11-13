// The Durable Object storage surface the runtime speaks. The single-key forms
// are what every emitted agent uses today; the batched, ranged and transactional
// forms are the per-entry storage track's (#3195, `design/tracks/per-entry-storage.md`).
// They are the Cloudflare KV-API forms, so a real SQLite-backed object satisfies
// this interface as it stands.
export interface DurableObjectListOptions {
  readonly prefix?: string;
  // Inclusive lower bound.
  readonly start?: string;
  // Exclusive lower bound. Not used with `start`.
  readonly startAfter?: string;
  // Exclusive upper bound.
  readonly end?: string;
  readonly limit?: number;
  readonly reverse?: boolean;
}

// The operations a `transaction` closure runs. Reads see the transaction's own
// writes; nothing is visible outside until the closure returns.
export interface DurableObjectTransaction {
  get<T>(key: string): Promise<T | undefined>;
  get<T>(keys: readonly string[]): Promise<Map<string, T>>;
  put(key: string, value: unknown): Promise<void>;
  put(entries: Readonly<Record<string, unknown>>): Promise<void>;
  delete(key: string): Promise<boolean>;
  delete(keys: readonly string[]): Promise<number>;
  list<T>(options?: DurableObjectListOptions): Promise<Map<string, T>>;
  rollback(): void;
}

export interface DurableObjectStorage {
  get<T>(key: string): Promise<T | undefined>;
  get<T>(keys: readonly string[]): Promise<Map<string, T>>;
  put(key: string, value: unknown): Promise<void>;
  put(entries: Readonly<Record<string, unknown>>): Promise<void>;
  delete(key: string): Promise<boolean>;
  delete(keys: readonly string[]): Promise<number>;
  list<T>(options?: DurableObjectListOptions): Promise<Map<string, T>>;
  // Runs `closure` as one atomic unit: every write commits together when it
  // returns, none when it throws or calls `rollback()`.
  transaction<T>(closure: (txn: DurableObjectTransaction) => Promise<T>): Promise<T>;
}

/** Cloudflare's documented limit on keys per batched `get`, `put` or `delete`. */
export const STORAGE_BATCH_LIMIT = 128;

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
  private inTransaction = false;

  // Operation counts, so a behavioural test can assert that a handler's storage
  // I/O does not grow with the size of the state (#3189). `keysRead` and
  // `keysWritten` count keys, `reads` and `writes` count calls.
  readonly stats = { reads: 0, writes: 0, lists: 0, transactions: 0, keysRead: 0, keysWritten: 0 };

  resetStats(): void {
    this.stats.reads = 0;
    this.stats.writes = 0;
    this.stats.lists = 0;
    this.stats.transactions = 0;
    this.stats.keysRead = 0;
    this.stats.keysWritten = 0;
  }

  /** The number of stored keys, for assertions about what a flush left behind. */
  get size(): number {
    return this.data.size;
  }

  async get<T>(key: string): Promise<T | undefined>;
  async get<T>(keys: readonly string[]): Promise<Map<string, T>>;
  async get<T>(keyOrKeys: string | readonly string[]): Promise<T | undefined | Map<string, T>> {
    if (typeof keyOrKeys === "string") {
      this.stats.reads += 1;
      this.stats.keysRead += 1;
      const value = this.data.get(keyOrKeys);
      return (value === undefined ? undefined : cloneStored(value)) as T | undefined;
    }
    checkBatch("get", keyOrKeys.length);
    this.stats.reads += 1;
    this.stats.keysRead += keyOrKeys.length;
    return readMany<T>(this.data, keyOrKeys);
  }

  async put(key: string, value: unknown): Promise<void>;
  async put(entries: Readonly<Record<string, unknown>>): Promise<void>;
  async put(keyOrEntries: string | Readonly<Record<string, unknown>>, value?: unknown): Promise<void> {
    if (typeof keyOrEntries === "string") {
      checkDefined(keyOrEntries, value);
      this.stats.writes += 1;
      this.stats.keysWritten += 1;
      this.data.set(keyOrEntries, cloneStored(value));
      return;
    }
    const entries = Object.entries(keyOrEntries);
    checkBatch("put", entries.length);
    for (const [k, v] of entries) checkDefined(k, v);
    this.stats.writes += 1;
    this.stats.keysWritten += entries.length;
    for (const [k, v] of entries) this.data.set(k, cloneStored(v));
  }

  async delete(key: string): Promise<boolean>;
  async delete(keys: readonly string[]): Promise<number>;
  async delete(keyOrKeys: string | readonly string[]): Promise<boolean | number> {
    if (typeof keyOrKeys === "string") {
      this.stats.writes += 1;
      this.stats.keysWritten += 1;
      return this.data.delete(keyOrKeys);
    }
    checkBatch("delete", keyOrKeys.length);
    this.stats.writes += 1;
    this.stats.keysWritten += keyOrKeys.length;
    let n = 0;
    for (const k of keyOrKeys) if (this.data.delete(k)) n += 1;
    return n;
  }

  async list<T>(options?: DurableObjectListOptions): Promise<Map<string, T>> {
    this.stats.lists += 1;
    const out = listFrom<T>(this.data, options);
    this.stats.keysRead += out.size;
    return out;
  }

  // Every write commits together when `closure` returns, and none when it throws
  // or calls `rollback()`. A transaction cannot nest, and two cannot overlap on
  // one storage: workerd's input gate keeps a transaction's reads and writes
  // from interleaving with another event's, which an in-memory double reproduces
  // by refusing the overlap instead of silently interleaving.
  async transaction<T>(closure: (txn: DurableObjectTransaction) => Promise<T>): Promise<T> {
    if (this.inTransaction) throw new Error("InMemoryStorage: transactions cannot overlap or nest");
    this.inTransaction = true;
    this.stats.transactions += 1;
    const staged = new Map<string, unknown>(); // a value, or `undefined` for a delete
    let rolledBack = false;
    const view = (): Map<string, unknown> => {
      const v = new Map(this.data);
      for (const [k, x] of staged) {
        if (x === undefined) v.delete(k);
        else v.set(k, x);
      }
      return v;
    };
    const txn: DurableObjectTransaction = {
      get: (async (keyOrKeys: string | readonly string[]) => {
        const v = view();
        if (typeof keyOrKeys === "string") {
          const x = v.get(keyOrKeys);
          return x === undefined ? undefined : cloneStored(x);
        }
        checkBatch("get", keyOrKeys.length);
        return readMany(v, keyOrKeys);
      }) as DurableObjectTransaction["get"],
      put: (async (keyOrEntries: string | Readonly<Record<string, unknown>>, value?: unknown) => {
        if (typeof keyOrEntries === "string") {
          checkDefined(keyOrEntries, value);
          staged.set(keyOrEntries, cloneStored(value));
          return;
        }
        const entries = Object.entries(keyOrEntries);
        checkBatch("put", entries.length);
        for (const [k, x] of entries) checkDefined(k, x);
        for (const [k, x] of entries) staged.set(k, cloneStored(x));
      }) as DurableObjectTransaction["put"],
      delete: (async (keyOrKeys: string | readonly string[]) => {
        if (typeof keyOrKeys === "string") {
          const had = view().has(keyOrKeys);
          staged.set(keyOrKeys, undefined);
          return had;
        }
        checkBatch("delete", keyOrKeys.length);
        const v = view();
        let n = 0;
        for (const k of keyOrKeys) {
          if (v.has(k)) n += 1;
          staged.set(k, undefined);
        }
        return n;
      }) as DurableObjectTransaction["delete"],
      list: async <U,>(options?: DurableObjectListOptions) => listFrom<U>(view(), options),
      rollback: () => {
        rolledBack = true;
      },
    };
    try {
      const result = await closure(txn);
      if (!rolledBack) {
        let written = 0;
        for (const [k, x] of staged) {
          written += 1;
          if (x === undefined) this.data.delete(k);
          else this.data.set(k, x);
        }
        this.stats.writes += 1;
        this.stats.keysWritten += written;
      }
      return result;
    } finally {
      this.inTransaction = false;
    }
  }
}

// The platform refuses an `undefined` value; the double is not more lenient, and
// a staged `undefined` is this double's own delete marker, so it must not slip in.
function checkDefined(key: string, value: unknown): void {
  if (value === undefined) throw new TypeError(`cannot store an undefined value under key ${key}`);
}

function checkBatch(op: string, n: number): void {
  if (n > STORAGE_BATCH_LIMIT) {
    throw new RangeError(
      `Durable Object storage ${op}() supports up to ${STORAGE_BATCH_LIMIT} keys at a time, got ${n}`,
    );
  }
}

function readMany<T>(data: ReadonlyMap<string, unknown>, keys: readonly string[]): Map<string, T> {
  const out = new Map<string, T>();
  // Ascending key order, as the platform returns a batched get.
  for (const k of [...keys].sort(utf8Compare)) {
    const v = data.get(k);
    if (v !== undefined) out.set(k, cloneStored(v) as T);
  }
  return out;
}

// Keys sort by their UTF-8 bytes, as Cloudflare documents: not by UTF-16 code
// unit, which orders a character above U+FFFF before one in U+E000..U+FFFF.
const encoder = new TextEncoder();
export function utf8Compare(a: string, b: string): number {
  if (a === b) return 0;
  const x = encoder.encode(a);
  const y = encoder.encode(b);
  const n = Math.min(x.length, y.length);
  for (let i = 0; i < n; i += 1) {
    const d = (x[i] as number) - (y[i] as number);
    if (d !== 0) return d;
  }
  return x.length - y.length;
}

function listFrom<T>(data: ReadonlyMap<string, unknown>, options?: DurableObjectListOptions): Map<string, T> {
  const prefix = options?.prefix ?? "";
  const keys: string[] = [];
  for (const k of data.keys()) {
    if (!k.startsWith(prefix)) continue;
    if (options?.start !== undefined && utf8Compare(k, options.start) < 0) continue;
    if (options?.startAfter !== undefined && utf8Compare(k, options.startAfter) <= 0) continue;
    if (options?.end !== undefined && utf8Compare(k, options.end) >= 0) continue;
    keys.push(k);
  }
  keys.sort(utf8Compare);
  if (options?.reverse === true) keys.reverse();
  const limited = options?.limit === undefined ? keys : keys.slice(0, options.limit);
  const out = new Map<string, T>();
  for (const k of limited) out.set(k, cloneStored(data.get(k)) as T);
  return out;
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

/** A character of a SQL identifier, as SQLite's tokenizer sees it: ASCII letters, digits, `_`, `$`, and (#3157) anything non-ASCII. */
function isWord(ch: string): boolean {
  return /[A-Za-z0-9_$]/.test(ch) || ch.charCodeAt(0) >= 0x80;
}

/**
 * What a statement's SQL takes: the count is SQLite's `sqlite3_bind_parameter_count`,
 * which D1 compares to the number bound. A bare `?` is one past the highest
 * index so far, `?N` is index N, and a named parameter (`:a`, `@a`, `$a`) is
 * one past the highest unless the name was seen. Quoted text (`'…'`, `"…"`,
 * `` `…` ``, `[…]`) and comments hold no parameters.
 */
function sqlParameters(sql: string): { count: number; numbered: string[]; bare: [number, number][] } {
  let count = 0;
  const numbered = new Set<string>();
  const bare: [number, number][] = []; // each bare `?`: its offset and SQLite index
  const names = new Set<string>();
  const n = sql.length;
  let i = 0;
  const skipTo = (close: string, doubled: boolean): void => {
    i++;
    while (i < n) {
      if (sql[i] === close) {
        if (doubled && sql[i + 1] === close) i += 2;
        else break;
      } else i++;
    }
    i++;
  };
  while (i < n) {
    const c = sql[i]!;
    if (c === "'" || c === '"' || c === "`") skipTo(c, true);
    else if (c === "[") skipTo("]", false);
    else if (c === "-" && sql[i + 1] === "-") {
      while (i < n && sql[i] !== "\n") i++;
    } else if (c === "/" && sql[i + 1] === "*") {
      const end = sql.indexOf("*/", i + 2);
      i = end < 0 ? n : end + 2;
    } else if (c === "?") {
      const at = i++;
      const start = i;
      while (i < n && sql[i]! >= "0" && sql[i]! <= "9") i++;
      if (i > start) {
        numbered.add(`?${sql.slice(start, i)}`); // the raw spelling, `?01` included
        count = Math.max(count, Number(sql.slice(start, i)));
      } else {
        count++;
        bare.push([at, count]);
      }
    } else if (
      // `$` continues an identifier (`a$b`); `:` and `@` always start a parameter, as in SQLite.
      (c === ":" || c === "@" || (c === "$" && !(i > 0 && isWord(sql[i - 1]!))))) {
      const start = i++;
      while (i < n && isWord(sql[i]!)) i++;
      if (i - start > 1) {
        const name = sql.slice(start, i);
        if (!names.has(name)) {
          names.add(name);
          count++;
        }
      }
    } else i++;
  }
  return { count, numbered: [...numbered], bare };
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
    // A mix with bare `?` is spelled out first (`?` becomes its index, as
    // SQLite numbers it), since the oldest Node binds only by name.
    let text = sql;
    let scan = sqlParameters(sql);
    if (scan.bare.length > 0 && scan.numbered.length > 0) {
      for (const [at, index] of [...scan.bare].reverse()) text = `${text.slice(0, at)}?${index}${text.slice(at + 1)}`;
      scan = sqlParameters(text);
    }
    const { count, numbered } = scan;
    const bound: unknown[] =
      numbered.length === 0
        ? values
        : [Object.fromEntries(numbered.map((name) => [name, values[Number(name.slice(1)) - 1] ?? null]))];
    // D1 refuses a count that differs from what the SQL uses, too many or too
    // few (#3003); SQLite alone would ignore the extras and bind NULL for the rest.
    // Every caller wraps `prepared()` in `attempt`, which words the failure.
    const prepared = () => {
      if (params.length !== count) {
        throw new Error("Wrong number of parameter bindings for SQL query.");
      }
      return db.prepare(text);
    };
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

/**
 * Version metadata (#3300): Cloudflare's Version metadata binding, which
 * `WorkersVersion` reads. `timestamp` is the version's creation time as an
 * ISO-8601 string; `tag` is empty when the version has none.
 */
export interface WorkerVersionMetadata {
  readonly id: string;
  readonly tag: string;
  readonly timestamp: string;
}

/**
 * The Version metadata binding of a multi-Worker integration test participant,
 * bound as its `env.CF_VERSION_METADATA`: a fixed version (id `"integration"`,
 * no tag, created at the epoch), so a case's assertions do not depend on when
 * it runs.
 */
export function makeIntegrationVersionMetadata(): WorkerVersionMetadata {
  return { id: "integration", tag: "", timestamp: "1970-01-01T00:00:00.000Z" };
}

/**
 * Workers AI (#3478): Cloudflare's AI binding, which `WorkersAi` reads. `run`
 * takes the model id and its input object, and answers the model's output (a
 * JSON value, or bytes or a stream for a model that sends those).
 */
export interface WorkerAiBinding {
  run(model: string, input: Record<string, unknown>): Promise<unknown>;
}

/**
 * The AI binding of a multi-Worker integration test participant, bound as its
 * `env.AI`. A test never reaches Workers AI, so `run` faults until the case
 * stubs the capability's operation. There is no fake model: a fabricated reply
 * would make a test pass for the wrong reason.
 */
export function makeIntegrationAi(): WorkerAiBinding {
  return {
    async run(): Promise<unknown> {
      throw new globalThis.Error(
        "bynk: the `Ai` binding is not stubbed, and a test never reaches Workers AI — stub the operation the case calls (`stub Ai.run(_, _) returns …`)",
      );
    },
  };
}

/**
 * An AI Search instance binding (#3480): Cloudflare's `ai_search` binding,
 * which `WorkersAiSearch` reads. Requests and replies are the platform's own
 * shapes (`ai_search_options`, `chunks`, `choices`), and the binding is the
 * instance itself, so its methods are called directly.
 */
export interface AiSearchInstance {
  search(request: Record<string, unknown>): Promise<unknown>;
  chatCompletions(request: Record<string, unknown>): Promise<unknown>;
}

/**
 * The AI Search binding of a multi-Worker integration test participant, bound
 * as its `env.<binding>`. A test never reaches AI Search, so every operation
 * faults, naming the binding, until the case stubs the capability's operation.
 */
export function makeIntegrationAiSearch(binding: string): AiSearchInstance {
  const unstubbed = async (): Promise<never> => {
    throw new globalThis.Error(
      `bynk: the \`${binding}\` AI Search binding is not stubbed, and a test never reaches AI Search — stub the operation the case calls (for example \`stub <Name>.search(_, _) returns …\`)`,
    );
  };
  return { search: unstubbed, chatCompletions: unstubbed };
}

/**
 * The Durable Object namespace of a `Container` instance (#3477), as the
 * multi-Worker integration harness binds it as `env.<binding>`. There is no
 * in-process container, so every call faults, naming the binding, until the
 * case stubs the instance (`stub Renderer.send(_, _) returns …`). An in-memory
 * echo is deliberately not offered: it would pass cases a real image fails.
 */
export function makeIntegrationContainer(binding: string): {
  idFromName(name: string): string;
  get(id: string): { fetch(request: unknown): Promise<never> };
} {
  return {
    idFromName: (name) => name,
    get: (id) => ({
      fetch: async (): Promise<never> => {
        throw new globalThis.Error(
          `bynk: the \`${binding}\` container \`${id}\` is not stubbed, and a test never starts a container — stub the operation the case calls (for example \`stub <Name>.send(_, _) returns …\`)`,
        );
      },
    }),
  };
}

/**
 * A Secrets Store secret binding (#3301): Cloudflare's `secrets_store_secrets`
 * binding, which `WorkersStoredSecret` reads. `get` resolves to the secret's
 * value, so nothing that holds one may log it.
 */
export interface SecretsStoreSecret {
  get(): Promise<string>;
}

/**
 * The Secrets Store binding of a multi-Worker integration test participant,
 * bound as its `env.<binding>`. A test never reaches the account's store, so
 * `get` faults, naming the binding and never a value, until the case stubs the
 * instance's `get`.
 */
export function makeIntegrationSecretsStoreSecret(binding: string): SecretsStoreSecret {
  return {
    async get(): Promise<string> {
      throw new globalThis.Error(
        `bynk: Secrets Store binding \`${binding}\` has no value in a test, which never reads the account's store — add \`stub <Capability>.get() returns …\``,
      );
    },
  };
}

/**
 * Cloudflare's `send_email` binding (#3302), which `WorkersEmail` calls with a
 * message builder. It resolves to the platform's message id, or rejects with an
 * `Error` carrying a `code` (`E_SENDER_NOT_VERIFIED`, …). A builder holds
 * message bodies and attachment content, so nothing that holds one may log it.
 */
export interface SendEmail {
  send(builder: Record<string, unknown>): Promise<{ messageId: string }>;
}

/**
 * The `send_email` binding of a multi-Worker integration test participant,
 * bound as its `env.<binding>`: an in-memory outbox. Every builder is kept (in
 * `sent`, in order) and answered with a deterministic id, so a test reads what
 * was sent and no mail leaves the process.
 */
export function makeIntegrationEmail(): SendEmail & { sent: Record<string, unknown>[] } {
  const sent: Record<string, unknown>[] = [];
  return {
    sent,
    async send(builder: Record<string, unknown>): Promise<{ messageId: string }> {
      sent.push(builder);
      return { messageId: `test-message-${sent.length}` };
    },
  };
}

/**
 * Cloudflare's Hyperdrive binding (#3318): `env.<binding>`, whose
 * `connectionString` the Postgres driver connects with. The string is a
 * secret, so nothing that reads it may log it.
 */
export interface Hyperdrive {
  readonly connectionString: string;
  /** A `MySql` instance's driver connects from the parts (Cloudflare's mysql2 guide); `password` is a secret. */
  readonly host?: string;
  readonly port?: number;
  readonly user?: string;
  readonly password?: string;
  readonly database?: string;
}

/**
 * The Hyperdrive binding of a multi-Worker integration test participant, bound
 * as its `env.<binding>`. A test never reaches a database, so reading its
 * `connectionString` faults, naming the binding, until the case stubs the
 * instance's operations (a stub answers before the binding is read).
 */
export function makeIntegrationHyperdrive(binding: string): Hyperdrive {
  return {
    get connectionString(): string {
      throw new globalThis.Error(
        `bynk: Hyperdrive binding \`${binding}\` has no database in a test, which never reaches a database — add \`stub <Capability>.all(_, _) returns …\` (or \`first\` or \`run\`)`,
      );
    },
  };
}

// #1653: the `__` aliases emitted code imports.
export { makeIntegrationAi as __makeIntegrationAi, makeIntegrationContainer as __makeIntegrationContainer, makeIntegrationAnalytics as __makeIntegrationAnalytics, makeIntegrationD1 as __makeIntegrationD1, makeIntegrationEmail as __makeIntegrationEmail, makeIntegrationHyperdrive as __makeIntegrationHyperdrive, makeIntegrationKv as __makeIntegrationKv, makeIntegrationQueue as __makeIntegrationQueue, makeIntegrationR2 as __makeIntegrationR2, makeIntegrationRateLimit as __makeIntegrationRateLimit, makeIntegrationSecretsStoreSecret as __makeIntegrationSecretsStoreSecret, makeIntegrationAiSearch as __makeIntegrationAiSearch, makeIntegrationVersionMetadata as __makeIntegrationVersionMetadata, makeTestState as __makeTestState };
export type { AiSearchInstance as __AiSearchInstance, AnalyticsDataset as __AnalyticsDataset, D1Database as __D1Database, DurableObjectState as __DurableObjectState, Hyperdrive as __Hyperdrive, KVNamespace as __KVNamespace, QueueProducer as __QueueProducer, R2Bucket as __R2Bucket, RateLimiter as __RateLimiter, SecretsStoreSecret as __SecretsStoreSecret, SendEmail as __SendEmail, WorkerAiBinding as __WorkerAiBinding, WorkerVersionMetadata as __WorkerVersionMetadata };
