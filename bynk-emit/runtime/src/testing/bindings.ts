// The test doubles the multi-Worker integration harness binds as a participant's `env.*`
// (back-half paydown 1B.3b, #4047, Decision D): every `makeIntegration*` factory, which used to
// ship in the production runtime modules they sit beside. No production emission names one, so a
// project's emitted `storage` module no longer carries them; a test build imports them from here
// through the `__` aliases. The platform interfaces they implement stay in `storage.ts`.

import { makeTestState, type AiSearchInstance, type D1Database, type DurableObjectState, type AnalyticsDataset, type AnalyticsSqlBinding, type D1PreparedStatement, type D1Result, type Hyperdrive, type KVNamespace, type QueueProducer, type R2Bucket, type R2GetOptions, type R2MultipartUpload, type R2ObjectBody, type R2ObjectMeta, type R2PutOptions, type RateLimiter, type SecretsStoreSecret, type SendEmail, type WorkerAiBinding, type WorkerImagesBinding, type WorkerImagesTransformer, type WorkerVersionMetadata } from "../storage.ts";
import { VECTORIZE_LIMITS, type VectorizeIndex } from "../vectorize.ts";
import { serialiseAgentClass, type DurableObjectNamespace, type DurableObjectStub } from "../agent.ts";
import { makeHibernatableTestState, type WebSocketHost } from "./websocket.ts";

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
    async get(key: string, options?: R2GetOptions): Promise<R2ObjectBody | null> {
      const s = objects.get(key);
      if (!s) return null;
      // #4049: a condition that does not hold answers the object with no body, as R2 does.
      const gate = options?.onlyIf;
      if (gate?.etagMatches !== undefined && s.meta.etag !== gate.etagMatches) return { ...s.meta } as unknown as R2ObjectBody;
      if (gate?.etagDoesNotMatch !== undefined && s.meta.etag === gate.etagDoesNotMatch) return { ...s.meta } as unknown as R2ObjectBody;
      // #4049: a range is clamped to the object, but one that starts at or past its end is refused, as R2
      // refuses it (10039), rather than answered with nothing.
      let served = { offset: 0, length: s.body.length };
      let ranged = false;
      const range = options?.range;
      if (range !== undefined) {
        ranged = true;
        // R2 refuses a negative offset, length or suffix, as the unit-tier double does; a handler usually
        // takes these from a client's `Range` header.
        if (("suffix" in range && range.suffix < 0) || ("offset" in range && (range.offset < 0 || (range.length ?? 0) < 0))) {
          throw new Error("R2: a range offset, length and suffix are not negative");
        }
        if ("suffix" in range) {
          const length = Math.min(range.suffix, s.body.length);
          served = { offset: s.body.length - length, length };
        } else {
          if (range.offset >= s.body.length) throw new Error("R2: the requested range is not satisfiable (10039)");
          served = { offset: range.offset, length: Math.min(range.length ?? s.body.length - range.offset, s.body.length - range.offset) };
        }
      }
      const bytes = s.body.subarray(served.offset, served.offset + served.length);
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
        ...(ranged ? { range: served } : {}),
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
      const delimiter = options?.delimiter;
      // A cursor that is a folded prefix skips everything under it, so the prefix is not listed twice.
      const past = (k: string): boolean =>
        after === undefined || (k > after && !(delimiter !== undefined && delimiter !== "" && after.endsWith(delimiter) && k.startsWith(after)));
      const keys = [...objects.keys()].filter((k) => k.startsWith(prefix) && past(k)).sort();
      // #4049: with a delimiter, a key that holds it after the prefix folds into the key up to and including
      // the delimiter, listed once, as R2 does. A page counts objects and prefixes together, in key order.
      const entries: Array<{ key: string; folded: boolean }> = [];
      for (const k of keys) {
        const at = delimiter === undefined || delimiter === "" ? -1 : k.indexOf(delimiter, prefix.length);
        const entry = at < 0 ? { key: k, folded: false } : { key: k.slice(0, at + delimiter!.length), folded: true };
        if (entry.folded && entries.some((e) => e.folded && e.key === entry.key)) continue;
        entries.push(entry);
      }
      const page = entries.slice(0, limit);
      const truncated = entries.length > page.length;
      return {
        objects: page.filter((e) => !e.folded).map((e) => objects.get(e.key)!.meta),
        delimitedPrefixes: page.filter((e) => e.folded).map((e) => e.key),
        truncated,
        cursor: truncated ? page[page.length - 1].key : undefined,
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
export function makeIntegrationD1(
  migrations: readonly (readonly [string, string])[] = [],
): D1Database & { readOnly(): D1Database } {
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
  // #3147: a `D1.peek` handle reads the database the app wrote, and never
  // writes it. Its statements run with `query_only` on, for the statement's
  // own (synchronous) execution only, so the app's own statements in flight
  // are never refused. SQLite refuses a write then, whatever the statement.
  const readOnly = <T>(guarded: boolean, f: () => T): T => {
    if (!guarded) return f();
    db.exec("PRAGMA query_only = ON");
    try {
      return f();
    } finally {
      db.exec("PRAGMA query_only = OFF");
    }
  };
  const statement = (sql: string, params: unknown[], guarded = false): D1PreparedStatement & { exec(): D1Result } => {
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
      bind: (...values: unknown[]) => statement(sql, values, guarded),
      all: async () => ({ results: attempt(() => readOnly(guarded, () => prepared().all(...bound))), meta: {} }),
      first: async () => attempt(() => readOnly(guarded, () => prepared().get(...bound))) ?? null,
      run: async () => ({ results: [], meta: meta(attempt(() => readOnly(guarded, () => prepared().run(...bound)))) }),
      exec: () => ({ results: [], meta: meta(attempt(() => readOnly(guarded, () => prepared().run(...bound)))) }),
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
    // #3147: the same database, read-only, for `D1.peek`.
    readOnly: (): D1Database => ({
      prepare: (sql: string) => statement(sql, [], true),
      async batch(): Promise<D1Result[]> {
        throw d1Error(new Error("a D1.peek handle is read-only"));
      },
    }),
  };
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
 * The marker every integration double puts on the fault it throws for an
 * unstubbed call (#3589). A binding in `bynk.cloudflare` that maps a platform
 * failure to an `Err` rethrows an error that carries it (`bynkNotStubbed ===
 * true`), so an unstubbed call faults the case and a handler's `Err(_)`
 * fallback cannot swallow it. Defined once, here; #3578's `Images` double uses
 * the same property.
 */
export function markNotStubbed(message: string): Error {
  return Object.assign(new globalThis.Error(message), { bynkNotStubbed: true });
}

/**
 * The Analytics SQL binding of a multi-Worker integration test participant, bound as its
 * `env.CF_ANALYTICS_SQL`. A test never reaches Cloudflare, so `query` faults until the case stubs
 * the capability's operation. There are no fake rows: a fabricated reply would make a test pass
 * for the wrong reason.
 */
export function makeIntegrationAnalyticsSql(): AnalyticsSqlBinding {
  return {
    async query(): Promise<unknown> {
      throw markNotStubbed(
        "bynk: the `AnalyticsSql` binding is not stubbed, and a test never reaches Analytics SQL — add `stub AnalyticsSql.query(_, _, _) returns …`",
      );
    },
  };
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
      throw markNotStubbed(
        "bynk: the `Ai` binding is not stubbed, and a test never reaches Workers AI — stub the operation the case calls (`stub Ai.run(_, _) returns …`)",
      );
    },
  };
}

/**
 * The Images binding of a multi-Worker integration test participant, bound as
 * its `env.IMAGES`. A test never reaches Cloudflare Images, so `input` faults
 * until the case stubs the capability's operation. There is no fake transform:
 * an image handed back unchanged would make a test pass for the wrong reason.
 */
// The double's fault is marked (`markNotStubbed`), which `WorkersImages`
// rethrows instead of mapping to an `ImagesError`: an unstubbed call must
// fault, not come back as a value a handler's fallback would swallow.
function notStubbed(stub: string): Error {
  return markNotStubbed(
    `bynk: the \`Images\` binding is not stubbed, and a test never reaches Cloudflare Images — stub the operation the case calls (${stub})`,
  );
}

export function makeIntegrationImages(): WorkerImagesBinding {
  return {
    async info(): Promise<never> {
      throw notStubbed("`stub Images.info(_) returns …`");
    },
    input(): WorkerImagesTransformer {
      throw notStubbed("`stub Images.transform(_, _, _) returns …`");
    },
  };
}

/**
 * The AI Search binding of a multi-Worker integration test participant, bound
 * as its `env.<binding>`. A test never reaches AI Search, so every operation
 * faults, naming the binding, until the case stubs the capability's operation.
 */
export function makeIntegrationAiSearch(binding: string): AiSearchInstance {
  const unstubbed = async (): Promise<never> => {
    throw markNotStubbed(
      `bynk: the \`${binding}\` AI Search binding is not stubbed, and a test never reaches AI Search — stub the operation the case calls (for example \`stub <Name>.search(_, _) returns …\`)`,
    );
  };
  return { search: unstubbed, chatCompletions: unstubbed };
}

/**
 * The mTLS certificate binding of a multi-Worker integration test participant (#3513 A), bound
 * as its `env.<binding>`. A test never reaches an origin, so `fetch` faults, naming the
 * instance, until the case stubs its `send`.
 */
export function makeIntegrationMtls(instance: string): { fetch(request: unknown): Promise<never> } {
  return {
    fetch: async (): Promise<never> => {
      throw markNotStubbed(
        `bynk: \`${instance}.send\` is not stubbed, and a test never reaches the origin — add \`stub ${instance}.send(_) returns …\``,
      );
    },
  };
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
        throw markNotStubbed(
          `bynk: the \`${binding}\` container \`${id}\` is not stubbed, and a test never starts a container — stub the operation the case calls (for example \`stub <Name>.send(_, _) returns …\`)`,
        );
      },
    }),
  };
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
      throw markNotStubbed(
        `bynk: Secrets Store binding \`${binding}\` has no value in a test, which never reads the account's store — add \`stub <Capability>.get() returns …\``,
      );
    },
  };
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
 * The Hyperdrive binding of a multi-Worker integration test participant, bound
 * as its `env.<binding>`. A test never reaches a database, so reading its
 * `connectionString` faults, naming the binding, until the case stubs the
 * instance's operations (a stub answers before the binding is read).
 */
