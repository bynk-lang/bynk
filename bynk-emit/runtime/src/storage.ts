import { __bynkCmpString } from "./compare.ts";
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
  // Deletes every key of this object's storage (#3931: `self.erase()`).
  deleteAll(): Promise<void>;
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

  async deleteAll(): Promise<void> {
    if (this.inTransaction) throw new Error("InMemoryStorage: deleteAll cannot run inside a transaction");
    this.stats.writes += 1;
    this.stats.keysWritten += this.data.size;
    this.data.clear();
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

// Keys sort by their UTF-8 bytes, as Cloudflare documents: that is code-point
// order, the same order as `String` comparison in Bynk (#3954), so there is one
// comparator.
export const utf8Compare = __bynkCmpString;

function listFrom<T>(data: ReadonlyMap<string, unknown>, options?: DurableObjectListOptions): Map<string, T> {
  // As Durable Object storage does (#3747), so a caller that passes both fails here too.
  if (options?.start !== undefined && options.startAfter !== undefined) {
    throw new TypeError("list() cannot be called with both start and startAfter values.");
  }
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
 * Standard-library P3 (#2342): the part of Cloudflare's rate limiting binding
 * that `WorkersRateLimit` uses. The limit and the period are the binding's
 * configuration, not arguments.
 */
export interface RateLimiter {
  limit(options: { key: string }): Promise<{ success: boolean }>;
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
 * Standard-library P3b (#2582): the part of Cloudflare's Analytics Engine
 * binding that `WorkersAnalytics` uses. The write is synchronous: the platform
 * batches it and sends it after the response.
 */
export interface AnalyticsDataset {
  writeDataPoint(point: { indexes?: string[]; blobs?: string[]; doubles?: number[] }): void;
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
 * Workers AI (#3478): Cloudflare's AI binding, which `WorkersAi` reads. `run`
 * takes the model id and its input object, and answers the model's output (a
 * JSON value, or bytes or a stream for a model that sends those).
 */
export interface WorkerAiBinding {
  run(model: string, input: Record<string, unknown>): Promise<unknown>;
}

/**
 * Analytics SQL (#3513 B): Cloudflare's account-scoped binding, which `WorkersAnalyticsSql`
 * reads. `query` takes one `SELECT` and its positional parameters, and answers the rows with
 * the cost of the query; a failure rejects with an error carrying a boolean `retryable`.
 */
export interface AnalyticsSqlBinding {
  query(request: { query: string; params?: unknown[] }): Promise<unknown>;
}

/**
 * Cloudflare Images (#3515): the `images` binding, which `WorkersImages` reads.
 * `input` starts a pipeline over the image's bytes; `transform` and `draw`
 * chain, and `output` finishes with the format. The result's `image()` is the
 * bytes as a stream and `contentType()` the type they were written with.
 */
export interface WorkerImagesTransformer {
  transform(options: Record<string, unknown>): WorkerImagesTransformer;
  draw(overlay: WorkerImagesTransformer, options?: Record<string, unknown>): WorkerImagesTransformer;
  output(options: Record<string, unknown>): Promise<WorkerImagesResult>;
}
export interface WorkerImagesResult {
  image(): ReadableStream<Uint8Array>;
  contentType(): string;
}
export interface WorkerImagesBinding {
  input(stream: ReadableStream<Uint8Array>): WorkerImagesTransformer;
  info(
    stream: ReadableStream<Uint8Array>,
  ): Promise<{ format: string; fileSize?: number; width?: number; height?: number }>;
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
 * A Secrets Store secret binding (#3301): Cloudflare's `secrets_store_secrets`
 * binding, which `WorkersStoredSecret` reads. `get` resolves to the secret's
 * value, so nothing that holds one may log it.
 */
export interface SecretsStoreSecret {
  get(): Promise<string>;
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
