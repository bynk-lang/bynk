// Bynk Test Explorer (v0.59).
//
// Runs a project's Bynk tests via `bynk test --format json` and reports
// pass/fail through the VS Code Test API, with click-through from a failing
// assertion to its `.bynk` source. The extension links no Rust — it shells the
// `bynk` driver (v0.138+, #487/#486), inheriting its richer `bynkc` resolution
// (`BYNK_BYNKC` → PATH → sibling-of-`bynk`) instead of reimplementing one here.
//
// Discovery (v0.67): `bynk test --no-run --format json` lists the project's
// suites/cases without running them — a pure compile, no `tsc`/`node`. The
// controller's `resolveHandler` seeds the tree from it when the Testing view
// opens and `refreshHandler` backs the Refresh control. A discovered case
// carries `outcome: "discovered"` and its declaration location, so the tree
// links to the `.bynk` source before any run. Without this seeding the tree is
// empty, VS Code shows its generic "no test provider" welcome, and `Run All` has
// no root item to dispatch, so a run can never be bootstrapped from the UI. A
// run (`--format json`, no `--no-run`) reconciles onto the same tree items
// (same suite name/kind, same case names) and adds pass/fail state.

import { execFile } from "node:child_process";
import * as vscode from "vscode";

import { bynkPath, compilerOverride } from "./tasks";
import { debugBynkTests } from "./debug";
import { BynkNotFoundError } from "./cli";
import { offerDoctor, reportBynkNotFound } from "./doctor";
import { coverageFiles, coverageUnavailable } from "./coverageModel";
import {
  caseKey,
  casesInFile,
  debugTarget,
  outcomeOf,
  planRun,
  retriesNote,
  runNotes,
  skippedLine,
  suiteFile,
  unreported,
  type ItemRef,
  type JsonCase,
  type JsonLocation,
  type JsonSuite,
  type PlanCase,
  type RequestRef,
  type RunPlan,
  type TestRun,
} from "./testPlan";

/** The slice — v0.78 — surface the discovered test locations + a change signal so the
 *  Test CodeLens provider (testCodeLens.ts) can place and refresh its lenses. */
export interface TestApi {
  /** Case declarations in `uri` — range + name (suites carry no discovered
   *  location). v0.127: the name feeds the per-case run/debug commands. */
  testCases(uri: vscode.Uri): { range: vscode.Range; name: string }[];
  /** Fires after a discovery settles (the tree may have changed). */
  onDidChangeTests: vscode.Event<void>;
}

/** The first discovered case item labelled `name`, across every suite — the
 *  per-case run/debug target (v0.127). Case names are unique within a suite; a
 *  cross-suite clash resolves to the first, matching the CLI's `--case`. */
function findCase(ctrl: vscode.TestController, name: string): vscode.TestItem | undefined {
  let found: vscode.TestItem | undefined;
  ctrl.items.forEach((suite) => {
    suite.children.forEach((c) => {
      if (!found && c.label === name) found = c;
    });
  });
  return found;
}

/** A comparable key for a source file: its path, folded on Windows where paths
 *  are case-insensitive. Used both for a case's file and for the active file. */
function fileKey(uri: vscode.Uri): string {
  return process.platform === "win32" ? uri.fsPath.toLowerCase() : uri.fsPath;
}

/** The tree as the run plan sees it: every discovered case with its suite and
 *  file. */
function planCases(ctrl: vscode.TestController): PlanCase[] {
  const cases: PlanCase[] = [];
  ctrl.items.forEach((suite) => {
    suite.children.forEach((c) => {
      cases.push({ suiteId: suite.id, name: c.label, file: c.uri ? fileKey(c.uri) : undefined });
    });
  });
  return cases;
}

/** A request's include/exclude lists as plain references: a top-level item is a
 *  suite, a child is a case of its parent. */
function requestRef(request: vscode.TestRunRequest): RequestRef {
  const ref = (i: vscode.TestItem): ItemRef =>
    i.parent ? { suiteId: i.parent.id, name: i.label } : { suiteId: i.id };
  return { include: request.include?.map(ref), exclude: request.exclude?.map(ref) };
}

