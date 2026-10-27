// #2471: read the modules a `bynk compile` wrote to a scratch directory. Split
// from `emittedBuild.ts` (which spawns the CLI, so needs `vscode`) so it
// unit-tests without a host.

import * as fs from "node:fs";
import * as path from "node:path";

import { parseEmitted, type EmittedFile } from "./emittedMap";

/** Every `*.ts` under `dir`, as paths relative to it, sorted. */
export function listModules(dir: string): string[] {
  const out: string[] = [];
  const walk = (rel: string) => {
    for (const entry of fs.readdirSync(path.join(dir, rel), { withFileTypes: true })) {
      const next = path.join(rel, entry.name);
      if (entry.isDirectory()) walk(next);
      else if (entry.name.endsWith(".ts") && !entry.name.endsWith(".d.ts")) out.push(next);
    }
  };
  walk("");
  return out.sort();
}

/** Read the modules under `dir` whose map names `sourcePath`, in path order. */
export function readModules(dir: string, sourcePath: string): EmittedFile[] {
  const files: EmittedFile[] = [];
  for (const rel of listModules(dir)) {
    let map: string;
    try {
      map = fs.readFileSync(path.join(dir, `${rel}.map`), "utf8");
    } catch {
      continue; // no map: not generated from a source file
    }
    const text = fs.readFileSync(path.join(dir, rel), "utf8");
    const file = parseEmitted(rel.split(path.sep).join("/"), text, map, sourcePath);
    if (file) files.push(file);
  }
  return files;
}