export function makeIntegrationHyperdrive(binding: string): Hyperdrive {
  return {
    get connectionString(): string {
      throw markNotStubbed(
        `bynk: Hyperdrive binding \`${binding}\` has no database in a test, which never reaches a database — add \`stub <Capability>.all(_, _) returns …\` (or \`first\` or \`run\`)`,
      );
    },
  };
}

// #3479: Vectorize, for tests. `makeIntegrationVectorize` is an exact, in-memory index of the
// `VectorizeIndex` shape (`../vectorize.ts`), built from a `Vectorize` instance's declaration
// (`@dimensions`, `@metric`, `@indexed`). The real binding runs over it in `bynk test`, so a test
// exercises the binding's own filter and error handling.
//
// It keeps what an index's contract lets a double keep: dimensions, metric, ids, namespaces,
// insert versus upsert, filters and `topK`. It differs from production in two ways, both
// documented on the capability: ranking is exact where production is approximate, and a write is
// visible at once where production takes some seconds. A filter on a property the declaration does
// not list in `@indexed` faults, naming the difference, instead of guessing what production's
// silent mismatch would do.
interface Stored {
  id: string;
  values: number[];
  space?: string;
  metadata?: Record<string, unknown>;
}

const bytes = (s: string): number => new TextEncoder().encode(s).length;