export function registerTesting(context: vscode.ExtensionContext): TestApi {
  const ctrl = vscode.tests.createTestController("bynk", "Bynk Tests");
  context.subscriptions.push(ctrl);

  // Compile failures surfaced by a run go to the Problems panel, exactly as the
  // `bynkc: check` task does — but in their own collection so a later clean run
  // clears them without disturbing the LSP's live diagnostics.
  const problems = vscode.languages.createDiagnosticCollection("bynk-tests");
  context.subscriptions.push(problems);

  const profile = ctrl.createRunProfile(
    "Run",
    vscode.TestRunProfileKind.Run,
    (request, token) => runHandler(ctrl, problems, request, token),
    true,
  );
  context.subscriptions.push(profile);

  // v0.72: the headline debug entry (ADR 0104, DECISION C). VS Code renders a
  // **Debug** action beside Run from this profile; it shells `bynk test
  // --inspect` and hands off to the JS debugger so a breakpoint in a test body
  // (or the code it exercises) pauses. The whole suite runs under the inspector;
  // the breakpoint decides where it stops.
  const debugProfile = ctrl.createRunProfile(
    "Debug",
    vscode.TestRunProfileKind.Debug,
    (request) => {
      // The selection decides the target: one case debugs just that case; a
      // suite or several cases cannot be narrowed (the launch filters by a
      // single `--case`), so the whole project runs and the user is told.
      const target = debugTarget(planCases(ctrl), requestRef(request));
      if (target.nothing) {
        void vscode.window.showInformationMessage(target.notice ?? "Bynk: nothing is selected to debug.");
        return;
      }
      if (target.notice) void vscode.window.showInformationMessage(target.notice);
      void debugBynkTests(target.caseName);
    },
    false,
  );
  context.subscriptions.push(debugProfile);

  // #2104: **Run with Coverage** — the Run command plus `--coverage`. VS Code's
  // Test Coverage view shows each `.bynk` file's line percentage, and the
  // editor gutter marks covered and uncovered lines.
  const coverageProfile = ctrl.createRunProfile(
    "Coverage",
    vscode.TestRunProfileKind.Coverage,
    (request, token) => runHandler(ctrl, problems, request, token, true),
    false,
  );
  coverageProfile.loadDetailedCoverage = async (_run, fc) => detailedCoverage(fc);
  context.subscriptions.push(coverageProfile);

  // v0.78: one discovery runner, *coalesced* (never stack the project compile —
  // a request arriving mid-run sets a rerun flag) and signalling `changed` when it
  // settles. The view-open/refresh handlers and the eager triggers all route here.
  const changed = new vscode.EventEmitter<void>();
  let discovering = false;
  let rerun = false;
  const runDiscovery = async (token?: vscode.CancellationToken): Promise<void> => {
    if (discovering) {
      rerun = true;
      return;
    }
    discovering = true;
    try {
      await discover(ctrl, problems, token);
    } finally {
      discovering = false;
      changed.fire();
      if (rerun) {
        rerun = false;
        void runDiscovery();
      }
    }
  };

  // Seed the tree when the Testing view first resolves its root, and back the
  // Refresh control. VS Code calls `resolveHandler(undefined)` to discover the
  // top-level tests; we build the whole tree in one shot (suites + cases).
  ctrl.resolveHandler = async (item) => {
    if (item) return;
    await runDiscovery();
  };
  ctrl.refreshHandler = (token) => runDiscovery(token);

  // v0.78: eager discovery — so the test items (and VS Code's native gutter glyphs,
  // and the CodeLens) exist whenever a `.bynk` file is on screen, not only when the
  // Testing view is open. Debounced (one project compile per edit-burst).
  let timer: ReturnType<typeof setTimeout> | undefined;
  const schedule = (): void => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void runDiscovery(), 500);
  };
  const isBynk = (d?: vscode.TextDocument): boolean => d?.languageId === "bynk";

  context.subscriptions.push(
    changed,
    vscode.workspace.onDidOpenTextDocument((d) => isBynk(d) && schedule()),
    vscode.window.onDidChangeActiveTextEditor((e) => isBynk(e?.document) && schedule()),
    vscode.workspace.onDidChangeTextDocument((e) => isBynk(e.document) && schedule()),
    vscode.commands.registerCommand("bynk.runTests", () => {
      void vscode.commands.executeCommand("testing.runAll");
    }),
    // v0.78: the CodeLens "Debug Test" target — the suite runs under the inspector,
    // the breakpoint pauses.
    vscode.commands.registerCommand("bynk.debugTests", () => {
      void debugBynkTests();
    }),
    // v0.127 (editor-currency slice 6): the per-case run/debug lens targets. Run
    // routes a single-item request through the run handler (which threads
    // `--case`), so the tree item lights up its own pass/fail; debug launches the
    // inspector filtered to the one case. An unknown name falls back to the whole
    // project, so a stale lens never runs nothing.
    vscode.commands.registerCommand("bynk.runTestCase", async (name: string) => {
      const item = findCase(ctrl, name);
      if (!item) {
        void vscode.commands.executeCommand("testing.runAll");
        return;
      }
      const request = new vscode.TestRunRequest([item]);
      const cts = new vscode.CancellationTokenSource();
      try {
        await runHandler(ctrl, problems, request, cts.token);
      } finally {
        cts.dispose();
      }
    }),
    vscode.commands.registerCommand("bynk.debugTestCase", (name: string) => {
      void debugBynkTests(name);
    }),
    // Slice A: run every test declared in a file. The cases come from the tree's
    // discovered locations, so it needs no new `bynk test` flag; the run plan then
    // decides between a process per case and one filtered whole-project run.
    vscode.commands.registerCommand("bynk.runTestsInFile", async (arg?: unknown) => {
      const uri =
        arg instanceof vscode.Uri ? arg : vscode.window.activeTextEditor?.document.uri;
      if (!uri) {
        void vscode.window.showInformationMessage("Bynk: open a .bynk file to run its tests.");
        return;
      }
      const refs = casesInFile(planCases(ctrl), fileKey(uri));
      const items = refs
        .map((r) => ctrl.items.get(r.suiteId)?.children.get(caseKey(r.suiteId, r.name ?? "")))
        .filter((i): i is vscode.TestItem => i !== undefined);
      if (items.length === 0) {
        void vscode.window.showInformationMessage(
          `Bynk: no tests are declared in ${uri.path.split("/").pop() ?? "this file"}.`,
        );
        return;
      }
      const cts = new vscode.CancellationTokenSource();
      try {
        await runHandler(ctrl, problems, new vscode.TestRunRequest(items), cts.token);
      } finally {
        cts.dispose();
      }
    }),
  );
  // Discover now if a `.bynk` file is already the active document on activation.
  if (isBynk(vscode.window.activeTextEditor?.document)) schedule();

  const testCases = (uri: vscode.Uri): { range: vscode.Range; name: string }[] => {
    const cases: { range: vscode.Range; name: string }[] = [];
    const target = uri.toString();
    ctrl.items.forEach((suite) => {
      suite.children.forEach((c) => {
        if (c.uri?.toString() === target && c.range) {
          cases.push({ range: c.range, name: c.label });
        }
      });
    });
    return cases;
  };

  return { testCases, onDidChangeTests: changed.event };
}

