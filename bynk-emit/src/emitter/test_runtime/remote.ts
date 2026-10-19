const __bynkTargets: { readonly [worker: string]: string | undefined } = globalThis.JSON.parse((globalThis as { process?: { env?: globalThis.Record<string, string | undefined> } }).process?.env?.["BYNK_TEST_TARGETS"] ?? "{}");
function __bynkRemoteBinding(worker: string): __ServiceBinding {
  return {
    fetch: async (req: globalThis.Request): Promise<globalThis.Response> => {
      const base = __bynkTargets[worker];
      if (base === undefined) throw new globalThis.Error(`the test target names no URL for the Worker \`${worker}\``);
      const url = new globalThis.URL(req.url);
      const body = req.method === "GET" || req.method === "HEAD" ? undefined : await req.arrayBuffer();
      return globalThis.fetch(base.replace(/\/+$/, "") + url.pathname + url.search, { method: req.method, headers: req.headers, body });
    },
  };
}
