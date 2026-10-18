// A Bynk project as the web worker holds it (#2102 slice 2, DECISION B): the
// files the client read through `vscode.workspace.fs`, overlaid with the live
// text of open editors, keyed by project-relative path, ready for
// `bynk_analyze_project`.
//
// Dependency-free (no `vscode`, no LSP packages) so the unit tests compile it
// directly (tsconfig.test.json).

import type { WasmDiagnostic } from "./convert";

/** One file the client read: its project-relative path, its URI, its text. */
export interface ProjectFile {
  path: string;
  uri: string;
  text: string;
}

/** A project as the client sends it: the folder holding `bynk.toml`. */
export interface ProjectSnapshot {
  /** The project root's URI, as `Uri.toString()` spells it. */
  root: string;
  files: ProjectFile[];
}

/** An open document: its URI and live text. */
export interface OpenDocument {
  uri: string;
  text: string;
}

/** What `bynk_analyze_project` takes, plus where each path's diagnostics go. */
export interface ProjectSources {
  /** `{ "<path>": "<text>" }`, the wasm wire map. */
  files: Record<string, string>;
  /** Path → URI, for every file in `files`. */
  uris: Map<string, string>;
}

export class Project {
  readonly root: string;
  private readonly byPath = new Map<string, ProjectFile>();

  constructor(snapshot: ProjectSnapshot) {
    this.root = snapshot.root.replace(/\/+$/, "");
    for (const f of snapshot.files) this.byPath.set(f.path, f);
  }

  /** Whether `uri` names a file under this project's root. */
  contains(uri: string): boolean {
    return uri.startsWith(`${this.root}/`);
  }

  /**
   * The project-relative path of `uri`: the client's own spelling when it read
   * the file, else the URI's remainder below the root, percent-decoded.
   */
  pathOf(uri: string): string | undefined {
    for (const f of this.byPath.values()) if (f.uri === uri) return f.path;
    if (!this.contains(uri)) return undefined;
    return uri
      .slice(this.root.length + 1)
      .split("/")
      .map((segment) => decodeURIComponent(segment))
      .join("/");
  }

  /** Record a file the client (re)read, or forget it (`text: null`). */
  update(file: { path: string; uri: string; text: string | null }): void {
    if (file.text === null) this.byPath.delete(file.path);
    else this.byPath.set(file.path, { path: file.path, uri: file.uri, text: file.text });
  }

  /** The project's sources, with each open document's live text on top. */
  sources(open: OpenDocument[]): ProjectSources {
    const files: Record<string, string> = {};
    const uris = new Map<string, string>();
    for (const f of this.byPath.values()) {
      files[f.path] = f.text;
      uris.set(f.path, f.uri);
    }
    for (const doc of open) {
      const path = this.pathOf(doc.uri);
      // An open file the walk would skip (`.history/`, `target/`) stays out,
      // as it was on load.
      if (path === undefined || !isWalkedPath(path)) continue;
      files[path] = doc.text;
      uris.set(path, doc.uri);
    }
    return { files, uris };
  }
}

/** Directories never walked: build output, dependencies, and dot-dirs (VCS). */
const SKIP_DIRS = new Set(["node_modules", "out", "target"]);

/** Whether the project walk skips a directory named `name`, at any depth. */
export function isSkippedDir(name: string): boolean {
  return SKIP_DIRS.has(name) || name.startsWith(".");
}

/**
 * Whether the initial walk would reach the file at project-relative `path`:
 * no directory on the way is skipped. Watcher events use it too, so a file
 * created under `target/` or a dot-dir stays out of the project, as it was
 * on load.
 */
export function isWalkedPath(path: string): boolean {
  return !path.split("/").slice(0, -1).some(isSkippedDir);
}

/**
 * The paths among `paths` that lie under the folder `dir` (all of them for the
 * project root, `""`). A deleted or renamed-away folder takes these with it.
 */
export function filesUnder(paths: string[], dir: string): string[] {
  if (dir === "") return [...paths];
  const prefix = dir.endsWith("/") ? dir : `${dir}/`;
  return paths.filter((p) => p.startsWith(prefix));
}

/** Every binding module the project's adapters name, from their current text. */
export function bindingSet(files: { path: string; text: string }[]): Set<string> {
  return new Set(
    files.filter((f) => f.path.endsWith(".bynk")).flatMap((f) => bindingModules(f.path, f.text)),
  );
}

/** The binding modules to read (`add`) and to forget (`drop`) going from `from` to `to`. */
export function bindingDelta(from: Set<string>, to: Set<string>): { add: string[]; drop: string[] } {
  return {
    add: [...to].filter((p) => !from.has(p)),
    drop: [...from].filter((p) => !to.has(p)),
  };
}

/**
 * The project-relative paths of the binding modules an adapter file names
 * (`binding "./idempotency.binding.ts"`), resolved against the file's own
 * directory. The analysis reads them from the project map
 * (`bynk.adapter.no_binding` otherwise), so the client loads them with the
 * `.bynk` files. A path that climbs above the project root is skipped.
 */
export function bindingModules(path: string, text: string): string[] {
  if (!/^\s*adapter\s/m.test(text)) return [];
  const dir = path.split("/").slice(0, -1);
  const out: string[] = [];
  for (const m of text.matchAll(/^\s*binding\s+"([^"]+)"/gm)) {
    const parts = [...dir];
    let escaped = false;
    for (const seg of m[1].split("/")) {
      if (seg === "" || seg === ".") continue;
      if (seg === "..") {
        if (parts.length === 0) escaped = true;
        else parts.pop();
      } else {
        parts.push(seg);
      }
    }
    if (!escaped) out.push(parts.join("/"));
  }
  return out;
}

/** The innermost project containing `uri`, if any. */
export function projectFor(projects: Iterable<Project>, uri: string): Project | undefined {
  let best: Project | undefined;
  for (const p of projects) {
    if (p.contains(uri) && (!best || p.root.length > best.root.length)) best = p;
  }
  return best;
}

/**
 * Project diagnostics grouped by file path. A diagnostic with no path (a
 * project-level one) is filed under `bynk.toml`, where the editor can show it.
 */
export function diagnosticsByPath(diags: WasmDiagnostic[]): Map<string, WasmDiagnostic[]> {
  const out = new Map<string, WasmDiagnostic[]>();
  for (const d of diags) {
    const path = d.path ?? "bynk.toml";
    const list = out.get(path);
    if (list) list.push(d);
    else out.set(path, [d]);
  }
  return out;
}