async function runHandler(
  ctrl: vscode.TestController,
  problems: vscode.DiagnosticCollection,
  request: vscode.TestRunRequest,
  token: vscode.CancellationToken,
  coverage = false,
): Promise<void> {
  const run = ctrl.createTestRun(request);
  problems.clear();

  const root = await findProjectRoot();
  if (!root) {
    run.appendOutput("Bynk: no bynk.toml found in the workspace.\r\n");
    run.end();
    return;
  }

  // Slice A: the selection decides the processes and what the run may report.
  // `--case` takes one name, so a handful of cases run one process each and
  // anything larger runs the whole project once; either way only the requested
  // items are reported, and excluded ones are not.
  const plan = planRun(planCases(ctrl), requestRef(request), { coverage });
  if (plan.steps.length === 0) {
    run.appendOutput("Bynk: nothing is selected to run.\r\n");
    run.end();
    return;
  }
  const items = new Map<string, vscode.TestItem>();
  ctrl.items.forEach((suite) => suite.children.forEach((c) => items.set(c.id, c)));
  for (const key of plan.wanted) {
    const item = items.get(key);
    if (item) run.enqueued(item);
  }

  const reported = new Set<string>();
  const noted = new Set<string>();
  let crashShown = false;
  let coverageDoc: TestRun | undefined;

  for (const step of plan.steps) {
    if (token.isCancellationRequested) break;

    let doc: TestRun;
    try {
      doc = await runBynkcTest(root, token, { caseName: step.caseName, coverage });
    } catch (e) {
      // A cancelled run kills the process, which leaves no parseable document:
      // that is the user stopping the run, not a failure to run.
      if (token.isCancellationRequested) break;
      run.appendOutput(`Bynk: could not run tests — ${String(e)}\r\n`);
      run.end();
      // #2095: `bynk` missing is the one gap the Doctor can't diagnose; any
      // other failure to run is most likely a missing test tool.
      if (e instanceof BynkNotFoundError) void reportBynkNotFound();
      else void offerDoctor("Bynk: could not run the tests.", "test");
      return;
    }

    // A compile failure has no test outcomes: route the diagnostics to the
    // Problems panel (the `bynkc` shape) and stop. These are not test results.
    if (doc.error?.kind === "compile") {
      routeCompileDiagnostics(problems, root, doc.error.diagnostics ?? []);
      run.appendOutput(
        "Bynk: the project did not compile — see the Problems panel.\r\n",
      );
      run.end();
      return;
    }

    for (const key of applyDocument(run, items, plan, doc, root, (suite, c) =>
      upsertCase(ctrl, upsertSuite(ctrl, suite), c, root),
    )) {
      reported.add(key);
    }

    for (const note of runNotes(doc)) {
      if (!noted.has(note)) {
        noted.add(note);
        run.appendOutput(`Bynk: ${note}.\r\n`);
      }
    }

    // A runtime crash: the prefix above already reported; surface the crash as a
    // run-level note (not a `bynkc` diagnostic — it isn't one).
    const unavailable = coverage ? coverageUnavailable(doc.error?.message) : undefined;
    if (unavailable) {
      // #2104: the environment can't measure coverage (only `tsx`): relay the
      // driver's own reason, rather than failing silently.
      run.appendOutput(`Bynk: coverage is unavailable — ${unavailable}\r\n`);
      if (!crashShown) {
        crashShown = true;
        void offerDoctor(`Bynk: coverage is unavailable. ${unavailable}`, "test");
      }
    } else if (doc.error?.kind === "runtime") {
      run.appendOutput(
        `Bynk: the test runner crashed — ${doc.error.message ?? "unknown error"}\r\n`,
      );
      if (doc.error.stderr) {
        run.appendOutput(doc.error.stderr.replace(/\n/g, "\r\n") + "\r\n");
      }
      if (!crashShown) {
        crashShown = true;
        void offerDoctor("Bynk: the test runner crashed.", "test");
      }
    }
    coverageDoc = doc;
  }

  // A requested item no result reached: say so, rather than leave it queued.
  if (!token.isCancellationRequested) {
    for (const key of unreported(plan, reported)) {
      const item = items.get(key);
      if (!item) continue;
      run.errored(
        item,
        new vscode.TestMessage(
          "This test was not in the run's results. The test tree may be out of date; " +
            "refresh the Testing view.",
        ),
      );
    }
  }

  // #2104: the run's `.bynk` line coverage, resolved against the project root
  // this run used. A coverage run is one process, so it is reported once.
  if (coverage && coverageDoc?.coverage) {
    for (const f of coverageFiles(coverageDoc.coverage, root.fsPath)) {
      const fc = new vscode.FileCoverage(
        vscode.Uri.file(f.file),
        new vscode.TestCoverageCount(f.covered, f.total),
      );
      coverageDetail.set(fc, f.statements);
      run.addCoverage(fc);
    }
  }

  run.end();
}

