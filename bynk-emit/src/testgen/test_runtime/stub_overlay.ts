function __bynkOverlay(base: unknown, stub: object, cap: string): unknown {
  const proxy: unknown = new globalThis.Proxy(stub, {
    get(target, prop) {
      // #4099: `Ai.generateValue[T]` is lowered over the binding's JSON-mode `generateJson`, which
      // is not an operation of `Ai` and so cannot be stubbed. In a test it reads the text of the
      // stubbed `generate` (`stub Ai.generate(_, _) returns Ok(Generation { text: "{…}", … })`),
      // so a case runs the same decode production does, with a model's reply for its input.
      if (cap === "Ai" && prop === "generateJson") {
        return async (model: unknown, request: unknown) => {
          const reply = await (proxy as { generate(m: unknown, r: unknown): Promise<{ tag: string; value: { text: string } }> }).generate(model, request);
          return reply.tag === "Ok" ? { tag: "Ok", value: reply.value.text } : reply;
        };
      }
      const t = target as Record<PropertyKey, unknown> & { __applies?: (op: string) => boolean };
      // An own property (the stub's state, or a wrapper the recording proxy
      // installed) always answers; a stubbed operation answers when it applies
      // to the running case.
      if (globalThis.Object.prototype.hasOwnProperty.call(t, prop)) return t[prop];
      if (prop in t && (t.__applies === undefined || typeof prop !== "string" || t.__applies(prop))) return t[prop];
      if (base !== undefined) {
        const v = (base as Record<PropertyKey, unknown>)[prop];
        // D20 (#3752): `Upstream.forward` in a test runs the shipped policy and then the
        // instance's `exchange`, which must be the stubbed or recorded one: it is called with the
        // overlay as `this`.
        if (typeof v === "function" && (v as { __bynkThroughOverlay?: boolean }).__bynkThroughOverlay === true) return v.bind(proxy);
        return typeof v === "function" ? v.bind(base) : v;
      }
      return () => {
        throw new globalThis.Error(`bynk: ${cap}.${String(prop)} is not stubbed, and the capability has no provider in this test`);
      };
    },
  });
  return proxy;
}
