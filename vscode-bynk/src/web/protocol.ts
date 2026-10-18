// Messages between the web extension's client and its worker (#2102).

/**
 * The notification the worker sends after a wasm trap. A Rust panic traps on
 * `wasm32-unknown-unknown` (`RuntimeError: unreachable`) and leaves the
 * instance's state (its shadow stack, its allocator) untrustworthy. The
 * wasm-bindgen glue keeps one instance per worker and can't be re-initialised,
 * so the client answers by restarting the worker (src/web/extension.ts).
 */
export const WASM_TRAPPED = "bynk/wasmTrapped";

/** What the client passes the worker in `initializationOptions`. */
export interface InitializationOptions {
  /** The URL of `out/web/bynk_wasm_bg.wasm` in this extension. */
  wasmUrl: string;
}