/** The slice of a `vscode.TestRun` a run document is applied through, so a test
 *  can drive {@link applyDocument} with a recording stand-in. */
export type RunSink = Pick<
  vscode.TestRun,
  "started" | "passed" | "failed" | "skipped" | "appendOutput"
>;

/** Apply one `bynk test --format json` document to a run: report each case it
 *  holds that the plan allows, and return the keys it reported.
 *
 *  An unnarrowed run reports every case the document returns, including one the
 *  tree has not seen yet (`createUnseen` makes its item); a narrowed run reports
 *  only what was requested, so a name shared by two suites, or the rest of the
 *  project when a whole-project run stands in for a selection, is not reported. */
export function applyDocument(
  run: RunSink,
  items: Map<string, vscode.TestItem>,
  plan: Pick<RunPlan, "wanted" | "narrowed">,
  doc: TestRun,
  root: vscode.Uri,
  createUnseen?: (suite: JsonSuite, c: JsonCase) => vscode.TestItem,
): Set<string> {
  const reported = new Set<string>();
  const wanted = new Set(plan.wanted);
  for (const suite of doc.suites ?? []) {
    const suiteId = suiteIdOf(suite);
    for (const c of suite.cases) {
      const key = caseKey(suiteId, c.name);
      if (plan.narrowed && !wanted.has(key)) continue;
      let item = items.get(key);
      if (!item) {
        if (plan.narrowed || !createUnseen) continue;
        item = createUnseen(suite, c);
        items.set(key, item);
      }
      reported.add(key);
      reportCase(run, item, root, c);
    }
  }
  return reported;
}

