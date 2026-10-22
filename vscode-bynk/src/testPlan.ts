// What the Test Explorer runs for a request, and how a run document maps onto it
// (Test Explorer depth track, Slice A). Dependency-free (no `vscode` import), so
// the unit tests compile against it directly; `testing.ts` feeds it the tree and
// the request and acts on the plan.
//
// Three things live here:
//   - the pinned `bynk test --format json` document (mirrors
//     `bynk-driver/src/test_json.rs`), with the outcome mapping that keeps a
//     `skipped` case from being read as a failure;
//   - the run plan: which `bynk test` processes a selection needs, and which
//     cases the run is allowed to report;
//   - the small selection helpers (a file's cases, a suite's file) the controller
//     uses for the file-level run and the suite `uri`.
//
// `--case` takes one exact name and matches across suites, so a selection of
// several items cannot be one process yet (track doc §3.8). The interim, settled
// in the Slice A proposal: a handful of individually-selected cases run as one
// process each, and anything larger (a suite, or more than a handful of cases)
// runs the whole project once. Either way only the requested items are reported.

import type { JsonCoverage } from "./coverageModel";

// ---------------------------------------------------------------------------
// The run document
// ---------------------------------------------------------------------------

export interface JsonLocation {
  path: string;
  line: number;
  col: number;
}

export interface JsonCase {
  name: string;
  /** "pass" / "fail" from a run, "skipped" for a case a remote run skipped
   *  (#2138), "discovered" in a `--no-run` document. Kept a string: a value this
   *  extension does not know is reported, not coerced (see {@link outcomeOf}). */
  outcome: string;
  message?: string;
  location?: JsonLocation;
  /** #2138: how often `--settle` retried the case in a remote run. */
  retries?: number;
  /** The case's effective tier (ADR 0153): its own `as <tier>`, else its suite's,
   *  else `unit`. Absent for a property, which has none, and from every case of a
   *  driver that predates the field (see {@link tierOf}). */
  tier?: string;
  /** A generative case's kind: `"property"` or `"history"`. Set by discovery for
   *  every generative case, and by a run for one that failed, with `seed` and
   *  (when shrunk) `counterexample`. */
  kind?: string;
  /** The run's root seed (`0x…`), as `--seed` takes it. */
  seed?: string;
  /** The shrunk counterexample, as the runner rendered it. */
  counterexample?: string;
}

export const TIERS = ["unit", "integration", "system"] as const;
export type Tier = (typeof TIERS)[number];

/** The tier a case is tagged with: the driver's per-case `tier`. `legacyKind` is
 *  the suite's `kind` (unit or integration, from the module name) and is passed
 *  only for a document from a driver that predates the field, where it is all
 *  there is; a driver that carries tiers omits one on purpose for a property, so
 *  that case is left untagged. A tier this extension does not know is not tagged. */
export function tierOf(c: Pick<JsonCase, "tier">, legacyKind?: string): Tier | undefined {
  const known = (v: string | undefined): Tier | undefined =>
    TIERS.find((t) => t === v);
  return known(c.tier) ?? known(legacyKind);
}

/** Whether a document comes from a driver that carries per-case tiers: any case
 *  has one. A document of only properties has none and reads as older, which only
 *  tags those properties by their suite's kind, and never as `system`. */
export function carriesTiers(suites: readonly Pick<JsonSuite, "cases">[]): boolean {
  return suites.some((s) => s.cases.some((c) => c.tier !== undefined));
}

export interface JsonSuite {
  name: string;
  /** "unit" or "integration" today; the driver derives it from the module name,
   *  not from a case's tier. */
  kind: string;
  cases: JsonCase[];
}

export interface JsonError {
  kind: "compile" | "runtime";
  message?: string;
  diagnostics?: string[];
  stderr?: string;
}

export interface TestRun {
  passed: number;
  failed: number;
  /** #2138: the cases a remote run skipped. Absent when zero. */
  skipped?: number;
  suites?: JsonSuite[];
  error?: JsonError;
  /** #2104: present for a `--coverage` run. */
  coverage?: JsonCoverage;
  /** #2138: where the `system` suites ran, `"remote:<target>"` under
   *  `--against`. Absent for an in-process run. */
  transport?: string;
}

export type Outcome = "passed" | "failed" | "skipped" | "discovered" | "unknown";

/** Map a case's `outcome` onto what the Test API reports. A value this
 *  extension does not know is `unknown`, which the controller reports as a
 *  failure naming the raw outcome rather than as a pass. */
export function outcomeOf(raw: string): Outcome {
  switch (raw) {
    case "pass":
      return "passed";
    case "fail":
      return "failed";
    case "skipped":
      return "skipped";
    case "discovered":
      return "discovered";
    default:
      return "unknown";
  }
}

