// Slice 4 workerd spike + integration test: does VS Code's built-in JavaScript
// debugger (`pwa-node` attach) reach `wrangler dev`'s V8 inspector and bind a
// breakpoint set in a `.bynk` handler — resolving it through the `.ts.map` that
// `wrangler`/esbuild composes into the worker bundle?
//
// This is the one open question slice 3 flagged: `wrangler`'s inspector requires
// an `Origin` header on the CDP WebSocket. If js-debug's attach binds the
// breakpoint, it sends one. Skipped when `wrangler` / `bynkc` / `node` are
// unavailable, so CI (no Cloudflare runtime) never depends on it.

import * as assert from "assert";
import * as path from "path";
import * as fs from "fs";
import { spawn, spawnSync, ChildProcess } from "child_process";
import * as vscode from "vscode";
import { assertFixtureChecks } from "./fixtureCheck";
import { stopTree } from "./stopTree";

function have(tool: string, args: string[] = ["--version"]): boolean {
  try {
    return spawnSync(tool, args, { stdio: "ignore" }).status === 0;
  } catch {
    return false;
  }
}

describe("Bynk debug attach (workerd)", () => {
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

  it("a breakpoint in a .bynk handler binds and pauses on a wrangler dev worker", async function () {
    if (!have("wrangler") || !have("bynkc") || !have("node")) {
      this.skip();
    }
    this.timeout(90_000);

    // Build the project *inside* the VS Code workspace folder, as a real user's
    // project is: js-debug anchors source-map resolution to the workspace, so a
    // project in an unrelated temp dir would mis-resolve the `.bynk` source.
    const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
    dir = fs.realpathSync(fs.mkdtempSync(path.join(workspace, ".wdbg-")));
    fs.mkdirSync(path.join(dir, "src"));
    fs.writeFileSync(path.join(dir, "bynk.toml"), "[project]\nname = \"svc\"\n");
    const svc =
      "context svc\n\nconsumes bynk { Logger }\n\nservice api from http {\n\ton GET(\"/\") () -> Effect[HttpResult[String]] by v: Visitor given Logger {\n\t\tlet _ <- Logger.info(\"hit\")\n\t\tOk(\"ok\")\n\t}\n}\n";
    fs.writeFileSync(path.join(dir, "src", "svc.bynk"), svc);
    const bynkLine = svc.split("\n").findIndex((l) => l.includes("Logger.info")) + 1;
    assertFixtureChecks(dir, "workerd debug");

    // Build the worker (Workers target, with source maps) via the release bynkc.
    const build = path.join(dir, "build");
    const compile = spawnSync(
      "bynkc",
      ["compile", path.join(dir, "src"), "--output", build, "--target", "workers"],
      { encoding: "utf8" },
    );
    assert.strictEqual(compile.status, 0, `bynkc compile failed: ${compile.stderr}`);
    const workerDir = path.join(build, "workers", "svc");
    assert.ok(fs.existsSync(path.join(workerDir, "handlers.ts.map")), "worker carries a map");

    const base = 9400 + (process.pid % 200) * 2;
    const inspectorPort = base + 1;
    const appPort = base;

    wrangler = spawn(
      "wrangler",
      [
        "dev",
        "--inspector-port",
        String(inspectorPort),
        "--port",
        String(appPort),
      ],
      // #2069: its own process group, so `stopTree` reaches workerd too.
      { cwd: workerDir, stdio: ["ignore", "ignore", "pipe"], detached: process.platform !== "win32" },
    );
    const verbose = process.env.BYNK_DEBUG_SPIKE === "verbose";
    let wlog = "";
    wrangler.stderr?.on("data", (b) => {
      wlog += String(b);
      if (verbose) console.log("[wrangler]", String(b).trim());
    });

    // Wait for the inspector to come up (CDP discovery endpoint responds).
    const ready = await waitFor(
      async () => {
        try {
          const r = await fetch(`http://127.0.0.1:${inspectorPort}/json`);
          const j = (await r.json()) as any[];
          return j.length > 0;
        } catch {
          return false;
        }
      },
      45_000,
    );
    assert.ok(ready, "wrangler dev inspector never became reachable");

    // Watch for a pause whose top frame is the `.bynk` handler.
    let hitBreakpoint = false;
    const tracker = vscode.debug.registerDebugAdapterTrackerFactory("*", {
      createDebugAdapterTracker(s: vscode.DebugSession) {
        return {
          async onDidSendMessage(m: any) {
            if (verbose) {
              if (m.event === "loadedSource") {
                console.log("[dap] loadedSource", m.body?.source?.path ?? m.body?.source?.name);
              } else if (m.event === "output") {
                console.log("[dap] output", String(m.body?.output).trim());
              } else if (m.command === "setBreakpoints" && m.body?.breakpoints) {
                console.log(
                  "[dap] setBreakpoints ->",
                  JSON.stringify(m.body.breakpoints.map((b: any) => ({ v: b.verified, l: b.line }))),
                );
              } else if (m.event === "breakpoint") {
                console.log("[dap] breakpoint", m.body?.breakpoint?.verified, m.body?.breakpoint?.line);
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
            if (name.endsWith("svc.bynk")) hitBreakpoint = true;
            else void s.customRequest("continue", { threadId });
          },
        };
      },
    });

    const bynkUri = vscode.Uri.file(path.join(dir, "src", "svc.bynk"));
    const bp = new vscode.SourceBreakpoint(
      new vscode.Location(bynkUri, new vscode.Position(bynkLine - 1, 0)),
    );
    vscode.debug.addBreakpoints([bp]);

    // Attach js-debug to wrangler's inspector. The `Origin`-header requirement is
    // js-debug's to satisfy — if the breakpoint binds, it sent one.
    const ok = await vscode.debug.startDebugging(undefined, {
      type: "node",
      request: "attach",
      name: "bynk-workerd-spike",
      port: inspectorPort,
      resolveSourceMapLocations: null,
      skipFiles: ["<node_internals>/**"],
    });
    assert.ok(ok, "startDebugging(attach to wrangler inspector) returned true");

    try {
      // Drive requests so the handler runs and hits the breakpoint.
      const deadline = Date.now() + 30_000;
      while (Date.now() < deadline && !hitBreakpoint) {
        fetch(`http://127.0.0.1:${appPort}/`).catch(() => {});
        await new Promise((r) => setTimeout(r, 500));
      }
      if (!hitBreakpoint && verbose) console.log("[wrangler-log]\n" + wlog);
      assert.ok(
        hitBreakpoint,
        "a breakpoint set in svc.bynk bound (through the composed bundle map) and paused on a worker request",
      );
    } finally {
      tracker.dispose();
      vscode.debug.removeBreakpoints([bp]);
      await vscode.debug.stopDebugging();
    }
  });
});

async function waitFor(cond: () => Promise<boolean>, timeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await cond()) return true;
    await new Promise((r) => setTimeout(r, 500));
  }
  return false;
}