/** Report one case's outcome. A skipped case is skipped, not failed; an outcome
 *  this extension does not know is reported as a failure that names it. */
function reportCase(
  run: RunSink,
  item: vscode.TestItem,
  root: vscode.Uri,
  c: JsonCase,
): void {
  run.started(item);
  switch (outcomeOf(c.outcome)) {
    case "passed":
      run.passed(item);
      break;
    case "skipped":
      run.skipped(item);
      run.appendOutput(`${skippedLine(c)}\r\n`);
      break;
    case "failed":
      run.failed(item, failureMessage(root, c));
      break;
    default:
      run.failed(
        item,
        new vscode.TestMessage(
          `Unexpected outcome "${c.outcome}" from bynk test` + (c.message ? `: ${c.message}` : "."),
        ),
      );
  }
  const retry = retriesNote(c);
  if (retry) run.appendOutput(`${retry}\r\n`);
}

/** Per-line statements for each `FileCoverage` a run added, for the coverage
 *  profile's `loadDetailedCoverage` (the editor's gutter marks). */
const coverageDetail = new WeakMap<vscode.FileCoverage, { line: number; executed: boolean }[]>();

function detailedCoverage(fc: vscode.FileCoverage): vscode.StatementCoverage[] {
  return (coverageDetail.get(fc) ?? []).map(
    (s) => new vscode.StatementCoverage(s.executed, new vscode.Position(s.line - 1, 0)),
  );
}

/** Discover tests without running them: shell `bynk test --no-run --format
 *  json` (a pure compile that lists suites/cases — no `tsc`, no `node`, no test
 *  execution) and (re)build the tree from its document. Backs both the resolve
 *  (view-open) and refresh handlers. Stale suites/cases are pruned so a removed
 *  test disappears on refresh; a transient failure leaves the existing tree
 *  untouched. A compile failure routes to the Problems panel, mirroring a run. */
async function discover(
  ctrl: vscode.TestController,
  problems: vscode.DiagnosticCollection,
  token?: vscode.CancellationToken,
): Promise<void> {
  const root = await findProjectRoot();
  if (!root) {
    ctrl.items.replace([]);
    return;
  }

  let doc: TestRun;
  try {
    doc = await runBynkcTest(root, token, { noRun: true });
  } catch {
    return; // network/exec hiccup — keep whatever the tree already shows
  }

  if (doc.error?.kind === "compile") {
    routeCompileDiagnostics(problems, root, doc.error.diagnostics ?? []);
    return;
  }
  problems.clear();
  reconcile(ctrl, doc.suites ?? [], root);
}

/** Bring the tree in line with `suites`: upsert each suite and its cases, then
 *  delete any item no longer present in the document. */
