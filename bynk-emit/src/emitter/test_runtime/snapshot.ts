// #2842: snapshot testing. The stored snapshot files are inlined by the compiler
// (`__bynkSnapshotFiles`), so this runtime reads nothing from disk. Under
// `--update-snapshots` (`BYNK_UPDATE_SNAPSHOTS=1`) a claim records its value instead
// of comparing, and `__bynkSnapshotReport` hands the driver the files to write.
type __SnapshotFile = { stored: string | null; live: readonly (readonly [string, string])[] };
const __bynkSnapshotUpdate: boolean = (globalThis as { process?: { env?: globalThis.Record<string, string | undefined> } }).process?.env?.["BYNK_UPDATE_SNAPSHOTS"] === "1";
let __bynkSnapshotAt: { file: string; case: string } = { file: "", case: "" };
const __bynkSnapshotTaken: globalThis.Record<string, globalThis.Record<string, globalThis.Record<string, unknown>>> = {};
const __bynkSnapshotStored: globalThis.Record<string, globalThis.Record<string, globalThis.Record<string, unknown>> | "corrupt"> = {};
function __bynkSnapshotCase(file: string, caseName: string): void {
  __bynkSnapshotAt = { file, case: caseName };
}
// A value with its object keys in sorted order throughout, so a snapshot is stable.
function __bynkSnapshotCanon(v: unknown): unknown {
  if (globalThis.Array.isArray(v)) return v.map(__bynkSnapshotCanon);
  if (v !== null && typeof v === "object") {
    const out: globalThis.Record<string, unknown> = {};
    for (const k of globalThis.Object.keys(v).sort()) out[k] = __bynkSnapshotCanon((v as globalThis.Record<string, unknown>)[k]);
    return out;
  }
  return v;
}
function __bynkSnapshotPretty(v: unknown): string {
  return globalThis.JSON.stringify(__bynkSnapshotCanon(v), null, 2);
}
function __bynkSnapshotLoad(file: string): globalThis.Record<string, globalThis.Record<string, unknown>> | "corrupt" {
  const known = __bynkSnapshotStored[file];
  if (known !== undefined) return known;
  const text = __bynkSnapshotFiles[file]?.stored ?? null;
  let parsed: globalThis.Record<string, globalThis.Record<string, unknown>> | "corrupt" = {};
  if (text !== null) {
    try {
      const v = globalThis.JSON.parse(text);
      parsed = v !== null && typeof v === "object" && !globalThis.Array.isArray(v) ? v : "corrupt";
    } catch {
      parsed = "corrupt";
    }
  }
  __bynkSnapshotStored[file] = parsed;
  return parsed;
}
const __bynkHas = (o: object, k: string): boolean => globalThis.Object.prototype.hasOwnProperty.call(o, k);
// A line diff of two pretty-printed values, in the order of the expected text.
function __bynkSnapshotDiff(expected: string, actual: string): string {
  const a = expected.split("\n");
  const b = actual.split("\n");
  const n = a.length;
  const m = b.length;
  const lcs: number[][] = globalThis.Array.from({ length: n + 1 }, () => new globalThis.Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i]![j] = a[i] === b[j] ? lcs[i + 1]![j + 1]! + 1 : globalThis.Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!);
    }
  }
  const out: string[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) { out.push("    " + a[i]); i++; j++; }
    else if (lcs[i + 1]![j]! >= lcs[i]![j + 1]!) { out.push("  - " + a[i]); i++; }
    else { out.push("  + " + b[j]); j++; }
  }
  while (i < n) { out.push("  - " + a[i]); i++; }
  while (j < m) { out.push("  + " + b[j]); j++; }
  return out.join("\n");
}
// Compare (or, under the update flag, record) the current case's snapshot `name`
// against `json`, the value's `Json.encode` text. Returns why it failed, or
// `undefined` when it holds.
function __bynkSnapshotCheck(name: string, json: string): string | undefined {
  const { file, case: caseName } = __bynkSnapshotAt;
  const actual = __bynkSnapshotPretty(globalThis.JSON.parse(json));
  const taken = ((__bynkSnapshotTaken[file] ??= {})[caseName] ??= {});
  if (__bynkHas(taken, name) && __bynkSnapshotPretty(taken[name]) !== actual) {
    return `snapshot "${name}" is taken twice in this case with different values`;
  }
  taken[name] = globalThis.JSON.parse(actual);
  if (__bynkSnapshotUpdate) return undefined;
  const stored = __bynkSnapshotLoad(file);
  if (stored === "corrupt") {
    return `the snapshot file for this suite is not a JSON object of cases; fix it, or run with --update-snapshots to rewrite it`;
  }
  if (!__bynkHas(stored, caseName) || stored[caseName] === null || typeof stored[caseName] !== "object" || !__bynkHas(stored[caseName]!, name)) {
    return `no snapshot "${name}" is recorded for this case; run with --update-snapshots to record it\n  actual:\n${actual.split("\n").map((l) => "    " + l).join("\n")}`;
  }
  const expected = __bynkSnapshotPretty(stored[caseName]![name]);
  if (expected === actual) return undefined;
  return `snapshot "${name}" differs (- stored, + actual); run with --update-snapshots to accept the change\n${__bynkSnapshotDiff(expected, actual)}`;
}
// One entry per suite file with a snapshot: its counts, and — under the update
// flag, when the file changes — the text to write (`null` to delete it).
export function __bynkSnapshotReport(): { file: string; written: number; updated: number; obsolete: number; content?: string | null }[] {
  const out: { file: string; written: number; updated: number; obsolete: number; content?: string | null }[] = [];
  for (const file of globalThis.Object.keys(__bynkSnapshotFiles).sort()) {
    const info = __bynkSnapshotFiles[file]!;
    const stored = __bynkSnapshotLoad(file);
    const base = stored === "corrupt" ? {} : stored;
    const live = new globalThis.Set(info.live.map(([c, n]) => c + "\u0000" + n));
    let obsolete = 0;
    for (const c of globalThis.Object.keys(base)) {
      const names = base[c];
      if (names === null || typeof names !== "object") continue;
      for (const n of globalThis.Object.keys(names)) if (!live.has(c + "\u0000" + n)) obsolete++;
    }
    let written = 0;
    let updated = 0;
    const next: globalThis.Record<string, globalThis.Record<string, unknown>> = {};
    for (const [c, n] of info.live) {
      const taken = __bynkSnapshotTaken[file]?.[c];
      const had = __bynkHas(base, c) && base[c] !== null && typeof base[c] === "object" && __bynkHas(base[c]!, n);
      if (taken !== undefined && __bynkHas(taken, n)) {
        if (__bynkSnapshotUpdate) {
          if (!had) written++;
          else if (__bynkSnapshotPretty(base[c]![n]) !== __bynkSnapshotPretty(taken[n])) updated++;
          (next[c] ??= {})[n] = taken[n];
          continue;
        }
      }
      if (had) (next[c] ??= {})[n] = base[c]![n];
    }
    const entry: { file: string; written: number; updated: number; obsolete: number; content?: string | null } = { file, written, updated, obsolete };
    if (__bynkSnapshotUpdate && (written > 0 || updated > 0 || obsolete > 0 || stored === "corrupt")) {
      entry.content = globalThis.Object.keys(next).length === 0 ? null : __bynkSnapshotPretty(next) + "\n";
    }
    out.push(entry);
  }
  return out;
}
