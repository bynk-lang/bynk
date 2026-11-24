// #4190: at `integration`, a stubbed cross-context call crosses the wire as a real one does. Its
// arguments are encoded and decoded, and so is its result, through the consumed context's own
// codecs. A value the decoder rejects fails the call with the decode error, as a real call does.
export function __bynkWireStub(
  inner: any,
  wire: Record<string, { args: Array<[any, any] | null>; ret: [any, any] | null }>,
): any {
  const trip = (codec: [any, any] | null, value: any): any => {
    if (codec === null) return value;
    const decoded = codec[1](codec[0](value));
    if (decoded.tag === "Err") {
      const e = decoded.error;
      const where = e.path === undefined ? "" : ` at ${e.path}`;
      const detail = e.violation?.message ?? e.details ?? "";
      const failure = new globalThis.Error(`BoundaryError: ${e.kind}${where}${detail === "" ? "" : `: ${detail}`}`);
      globalThis.Object.defineProperty(failure, "boundaryError", { value: e, enumerable: false });
      throw failure;
    }
    return decoded.value;
  };
  const own: Record<string, unknown> = {};
  for (const op of globalThis.Object.keys(wire)) {
    own[op] = async (...args: any[]) => {
      const sent = args.map((a, i) => trip(wire[op].args[i] ?? null, a));
      return trip(wire[op].ret, await inner[op](...sent));
    };
  }
  return new globalThis.Proxy(own, {
    get(target, prop) {
      if (globalThis.Object.prototype.hasOwnProperty.call(target, prop)) return target[prop as string];
      const v = inner[prop];
      return typeof v === "function" ? v.bind(inner) : v;
    },
  });
}
