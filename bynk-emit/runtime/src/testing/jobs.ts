import { __bynkShow } from "./expectation.ts";

// #3541: `report` records a detached failure on the case, as the HTTP and service double's does (send.ts), so a detached `Err` in a cron or queue handler is counted in `detached_failures` and not left to `console.error`.
export type __BynkJobDetachReport = { kind: string; site: string; type: string | null; variant: string | null };
export type __BynkJobExec = { pending: Promise<unknown>[]; waitUntil(promise: Promise<unknown>): void; report(record: __BynkJobDetachReport): void; __bynkResult?: unknown };
export function __bynkJobExec(): __BynkJobExec {
  const pending: Promise<unknown>[] = [];
  const reports = ((globalThis as unknown as { __bynkDetachedReports?: __BynkJobDetachReport[] }).__bynkDetachedReports ??= []);
  return { pending, waitUntil(promise: Promise<unknown>): void { pending.push(promise); }, report(record: __BynkJobDetachReport): void { reports.push(record); } };
}
export async function __bynkJobSettle(exec: __BynkJobExec, run: Promise<void>): Promise<void> {
  await run;
  while (exec.pending.length > 0) {
    await Promise.allSettled(exec.pending.splice(0));
  }
}
export function __bynkFault(what: string, e: unknown): Error {
  return new globalThis.Error(`${what} faulted: ${e instanceof globalThis.Error ? e.message : globalThis.String(e)}`);
}
export async function __bynkScheduled<T>(cron: string, run: (exec: __BynkJobExec) => Promise<void>): Promise<T> {
  const exec = __bynkJobExec();
  const log = globalThis.console.error;
  // #3872: the entry reports an `Err` run (a structured record, and a fault to the platform). The double records
  // that report without counting it (the list is this case's: reset per call): the case reads the `Err` as the result, so counting would report it twice.
  const cronReports: unknown[] = ((globalThis as unknown as { __bynkCronReports?: unknown[] }).__bynkCronReports = []);
  globalThis.console.error = (...args: unknown[]): void => {
    const record = args[0] as { message?: unknown } | null;
    if (typeof record === "object" && record !== null && record.message === "a cron run returned Err") cronReports.push(record);
    else log(...args);
  };
  try {
    await __bynkJobSettle(exec, run(exec));
  } catch (e) {
    throw __bynkFault(`the cron handler for "${cron}"`, e);
  } finally {
    globalThis.console.error = log;
  }
  if (!("__bynkResult" in exec)) throw new globalThis.Error(`the Worker ran no cron handler for "${cron}"`);
  return exec.__bynkResult as T;
}
export type __BynkQueueMessage = { readonly id: string; readonly timestamp: Date; readonly attempts: number; readonly body: unknown; ack(): void; retry(): void };
export type __BynkQueueBatch = { readonly queue: string; readonly messages: ReadonlyArray<__BynkQueueMessage>; ackAll(): void; retryAll(): void };
export async function __bynkDeliver<T>(queue: string, body: unknown, run: (batch: __BynkQueueBatch, exec: __BynkJobExec) => Promise<void>): Promise<T> {
  const wire: unknown = globalThis.JSON.parse(globalThis.JSON.stringify(body ?? null));
  let verdict: { tag: "Ack" } | { tag: "Retry"; reason: string } | undefined;
  let reason = "";
  const message: __BynkQueueMessage = {
    id: "bynk-test-message-1", timestamp: new globalThis.Date(0), attempts: 1, body: wire,
    ack(): void { verdict ??= { tag: "Ack" }; },
    retry(): void { verdict ??= { tag: "Retry", reason }; },
  };
  const batch: __BynkQueueBatch = { queue, messages: [message], ackAll(): void { message.ack(); }, retryAll(): void { message.retry(); } };
  const exec = __bynkJobExec();
  const log = globalThis.console.error;
  const prefix = `queue ${queue} `;
  // The entry logs why it retries; that becomes the `Retry`'s reason.
  globalThis.console.error = (...args: unknown[]): void => {
    const label = args[0];
    if (typeof label !== "string" || !label.startsWith(prefix)) { log(...args); return; }
    const what = label.slice(prefix.length);
    const detail = typeof args[1] === "string" ? args[1] : __bynkShow(args[1]);
    reason = what === "retry" ? detail : `${what}: ${detail}`;
  };
  try {
    await __bynkJobSettle(exec, run(batch, exec));
  } catch (e) {
    throw __bynkFault(`the consumer of queue "${queue}"`, e);
  } finally {
    globalThis.console.error = log;
  }
  return (verdict ?? { tag: "Retry", reason: "the consumer neither acked nor retried the message" }) as T;
}
