// Slice 4 integration test: the *production* path. Drives the extension's own
// `bynk` DebugConfigurationProvider end to end — `startDebugging({ type: "bynk",
// mode: "test" })` shells `bynk test --inspect` (#486: through the driver, not
// `bynkc` directly), parses the inspector port, attaches VS Code's JS debugger,
// and a breakpoint set in a `.bynk` test pauses.
//
// Unlike debug.test.ts (which hand-authors a `.ts`+map to isolate the attach
// mechanics), this exercises the real compiler output and the real provider, so
// it also covers the v0.72 emitter fix: the map `sources` are the `.bynk` files'
// absolute paths, so an editor breakpoint resolves to the loaded source.
//
// Guarded on `bynk` + `bynkc` + `node` (DECISION D) — the CI harness provisions
// them on PATH; elsewhere the test skips. The project is built *inside* the
// workspace folder, as a real user's is (js-debug anchors source resolution to it).

import * as assert from "assert";
import * as path from "path";
import * as fs from "fs";
import { spawnSync } from "child_process";
import * as vscode from "vscode";

import { assertFixtureChecks } from "./fixtureCheck";

function have(tool: string): boolean {
  try {
    return spawnSync(tool, ["--version"], { stdio: "ignore" }).status === 0;
  } catch {
    return false;
  }
}

/** True when `node` on PATH is ≥ 22.6 — the floor for `bynkc test --inspect`,
 *  which runs the emitted `.ts` under `--experimental-strip-types`. */
function nodeStripsTypes(): boolean {
  try {
    const v = spawnSync("node", ["--version"], { encoding: "utf8" }).stdout.trim();
    const m = v.match(/^v(\d+)\.(\d+)/);
    if (!m) return false;
    const [maj, min] = [Number(m[1]), Number(m[2])];
    return maj > 22 || (maj === 22 && min >= 6);
  } catch {
    return false;
  }
}

describe("Bynk debug provider (test mode)", () => {
  let dir: string;

  after(() => {
    if (dir) {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        /* best-effort */
      }
    }
  });

  it("startDebugging({type:'bynk', mode:'test'}) pauses at a .bynk breakpoint", async function () {
    if (!have("bynk") || !have("bynkc") || !have("node") || !nodeStripsTypes()) this.skip();
    this.timeout(90_000);

    const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
    dir = fs.realpathSync(fs.mkdtempSync(path.join(workspace, ".dbg-")));
    fs.mkdirSync(path.join(dir, "src"));
    fs.mkdirSync(path.join(dir, "tests"));
    fs.writeFileSync(
      path.join(dir, "bynk.toml"),
      "[project]\nname = \"dbg\"\n\n[paths]\ninclude = [\"src\", \"tests\"]\n",
    );
    fs.writeFileSync(
      path.join(dir, "src", "calc.bynk"),
      "commons calc {\n  fn dbl(n: Int) -> Int { n + n }\n}\n",
    );
    const testSrc =
      "suite calc {\n  case \"doubles\" {\n    let r = dbl(3)\n    expect r == 6\n  }\n}\n";
    fs.writeFileSync(path.join(dir, "tests", "calc.bynk"), testSrc);
    const bpLine = testSrc.split("\n").findIndex((l) => l.includes("let r =")) + 1;
    assertFixtureChecks(dir, "debug provider");

    // Open the test file so the provider's `find_project_root` (active-editor walk)
    // resolves to this subdir project, not the fixture root.
    const testUri = vscode.Uri.file(path.join(dir, "tests", "calc.bynk"));
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(testUri));

    // Watch for a pause whose top frame is the `.bynk` test.
    let hitBreakpoint = false;
    const verbose = process.env.BYNK_DEBUG_SPIKE === "verbose";
    const tracker = vscode.debug.registerDebugAdapterTrackerFactory("*", {
      createDebugAdapterTracker(s: vscode.DebugSession) {
        return {
          async onDidSendMessage(m: any) {
            if (verbose) {
              if (m.event === "loadedSource" && /\.(ts|bynk)$/.test(m.body?.source?.path ?? "")) {
                console.log("[dap] loadedSource", m.body?.source?.path);
              } else if (m.command === "setBreakpoints" && m.body?.breakpoints) {
                console.log("[dap] setBreakpoints ->", JSON.stringify(m.body.breakpoints.map((b: any) => ({ v: b.verified, l: b.line }))));
              } else if (m.event === "breakpoint") {
                console.log("[dap] breakpoint-event", m.body?.breakpoint?.verified, m.body?.breakpoint?.line);
              } else if (m.event === "output" && /Debugger|listening/.test(String(m.body?.output))) {
                console.log("[dap] output", String(m.body?.output).trim());
              }
            }
            if (m.type !== "event" || m.event !== "stopped") return;
            const threadId = m.body?.threadId ?? 0;
            let name = "";
            try {
              const st: any = await s.customRequest("stackTrace", { threadId, levels: 1 });
              name = st?.stackFrames?.[0]?.source?.name ?? "";
            } catch {
              /* ignore */
            }
            if (verbose) console.log("[dap] stopped", m.body?.reason, name);
            if (name.endsWith("calc.bynk")) hitBreakpoint = true;
            else void s.customRequest("continue", { threadId }); // entry / glue
          },
        };
      },
    });

    if (verbose) {
      vscode.debug.onDidStartDebugSession((s) =>
        console.log("[dap] session start", s.type, "port", (s.configuration as any).port),
      );
    }

    const bp = new vscode.SourceBreakpoint(
      new vscode.Location(testUri, new vscode.Position(bpLine - 1, 0)),
    );
    vscode.debug.addBreakpoints([bp]);

    // The real provider: compiles, launches `bynkc test --inspect`, attaches.
    const started = await vscode.debug.startDebugging(undefined, {
      type: "bynk",
      request: "launch",
      name: "Debug Bynk tests",
      mode: "test",
    });
    assert.ok(started, "startDebugging(type:bynk) returned true");

    try {
      const deadline = Date.now() + 60_000;
      while (Date.now() < deadline && !hitBreakpoint) {
        await new Promise((r) => setTimeout(r, 250));
      }
      assert.ok(
        hitBreakpoint,
        "a breakpoint set in tests/calc.bynk bound (through the real bynkc output map) and paused",
      );
    } finally {
      tracker.dispose();
      vscode.debug.removeBreakpoints([bp]);
      await vscode.debug.stopDebugging();
    }
  });
});
