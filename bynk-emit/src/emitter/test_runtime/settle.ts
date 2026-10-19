let __bynkRetries = 0;
const __bynkSettleMs: number = globalThis.Number((globalThis as { process?: { env?: globalThis.Record<string, string | undefined> } }).process?.env?.["BYNK_TEST_SETTLE_MS"] ?? "0") || 0;
async function __bynkSettle<T>(attempt: () => Promise<T>): Promise<T> {
  const deadline = globalThis.Date.now() + __bynkSettleMs;
  for (;;) {
    try {
      return await attempt();
    } catch (e) {
      if (!(e instanceof ExpectationError) || globalThis.Date.now() >= deadline) throw e;
      __bynkRetries += 1;
      await new globalThis.Promise((resolve) => globalThis.setTimeout(resolve, 100));
    }
  }
}
