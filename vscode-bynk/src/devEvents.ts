// The dependency-free core of the dev-server controller (#2094): the
// `bynk dev --format json` event stream, the session state it drives, and the
// text the status bar and terminal show for it. No `vscode` import, so the unit
// tests compile against it directly (see tsconfig.test.json); `devServer.ts`
// wires it to the editor.

/** One served context, from a `serving` event. */
export interface ServedContext {
  /** The worker-dir form `--context` accepts (`a-b` for context `a.b`). */
  name: string;
  port: number;
  url: string;
  inspectorPort: number | null;
}

/** A `bynk dev --format json` lifecycle event. The driver may add events and
 *  fields; `parseDevEvent` drops what it does not know rather than failing. */
export type DevEvent =
  | { event: "serving"; contexts: ServedContext[] }
  | { event: "rebuilding" }
  | { event: "rebuild_ok" }
  | { event: "rebuild_failed" }
  | { event: "stopped"; code: number };

/** Parse one stdout line as a dev event. Anything else (an unknown event, a
 *  line that is not a JSON object with an `event` string, a malformed context)
 *  is `undefined`, and the caller passes the line through as plain output. */
export function parseDevEvent(line: string): DevEvent | undefined {
  const text = line.trim();
  if (!text.startsWith("{")) return undefined;
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return undefined;
  }
  if (!raw || typeof raw !== "object") return undefined;
  const obj = raw as Record<string, unknown>;
  switch (obj.event) {
    case "serving": {
      if (!Array.isArray(obj.contexts)) return undefined;
      const contexts: ServedContext[] = [];
      for (const c of obj.contexts) {
        const ctx = parseContext(c);
        if (!ctx) return undefined;
        contexts.push(ctx);
      }
      return { event: "serving", contexts };
    }
    case "rebuilding":
    case "rebuild_ok":
    case "rebuild_failed":
      return { event: obj.event };
    case "stopped":
      return { event: "stopped", code: typeof obj.code === "number" ? obj.code : 0 };
    default:
      return undefined;
  }
}

function parseContext(raw: unknown): ServedContext | undefined {
  if (!raw || typeof raw !== "object") return undefined;
  const c = raw as Record<string, unknown>;
  if (typeof c.name !== "string" || typeof c.port !== "number") return undefined;
  return {
    name: c.name,
    port: c.port,
    url: typeof c.url === "string" ? c.url : `http://localhost:${c.port}`,
    inspectorPort: typeof c.inspector_port === "number" ? c.inspector_port : null,
  };
}

/** Where a dev session is. `starting` until the driver reports `serving`;
 *  `serving` until every context answers (the controller probes the ports),
 *  then `ready`; `rebuilding` / `failed` around a rebuild; `stopped` at the end. */
export type DevPhase = "starting" | "serving" | "ready" | "rebuilding" | "failed" | "stopped";

export interface DevState {
  phase: DevPhase;
  contexts: ServedContext[];
  /** The exit code, once `stopped`. `null` when the process died of a signal. */
  exitCode?: number | null;
}

export const INITIAL_DEV_STATE: DevState = { phase: "starting", contexts: [] };

/** A state change: a driver event, or one the controller observes itself
 *  (`ready` once the ports answer, `exited` when the process ends). */
export type DevTransition = DevEvent | { event: "ready" } | { event: "exited"; code: number | null };

/** The next state after `t`. Pure; the controller feeds it every event. */
export function reduceDevState(state: DevState, t: DevTransition): DevState {
  switch (t.event) {
    case "serving":
      return { phase: "serving", contexts: t.contexts };
    case "ready":
      return state.phase === "serving" ? { ...state, phase: "ready" } : state;
    case "rebuilding":
      return { ...state, phase: "rebuilding" };
    case "rebuild_ok":
      return { ...state, phase: "ready" };
    case "rebuild_failed":
      return { ...state, phase: "failed" };
    case "stopped":
      return { ...state, phase: "stopped", exitCode: t.code };
    case "exited":
      // `stopped` usually arrives first and carries the driver's own code;
      // keep it, since a signal-killed driver reports `null` here.
      return state.phase === "stopped"
        ? state
        : { ...state, phase: "stopped", exitCode: t.code };
  }
}