function refuse(message: string): never {
  throw new globalThis.Error(message);
}

// The double's own faults start with `bynk:`, which `WorkersVectorize` lets
// through instead of reading as a platform refusal.
function unmodelled(message: string): never {
  throw new globalThis.Error(`bynk: the Vectorize test double ${message}`);
}

function score(metric: string, query: number[], values: number[]): number {
  let dot = 0;
  let q = 0;
  let v = 0;
  let sq = 0;
  for (let i = 0; i < query.length; i++) {
    dot += query[i] * values[i];
    q += query[i] * query[i];
    v += values[i] * values[i];
    sq += (query[i] - values[i]) ** 2;
  }
  switch (metric) {
    case "cosine":
      return q === 0 || v === 0 ? 0 : dot / (Math.sqrt(q) * Math.sqrt(v));
    case "euclidean":
      return Math.sqrt(sq);
    default:
      // The platform reports the negative dot product, so a smaller score is nearer.
      return -dot;
  }
}

// A property by its dotted path (`author.name`).
function property(metadata: Record<string, unknown> | undefined, path: string): unknown {
  let at: unknown = metadata;
  for (const part of path.split(".")) {
    if (at === null || typeof at !== "object" || !Object.prototype.hasOwnProperty.call(at, part)) return undefined;
    at = (at as Record<string, unknown>)[part];
  }
  return at;
}

