// #3479: Vectorize, for tests. `VectorizeIndex` is the shape of Cloudflare's
// binding that `WorkersVectorize` reads, and `makeIntegrationVectorize` is an
// exact, in-memory index of that shape, built from a `Vectorize` instance's
// declaration (`@dimensions`, `@metric`, `@indexed`). The real binding runs
// over it in `bynk test`, so a test exercises the binding's own filter and
// error handling.
//
// It keeps what an index's contract lets a double keep: dimensions, metric,
// ids, namespaces, insert versus upsert, filters and `topK`. It differs from
// production in two ways, both documented on the capability: ranking is exact
// where production is approximate, and a write is visible at once where
// production takes some seconds. A filter on a property the declaration does
// not list in `@indexed` faults, naming the difference, instead of guessing
// what production's silent mismatch would do.

/**
 * A Vectorize index binding: Cloudflare's `vectorize` binding. Vectors and
 * results are the platform's own shapes (`id`, `values`, `namespace`,
 * `metadata`, `matches`); a write answers a `{ mutationId }`.
 */
export interface VectorizeIndex {
  upsert(vectors: Record<string, unknown>[]): Promise<unknown>;
  insert(vectors: Record<string, unknown>[]): Promise<unknown>;
  query(vector: number[], options: Record<string, unknown>): Promise<unknown>;
  getByIds(ids: string[]): Promise<unknown>;
  deleteByIds(ids: string[]): Promise<unknown>;
  describe(): Promise<unknown>;
}

/**
 * The platform limits the double enforces, from Cloudflare's Vectorize limits
 * page as read on 2026-11-13. They drift, which is why the binding does not
 * hard-code them and the platform's refusal comes back as `VectorInvalidInput`;
 * this is the one table the double reads, pinned by a test.
 */
export const VECTORIZE_LIMITS = {
  readOn: "2026-11-13",
  topK: 100,
  topKWithValuesOrAllMetadata: 50,
  upsertBatch: 1000,
  metadataBytes: 10 * 1024,
  idBytes: 64,
  namespaceBytes: 64,
  filterBytes: 2048,
  filterKeyChars: 512,
} as const;

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
    if (at === null || typeof at !== "object" || !(part in (at as object))) return undefined;
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

function checkFilter(filter: Record<string, unknown>, indexed: ReadonlySet<string>): void {
  if (Object.keys(filter).length === 0) refuse("filter must be a non-empty object");
  if (bytes(JSON.stringify(filter)) >= VECTORIZE_LIMITS.filterBytes) {
    refuse(`filter exceeds the maximum of ${VECTORIZE_LIMITS.filterBytes} bytes`);
  }
  for (const key of Object.keys(filter)) {
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
      const nearest = metric === "cosine" ? -1 : 1;
      const ranked = [...vectors.values()]
        .filter((v) => v.space === space && (filter === undefined || matches(v, filter)))
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
            m.metadata =
              scope === "all"
                ? v.metadata
                : Object.fromEntries(Object.entries(v.metadata).filter(([k]) => indexedNames.has(k)));
          }
          return m;
        }),
      };
    },

    async getByIds(ids) {
      return ids
        .map((id) => vectors.get(id))
        .filter((v): v is Stored => v !== undefined)
        .map((v) => ({ id: v.id, values: [...v.values], "namespace": v.space, metadata: v.metadata }));
    },

    async deleteByIds(ids) {
      for (const id of ids) vectors.delete(id);
      return mutation();
    },

    async describe() {
      return { dimensions, metric };
    },
  };
}

// #1653: the `__` aliases emitted code imports.
export { makeIntegrationVectorize as __makeIntegrationVectorize };
export type { VectorizeIndex as __VectorizeIndex };
