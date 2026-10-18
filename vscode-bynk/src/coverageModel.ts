// The dependency-free core of the Test Explorer's coverage profile (#2104): the
// `coverage` block of `bynk test --coverage --format json`, resolved to files
// and per-line statements the editor's coverage API takes. No `vscode` import,
// so the unit tests compile against it directly (see tsconfig.test.json);
// testing.ts maps the result onto `vscode.FileCoverage` / `StatementCoverage`.
//
// Coverage is line-level (no branches) and whole-run (no per-test attribution,
// #2104 DECISION B), attributed by the driver to `.bynk` source lines.

import * as path from "node:path";

/** One file of the driver's `coverage` block. */
export interface JsonFileCoverage {
  /** Project-relative, forward-slashed. */
  path: string;
  covered: number;
  lines: number;
  percent?: number;
  uncovered?: number[];
  /** #2104 DECISION A: the covered lines. Absent from an older driver. */
  covered_lines?: number[];
}

export interface JsonCoverage {
  covered: number;
  lines: number;
  percent?: number;
  files: JsonFileCoverage[];
}

/** One measured file, resolved for the editor. */
export interface FileLines {
  /** Absolute path: the project-relative path joined to the run's root. */
  file: string;
  covered: number;
  total: number;
  /** Executable lines, 1-based and ascending, with whether each ran. Lists
   *  every executable line when the driver sends `covered_lines`; otherwise
   *  only the uncovered ones, which is all an older driver reports. */
  statements: { line: number; executed: boolean }[];
}

const nums = (v: unknown): number[] =>
  Array.isArray(v) ? v.filter((n): n is number => typeof n === "number" && n > 0) : [];

/** The files of a coverage block, resolved against `root`, the project root
 *  the run used (not the workspace folder: a nested project's paths are
 *  relative to its own root). Lenient about unknown and missing fields. */
export function coverageFiles(coverage: unknown, root: string): FileLines[] {
  if (!coverage || typeof coverage !== "object") return [];
  const files = (coverage as { files?: unknown }).files;
  if (!Array.isArray(files)) return [];
  const out: FileLines[] = [];
  for (const raw of files) {
    if (!raw || typeof raw !== "object") continue;
    const f = raw as Record<string, unknown>;
    if (typeof f.path !== "string" || !f.path) continue;
    const executed = new Map<number, boolean>();
    for (const l of nums(f.uncovered)) executed.set(l, false);
    for (const l of nums(f.covered_lines)) executed.set(l, true);
    out.push({
      file: path.resolve(root, ...f.path.split("/")),
      covered: typeof f.covered === "number" ? f.covered : 0,
      total: typeof f.lines === "number" ? f.lines : 0,
      statements: [...executed.entries()]
        .sort(([a], [b]) => a - b)
        .map(([line, ran]) => ({ line, executed: ran })),
    });
  }
  return out;
}

/** The driver's reason a coverage run could not happen (only `tsx`, or a
 *  conflicting flag), from a runtime-error document's message, or `undefined`
 *  when the message is some other runtime failure. */
export function coverageUnavailable(message: string | undefined): string | undefined {
  return message && message.includes("--coverage") ? message : undefined;
}
