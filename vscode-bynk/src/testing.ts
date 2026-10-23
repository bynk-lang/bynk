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

import { execFile, spawn } from "node:child_process";
import * as vscode from "vscode";

import { bynkPath, compilerOverride } from "./tasks";
import { debugBynkTests } from "./debug";
import { devServerRunning, stopDevServer } from "./devServer";
import { BynkNotFoundError } from "./cli";
import { offerDoctor, reportBynkNotFound } from "./doctor";
import { coverageFiles, coverageUnavailable } from "./coverageModel";
import { ContinuousScheduler, FailureOnce, runPass } from "./continuousRun";
import type { ViewRefresh } from "./viewRefresh";
import { EventReader, RunAssembler, driverSupport, rejectsNdjson, rejectsSets } from "./testStream";
import {
  caseKey,
  casesInFile,
  coverageScope,
  debugTarget,
  outcomeOf,
  planRun,
  retriesNote,
  runNotes,
  skippedLine,
  suiteFile,
  systemOnly,
  carriesTiers,
  declarationNotes,
  tierOf,
  TIERS,
  unreported,
  type ItemRef,
  type JsonCase,
  type JsonLocation,
  type JsonSuite,
  type PlanCase,
  type RequestRef,
  type RunPlan,
  type Tier,
  type TestRun,
} from "./testPlan";
import {
  fingerprintOf,
  pinDescription,
  pinState,
  seedFor,
  validSeed,
  type Pin,
  type SeedFailure,
} from "./testSeeds";
import {
  DEV_TARGET,
  offersSkipUnsupported,
  scanTargets,
  targetChoices,
  targetDetail,
} from "./testTargets";

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

/** One tag per tier, so the Testing view's filter selects by tier. The tag id is
 *  the tier's own name. */
const tierTags = new Map<Tier, vscode.TestTag>(TIERS.map((t) => [t, new vscode.TestTag(t)]));

/** The key under which a workspace remembers its pinned seeds. */
const PINS_KEY = "bynk.test.pins";

/** What the controller remembers about seeds: pins (kept in workspace state, never
 *  in `bynk.toml` or source, so a pin never freezes a property for anyone else),
 *  the seed each case's latest failure reported, and each suite's generative
 *  cases as discovery last listed them (a pin's fingerprint). */
interface SeedState {
  pins: Map<string, Pin>;
  failures: Map<string, SeedFailure>;
  generative: Map<string, string[]>;
  store: vscode.Memento;
}

let seeds: SeedState | undefined;

function loadPins(store: vscode.Memento): Map<string, Pin> {
  const pins = new Map<string, Pin>();
  const raw = store.get<Record<string, unknown>>(PINS_KEY) ?? {};
  for (const [key, v] of Object.entries(raw)) {
    const pin = v as Partial<Pin> | undefined;
    if (pin && validSeed(pin.seed) && Array.isArray(pin.fingerprint)) {
      pins.set(key, { seed: pin.seed, fingerprint: pin.fingerprint.map(String) });
    }
  }
  return pins;
}

function savePins(state: SeedState): Thenable<void> {
  return state.store.update(PINS_KEY, Object.fromEntries(state.pins));
}

/** The generative cases of the suite a case key belongs to. */
function suiteFingerprint(state: SeedState, key: string): string[] {
  return state.generative.get(key.slice(0, key.indexOf("::"))) ?? [];
}

/** Show each pin beside its test: its seed, or that it is stale. */
function describePins(ctrl: vscode.TestController, state: SeedState): void {
  ctrl.items.forEach((suite) => {
    suite.children.forEach((c) => {
      const pin = state.pins.get(c.id);
      c.description = pin ? pinDescription(pin, pinState(pin, suiteFingerprint(state, c.id))) : undefined;
    });
  });
}

/** The case item a command was given: the tree item itself, or anything with its
 *  id (a menu hands over the item; the tests hand over an id). */
function caseItem(ctrl: vscode.TestController, arg: unknown): vscode.TestItem | undefined {
  const id = typeof arg === "string" ? arg : (arg as { id?: unknown } | undefined)?.id;
  if (typeof id !== "string") return undefined;
  let found: vscode.TestItem | undefined;
  ctrl.items.forEach((suite) => {
    const c = suite.children.get(id);
    if (c) found = c;
  });
  return found;
}

