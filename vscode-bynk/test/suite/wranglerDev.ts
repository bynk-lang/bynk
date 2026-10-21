// #2180: start `wrangler dev` for the workerd debug tests and wait for its V8
// inspector, keeping what wrangler prints so a failure says why.
//
// Both workerd tests used to discard wrangler's stdout, so a wrangler that
// exited at once (for instance because its bundled workerd was older than the
// worker's `compatibility_date`) surfaced only as "inspector never became
// reachable". This helper captures stdout and stderr, stops waiting as soon as
// wrangler exits, and puts the captured output in the assertion message.
//
// A workerd too old for the emitted `compatibility_date` is a property of the
// machine's wrangler install, not of the change under test, so the test is
// skipped (with the reason logged) as it is when wrangler is absent.

import * as assert from "assert";
import { spawn, ChildProcess } from "child_process";

export interface WranglerDev {
  child: ChildProcess;
  /** Everything wrangler has written to stdout and stderr so far. */
  output(): string;
  /** How wrangler ended (`code 1`, `signal SIGTERM`) once it has exited and
   *  its output is drained; undefined while it runs. */
  exited(): string | undefined;
}

/** Spawn `wrangler dev` in `cwd` on the given ports. Under
 *  `BYNK_DEBUG_SPIKE=verbose` its output is echoed as it arrives. */
export function startWranglerDev(cwd: string, inspectorPort: number, appPort: number): WranglerDev {
  const child = spawn(
    "wrangler",
    ["dev", "--inspector-port", String(inspectorPort), "--port", String(appPort)],
    // #2069: its own process group, so `stopTree` reaches workerd too.
    { cwd, stdio: ["ignore", "pipe", "pipe"], detached: process.platform !== "win32" },
  );
  const verbose = process.env.BYNK_DEBUG_SPIKE === "verbose";
  let log = "";
  const collect = (b: Buffer) => {
    log += String(b);
    if (verbose) console.log("[wrangler]", String(b).trim());
  };
  child.stdout?.on("data", collect);
  child.stderr?.on("data", collect);
  // `close`, not `exit`: it fires once stdout and stderr are drained, so the
  // output is complete by the time a caller reports it.
  let ended: string | undefined;
  child.on("close", (code, signal) => {
    ended = signal ? `signal ${signal}` : `code ${code}`;
  });
  child.on("error", (e) => {
    ended = `error ${e.message}`;
  });
  return { child, output: () => log, exited: () => ended };
}

// workerd's refusal when the worker asks for a newer date than it supports, e.g.
// `This Worker requires compatibility date "2026-07-01", but the newest date
// supported by this server binary is "2026-06-24".`
const STALE_WORKERD =
  /requires compatibility date "([0-9-]+)", but the newest date supported by this server binary is "([0-9-]+)"/;

/** Wait until the inspector's CDP discovery endpoint lists a target. Skips the
 *  test when wrangler's workerd is too old for the worker, and otherwise fails
 *  with wrangler's captured output if the inspector never comes up. */
export async function awaitInspector(
  ctx: Mocha.Context,
  dev: WranglerDev,
  inspectorPort: number,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline && dev.exited() === undefined) {
    try {
      const r = await fetch(`http://127.0.0.1:${inspectorPort}/json`);
      if (((await r.json()) as any[]).length > 0) return;
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  const out = dev.output();
  const exited = dev.exited();
  const stale = STALE_WORKERD.exec(out);
  if (stale) {
    console.log(
      `[#2180] skipping: wrangler's workerd supports compatibility dates up to ${stale[2]}, ` +
        `but the worker requires ${stale[1]}; update wrangler to run this test`,
    );
    ctx.skip();
  }
  assert.fail(
    `wrangler dev inspector never became reachable` +
      (exited !== undefined ? ` (wrangler ended with ${exited})` : ` within ${timeoutMs} ms`) +
      `; wrangler output:\n${out.trim() || "(none)"}`,
  );
}
