function __bynkOverlay(base: unknown, stub: object, cap: string): unknown {
  return new globalThis.Proxy(stub, {
    get(target, prop) {
      const t = target as Record<PropertyKey, unknown> & { __applies?: (op: string) => boolean };
      // An own property (the stub's state, or a wrapper the recording proxy
      // installed) always answers; a stubbed operation answers when it applies
      // to the running case.
      if (globalThis.Object.prototype.hasOwnProperty.call(t, prop)) return t[prop];
      if (prop in t && (t.__applies === undefined || typeof prop !== "string" || t.__applies(prop))) return t[prop];
      if (base !== undefined) {
        const v = (base as Record<PropertyKey, unknown>)[prop];
        return typeof v === "function" ? v.bind(base) : v;
      }
      return () => {
        throw new globalThis.Error(`bynk: ${cap}.${String(prop)} is not stubbed, and the capability has no provider in this test`);
      };
    },
  });
}