/** The tier a case item is tagged with, if any. */
function tierOfItem(item: vscode.TestItem): Tier | undefined {
  return TIERS.find((t) => item.tags.some((tag) => tag.id === t));
}

/** The tree as the run plan sees it: every discovered case with its suite, file
 *  and tier. */
function planCases(ctrl: vscode.TestController): PlanCase[] {
  const cases: PlanCase[] = [];
  ctrl.items.forEach((suite) => {
    suite.children.forEach((c) => {
      cases.push({
        suiteId: suite.id,
        name: c.label,
        file: c.uri ? fileKey(c.uri) : undefined,
        tier: tierOfItem(c),
      });
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

/** A run against a named target (`bynk test --against`). */
interface AgainstRun {
  target: string;
  /** Pass `--skip-unsupported`: offered as a retry, never the default. */
  skipUnsupported: boolean;
}

export function registerTesting(context: vscode.ExtensionContext, hub: ViewRefresh): TestApi {
  // What a driver understands is learned from its refusals (see `driverSupport`),
  // so it is forgotten when the user points the extension at a different binary.
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("bynk.bynkPath") || e.affectsConfiguration("bynk.compilerPath")) {
        driverSupport.ndjson = true;
        driverSupport.sets = true;
      }
    }),
  );
  const ctrl = vscode.tests.createTestController("bynk", "Bynk Tests");
  context.subscriptions.push(ctrl);
  const seedState: SeedState = {
    pins: loadPins(context.workspaceState),
    failures: new Map(),
    generative: new Map(),
    store: context.workspaceState,
  };
  seeds = seedState;

  // Compile failures surfaced by a run go to the Problems panel, exactly as the
  // `bynkc: check` task does — but in their own collection so a later clean run
  // clears them without disturbing the LSP's live diagnostics.
  const problems = vscode.languages.createDiagnosticCollection("bynk-tests");
  context.subscriptions.push(problems);

  // Slice E: the Run profile alone supports a continuous run (DECISION C). Debug is
  // interactive, and Coverage needs the `tsc → node` path, so on a machine with only
  // `tsx` every save would raise the "coverage is unavailable" notice.
  const profile = ctrl.createRunProfile(
    "Run",
    vscode.TestRunProfileKind.Run,
    (request, token) => (request.continuous ? watchTests(request, token) : runHandler(ctrl, problems, request, token)),
    true,
    undefined,
    true,
  );
  context.subscriptions.push(profile);

  // Slice E: the Watch toggle. Each continuous request is a set of items to keep
  // rerunning; they share one scheduler, so two watches never run at once, and one
  // failure policy, so a missing `bynk` is announced once for the session (DECISION
  // D). The session lasts while any watch is on. Turning Watch on does not itself
  // run anything: the tests run when a model source next changes.
  const watched = new Set<vscode.TestRunRequest>();
  let session: { scheduler: ContinuousScheduler; changes: vscode.Disposable } | undefined;
  const endSession = (): void => {
    session?.changes.dispose();
    session?.scheduler.dispose();
    session = undefined;
  };
  context.subscriptions.push({ dispose: endSession });

  const watchTests = (request: vscode.TestRunRequest, token: vscode.CancellationToken): void => {
    watched.add(request);
    if (!session) {
      const once = new FailureOnce();
      const scheduler = new ContinuousScheduler((slot) =>
        runPass([...watched], slot, once, async (watch) => {
          const cts = new vscode.CancellationTokenSource();
          const stop = slot.onCancel(() => cts.cancel());
          try {
            const rerun = new vscode.TestRunRequest(watch.include, watch.exclude, watch.profile, true);
            await runHandler(ctrl, problems, rerun, cts.token, false, undefined, undefined, once);
          } finally {
            stop();
            cts.dispose();
          }
        }),
      );
      session = { scheduler, changes: hub.onDidChangeSource(() => scheduler.signal()) };
    }
    token.onCancellationRequested(() => {
      watched.delete(request);
      if (watched.size === 0) endSession();
    });
  };

  // Test seam (not contributed, so never in the palette): turning Watch on in the real
  // UI goes through a quick pick, so the integration suite switches it here instead.
  let seamWatch: vscode.CancellationTokenSource | undefined;
  context.subscriptions.push(
    vscode.commands.registerCommand("bynk._watchTests", (on: boolean) => {
      seamWatch?.cancel();
      seamWatch?.dispose();
      seamWatch = undefined;
      if (!on) return;
      seamWatch = new vscode.CancellationTokenSource();
      watchTests(new vscode.TestRunRequest(undefined, undefined, profile, true), seamWatch.token);
    }),
  );

  // Slice B: run the `system` suites against a named target (`bynk test
  // --against`). One profile with a quick pick rather than one per target, so the
  // profile list does not track the manifest. Run-only: the driver refuses
  // `--against` with `--inspect` or `--coverage`, so there is no Debug or Coverage
  // variant of it. The extension passes only a name the manifest declares (or
  // `dev`, or one typed by hand, which the driver validates); it never builds an
  // endpoint, sets `BYNK_TEST_TARGETS` or reads a secret.
  const runAgainst = async (
    request: vscode.TestRunRequest,
    token: vscode.CancellationToken,
  ): Promise<void> => {
    const target = await pickTarget(context);
    if (!target) return;
    if (target === DEV_TARGET && devServerRunning()) {
      // Like debug (#2094 DECISION C): a running `bynk dev` holds the ports the
      // test run's own dev server needs. Ask rather than fail on a clash.
      const stop = "Stop and Run";
      const choice = await vscode.window.showWarningMessage(
        "A Bynk dev server is running. Stop the running dev server and run the tests against dev?",
        { modal: true },
        stop,
      );
      if (choice !== stop) return;
      await stopDevServer();
    }
    await runHandler(ctrl, problems, request, token, false, { target, skipUnsupported: false });
  };
  context.subscriptions.push(
    ctrl.createRunProfile("Run against…", vscode.TestRunProfileKind.Run, runAgainst, false),
  );

  // v0.72: the headline debug entry (ADR 0104, DECISION C). VS Code renders a
  // **Debug** action beside Run from this profile; it shells `bynk test
  // --inspect` and hands off to the JS debugger so a breakpoint in a test body
  // (or the code it exercises) pauses. The whole suite runs under the inspector;
  // the breakpoint decides where it stops.
  const debugProfile = ctrl.createRunProfile(
    "Debug",
    vscode.TestRunProfileKind.Debug,
    (request) => {
      // The selection decides the target: one case debugs just that case, and a
      // suite or several cases debug as one set (a driver that predates the set
      // filter cannot narrow them, so the whole project runs and the user is told).
      const target = debugTarget(planCases(ctrl), requestRef(request), { sets: driverSupport.sets });
      if (target.nothing) {
        void vscode.window.showInformationMessage(target.notice ?? "Bynk: nothing is selected to debug.");
        return;
      }
      if (target.notice) void vscode.window.showInformationMessage(target.notice);
      void debugBynkTests({ caseName: target.caseName, suites: target.suites, cases: target.cases });
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
    // Slice B: the palette entry for the same thing, over every system test.
    vscode.commands.registerCommand("bynk.runTestsAgainst", async () => {
      const cts = new vscode.CancellationTokenSource();
      try {
        await runAgainst(new vscode.TestRunRequest(), cts.token);
      } finally {
        cts.dispose();
      }
    }),
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
    // Slice C: reproduce, pin and unpin a generative test's seed. The menu hands
    // over the test item; each needs a seed that a failed run reported.
    vscode.commands.registerCommand("bynk.rerunWithSeed", async (arg?: unknown) => {
      const item = caseItem(ctrl, arg);
      const failure = item && seedState.failures.get(item.id);
      if (!item || !failure) {
        void vscode.window.showInformationMessage(
          "Bynk: this test has no failed run with a seed to reproduce. Run it, and let it fail, first.",
        );
        return;
      }
      const cts = new vscode.CancellationTokenSource();
      try {
        await runHandler(ctrl, problems, new vscode.TestRunRequest([item]), cts.token, false, undefined, failure.seed);
      } finally {
        cts.dispose();
      }
    }),
    vscode.commands.registerCommand("bynk.pinSeed", async (arg?: unknown) => {
      const item = caseItem(ctrl, arg);
      const failure = item && seedState.failures.get(item.id);
      if (!item || !failure) {
        void vscode.window.showInformationMessage(
          "Bynk: this test has no failed run with a seed to pin. Run it, and let it fail, first.",
        );
        return;
      }
      seedState.pins.set(item.id, { seed: failure.seed, fingerprint: suiteFingerprint(seedState, item.id) });
      await savePins(seedState);
      describePins(ctrl, seedState);
    }),
    vscode.commands.registerCommand("bynk.unpinSeed", async (arg?: unknown) => {
      const item = caseItem(ctrl, arg);
      if (!item || !seedState.pins.delete(item.id)) return;
      await savePins(seedState);
      describePins(ctrl, seedState);
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
  against?: AgainstRun,
  seedOverride?: string,
  once?: FailureOnce,
): Promise<void> {
  // Slice E (DECISION F): a coverage run of one case is named for it, so the Coverage
  // view says which case the report measures and it is never taken for the project's.
  const scope = coverage ? coverageScope(planRun(planCases(ctrl), requestRef(request), { coverage: true })) : undefined;
  const run = ctrl.createTestRun(request, scope?.kind === "case" ? `Coverage: ${scope.name}` : undefined);
  problems.clear();
  // DECISION D: in a continuous session a notification is shown once, and every
  // failure is still written to the run's output.
  const notify = (kind: string, show: () => void): void => {
    if (!once || once.first(kind)) show();
  };

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
  let cases = planCases(ctrl);
  let requested = requestRef(request);
  if (against) {
    // Only the `system` suites run against a target; anything else selected is
    // left as it was.
    const system = systemOnly(cases, requested);
    if (system.empty) {
      run.appendOutput(`Bynk: none of the selected tests is system-tier, so nothing runs against ${against.target}.\r\n`);
      run.end();
      return;
    }
    cases = system.cases;
    requested = system.request;
    run.appendOutput(`Bynk: running the system tests against ${against.target}.\r\n`);
    if (against.target !== DEV_TARGET) run.appendOutput(`Bynk: ${targetDetail(against.target)}\r\n`);
  }
  const planned = planRun(cases, requested, { coverage, remote: against !== undefined, sets: driverSupport.sets });
  // The tree holds every tier, so a run against a target reports only what it
  // asked for, not whatever else the document happens to carry.
  const plan: RunPlan = against ? { ...planned, narrowed: true } : planned;
  if (plan.steps.length === 0) {
    run.appendOutput("Bynk: nothing is selected to run.\r\n");
    run.end();
    return;
  }
  // DECISION E: a coverage report is one case's or the whole project's, never a merge
  // of processes; a selection that is neither runs the project, and says so. Said from
  // the plan that runs, which the tree may have changed under since the run was named.
  const measured = coverage ? coverageScope(plan) : undefined;
  if (measured?.kind === "case") {
    run.appendOutput(`Bynk: coverage for the case "${measured.name}" alone — not the project's coverage report.\r\n`);
  } else if (measured?.narrowed) {
    // One item selected and yet the project ran: the plan only does that for a name
    // another suite shares, which `--case` would run in both.
    const why = plan.wanted.length === 1 ? " (another suite has a case with this name, and `--case` would run both)" : "";
    run.appendOutput(
      `Bynk: coverage measures one case or the whole project, so this ran the whole project${why}; the report is the project's, not the selection's.\r\n`,
    );
  }
  const items = new Map<string, vscode.TestItem>();
  ctrl.items.forEach((suite) => suite.children.forEach((c) => items.set(c.id, c)));
  for (const key of plan.wanted) {
    const item = items.get(key);
    if (item) run.enqueued(item);
  }

  // Slice C: a pinned seed (or the one "Re-run with this seed" supplies) goes to a
  // run of exactly one case, since the seed is the run's root and one value per
  // process; a whole-project or suite run always draws a fresh one.
  const only = plan.steps.length === 1 && plan.steps[0].caseName !== undefined && plan.wanted.length === 1 && !against;
  let seed: string | undefined;
  if (only && seeds) {
    const key = plan.wanted[0];
    const chosen = seedOverride
      ? { seed: seedOverride, note: undefined }
      : seedFor(key, seeds.pins, (k) => suiteFingerprint(seeds as SeedState, k), key.slice(key.indexOf("::") + 2));
    seed = chosen.seed;
    if (chosen.note) run.appendOutput(`Bynk: ${chosen.note}.\r\n`);
    if (seed) run.appendOutput(`Bynk: running with seed ${seed}.\r\n`);
  }

  const reported = new Set<string>();
  const noted = new Set<string>();
  let crashShown = false;
  let coverageDoc: TestRun | undefined;

  for (const step of plan.steps) {
    if (token.isCancellationRequested) break;

    // Slice D: apply the results to the tree one case at a time, as the driver
    // reports each, rather than all at once when it exits. Coverage is a whole-run
    // report and stays the one document, as does a driver that has no stream.
    const applyResults = (results: TestRun, whole: TestRun = results) => {
      // A run that was told to stop (a newer change superseded it, DECISION B) shows
      // nothing more: its results describe code that has since changed.
      if (token.isCancellationRequested) return;
      for (const key of applyDocument(run, items, plan, results, root, (suite, c) =>
        upsertCase(ctrl, upsertSuite(ctrl, suite), c, root, carriesTiers(whole.suites ?? []) ? undefined : suite.kind),
      )) {
        reported.add(key);
      }
    };
    const stepOptions = {
      caseName: step.caseName,
      suites: step.suites,
      cases: step.cases,
      coverage,
      against: against?.target,
      skipUnsupported: against?.skipUnsupported,
      seed,
    };
    let doc: TestRun;
    let streamed = false;
    try {
      if (!coverage && driverSupport.ndjson) {
        ({ doc, streamed } = await runBynkcTestStream(root, token, stepOptions, (shown, soFar) =>
          applyResults({ passed: 0, failed: 0, suites: [shown] }, soFar),
        ));
      } else {
        doc = await runBynkcTest(root, token, stepOptions);
      }
    } catch (e) {
      // A cancelled run kills the process, which leaves no parseable document:
      // that is the user stopping the run, not a failure to run.
      if (token.isCancellationRequested) break;
      // A driver that predates the set filter (#2273) rejects a repeated `--case` or
      // a `--suite` before it runs anything. Remember that and plan the selection as
      // it was before: one process per case, or the whole project.
      if (driverSupport.sets && (step.suites || step.cases) && rejectsSets(String(e))) {
        driverSupport.sets = false;
        run.appendOutput("Bynk: this `bynk` cannot run several tests in one process; running them one at a time.\r\n");
        run.end();
        return runHandler(ctrl, problems, request, token, coverage, against, seedOverride, once);
      }
      run.appendOutput(`Bynk: could not run tests — ${String(e)}\r\n`);
      run.end();
      // #2095: `bynk` missing is the one gap the Doctor can't diagnose; any
      // other failure to run is most likely a missing test tool.
      if (e instanceof BynkNotFoundError) notify("bynk-not-found", () => void reportBynkNotFound());
      else notify("could-not-run", () => void offerDoctor("Bynk: could not run the tests.", "test"));
      return;
    }
    // The process can finish just as a newer change cancels the run; its document is
    // stale all the same.
    if (token.isCancellationRequested) break;

    // A compile failure has no test outcomes: route the diagnostics to the
    // Problems panel (the `bynkc` shape) and stop. These are not test results.
    if (doc.error?.kind === "compile") {
      routeCompileDiagnostics(problems, root, doc.error.diagnostics ?? []);
      run.appendOutput(
        "Bynk: the project did not compile — see the Problems panel.\r\n",
      );
      run.end();
      if (against && !against.skipUnsupported && offersSkipUnsupported(doc.error.diagnostics ?? [])) {
        offerSkipUnsupported(ctrl, problems, request, against);
      }
      return;
    }

    if (!streamed) applyResults(doc);
    rememberSeeds(doc, reported);

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
    } else if (against && doc.error?.kind === "runtime" && !doc.suites?.length) {
      // The driver declined the run (an unknown or production target, a missing
      // secret, an unreachable target): its own message says why, and nothing
      // crashed, so no Doctor offer.
      run.appendOutput(`Bynk: the run against ${against.target} did not happen — ${doc.error.message ?? "unknown error"}\r\n`);
    } else if (doc.error?.kind === "runtime") {
      run.appendOutput(
        `Bynk: the test runner crashed — ${doc.error.message ?? "unknown error"}\r\n`,
      );
      if (doc.error.stderr) {
        run.appendOutput(doc.error.stderr.replace(/\n/g, "\r\n") + "\r\n");
      }
      if (!crashShown) {
        crashShown = true;
        notify("crash", () => void offerDoctor("Bynk: the test runner crashed.", "test"));
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
      run.passed(item, c.durationMs);
      break;
    case "skipped":
      run.skipped(item);
      run.appendOutput(`${skippedLine(c)}\r\n`);
      break;
    case "failed":
      run.failed(item, failureMessages(root, c), c.durationMs);
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
  const legacy = !carriesTiers(suites);
  for (const suite of suites) {
    // A suite links to its file when all its cases are declared in one. The item's
    // `uri` is fixed at creation, so a suite whose file changed is recreated.
    const file = suiteFile(
      suite.cases.map((c) => (c.location ? vscode.Uri.joinPath(root, c.location.path).toString() : undefined)),
    );
    const suiteItem = upsertSuite(ctrl, suite, { uri: file ? vscode.Uri.parse(file) : undefined });
    liveSuites.add(suiteItem.id);
    seeds?.generative.set(suiteItem.id, fingerprintOf(suite.cases));

    const liveCases = new Set<string>();
    for (const c of suite.cases) {
      liveCases.add(upsertCase(ctrl, suiteItem, c, root, legacy ? suite.kind : undefined).id);
    }
    prune(suiteItem.children, liveCases);
  }
  prune(ctrl.items, liveSuites);
  if (seeds) {
    // A pin for a case that is gone has nothing to attach to; the others show
    // whether they still hold.
    let changed = false;
    for (const key of [...seeds.pins.keys()]) {
      const suiteId = key.slice(0, key.indexOf("::"));
      if (liveSuites.has(suiteId) && !ctrl.items.get(suiteId)?.children.get(key)) {
        seeds.pins.delete(key);
        changed = true;
      }
    }
    if (changed) void savePins(seeds);
    describePins(ctrl, seeds);
  }
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
function failureMessages(root: vscode.Uri, c: JsonCase): vscode.TestMessage[] {
  const at = (msg: vscode.TestMessage, where: JsonLocation | undefined = c.location): vscode.TestMessage => {
    if (where) {
      const uri = vscode.Uri.joinPath(root, where.path);
      msg.location = new vscode.Location(uri, sourcePosition(where));
    }
    return msg;
  };
  const messages = [at(new vscode.TestMessage(c.message ?? "test failed"))];
  // #2272: the declaration that refused the case, or a step of it. Each line opens
  // the declaration; one the driver could not place opens the case's own position.
  for (const note of declarationNotes(c)) {
    messages.push(at(new vscode.TestMessage(note.text), note.location ?? c.location));
  }
  // Slice C: a failed generative case also reports how to reproduce it as data,
  // in addition to the unchanged text above.
  if (c.counterexample) messages.push(at(new vscode.TestMessage(`Counterexample: ${c.counterexample}`)));
  if (c.seed) {
    messages.push(
      at(new vscode.TestMessage(`Seed: ${c.seed}. Use "Re-run with this seed" on the test to reproduce this failure.`)),
    );
  }
  return messages;
}

/** Remember the seed each reported case's latest run gave: a failed generative
 *  case's, which "Re-run with this seed" and "Pin this seed" use, replacing an
 *  earlier one; a pass or a case without one clears it. */
function rememberSeeds(doc: TestRun, reported: ReadonlySet<string>): void {
  if (!seeds) return;
  for (const suite of doc.suites ?? []) {
    for (const c of suite.cases) {
      const key = caseKey(suiteIdOf(suite), c.name);
      if (!reported.has(key)) continue;
      if (validSeed(c.seed)) seeds.failures.set(key, { seed: c.seed, counterexample: c.counterexample });
      else seeds.failures.delete(key);
    }
  }
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
  legacyKind?: string,
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
  // The tier tag: the driver's per-case tier (the suite's kind only for a driver
  // that predates it). Left as it was when none is known, so a document without
  // one never clears a tag.
  const tier = tierOf(c, legacyKind);
  if (tier) item.tags = [tierTags.get(tier) as vscode.TestTag];
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
interface TestRunOptions {
  noRun?: boolean;
  caseName?: string;
  /** A set (#2273): repeated `--suite` and `--case`, combined as "and". */
  suites?: string[];
  cases?: string[];
  coverage?: boolean;
  against?: string;
  skipUnsupported?: boolean;
  seed?: string;
}

/** The `bynk test` arguments for a run, in `--format json` (the one document) or
 *  `--format ndjson` (the stream). */
function testArgs(root: vscode.Uri, opts: TestRunOptions | undefined, format: "json" | "ndjson"): string[] {
  // #2104: a coverage run names the project by its absolute path, so the
  // coverage block's paths are project-relative even from a driver that
  // collapses them under a relative `.` (fixed alongside this change).
  const input = opts?.coverage ? root.fsPath : ".";
  const args = opts?.noRun
    ? ["test", input, "--no-run", "--format", format]
    : ["test", input, "--format", format];
  // v0.127: the per-case run filter (no effect alongside `--no-run` discovery).
  if (opts?.caseName) args.push("--case", opts.caseName);
  for (const suite of opts?.suites ?? []) args.push("--suite", suite);
  for (const name of opts?.cases ?? []) args.push("--case", name);
  if (opts?.coverage) args.push("--coverage");
  // Slice C: reproduce a generative case's stream. Only a seed the runner reported
  // is passed.
  if (opts?.seed && validSeed(opts.seed)) args.push("--seed", opts.seed);
  // Slice B: run the `system` suites against a named target. The name is passed
  // through untouched; the driver decides whether it is safe.
  if (opts?.against) args.push("--against", opts.against);
  if (opts?.against && opts.skipUnsupported) args.push("--skip-unsupported");
  return args;
}

function runBynkcTest(
  root: vscode.Uri,
  token?: vscode.CancellationToken,
  opts?: TestRunOptions,
): Promise<TestRun> {
  const args = testArgs(root, opts, "json");
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

/** Run `bynk test . --format ndjson` and apply each case through `onSuite` (as its
 *  suite holding that one case) as the driver reports it, so the Testing view fills in
 *  while the run is going. Resolves
 *  with the whole run's document (also assembled from the events) and whether the
 *  suites were streamed: a driver that rejects `ndjson` is run again as `json`, and
 *  its suites are for the caller to apply. A process that wrote no events is
 *  rejected, as `runBynkcTest` rejects one with no document. */
export function runBynkcTestStream(
  root: vscode.Uri,
  token: vscode.CancellationToken,
  opts: TestRunOptions,
  onSuite: (suite: JsonSuite, soFar: TestRun) => void,
): Promise<{ doc: TestRun; streamed: boolean }> {
  const override = compilerOverride();
  return new Promise((resolve, reject) => {
    const child = spawn(bynkPath(), testArgs(root, opts, "ndjson"), {
      cwd: root.fsPath,
      env: override ? { ...process.env, BYNK_BYNKC: override } : undefined,
    });
    const reader = new EventReader();
    const assembler = new RunAssembler();
    let stderr = "";
    const handle = (events: ReturnType<EventReader["push"]>) => {
      for (const event of events) {
        const done = assembler.apply(event);
        if (done) onSuite(done, assembler.doc);
      }
    };
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => handle(reader.push(chunk)));
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", (err) => {
      reject((err as NodeJS.ErrnoException).code === "ENOENT" ? new BynkNotFoundError(bynkPath(), err) : err);
    });
    child.on("close", () => {
      handle(reader.end());
      // A stopped run is the user's doing, not a runner that died: reject, as the
      // one-document run does when its process is killed, so the caller sees the
      // cancellation instead of a crash report assembled from a cut-off stream.
      if (token.isCancellationRequested) {
        reject(new Error("the run was cancelled"));
        return;
      }
      if (!assembler.hasEvents) {
        if (rejectsNdjson(stderr)) {
          driverSupport.ndjson = false;
          runBynkcTest(root, token, opts).then((doc) => resolve({ doc, streamed: false }), reject);
        } else {
          reject(new Error(stderr.trim() || "no output from `bynk test`"));
        }
        return;
      }
      resolve({ doc: assembler.finish(stderr), streamed: true });
    });
    const cancel = token.onCancellationRequested(() => child.kill());
    child.on("close", () => cancel.dispose());
  });
}

/** The key under which a workspace remembers the last target it ran against. */
const LAST_TARGET_KEY = "bynk.test.lastTarget";

/** The target names the manifest declares. A missing or unreadable manifest has
 *  none, which leaves `dev` and a typed name. */
async function manifestTargets(root: vscode.Uri): Promise<string[]> {
  try {
    const bytes = await vscode.workspace.fs.readFile(vscode.Uri.joinPath(root, "bynk.toml"));
    return scanTargets(Buffer.from(bytes).toString("utf8"));
  } catch {
    return [];
  }
}

/** Ask which target to run against: `dev`, the manifest's targets (the last
 *  choice first), or a name typed by hand. Remembers the choice per workspace.
 *  A bare URL is refused here with a message, since the driver would refuse it
 *  and a target name is all the extension ever passes. */
async function pickTarget(context: vscode.ExtensionContext): Promise<string | undefined> {
  const root = await findProjectRoot();
  if (!root) {
    void vscode.window.showErrorMessage("Bynk: no bynk.toml found in the workspace.");
    return undefined;
  }
  const last = context.workspaceState.get<string>(LAST_TARGET_KEY);
  const MANUAL = "Enter a target name…";
  const choices = targetChoices(await manifestTargets(root), last);
  const picked = await vscode.window.showQuickPick(
    [
      ...choices.map((name) => ({
        label: name,
        description: name === last ? "last used" : undefined,
        detail: targetDetail(name),
      })),
      { label: MANUAL, detail: "A target declared in a form the list does not show." },
    ],
    { title: "Run the system tests against…", placeHolder: "Choose a target" },
  );
  if (!picked) return undefined;
  let target = picked.label;
  if (target === MANUAL) {
    const typed = await vscode.window.showInputBox({
      title: "Run the system tests against…",
      prompt: "A target name from [test.targets] in bynk.toml, or dev",
      validateInput: (v) =>
        /:\/\//.test(v)
          ? "Enter a target name, not a URL: the driver runs only against a named target."
          : /^[A-Za-z0-9_-]+$/.test(v.trim())
            ? undefined
            : "Enter a target name: letters, digits, `-` and `_`.",
    });
    if (!typed) return undefined;
    target = typed.trim();
  }
  await context.workspaceState.update(LAST_TARGET_KEY, target);
  return target;
}

/** After the driver refused a run for a case a real runtime cannot honour, offer
 *  to run again skipping those cases. Never the default: the refusal is shown
 *  first, and skipping is the user's call. */
function offerSkipUnsupported(
  ctrl: vscode.TestController,
  problems: vscode.DiagnosticCollection,
  request: vscode.TestRunRequest,
  against: AgainstRun,
): void {
  const retry = "Retry, skipping unsupported cases";
  void vscode.window
    .showWarningMessage(
      `Bynk: a system test cannot run against ${against.target} (see the Problems panel).`,
      retry,
    )
    .then(async (choice) => {
      if (choice !== retry) return;
      const cts = new vscode.CancellationTokenSource();
      try {
        await runHandler(ctrl, problems, request, cts.token, false, { ...against, skipUnsupported: true });
      } finally {
        cts.dispose();
      }
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
