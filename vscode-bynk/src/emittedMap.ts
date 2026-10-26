// #2471: the lookups behind "Bynk: Show Compiled Output" — pure, no `vscode`
// import, so they unit-test without a host.
//
// The emitter's source maps are line-level (ADR 0103): each generated line
// maps to the source line of its nearest enclosing statement. The server
// decodes the map to `lines[n] = [source id, source line] | null` (both
// 0-based; see `bynk-lsp/src/emitted_request.rs`), and these functions answer
// both directions from it.

/** One generated module and the decoded map back to its sources. */
export interface EmittedFile {
  /** Output path, relative to the output root (`greet.ts`). */
  path: string;
  text: string;
  sources: string[];
  /** Which entry of `sources` is the file being shown. */
  sourceIndex: number;
  lines: ([number, number] | null)[];
}

/** An inclusive range of whole lines (0-based). */
export interface LineRange {
  start: number;
  end: number;
}

/**
 * The generated lines for the source `line`: the run produced by the nearest
 * mapped statement at or before it (a cursor inside a statement, or on blank
 * space after it, lands on that statement). `null` before the file's first
 * mapped statement.
 */
export function generatedRangeForSource(file: EmittedFile, line: number): LineRange | null {
  let best = -1;
  for (const entry of file.lines) {
    if (entry && entry[0] === file.sourceIndex && entry[1] <= line && entry[1] > best) {
      best = entry[1];
    }
  }
  if (best < 0) return null;
  const matches = (i: number) => {
    const e = file.lines[i];
    return !!e && e[0] === file.sourceIndex && e[1] === best;
  };
  const first = file.lines.findIndex((_, i) => matches(i));
  let end = first;
  while (end + 1 < file.lines.length && matches(end + 1)) end++;
  return { start: first, end };
}

/**
 * The source lines for the generated `line`: from its mapped statement up to
 * the next mapped statement start, with trailing lines `isBlank` calls blank
 * trimmed (`isBlank` reads the source text). `null` when the line is unmapped
 * or maps into a different source (a test module aggregates several files).
 */
export function sourceRangeForGenerated(
  file: EmittedFile,
  line: number,
  isBlank: (sourceLine: number) => boolean,
): LineRange | null {
  const entry = file.lines[line];
  if (!entry || entry[0] !== file.sourceIndex) return null;
  const start = entry[1];
  let next = Infinity;
  for (const e of file.lines) {
    if (e && e[0] === file.sourceIndex && e[1] > start && e[1] < next) next = e[1];
  }
  let end = next === Infinity ? start : next - 1;
  while (end > start && isBlank(end)) end--;
  return { start, end };
}
