interface __BynkCacheDouble {
  purged: string[][];
  succeeds: boolean;
  purge(options: { pathPrefixes: string[] }): Promise<{ success: boolean }>;
}
function __bynkExec(): { waitUntil(promise: Promise<unknown>): void; pending: Promise<unknown>[]; cache: __BynkCacheDouble } {
  const pending: Promise<unknown>[] = [];
  const cache: __BynkCacheDouble = {
    purged: [],
    succeeds: true,
    async purge(options: { pathPrefixes: string[] }): Promise<{ success: boolean }> {
      cache.purged.push([...options.pathPrefixes]);
      return { success: cache.succeeds };
    },
  };
  return { pending, cache, waitUntil(promise: Promise<unknown>): void { pending.push(promise); } };
}
async function __bynkSettle<T>(deps: { __exec: { pending: Promise<unknown>[] } }, call: Promise<T>): Promise<T> {
  const result = await call;
  while (deps.__exec.pending.length > 0) {
    await Promise.allSettled(deps.__exec.pending.splice(0));
  }
  return result;
}
function __bynkFetchSettled<T>(fetch: (exec: { waitUntil(promise: Promise<unknown>): void }) => Promise<T>): Promise<T> {
  const __exec = __bynkExec();
  return __bynkSettle({ __exec }, fetch(__exec));
}
