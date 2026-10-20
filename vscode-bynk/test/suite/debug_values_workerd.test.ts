// Semantic-debugging slice 1 — the workerd payoff (flips slice 5's raw guard).
//
// Slice 5's in-debuggee `customDescriptionGenerator` couldn't run on `workerd`
// (the runtime rejects the evaluation), so the dev path showed the raw
// `{tag: "Some", value: "hi"}`. Slice 1's interposer rewrites the value *editor-
// side* (ADR 0105) — runtime-agnostic — so it works here too. This test drives a
// real worker handler with an Option local, attaches with the `__bynkChild` marker
// (so the extension's production tracker fires), and asserts the value now reads
// `Some("hi")` over `wrangler`'s inspector.
//
// Skipped without `wrangler`/`bynkc`/`node` (CI has no Cloudflare runtime), or
// when wrangler's workerd is too old for the emitted `compatibility_date`
// (#2180); built inside the workspace folder like slice 4's workerd test.

import * as assert from "assert";
import * as path from "path";
import * as fs from "fs";
import { spawnSync, ChildProcess } from "child_process";
import * as vscode from "vscode";
import { assertFixtureChecks } from "./fixtureCheck";
import { stopTree } from "./stopTree";
import { awaitInspector, startWranglerDev } from "./wranglerDev";

function have(tool: string): boolean {
  try {
    return spawnSync(tool, ["--version"], { stdio: "ignore" }).status === 0;
  } catch {
    return false;
  }
}

describe("Bynk debug values (workerd)", () => {
  let dir: string;
  let wrangler: ChildProcess | undefined;

  after(async () => {
    await stopTree(wrangler);
    if (dir) {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        /* best-effort */
      }
    }
  });

  it("renders a worker handler's Option local in Bynk syntax via the editor-side interposer", async function () {
    if (!have("wrangler") || !have("bynkc") || !have("node")) this.skip();
    this.timeout(90_000);
    const verbose = process.env.BYNK_DEBUG_SPIKE === "verbose";

    const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
    dir = fs.realpathSync(fs.mkdtempSync(path.join(workspace, ".vwdbg-")));
    fs.mkdirSync(path.join(dir, "src"));
    fs.writeFileSync(path.join(dir, "bynk.toml"), "[project]\nname = \"svc\"\n");
    // `opt` is a tagged Option local, in scope at the `Logger.info` line.
    const svc = [
      "context svc",
      "",
      "consumes bynk { Logger }",
      "",
      "service api from http {",
      "\ton GET(\"/\") () -> Effect[HttpResult[String]] by v: Visitor given Logger {",
      "\t\tlet opt = Some(\"hi\")",
      "\t\tlet _ <- Logger.info(\"hit\")",
      "\t\tmatch opt {",
      "\t\t\tSome(s) => Ok(s)",
      "\t\t\tNone => Ok(\"none\")",
      "\t\t}",
      "\t}",
      "}",
      "",
    ].join("\n");
    fs.writeFileSync(path.join(dir, "src", "svc.bynk"), svc);
    const bpLine = svc.split("\n").findIndex((l) => l.includes("Logger.info")) + 1;
    assertFixtureChecks(dir, "workerd debug-values");

    const build = path.join(dir, "build");
    const compile = spawnSync(
      "bynkc",
      ["compile", path.join(dir, "src"), "--output", build, "--target", "workers"],
      { encoding: "utf8" },
    );
    assert.strictEqual(compile.status, 0, `bynkc compile failed: ${compile.stderr}`);
    const workerDir = path.join(build, "workers", "svc");

    const base = 9500 + (process.pid % 200) * 2;
    const inspectorPort = base + 1;
    const appPort = base;
    const dev = startWranglerDev(workerDir, inspectorPort, appPort);
    wrangler = dev.child;
    await awaitInspector(this, dev, inspectorPort, 45_000);

    let optValue: string | undefined;
    const tracker = vscode.debug.registerDebugAdapterTrackerFactory("*", {
      createDebugAdapterTracker(s: vscode.DebugSession) {
        return {
          async onDidSendMessage(m: any) {
            if (m.type !== "event" || m.event !== "stopped") return;
            const threadId = m.body?.threadId ?? 0;
            let frame: any;
            try {
              const st: any = await s.customRequest("stackTrace", { threadId, levels: 1 });
              frame = st?.stackFrames?.[0];
            } catch {
              /* ignore */
            }
            if (!frame || !String(frame.source?.name ?? "").endsWith("svc.bynk")) {
              void s.customRequest("continue", { threadId });
              return;
            }
            try {
              const sc: any = await s.customRequest("scopes", { frameId: frame.id });
              for (const scope of sc.scopes) {
                const vars: any = await s.customRequest("variables", {
                  variablesReference: scope.variablesReference,
                });
                const opt = vars.variables.find((x: any) => x.name === "opt");
                if (opt) {
                  optValue = opt.value;
                  if (verbose) console.log("[var] opt =>", opt.value);
                }
              }
            } catch (e) {
              if (verbose) console.log("[err]", String(e));
            }
          },
        };
      },
    });

    const bp = new vscode.SourceBreakpoint(
      new vscode.Location(
        vscode.Uri.file(path.join(dir, "src", "svc.bynk")),
        new vscode.Position(bpLine - 1, 0),
      ),
    );
    vscode.debug.addBreakpoints([bp]);

    // No `customDescriptionGenerator` (workerd rejects it) — but the `__bynkChild`
    // marker makes the extension's production tracker fire and rewrite the value
    // *editor-side*. That is the slice-1 payoff: Bynk vocabulary on workerd.
    const ok = await vscode.debug.startDebugging(undefined, {
      type: "node",
      request: "attach",
      name: "bynk-values-workerd",
      port: inspectorPort,
      resolveSourceMapLocations: null,
      skipFiles: ["<node_internals>/**"],
      __bynkChild: "values-workerd",
    } as vscode.DebugConfiguration);
    assert.ok(ok, "startDebugging returned true");

    try {
      const deadline = Date.now() + 30_000;
      while (Date.now() < deadline && optValue === undefined) {
        fetch(`http://127.0.0.1:${appPort}/`).catch(() => {});
        await new Promise((r) => setTimeout(r, 500));
      }
      if (verbose) console.log("[result] optValue=", optValue);
      // The editor-side interposer renders the Option in Bynk syntax — over
      // wrangler's inspector, where slice 5's in-debuggee generator could not.
      assert.strictEqual(
        optValue,
        'Some("hi")',
        `Option renders in Bynk syntax on workerd, got ${optValue}`,
      );
    } finally {
      tracker.dispose();
      vscode.debug.removeBreakpoints([bp]);
      await vscode.debug.stopDebugging();
    }
  });
});
