// Seeds for generative tests (Test Explorer depth track, Slice C): which seed a
// run is given, and whether a pinned one still means what it did. Dependency-free
// (no `vscode` import), so the unit tests compile against it directly.
//
// A property draws from a stream seeded by `mix(rootSeed, ordinal)`, where the
// ordinal is the property's position among its module's generative cases. So a
// root seed reproduces a property's failure only while the generative cases before
// it are the same: a pin keeps that list (its fingerprint), and one that no longer
// matches is stale, shown as such and never passed to `--seed`.

/** A pinned seed, with the generative cases of its suite as they were when it
 *  was taken. */
export interface Pin {
  seed: string;
  fingerprint: string[];
}

/** What a failed generative case reported. */
export interface SeedFailure {
  seed: string;
  counterexample?: string;
}

/** The runner reports a seed as `0x…` hex and `--seed` takes it back. Anything
 *  else (an edited workspace state, say) is not passed to the driver. */
export function validSeed(seed: unknown): seed is string {
  return typeof seed === "string" && /^0x[0-9a-f]{1,8}$/i.test(seed);
}

/** The names of a suite's generative cases, in order: the cases whose stream a
 *  seed's meaning depends on. An example-based case is not in it, so editing one
 *  does not make a pin stale. */
export function fingerprintOf(cases: readonly { name: string; kind?: string }[]): string[] {
  return cases.filter((c) => c.kind === "property" || c.kind === "history").map((c) => c.name);
}

export type PinState = "fresh" | "stale";

/** A pin is fresh while the suite's generative cases are the same list, in the
 *  same order. */
export function pinState(pin: Pin, current: readonly string[]): PinState {
  return pin.fingerprint.length === current.length && pin.fingerprint.every((n, i) => n === current[i])
    ? "fresh"
    : "stale";
}

/** The text a pinned item shows beside its name. */
export function pinDescription(pin: Pin, state: PinState): string {
  return state === "stale" ? `seed ${pin.seed} (stale)` : `seed ${pin.seed}`;
}

/** The seed a run passes, and a line for the run output when a pin was held back.
 *  A pin applies only to a single-case run of that case, because the seed is the
 *  run's root seed, one value per process: `single` is that case's key when the
 *  run is exactly one case (and not another kind of run), else `undefined`. */
export function seedFor(
  single: string | undefined,
  pins: ReadonlyMap<string, Pin>,
  fingerprintOfSuite: (key: string) => string[],
  name: string,
): { seed?: string; note?: string } {
  if (single === undefined) return {};
  const pin = pins.get(single);
  if (!pin) return {};
  if (!validSeed(pin.seed)) return { note: `the pinned seed for ${name} is not a seed, so a fresh one is used` };
  if (pinState(pin, fingerprintOfSuite(single)) === "stale") {
    return {
      note:
        `the pinned seed ${pin.seed} for ${name} is stale: the suite's property tests changed, ` +
        `so it could draw a different stream. A fresh seed is used; unpin or pin again to refresh it`,
    };
  }
  return { seed: pin.seed };
}
