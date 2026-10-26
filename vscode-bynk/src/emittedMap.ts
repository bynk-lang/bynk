// #2471: the lookups behind "Bynk: Show Compiled Output" — pure, no `vscode`
// import, so they unit-test without a host.
//
// The emitter's source maps are line-level (ADR 0103): each generated line
// maps to the source line of its nearest enclosing statement. `parseEmitted`
// decodes a map to `lines[n] = [source id, source line] | null` (both
// 0-based), and the lookup functions answer both directions from it.

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/** Decode a base64 VLQ run, or `null` on a character outside the alphabet. */
export function decodeVlq(s: string): number[] | null {
  const out: number[] = [];
  let shift = 0;
  let acc = 0;
  for (const c of s) {
    const d = B64.indexOf(c);
    if (d < 0) return null;
    acc += (d & 31) * 2 ** shift;
    if (d & 32) {
      shift += 5;
      if (shift > 50) return null;
    } else {
      out.push(acc % 2 === 1 ? -Math.floor(acc / 2) : Math.floor(acc / 2));
      shift = 0;
      acc = 0;
    }
  }
  return out;
}

/**
 * Decode a v3 `mappings` string to one `[source id, source line]` per
 * generated line. The emitter writes one segment per line; for any valid v3
 * map with several, the first is the line's start and is the one used. A line
 * with no segment is `null`. `null` for a malformed string.
 */
export function decodeMappings(mappings: string): ([number, number] | null)[] | null {
  const lines: ([number, number] | null)[] = [];
  let src = 0;
  let line = 0;
  for (const group of mappings.split(";")) {
    const first = group.split(",")[0];
    if (first === "") {
      lines.push(null);
      continue;
    }
    const f = decodeVlq(first);
    if (!f) return null;
    if (f.length < 4) {
      lines.push(null);
      continue;
    }
    src += f[1];
    line += f[2];
    if (src < 0 || line < 0) return null;
    lines.push([src, line]);
  }
  return lines;
}

/** The map's source names compared the way the emitter spells them: absolute,
 *  `/`-separated, and case-insensitive where the file system is. */
function normalise(p: string): string {
  const s = p.replace(/\\/g, "/");
  return process.platform === "win32" ? s.toLowerCase() : s;
}

/**
 * Parse a generated module and its v3 source map, for the `.bynk` file at
 * `sourcePath`. `undefined` when the map is malformed or does not list that
 * file (the module comes from other sources).
 */
export function parseEmitted(
  outputPath: string,
  text: string,
  mapJson: string,
  sourcePath: string,
): EmittedFile | undefined {
  let map: { sources?: unknown; mappings?: unknown };
  try {
    map = JSON.parse(mapJson);
  } catch {
    return undefined;
  }
  if (!Array.isArray(map.sources) || typeof map.mappings !== "string") return undefined;
  const sources = map.sources.filter((s): s is string => typeof s === "string");
  const want = normalise(sourcePath);
  const sourceIndex = sources.findIndex((s) => normalise(s) === want);
  if (sourceIndex < 0) return undefined;
  const lines = decodeMappings(map.mappings);
  if (!lines) return undefined;
  return { path: outputPath, text, sources, sourceIndex, lines };
}

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
