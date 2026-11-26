interface __BynkCacheDouble {
  purged: string[][];
  succeeds: boolean;
  purge(options: { pathPrefixes: string[] }): Promise<{ success: boolean }>;
}
// #3437: what a detached effect's failure reports. The double's `report` records
// each one on `__bynkDetachedReports`, which the runner drains after each case (`globalThis.__bynkDetachedReports`); a
// report does not fail the case, as on Workers (ADR 0478). Attribution is best-effort: a report landing after its case was drained (a case that threw before `__bynkSettle` finished) shows under the next case.
type __BynkDetachedReport = { kind: string; site: string; type: string | null; variant: string | null };
// One list on `globalThis`, shared by every module's double.
const __bynkDetachedReports: __BynkDetachedReport[] = ((globalThis as unknown as { __bynkDetachedReports?: __BynkDetachedReport[] }).__bynkDetachedReports ??= []);
function __bynkExec(): { waitUntil(promise: Promise<unknown>): void; report(record: __BynkDetachedReport): void; pending: Promise<unknown>[]; cache: __BynkCacheDouble } {
  const pending: Promise<unknown>[] = [];
  const cache: __BynkCacheDouble = {
    purged: [],
    succeeds: true,
    async purge(options: { pathPrefixes: string[] }): Promise<{ success: boolean }> {
      cache.purged.push([...options.pathPrefixes]);
      return { success: cache.succeeds };
    },
  };
  return { pending, cache, waitUntil(promise: Promise<unknown>): void { pending.push(promise); }, report(record: __BynkDetachedReport): void { __bynkDetachedReports.push(record); } };
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