/** Run-level lines for a document: where it ran and how much was skipped. Empty
 *  for an ordinary in-process run, so its output is unchanged. */
export function runNotes(doc: Pick<TestRun, "skipped" | "transport">): string[] {
  const notes: string[] = [];
  if (doc.transport) notes.push(`ran the system suites against ${doc.transport.replace(/^remote:/, "")}`);
  if (doc.skipped) {
    notes.push(`${doc.skipped} ${doc.skipped === 1 ? "case was" : "cases were"} skipped`);
  }
  return notes;
}

/** One line for a skipped case: its name, the driver's reason when it gave one,
 *  and a retry count if the case was retried. */
export function skippedLine(c: Pick<JsonCase, "name" | "message">): string {
  return c.message ? `skipped: ${c.name} — ${c.message}` : `skipped: ${c.name}`;
}

/** A retry note for a case `--settle` retried, or `undefined`. */
export function retriesNote(c: Pick<JsonCase, "name" | "retries">): string | undefined {
  return c.retries && c.retries > 0
    ? `${c.name}: retried ${c.retries} ${c.retries === 1 ? "time" : "times"}`
    : undefined;
}

// ---------------------------------------------------------------------------
// The run plan
// ---------------------------------------------------------------------------

/** One discovered case in the tree. `suiteId` is the suite item's id; `file` is
 *  an opaque, comparable key for the case's source file (the controller uses the
 *  item's URI string). */
export interface PlanCase {
  suiteId: string;
  name: string;
  file?: string;
  tier?: Tier;
}

/** A requested or excluded item: a whole suite (no `name`) or one case. */
export interface ItemRef {
  suiteId: string;
  name?: string;
}

export interface RequestRef {
  /** Absent or empty means "everything". */
  include?: readonly ItemRef[];
  exclude?: readonly ItemRef[];
}

/** One `bynk test` process: `caseName` unset runs the whole project. */
export interface RunStep {
  caseName?: string;
}

export interface RunPlan {
  /** The case keys the run may report, in tree order. */
  wanted: string[];
  /** The processes to run, in order. Empty when nothing is selected. */
  steps: RunStep[];
  /** The selection is narrower than the whole tree. */
  narrowed: boolean;
}

/** The most individually-selected cases that run as one process each. Beyond
 *  this, repeating the compile per case costs more than one whole-project run
 *  whose results are filtered. */
export const PER_CASE_LIMIT = 5;

/** The key a case is reported under: its suite item id and its name, which is
 *  also how the controller builds the case item's id. */
export function caseKey(suiteId: string, name: string): string {
  return `${suiteId}::${name}`;
}

interface Selection {
  keys: Set<string>;
  /** The selection was expanded from a suite item or from "everything", rather
   *  than named case by case. */
  expanded: boolean;
}

function select(all: readonly PlanCase[], request: RequestRef): Selection {
  const keys = new Set<string>();
  let expanded = false;
  const include = request.include ?? [];
  if (include.length === 0) {
    expanded = true;
    for (const c of all) keys.add(caseKey(c.suiteId, c.name));
  } else {
    for (const ref of include) {
      if (ref.name === undefined) {
        expanded = true;
        for (const c of all) if (c.suiteId === ref.suiteId) keys.add(caseKey(c.suiteId, c.name));
      } else {
        keys.add(caseKey(ref.suiteId, ref.name));
      }
    }
  }
  for (const ref of request.exclude ?? []) {
    if (ref.name === undefined) {
      for (const c of all) if (c.suiteId === ref.suiteId) keys.delete(caseKey(c.suiteId, c.name));
    } else {
      keys.delete(caseKey(ref.suiteId, ref.name));
    }
  }
  return { keys, expanded };
}

/** Restrict a request to the `system` cases, for a run against a target: only
 *  the `system` suites run there. A suite stays a suite reference (it expands
 *  over the system cases that remain); a named case that is not `system` is
 *  dropped, and so is an exclusion of one. `empty` is set when the request named
 *  things and none was a system case. */
export function systemOnly(
  all: readonly PlanCase[],
  request: RequestRef,
): { cases: PlanCase[]; request: RequestRef; empty: boolean } {
  const cases = all.filter((c) => c.tier === "system");
  // A suite reference counts only if the suite holds a system case: selecting
  // one that holds none must not start a run against a target for nothing.
  const isSystem = (r: ItemRef): boolean =>
    cases.some((c) => c.suiteId === r.suiteId && (r.name === undefined || c.name === r.name));
  const include = request.include?.filter(isSystem);
  const requested = (request.include ?? []).length > 0;
  return {
    cases,
    request: { include, exclude: request.exclude?.filter(isSystem) },
    empty: cases.length === 0 || (requested && (include ?? []).length === 0),
  };
}

