// #2115: a guard for the integration tests that write a Bynk project and then
// compile, test or debug it. When the language moves on (as `by` did, from
// before the parameter list to after it), a stale fixture otherwise fails
// later and opaquely — a compile assertion deep in a debug test, or a debug
// session that never starts. Checking the fixture first names the real cause.

import * as assert from "assert";
import { spawnSync } from "child_process";

/** Fail the test, with `bynkc`'s diagnostics, unless the project at
 *  `projectDir` passes `bynkc check`. Requires `bynkc` on PATH (callers
 *  already skip without it). */
export function assertFixtureChecks(projectDir: string, what: string): void {
  const r = spawnSync("bynkc", ["check", projectDir], { encoding: "utf8" });
  assert.strictEqual(
    r.status,
    0,
    `the ${what} fixture no longer passes \`bynkc check\` — update it to the current syntax:\n${r.stderr}${r.stdout}`,
  );
}