function reconcile(
  ctrl: vscode.TestController,
  suites: JsonSuite[],
  root: vscode.Uri,
): void {
  const liveSuites = new Set<string>();
  for (const suite of suites) {
    // A suite links to its file when all its cases are declared in one. The item's
    // `uri` is fixed at creation, so a suite whose file changed is recreated.
    const file = suiteFile(
      suite.cases.map((c) => (c.location ? vscode.Uri.joinPath(root, c.location.path).toString() : undefined)),
    );
    const suiteItem = upsertSuite(ctrl, suite, { uri: file ? vscode.Uri.parse(file) : undefined });
    liveSuites.add(suiteItem.id);

    const liveCases = new Set<string>();
    for (const c of suite.cases) {
      liveCases.add(upsertCase(ctrl, suiteItem, c, root).id);
    }
    prune(suiteItem.children, liveCases);
  }
  prune(ctrl.items, liveSuites);
}

/** Delete every item in `collection` whose id is not in `keep`. Collected first,
 *  then deleted, to avoid mutating the collection mid-iteration. */
function prune(
  collection: vscode.TestItemCollection,
  keep: Set<string>,
): void {
  const stale: string[] = [];
  collection.forEach((item) => {
    if (!keep.has(item.id)) stale.push(item.id);
  });
  for (const id of stale) collection.delete(id);
}

/** A `path:line:col` document location as a 0-indexed VS Code `Position` (the
 *  document's line/col are 1-indexed). */
function sourcePosition(loc: JsonLocation): vscode.Position {
  return new vscode.Position(Math.max(0, loc.line - 1), Math.max(0, loc.col - 1));
}

/** Build the `TestMessage` for a failed case, with a `Location` for
 *  click-through when the case carries a `path:line:col`. */
function failureMessage(root: vscode.Uri, c: JsonCase): vscode.TestMessage {
  const msg = new vscode.TestMessage(c.message ?? "test failed");
  if (c.location) {
    const uri = vscode.Uri.joinPath(root, c.location.path);
    msg.location = new vscode.Location(uri, sourcePosition(c.location));
  }
  return msg;
}

const PREFIX = "bynk-test:";

/** A suite item's id: its kind and name, so a unit and an integration suite of
 *  the same name stay distinct. */
function suiteIdOf(suite: JsonSuite): string {
  return `${PREFIX}${suite.kind}:${suite.name}`;
}

/** Find or create a suite item. With `place`, the item's `uri` is brought in line
 *  with `place.uri` (discovery knows the cases' files); without it, an existing
 *  item is left as it is (a run document's passing cases carry no location). */
function upsertSuite(
  ctrl: vscode.TestController,
  suite: JsonSuite,
  place?: { uri: vscode.Uri | undefined },
): vscode.TestItem {
  const id = suiteIdOf(suite);
  let item = ctrl.items.get(id);
  if (item && place && item.uri?.toString() !== place.uri?.toString()) {
    ctrl.items.delete(id);
    item = undefined;
  }
  if (!item) {
    const label = suite.kind === "integration" ? `${suite.name} (integration)` : suite.name;
    item = ctrl.createTestItem(id, label, place?.uri);
    ctrl.items.add(item);
  }
  return item;
}

function upsertCase(
  ctrl: vscode.TestController,
  suiteItem: vscode.TestItem,
  c: JsonCase,
  root: vscode.Uri,
): vscode.TestItem {
  const id = `${suiteItem.id}::${c.name}`;
  let item = suiteItem.children.get(id);
  if (!item) {
    // A discovered case carries its declaration `location`, so the tree links to
    // the `.bynk` source before any run. The uri is fixed at creation; the range
    // is refreshed each pass (a passing run case carries no location, so we never
    // clobber a discovered one with `undefined`).
    const uri = c.location
      ? vscode.Uri.joinPath(root, c.location.path)
      : undefined;
    item = ctrl.createTestItem(id, c.name, uri);
    suiteItem.children.add(item);
  }
  if (c.location) {
    const pos = sourcePosition(c.location);
    item.range = new vscode.Range(pos, pos);
  }
  return item;
}

/** Route `path:line:col: severity[category]: message` lines (the same shape the
 *  `$bynkc` problem-matcher parses) into the Problems panel. */
