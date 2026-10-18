// Project loading for the web extension (#2102 slice 2, DECISION B): read each
// Bynk project in the workspace through `vscode.workspace.fs`, which works on
// every web host's virtual filesystem (vscode.dev, github.dev, the test host),
// and keep the worker's copy current as files change.

import * as vscode from "vscode";

import {
  bindingModules,
  isSkippedDir,
  type ProjectFile,
  type ProjectSnapshot,
} from "./project";

/**
 * The most `.bynk` files read for one project. A larger tree stays in
 * single-file mode rather than stalling the browser on its first analysis.
 */
export const MAX_PROJECT_FILES = 2000;

const decoder = new TextDecoder();

async function readText(uri: vscode.Uri): Promise<string | undefined> {
  try {
    return decoder.decode(await vscode.workspace.fs.readFile(uri));
  } catch {
    return undefined;
  }
}

async function exists(uri: vscode.Uri): Promise<boolean> {
  try {
    await vscode.workspace.fs.stat(uri);
    return true;
  } catch {
    return false;
  }
}

/**
 * The project rooted at `folder`, if it has a `bynk.toml`: that manifest and
 * every `.bynk` file below it, at project-relative paths. `undefined` when
 * there is no manifest or the tree is over `MAX_PROJECT_FILES`.
 */
export async function loadProject(folder: vscode.Uri): Promise<ProjectSnapshot | undefined> {
  const manifest = vscode.Uri.joinPath(folder, "bynk.toml");
  if (!(await exists(manifest))) return undefined;

  const found: { path: string; uri: vscode.Uri }[] = [{ path: "bynk.toml", uri: manifest }];
  let bynkFiles = 0;
  const walk = async (dir: vscode.Uri, rel: string): Promise<boolean> => {
    let entries: [string, vscode.FileType][];
    try {
      entries = await vscode.workspace.fs.readDirectory(dir);
    } catch {
      return true;
    }
    for (const [name, type] of entries) {
      const path = rel ? `${rel}/${name}` : name;
      const uri = vscode.Uri.joinPath(dir, name);
      if (type & vscode.FileType.Directory) {
        if (isSkippedDir(name)) continue;
        if (!(await walk(uri, path))) return false;
      } else if (name.endsWith(".bynk")) {
        if (++bynkFiles > MAX_PROJECT_FILES) return false;
        found.push({ path, uri });
      }
    }
    return true;
  };
  if (!(await walk(folder, ""))) return undefined;

  const files: ProjectFile[] = [];
  const read = async (path: string, uri: vscode.Uri): Promise<void> => {
    const text = await readText(uri);
    if (text !== undefined) files.push({ path, uri: uri.toString(), text });
  };
  await Promise.all(found.map(({ path, uri }) => read(path, uri)));
  // An adapter's binding module is read by the analysis too.
  const modules = new Set(files.flatMap((f) => bindingModules(f.path, f.text)));
  await Promise.all(
    [...modules].map((path) => read(path, vscode.Uri.joinPath(folder, ...path.split("/")))),
  );
  // No trailing `/`, so `rootOf` and the worker agree on it (a folder at the
  // root of a filesystem, like the test host's `vscode-test-web://mount/`, has one).
  return { root: folder.toString().replace(/\/+$/, ""), files };
}

/** Every workspace folder that is a Bynk project. */
export async function loadProjects(): Promise<ProjectSnapshot[]> {
  const folders = vscode.workspace.workspaceFolders ?? [];
  const loaded = await Promise.all(folders.map((f) => loadProject(f.uri)));
  return loaded.filter((p): p is ProjectSnapshot => p !== undefined);
}

/** The project root `uri` lies under, from the roots currently loaded. */
export function rootOf(roots: Iterable<string>, uri: vscode.Uri): string | undefined {
  const s = uri.toString();
  let best: string | undefined;
  for (const root of roots) {
    if (s.startsWith(`${root}/`) && (!best || root.length > best.length)) best = root;
  }
  return best;
}

/** `uri`'s path below `root` (both `Uri.toString()` spellings). */
export function relativeTo(root: string, uri: vscode.Uri): string {
  const rootPath = vscode.Uri.parse(root).path.replace(/\/+$/, "");
  return uri.path.slice(rootPath.length + 1);
}

/** Read one changed file for the worker, or `null` when it is gone. */
export async function readChanged(uri: vscode.Uri): Promise<string | null> {
  return (await readText(uri)) ?? null;
}
