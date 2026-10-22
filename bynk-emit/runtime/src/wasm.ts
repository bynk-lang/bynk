// #2321: instantiate a WebAssembly module an adapter binding includes
// (`binding "…" includes ["./x.wasm"]`). The compiler generates, per included
// module, a `./x.wasm.js` whose default export is the compiled
// `WebAssembly.Module` — loaded by Wrangler on Workers, by `bynk test` on Node —
// so a binding imports it and calls this without knowing which platform it is on.
export async function loadWasm(
  module: WebAssembly.Module,
  imports: WebAssembly.Imports = {},
): Promise<WebAssembly.Instance> {
  return await WebAssembly.instantiate(module, imports);
}