const sameKind = (a: unknown, b: unknown): boolean => typeof a === typeof b;

// One operator over one property. A property the vector lacks matches nothing:
// the platform's answer for `$ne` and `$nin` on a missing property is not
// documented, so the double does not match it.
function holds(value: unknown, op: string, operand: unknown): boolean {
  if (value === undefined) return false;
  switch (op) {
    case "$eq":
      return value === operand;
    case "$ne":
      return value !== operand;
    case "$in":
      return (operand as unknown[]).includes(value);
    case "$nin":
      return !(operand as unknown[]).includes(value);
    case "$lt":
      return sameKind(value, operand) && (value as number | string) < (operand as number | string);
    case "$lte":
      return sameKind(value, operand) && (value as number | string) <= (operand as number | string);
    case "$gt":
      return sameKind(value, operand) && (value as number | string) > (operand as number | string);
    case "$gte":
      return sameKind(value, operand) && (value as number | string) >= (operand as number | string);
    default:
      return refuse(`unknown filter operator ${op}`);
  }
}

function matches(stored: Stored, filter: Record<string, unknown>): boolean {
  return Object.entries(filter).every(([key, condition]) => {
    const value = property(stored.metadata, key);
    if (condition !== null && typeof condition === "object" && !Array.isArray(condition)) {
      return Object.entries(condition as Record<string, unknown>).every(([op, operand]) => holds(value, op, operand));
    }
    return holds(value, "$eq", condition);
  });
}

function define(target: Record<string, unknown>, key: string, value: unknown): void {
  Object.defineProperty(target, key, { value, enumerable: true, writable: true, configurable: true });
}

// The indexed properties of `metadata`, each dotted path read and nested again
// (`author.name` keeps `{ author: { name } }`), as the platform returns them.
function indexedOnly(metadata: Record<string, unknown>, indexed: ReadonlySet<string>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const path of indexed) {
    const value = property(metadata, path);
    if (value === undefined) continue;
    // Own properties only, set by definition: a path segment such as
    // `__proto__` never reaches (or writes to) `Object.prototype`.
    const parts = path.split(".");
    let at = out;
    for (const part of parts.slice(0, -1)) {
      if (!Object.prototype.hasOwnProperty.call(at, part)) define(at, part, {});
      at = at[part] as Record<string, unknown>;
    }
    define(at, parts[parts.length - 1], structuredClone(value));
  }
  return out;
}

const OPERATORS = new Set(["$eq", "$ne", "$in", "$nin", "$lt", "$lte", "$gt", "$gte"]);