/** Plan a run. `coverage` and `remote` limit it to one process, because a
 *  coverage report is one process's and a run against a target starts the
 *  target (or its dev server) once; a single case still uses `--case`, anything
 *  else runs the whole project and is filtered. */
export function planRun(
  all: readonly PlanCase[],
  request: RequestRef,
  opts: { coverage?: boolean; remote?: boolean } = {},
): RunPlan {
  const { keys, expanded } = select(all, request);
  // Tree order, restricted to what is selected and known. A requested case the
  // tree does not hold (a stale item) is still wanted: it will be reported
  // missing rather than silently dropped.
  const known = all.map((c) => caseKey(c.suiteId, c.name));
  const knownSet = new Set(known);
  const wanted = known.filter((k) => keys.has(k));
  for (const k of keys) if (!knownSet.has(k)) wanted.push(k);

  if (wanted.length === 0) {
    // "Run everything" with nothing discovered yet (discovery still in flight, or
    // it hit a compile error) still runs the project: the run reports the
    // compile diagnostics, or fills the tree from its results. Only a selection
    // that came to nothing, once excluded, has nothing to run.
    const everything = (request.include ?? []).length === 0 && (request.exclude ?? []).length === 0;
    return everything ? { wanted, steps: [{}], narrowed: false } : { wanted, steps: [], narrowed: true };
  }

  const narrowed = wanted.length < known.length || wanted.some((k) => !knownSet.has(k));
  if (!narrowed) return { wanted, steps: [{}], narrowed };

  const names: string[] = [];
  for (const k of wanted) {
    const name = k.slice(k.indexOf("::") + 2);
    if (!names.includes(name)) names.push(name);
  }

  if (opts.coverage || opts.remote) {
    return { wanted, steps: names.length === 1 ? [{ caseName: names[0] }] : [{}], narrowed };
  }
  if (!expanded && names.length <= PER_CASE_LIMIT) {
    return { wanted, steps: names.map((caseName) => ({ caseName })), narrowed };
  }
  return { wanted, steps: [{}], narrowed };
}

/** Whether a result for (`suiteId`, `name`) is one the run was asked for. */
export function isWanted(plan: Pick<RunPlan, "wanted">, suiteId: string, name: string): boolean {
  return plan.wanted.includes(caseKey(suiteId, name));
}

/** The wanted keys no result reached, so the controller can say so. */
export function unreported(plan: Pick<RunPlan, "wanted">, reported: ReadonlySet<string>): string[] {
  return plan.wanted.filter((k) => !reported.has(k));
}

// ---------------------------------------------------------------------------
// Debug
// ---------------------------------------------------------------------------

/** What Debug does with a selection. The debug launch filters by one case name
 *  (`--case`), so a suite or several cases cannot be narrowed: they debug the
 *  whole project, and the user is told. */
export function debugTarget(
  all: readonly PlanCase[],
  request: RequestRef,
): { caseName?: string; notice?: string; nothing?: boolean } {
  const plan = planRun(all, request);
  if (plan.steps.length === 0) {
    return { notice: "Bynk: nothing is selected to debug.", nothing: true };
  }
  if (!plan.narrowed) return {};
  if (plan.wanted.length === 1) {
    const k = plan.wanted[0];
    const caseName = k.slice(k.indexOf("::") + 2);
    // `--case` matches by name across suites, and the debugger cannot filter any
    // finer, so a name another suite shares runs that suite's case too.
    const suiteId = k.slice(0, k.indexOf("::"));
    const shared = all.some((c) => c.name === caseName && c.suiteId !== suiteId);
    return shared
      ? {
          caseName,
          notice:
            "Bynk: another suite has a test with the same name, and the debugger filters by name, " +
            "so that one runs under the inspector too.",
        }
      : { caseName };
  }
  return {
    notice:
      "Bynk: debugging filters by a single case, so the whole project runs under the inspector. " +
      "Select one test to debug just that case.",
  };
}

// ---------------------------------------------------------------------------
// Files and suites
// ---------------------------------------------------------------------------

/** The cases declared in `file`, as requested items (for "Run Tests in This
 *  File"). */
export function casesInFile(all: readonly PlanCase[], file: string): ItemRef[] {
  return all.filter((c) => c.file === file).map((c) => ({ suiteId: c.suiteId, name: c.name }));
}

/** The one file a suite's cases share, or `undefined` when they are in several
 *  files, or when any has no known file. Correct whichever way the suite-to-file
 *  relation falls (track doc §3.1). */
export function suiteFile(files: readonly (string | undefined)[]): string | undefined {
  if (files.length === 0) return undefined;
  const first = files[0];
  if (first === undefined) return undefined;
  return files.every((f) => f === first) ? first : undefined;
}
