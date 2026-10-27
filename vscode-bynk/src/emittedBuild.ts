// #2471: compile the project to a scratch directory through the CLI and read
// back the modules that come from one `.bynk` file.
//
// `bynk compile --sources-stdin` takes the project's unsaved buffers as JSON on
// stdin, so what is compiled is what is on screen, and writes each module with
// its source map under `--output`. Nothing outside that directory changes (a
// preview build neither reads nor writes `bynk.schema.lock`). The per-module
// runtime (`__bynk_runtime/…`, ADR 0615) is written too but belongs to no
// source file, so its maps never name one and it is not read.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import type * as vscode from "vscode";

import { runBynk } from "./cli";
import type { EmittedFile } from "./emittedMap";
import { readModules } from "./emittedModules";

export type BuildResult =
  | { ok: true; files: EmittedFile[] }
  | { ok: false; message: string };

/**
 * Compile the project at `root` with `buffers` (absolute path -> text) layered
 * over disk, and return the modules generated from `sourcePath`. A failed
 * compile (diagnostics, no compiler) is `{ ok: false }` with the reason.
 */
export async function buildModules(
  root: string,
  sourcePath: string,
  buffers: Record<string, string>,
  token?: vscode.CancellationToken,
): Promise<BuildResult> {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-emitted-"));
  try {
    const result = await runBynk(
      ["compile", root, "--output", scratch, "--sources-stdin"],
      root,
      token,
      JSON.stringify(buffers),
    );
    if (token?.isCancellationRequested) return { ok: false, message: "cancelled" };
    if (result.code !== 0) {
      const reason = (result.stderr || result.stdout).trim().split("\n")[0];
      return { ok: false, message: reason || `bynk compile exited with ${result.code}` };
    }
    return { ok: true, files: readModules(scratch, sourcePath) };
  } catch (e) {
    return { ok: false, message: e instanceof Error ? e.message : String(e) };
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
}