/** The status-bar text and tooltip for a state. */
export function devStatus(state: DevState): { text: string; tooltip: string } {
  const n = state.contexts.length;
  const count = `${n} context${n === 1 ? "" : "s"}`;
  const urls = state.contexts.map((c) => `${c.name}: ${c.url}`).join("\n");
  switch (state.phase) {
    case "starting":
      return { text: "$(loading~spin) dev: starting", tooltip: "bynk dev is building and starting." };
    case "serving":
      return { text: `$(loading~spin) dev: ${count}`, tooltip: `bynk dev is starting.\n${urls}` };
    case "ready":
      return { text: `$(radio-tower) dev: ${count}`, tooltip: `bynk dev is serving.\n${urls}` };
    case "rebuilding":
      return { text: `$(sync~spin) dev: ${count}`, tooltip: `bynk dev is rebuilding.\n${urls}` };
    case "failed":
      return {
        text: `$(error) dev: ${count}`,
        tooltip: `The last rebuild failed; the last good build is still serving.\n${urls}`,
      };
    case "stopped":
      return { text: "$(debug-stop) dev: stopped", tooltip: "bynk dev has stopped." };
  }
}

/** The marker line the controller writes to the dev terminal for a transition
 *  out of `prev`, or `undefined` for none. The `$bynk-dev` problem matcher's
 *  background patterns key on these (`starting`/`rebuilding` begin a cycle;
 *  `ready`, `rebuilt`, `rebuild failed` and `stopped` end one), so a
 *  `preLaunchTask` waits until the workers answer. Keep package.json in step. */
export function devMarker(t: DevTransition, prev: DevState): string | undefined {
  switch (t.event) {
    case "serving":
      return undefined; // the driver's own report says what is starting
    case "ready":
      return prev.phase === "serving"
        ? `[bynk dev] ready: ${prev.contexts.map((c) => `${c.name} ${c.url}`).join(", ")}`
        : undefined;
    case "rebuilding":
      return "[bynk dev] rebuilding";
    case "rebuild_ok":
      return "[bynk dev] rebuilt";
    case "rebuild_failed":
      return "[bynk dev] rebuild failed (see Problems); the last good build is still serving";
    case "stopped":
    case "exited":
      if (prev.phase === "stopped") return undefined;
      return `[bynk dev] stopped${t.code === null ? "" : ` (exit ${t.code})`}`;
  }
}

/** The line that opens a session in the terminal (a background-matcher begin). */
export function devStartMarker(args: string[]): string {
  return `[bynk dev] starting: bynk ${args.join(" ")}`;
}

export interface DevArgsOptions {
  /** Contexts to serve (dotted or worker-dir form); empty serves them all. */
  contexts?: string[];
  basePort?: number | null;
  /** Forwarded to `wrangler dev` after `--` (`bynk.dev.args`). */
  extraArgs?: string[];
}

/** The `bynk dev` argument list for a session. */
export function devArgs(opts: DevArgsOptions): string[] {
  const args = ["dev", "--format", "json"];
  for (const c of opts.contexts ?? []) args.push("--context", c);
  if (typeof opts.basePort === "number") args.push("--base-port", String(opts.basePort));
  if (opts.extraArgs && opts.extraArgs.length > 0) args.push("--", ...opts.extraArgs);
  return args;
}

/** The contexts a `.bynk` source declares (`context a.b` at the start of a
 *  line), for the Start quick pick. Commons and adapters declare none. */
export function contextNames(source: string): string[] {
  const names: string[] = [];
  for (const m of source.matchAll(/^context\s+([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)/gm)) {
    names.push(m[1]);
  }
  return names;
}
