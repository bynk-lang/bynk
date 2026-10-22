// The targets a run can go against, and how a refused run is read (Test Explorer
// depth track, Slice B). Dependency-free (no `vscode` import) so the unit tests
// compile against it directly.
//
// The extension reads only target *names* from `bynk.toml`, never a `url`, a
// `production` flag or a secret. The driver is the one place that decides whether
// a name is safe to run against, and it refuses an unknown name, a bare URL and a
// production target itself (ADR 0527/0528/0532); the extension passes a name it
// found in the manifest and relays the driver's refusal.

/** The target `bynk test --against dev` runs against: a local `bynk dev` the
 *  driver starts itself. Always offered, and not a manifest entry. */
export const DEV_TARGET = "dev";

/** The target names declared as `[test.targets.<name>]` tables, in file order,
 *  without duplicates. A header scan, not a TOML parse: it takes a bare name and a
 *  quoted one, and misses an inline or dotted form (`[test]` with
 *  `targets.x.url = …`), which the driver still accepts when the name is typed. */
export function scanTargets(toml: string): string[] {
  const names: string[] = [];
  for (const raw of toml.split(/\r?\n/)) {
    const line = raw.trim();
    // `[test.targets.preview]`, `[test.targets."pre view"]`, `[ test . targets . 'x' ]`,
    // optionally followed by a comment.
    const m = /^\[\s*test\s*\.\s*targets\s*\.\s*(?:"((?:[^"\\]|\\.)*)"|'([^']*)'|([A-Za-z0-9_-]+))\s*\]\s*(?:#.*)?$/.exec(
      line,
    );
    if (!m) continue;
    const name = m[1] !== undefined ? m[1].replace(/\\(.)/g, "$1") : (m[2] ?? m[3]);
    if (name && name !== DEV_TARGET && !names.includes(name)) names.push(name);
  }
  return names;
}

/** What the quick pick offers: `dev` first, then the manifest's targets, with the
 *  last choice (when it is still on offer) at the top. */
export function targetChoices(manifest: readonly string[], last?: string): string[] {
  const all = [DEV_TARGET, ...manifest.filter((n) => n !== DEV_TARGET)];
  return last && all.includes(last) ? [last, ...all.filter((n) => n !== last)] : all;
}

/** One line on what running against a target does, for the quick pick and the
 *  profile's description. A deployed target keeps its state between runs
 *  (ADR 0532); `dev` is a fresh local server. */
export function targetDetail(name: string): string {
  return name === DEV_TARGET
    ? "Starts a local `bynk dev`, runs the system suites against it, and stops it."
    : "A deployed target keeps its state between runs, so a case can see what an earlier run left.";
}

/** Whether the driver refused a run because a case uses something a real runtime
 *  cannot honour, which `--skip-unsupported` would skip. The refusal is a compile
 *  diagnostic that says so (`bynk-check/src/test_suites.rs`). */
export function offersSkipUnsupported(diagnostics: readonly string[]): boolean {
  return diagnostics.some((d) => d.includes("--skip-unsupported"));
}
