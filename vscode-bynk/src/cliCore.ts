// The dependency-free half of the `bynk` CLI runner (#2094): line splitting for
// streamed output, the `--format short` diagnostic shape, and terminal text
// conversion. No `vscode` import, so the unit tests compile against it directly
// (see tsconfig.test.json). The `vscode`-facing half is `cli.ts`.

import * as path from "node:path";

/** Splits a stream of text chunks into complete lines. A chunk boundary can fall
 *  anywhere, including inside a line or between `\r` and `\n`; `push` returns
 *  only the lines a chunk completed and keeps the rest for the next one. */
export class LineSplitter {
  private buf = "";

  /** Feed a chunk; returns the lines it completed, without their terminators. */
  push(chunk: string): string[] {
    this.buf += chunk;
    const parts = this.buf.split(/\r?\n/);
    this.buf = parts.pop() ?? "";
    return parts;
  }

  /** The trailing partial line, if any, once the stream has ended. */
  flush(): string[] {
    const rest = this.buf;
    this.buf = "";
    return rest ? [rest] : [];
  }
}

/** One `path:line:col: severity[code]: message` diagnostic, the shape
 *  `bynk check --format short` prints and the `$bynkc` problem matcher parses.
 *  `line` and `column` are 1-based, as printed. */
export interface ShortDiagnostic {
  file: string;
  line: number;
  column: number;
  severity: "error" | "warning";
  code: string;
  message: string;
}

const SHORT_DIAGNOSTIC = /^(.+?):(\d+):(\d+): (error|warning)\[([^\]]+)\]: (.+)$/;

/** Parse one `--format short` diagnostic line, or `undefined` for any other line. */
export function parseShortDiagnostic(line: string): ShortDiagnostic | undefined {
  const m = SHORT_DIAGNOSTIC.exec(line);
  if (!m) return undefined;
  return {
    file: m[1],
    line: Number(m[2]),
    column: Number(m[3]),
    severity: m[4] as "error" | "warning",
    code: m[5],
    message: m[6],
  };
}

/** The file a `--format short` diagnostic names, as an absolute path: an
 *  absolute path as given, a relative one joined to the `root` the command
 *  ran in. */
export function diagnosticPath(root: string, file: string): string {
  return path.isAbsolute(file) ? path.normalize(file) : path.join(root, file);
}

/** What a failed launch should offer: `install` when `bynk` itself could not
 *  be started (the spawn's own `ENOENT`, read from the error's `code`), so the
 *  Doctor, which runs through `bynk`, can't help; `doctor` for anything else.
 *  A debuggee whose stderr merely mentions "ENOENT" is still a `doctor` case. */
export function launchFailureAction(e: unknown): "install" | "doctor" {
  return (e as { code?: unknown } | null)?.code === "ENOENT" ? "install" : "doctor";
}

/** Text for a pseudoterminal, which needs `\r\n` line ends: a bare `\n` only
 *  moves down a row, so lines would stair-step across the terminal. */
export function toTerminalText(text: string): string {
  return text.replace(/\r?\n/g, "\r\n");
}

/** Escape text for an HTML text node or a double-quoted attribute, for the
 *  host-rendered views (deploy plan, doctor) that build their markup from CLI
 *  output. */
export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}