function routeCompileDiagnostics(
  problems: vscode.DiagnosticCollection,
  root: vscode.Uri,
  lines: string[],
): void {
  const re = /^(.+?):(\d+):(\d+): (error|warning)\[([^\]]+)\]: (.+)$/;
  const byFile = new Map<string, vscode.Diagnostic[]>();
  for (const line of lines) {
    const m = re.exec(line);
    if (!m) continue;
    const [, file, lineStr, colStr, sev, code, message] = m;
    const uri = vscode.Uri.joinPath(root, file);
    const pos = new vscode.Position(
      Math.max(0, Number(lineStr) - 1),
      Math.max(0, Number(colStr) - 1),
    );
    const diag = new vscode.Diagnostic(
      new vscode.Range(pos, pos),
      message,
      sev === "error"
        ? vscode.DiagnosticSeverity.Error
        : vscode.DiagnosticSeverity.Warning,
    );
    diag.code = code;
    diag.source = "bynkc";
    const key = uri.toString();
    const list = byFile.get(key);
    if (list) list.push(diag);
    else byFile.set(key, [diag]);
  }
  for (const [key, diags] of byFile) {
    problems.set(vscode.Uri.parse(key), diags);
  }
}

/** Run `bynk test . --format json` at `root` and parse its document. With
 *  `{ noRun: true }` it adds `--no-run` — a pure discovery compile that lists
 *  suites/cases without running them. A non-zero exit is normal (test failures),
 *  so we parse stdout regardless and only reject when there is no parseable
 *  document at all. Goes through `bynk` rather than shelling `bynkc` directly
 *  (#486): the driver resolves its compiler as `BYNK_BYNKC` → PATH →
 *  sibling-of-`bynk`, so a driver-first install works here too; `bynk.compilerPath`
 *  is forwarded as `BYNK_BYNKC` to keep pinning an exact `bynkc` when set. */
function runBynkcTest(
  root: vscode.Uri,
  token?: vscode.CancellationToken,
  opts?: { noRun?: boolean; caseName?: string; coverage?: boolean },
): Promise<TestRun> {
  // #2104: a coverage run names the project by its absolute path, so the
  // coverage block's paths are project-relative even from a driver that
  // collapses them under a relative `.` (fixed alongside this change).
  const input = opts?.coverage ? root.fsPath : ".";
  const args = opts?.noRun
    ? ["test", input, "--no-run", "--format", "json"]
    : ["test", input, "--format", "json"];
  // v0.127: the per-case run filter (no effect alongside `--no-run` discovery).
  if (opts?.caseName) args.push("--case", opts.caseName);
  if (opts?.coverage) args.push("--coverage");
  const override = compilerOverride();
  return new Promise((resolve, reject) => {
    const child = execFile(
      bynkPath(),
      args,
      {
        cwd: root.fsPath,
        maxBuffer: 64 * 1024 * 1024,
        env: override ? { ...process.env, BYNK_BYNKC: override } : undefined,
      },
      (err, stdout, stderr) => {
        if ((err as NodeJS.ErrnoException | null)?.code === "ENOENT") {
          reject(new BynkNotFoundError(bynkPath(), err));
          return;
        }
        const text = stdout.trim();
        if (!text) {
          reject(new Error(stderr.trim() || "no output from `bynk test`"));
          return;
        }
        try {
          resolve(JSON.parse(text) as TestRun);
        } catch (e) {
          reject(new Error(`could not parse \`bynk test\` output: ${String(e)}`));
        }
      },
    );
    token?.onCancellationRequested(() => child.kill());
  });
}

/** The directory of the nearest `bynk.toml` — walking up from the active
 *  `.bynk` file, then falling back to the workspace-folder roots. Mirrors the
 *  rooting in extension.ts / the LSP's `find_project_root`. */
async function findProjectRoot(): Promise<vscode.Uri | undefined> {
  const exists = async (uri: vscode.Uri): Promise<boolean> => {
    try {
      await vscode.workspace.fs.stat(uri);
      return true;
    } catch {
      return false;
    }
  };

  const active = vscode.window.activeTextEditor?.document;
  if (active?.languageId === "bynk" && active.uri.scheme === "file") {
    let dir = vscode.Uri.joinPath(active.uri, "..");
    for (;;) {
      if (await exists(vscode.Uri.joinPath(dir, "bynk.toml"))) return dir;
      const parent = vscode.Uri.joinPath(dir, "..");
      if (parent.path === dir.path) break;
      dir = parent;
    }
  }
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    if (await exists(vscode.Uri.joinPath(folder.uri, "bynk.toml"))) {
      return folder.uri;
    }
  }
  return undefined;
}
