// Calling into `bynk-wasm` from the web worker (#2102): load on first use,
// parse the JSON reply, and report failure explicitly, so a caller can tell
// "the analysis found nothing" from "the analysis didn't run".
//
// Dependency-free (no `vscode`, no LSP packages, no wasm import: the loader
// and the call are passed in) so the unit tests compile it directly.

/** The outcome of one wasm call. */
export type WasmOutcome<T> = { ok: true; value: T } | { ok: false };

export interface WasmCallerHooks {
  /** Load and instantiate the module; called before every call, cached by the caller's loader. */
  load: () => Promise<void>;
  /** The first trap: the instance is poisoned and the worker should be replaced. */
  onTrap: () => void;
  log: (message: string) => void;
  /** Whether `err` is a wasm trap (`WebAssembly.RuntimeError` in the worker). */
  isTrap: (err: unknown) => boolean;
}

export class WasmCaller {
  private poisoned = false;

  constructor(private readonly hooks: WasmCallerHooks) {}

  /** Whether a trap has poisoned this instance. */
  get isPoisoned(): boolean {
    return this.poisoned;
  }

  /**
   * Run one call. A failed load, a throw, an unparsable reply, or a poisoned
   * instance is `{ ok: false }`. After the first trap the instance is never
   * called again: every later call fails without running, until the client
   * replaces the worker.
   */
  async call<T>(run: () => string): Promise<WasmOutcome<T>> {
    if (this.poisoned) return { ok: false };
    try {
      await this.hooks.load();
      return { ok: true, value: JSON.parse(run()) as T };
    } catch (err) {
      this.hooks.log(`bynk-wasm call failed: ${String(err)}`);
      if (this.hooks.isTrap(err)) {
        this.poisoned = true;
        this.hooks.onTrap();
      }
      return { ok: false };
    }
  }
}