// A condition's operators and operands, checked before any vector is matched,
// so a malformed filter is refused whatever the index holds.
function checkCondition(key: string, condition: unknown): void {
  if (condition === null || typeof condition !== "object" || Array.isArray(condition)) return;
  const entries = Object.entries(condition as Record<string, unknown>);
  if (entries.length === 0) refuse(`the condition for ${JSON.stringify(key)} must not be empty`);
  for (const [op, operand] of entries) {
    if (!OPERATORS.has(op)) refuse(`unknown filter operator ${op}`);
    if ((op === "$in" || op === "$nin") && !Array.isArray(operand)) {
      refuse(`${op} takes a list of values`);
    }
  }
}

function checkFilter(filter: Record<string, unknown>, indexed: ReadonlySet<string>): void {
  if (Object.keys(filter).length === 0) refuse("filter must be a non-empty object");
  if (bytes(JSON.stringify(filter)) >= VECTORIZE_LIMITS.filterBytes) {
    refuse(`filter exceeds the maximum of ${VECTORIZE_LIMITS.filterBytes} bytes`);
  }
  for (const [key, condition] of Object.entries(filter)) {
    checkCondition(key, condition);
    if (key === "" || key.includes('"') || key.startsWith("$") || key.length > VECTORIZE_LIMITS.filterKeyChars) {
      refuse(`invalid filter key ${JSON.stringify(key)}`);
    }
    if (!indexed.has(key)) {
      unmodelled(
        `does not model a filter on \`${key}\`, which is not in the instance's @indexed list: production silently matches nothing for it. Add \`"${key}:<type>"\` to @indexed, or stub the query`,
      );
    }
  }
}

/**
 * An exact, in-memory Vectorize index for an instance declared with
 * `dimensions`, `metric` and the `indexed` entries (`name:type`). Mutation ids
 * are `m1`, `m2`, … and a write is queryable at once.
 */
