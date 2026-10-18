// Messages between the web extension's client and its worker (#2102).

import type { ProjectSnapshot } from "./project";

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

/**
 * Client → worker: the workspace's Bynk projects (every workspace folder with a
 * `bynk.toml` at its root), each with its `bynk.toml` and `.bynk` files read
 * through `vscode.workspace.fs`. Replaces whatever the worker held. Sent once
 * the client starts, and again after a worker restart (#2102 slice 2).
 */
export const PROJECTS = "bynk/projects";

export interface ProjectsParams {
  projects: ProjectSnapshot[];
}

/** Client → worker: one project file changed on disk (`text: null`: deleted). */
export const PROJECT_FILE = "bynk/projectFile";

export interface ProjectFileParams {
  root: string;
  path: string;
  uri: string;
  text: string | null;
}
