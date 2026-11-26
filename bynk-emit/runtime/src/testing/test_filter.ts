export type Result = {
  name: string;
  pass: boolean;
  error?: { message: string; location: string; actual?: string; violation?: { kind: string; agent: string; name: string; location: string }; refusals?: { index: number; call: string; kind: string; agent: string; name: string; location: string }[] };
  durationMs?: number;
  retries?: number;
  kind?: string;
  seed?: string;
  counterexample?: string;
};
export type Filter = {
  program?: string;
  suites?: string[];
  cases?: string[];
  partial?: boolean;
  skipped?: { suite: string; case: string }[];
};
export const filter: Filter | undefined =
  process.env.BYNK_TEST_FILTER === undefined ? undefined : globalThis.JSON.parse(process.env.BYNK_TEST_FILTER);
export const matchedSuites = new Set<string>();
export const matchedCases = new Set<string>();

export function selected(suite: string): boolean {
  if (filter?.suites === undefined) return true;
  if (!filter.suites.includes(suite)) return false;
  matchedSuites.add(suite);
  return true;
}

// How many cases the filter selects in a module, known before any of them runs: the
// `tests` of its `suite-begin`. A name is counted once per case that bears it.
export function planned(names: readonly string[]): number {
  if (filter?.cases === undefined) return names.length;
  let count = 0;
  for (const name of filter.cases) count += names.filter((n) => n === name).length;
  return count;
}

// Runs a module's selected cases, handing each result to `onCase` as it finishes.
export async function runSelected(
  run: (only?: string, onCase?: (r: Result) => void) => Promise<Result[]>,
  onCase: (r: Result) => void,
): Promise<Result[]> {
  if (filter?.cases === undefined) return run(undefined, onCase);
  const results: Result[] = [];
  for (const name of filter.cases) {
    const found = await run(name, onCase);
    if (found.length > 0) matchedCases.add(name);
    results.push(...found);
  }
  return results;
}

// A case the build skipped is not in it but exists, so a value naming one is matched.
for (const s of filter?.skipped ?? []) {
  if (selected(s.suite) && filter?.cases?.includes(s.case)) matchedCases.add(s.case);
}

export function unmatched(): { suites?: string[]; cases?: string[] } | undefined {
  if (filter === undefined || filter.partial === true) return undefined;
  const suites = (filter.suites ?? []).filter((s) => !matchedSuites.has(s));
  const cases = (filter.cases ?? []).filter((c) => !matchedCases.has(c));
  if (suites.length === 0 && cases.length === 0) return undefined;
  return { ...(suites.length > 0 ? { suites } : {}), ...(cases.length > 0 ? { cases } : {}) };
}

export function reportUnmatched(): void {
  const left = unmatched();
  if (left === undefined) return;
  const program = filter?.program ?? "bynk";
  for (const s of left.suites ?? []) {
    globalThis.console.error(`${program} test: no suite named --suite ${globalThis.JSON.stringify(s)}`);
  }
  for (const c of left.cases ?? []) {
    globalThis.console.error(`${program} test: no case matched --case ${globalThis.JSON.stringify(c)}`);
  }
}