export function makeIntegrationVectorize(
  dimensions: number,
  metric: string,
  indexed: readonly string[] = [],
): VectorizeIndex {
  const indexedNames: ReadonlySet<string> = new Set(indexed.map((entry) => entry.split(":")[0]));
  const vectors = new Map<string, Stored>();
  let mutations = 0;

  const mutation = (): { mutationId: string } => ({ mutationId: `m${++mutations}` });

  const checked = (batch: Record<string, unknown>[]): Stored[] => {
    if (batch.length > VECTORIZE_LIMITS.upsertBatch) {
      refuse(`batch of ${batch.length} vectors exceeds the maximum of ${VECTORIZE_LIMITS.upsertBatch}`);
    }
    return batch.map((v) => {
      const id = String(v.id);
      const values = Array.from(v.values as number[]);
      if (id === "" || bytes(id) > VECTORIZE_LIMITS.idBytes) {
        refuse(`vector id ${JSON.stringify(id)} must be 1 to ${VECTORIZE_LIMITS.idBytes} bytes`);
      }
      if (values.length !== dimensions) {
        refuse(`dimension mismatch: vector ${JSON.stringify(id)} has ${values.length} dimensions, but the index has ${dimensions}`);
      }
      const space = v["namespace"] as string | undefined;
      if (space !== undefined && bytes(space) > VECTORIZE_LIMITS.namespaceBytes) {
        refuse(`namespace exceeds the maximum of ${VECTORIZE_LIMITS.namespaceBytes} bytes`);
      }
      const metadata = v.metadata as Record<string, unknown> | undefined;
      if (metadata !== undefined && bytes(JSON.stringify(metadata)) > VECTORIZE_LIMITS.metadataBytes) {
        refuse(`metadata of vector ${JSON.stringify(id)} exceeds the maximum of ${VECTORIZE_LIMITS.metadataBytes} bytes`);
      }
      return { id, values, space, metadata };
    });
  };

  return {
    async upsert(batch) {
      // Replaces in full: no merge of values or metadata.
      for (const v of checked(batch)) vectors.set(v.id, v);
      return mutation();
    },

    async insert(batch) {
      for (const v of checked(batch)) if (!vectors.has(v.id)) vectors.set(v.id, v);
      return mutation();
    },

    async query(vector, options) {
      if (vector.length !== dimensions) {
        refuse(`dimension mismatch: query vector has ${vector.length} dimensions, but the index has ${dimensions}`);
      }
      const topK = (options.topK as number | undefined) ?? 5;
      const returnValues = options.returnValues === true;
      const scope = (options.returnMetadata as string | undefined) ?? "none";
      if (!Number.isInteger(topK) || topK < 1 || topK > VECTORIZE_LIMITS.topK) {
        refuse(`topK must be between 1 and ${VECTORIZE_LIMITS.topK}`);
      }
      if ((returnValues || scope === "all") && topK > VECTORIZE_LIMITS.topKWithValuesOrAllMetadata) {
        refuse(`topK is limited to a maximum of ${VECTORIZE_LIMITS.topKWithValuesOrAllMetadata} with returnValues or all metadata`);
      }
      const filter = options.filter as Record<string, unknown> | undefined;
      if (filter !== undefined) checkFilter(filter, indexedNames);
      const space = options["namespace"] as string | undefined;
      // A query that names no namespace searches every vector, namespaced or
      // not (observed on a real index, #3479); naming one restricts it.
      const nearest = metric === "cosine" ? -1 : 1;
      const ranked = [...vectors.values()]
        .filter((v) => (space === undefined || v.space === space) && (filter === undefined || matches(v, filter)))
        .map((v) => ({ v, score: score(metric, vector, v.values) }))
        .sort((a, b) => nearest * (a.score - b.score))
        .slice(0, topK);
      return {
        count: ranked.length,
        matches: ranked.map(({ v, score: s }) => {
          const m: Record<string, unknown> = { id: v.id, score: s };
          if (returnValues) m.values = [...v.values];
          if (v.space !== undefined) m["namespace"] = v.space;
          if (v.metadata !== undefined && scope !== "none") {
            m.metadata = scope === "all" ? structuredClone(v.metadata) : indexedOnly(v.metadata, indexedNames);
          }
          return m;
        }),
      };
    },

    async getByIds(ids) {
      return ids
        .map((id) => vectors.get(id))
        .filter((v): v is Stored => v !== undefined)
        .map((v) => ({ id: v.id, values: [...v.values], "namespace": v.space, metadata: structuredClone(v.metadata) }));
    },

    async deleteByIds(ids) {
      for (const id of ids) vectors.delete(id);
      return mutation();
    },

    async describe() {
      // The real `describe()` reports no `metric`; neither does the double.
      return { dimensions, vectorCount: vectors.size };
    },
  };
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
  queueWaitMs?: number,
): DurableObjectNamespace {
  const instances = new Map<string, { fetch(request: Request): Promise<Response> }>();
  const doNamespace: DurableObjectNamespace = {
    // #3819: a jurisdiction-restricted view mints its own ids, as Cloudflare's does, so an instance
    // reached through `jurisdiction("eu")` is not the one the unrestricted namespace reaches.
    jurisdiction(jurisdiction: string): DurableObjectNamespace {
      return {
        idFromName: (name: string): unknown => `${jurisdiction}/${name}`,
        get: (id: unknown): DurableObjectStub => doNamespace.get(id),
      };
    },
    idFromName(name: string): unknown {
      return name;
    },
    get(id: unknown): DurableObjectStub {
      const k = String(id);
      let inst = instances.get(k);
      if (inst === undefined) {
        // #3901: the state accepts sockets for hibernation, and their frames reach this instance's own
        // `webSocketMessage` / `webSocketClose`, as the platform delivers them to a Durable Object.
        const holder: { host?: WebSocketHost } = {};
        inst = new (serialiseAgentClass(cls, { waitMs: queueWaitMs }))(makeHibernatableTestState(k, () => holder.host), env);
        holder.host = inst as unknown as WebSocketHost;
        instances.set(k, inst);
      }
      const target = inst;
      return {
        fetch: (input: string, init?: unknown): Promise<Response> =>
          target.fetch(new Request(input, init as RequestInit)),
      };
    },
  };
  return doNamespace;
}
