// #2097: which Bynk project a file belongs to — the directory of the nearest
// `bynk.toml` at or above it, the same rule the language server uses. The
// project sidebar groups files by it, and the architecture-model cache keys
// on it, so the map and the sidebar share one request per project however
// they name the project's files.

import * as vscode from "vscode";

const cache = new Map<string, Promise<vscode.Uri | undefined>>();

async function exists(uri: vscode.Uri): Promise<boolean> {
  try {
    await vscode.workspace.fs.stat(uri);
    return true;
  } catch {
    return false;
  }
}

async function lookUp(dir: vscode.Uri): Promise<vscode.Uri | undefined> {
  if (await exists(vscode.Uri.joinPath(dir, "bynk.toml"))) return dir;
  const parent = vscode.Uri.joinPath(dir, "..");
  if (parent.path === dir.path) return undefined;
  return rootOfDir(parent);
}

function rootOfDir(dir: vscode.Uri): Promise<vscode.Uri | undefined> {
  const key = dir.toString();
  let hit = cache.get(key);
  if (!hit) {
    hit = lookUp(dir);
    cache.set(key, hit);
  }
  return hit;
}

/** The project root (the directory holding the nearest `bynk.toml`) for a
 *  file, or `undefined` when no manifest is at or above it. */
export function projectRootFor(file: vscode.Uri): Promise<vscode.Uri | undefined> {
  return rootOfDir(vscode.Uri.joinPath(file, ".."));
}

/** Forget cached lookups (a `bynk.toml` was created or deleted). */
export function clearProjectRoots(): void {
  cache.clear();
}
