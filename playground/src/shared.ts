// Shared types + origin config for the Bynk playground (in-browser track, slice 4).
//
// Two origins, by design (the safety boundary — ADR 0140): the **app** origin
// hosts the editor + the wasm compiler; the **sandbox** origin hosts the
// execution document (a sandboxed iframe wrapping a Web Worker). They talk only
// over `postMessage`, each validating the other.
//
// Origins are injected at build time (esbuild `define`); the defaults are the
// production hosts, overridden to localhost ports for local verification.
declare const __APP_ORIGIN__: string;
declare const __SANDBOX_ORIGIN__: string;

export const APP_ORIGIN: string =
  typeof __APP_ORIGIN__ === "string" ? __APP_ORIGIN__ : "https://playground.bynk-lang.org";
export const SANDBOX_ORIGIN: string =
  typeof __SANDBOX_ORIGIN__ === "string" ? __SANDBOX_ORIGIN__ : "https://sandbox.bynk-lang.org";

/// One emitted JavaScript module of a compiled program.
export interface EmittedFile {
  path: string;
  /// "user": authored from the user's source; "wiring": `compose.js`;
  /// "support": `runtime.js`, `bynk-browser.js` and injected first-party units.
  kind: "user" | "wiring" | "support";
  contents: string;
}

/// A diagnostic (mirrors the wasm `bynk_compile` / `bynk_analyze` JSON shape).
export interface Diagnostic {
  path: string | null;
  line: number;
  col: number;
  /// Byte offsets of the span (for the editor's inline lint range).
  from: number;
  to: number;
  severity: "error" | "warning";
  category: string;
  message: string;
}

/// The wasm `bynk_compile` result.
export interface CompileResult {
  ok: boolean;
  files: EmittedFile[];
  diagnostics: Diagnostic[];
}

/// app → sandbox: "run this module graph".
export interface RunRequest {
  kind: "bynk-run";
  /// Correlates the reply with this request.
  id: number;
  files: EmittedFile[];
  /// Wall-clock budget; the Worker is terminated past it.
  timeoutMs: number;
}

/// sandbox → app: the outcome of a run.
export interface RunReply {
  kind: "bynk-result";
  id: number;
  /// Captured `Logger` output, in order.
  logs: { level: "info" | "error"; message: string }[];
  /// The entry's returned value, JSON-ish stringified (when it completed).
  value?: string;
  /// A thrown error / unhandled rejection from the program.
  error?: string;
  /// True when the Worker hit the wall-clock budget and was terminated.
  timedOut?: boolean;
  /// True when the graph had no zero-argument entry to invoke.
  noEntry?: boolean;
}

/// sandbox → app, once: the execution document is ready to receive runs.
export interface SandboxReady {
  kind: "bynk-sandbox-ready";
}
