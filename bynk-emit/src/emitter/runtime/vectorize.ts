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

// #1653: the `__` aliases emitted code imports.
export type { VectorizeIndex as __VectorizeIndex };
