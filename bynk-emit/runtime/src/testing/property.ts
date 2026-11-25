import { ExpectationError, __bynkViolationOf } from "./expectation.ts";

export function __bynkRootSeed(): number {
  const env = (globalThis as any) && (globalThis as any).process && (globalThis as any).process.env;
  const s = env && env.BYNK_TEST_SEED;
  if (s) { const n = parseInt(String(s), 16); if (Number.isFinite(n)) return n >>> 0; }
  return (Math.floor(Math.random() * 0x100000000)) >>> 0;
}
export const __bynkSeed: number = __bynkRootSeed();
export function __bynkMix(a: number, b: number): number {
  let h = (a ^ Math.imul(b + 1, 0x9e3779b1)) >>> 0;
  h = Math.imul(h ^ (h >>> 16), 0x45d9f3b) >>> 0;
  h = Math.imul(h ^ (h >>> 16), 0x45d9f3b) >>> 0;
  return (h ^ (h >>> 16)) >>> 0;
}
export function __bynkRng(seed: number) {
  let s = seed >>> 0;
  const next = () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  return {
    next,
    int(lo: bigint, hi: bigint): bigint {
      if (hi <= lo) return lo;
      const span = hi - lo + 1n;
      let r = 0n, bound = span;
      while (bound > 0n) {
        r = (r << 32n) | BigInt(Math.floor(next() * 0x100000000));
        bound >>= 32n;
      }
      return lo + (r % span);
    },
    float(lo: number, hi: number): number { return lo + next() * (hi - lo); },
    str(min: number, max: number): string {
      const len = min + Math.floor(next() * (max - min + 1));
      const alpha = "abcdefghijklmnopqrstuvwxyz";
      let out = "";
      for (let i = 0; i < len; i++) out += alpha[Math.floor(next() * alpha.length)];
      return out;
    },
    // #4199: draws across a plain type's realistic domain. `intWide` picks a magnitude tier
    // (small, 32-bit, safe-integer) so small values stay common and the extremes still appear,
    // always inside `[lo, hi]`.
    intWide(lo: bigint, hi: bigint, extra: bigint[] = []): bigint {
      if (hi <= lo) return lo;
      const tiers = [10n, 1000n, 2147483647n, ...extra];
      const span = tiers[Math.floor(next() * tiers.length)];
      let a = lo > -span ? lo : -span;
      let b = hi < span ? hi : span;
      if (a > b) { a = lo; b = lo + span < hi ? lo + span : hi; }
      if (hi < -span) { b = hi; a = hi - span > lo ? hi - span : lo; }
      return this.int(a, b);
    },
    // Finite floats only: 0 and -0, ordinary values, and extreme magnitudes. `wide` is false
    // when a refinement bounds the range.
    floatWide(lo: number, hi: number, wide: boolean): number {
      if (!wide) return lo + next() * (hi - lo);
      const r = next();
      if (r < 0.05) return 0;
      if (r < 0.1) return -0;
      if (r < 0.75) return -1000 + next() * 2000;
      const sign = next() < 0.5 ? -1 : 1;
      return sign * (1 + next()) * Math.pow(10, Math.floor(next() * 600) - 300);
    },
    // Whitespace, punctuation, accented, CJK and astral characters beside ASCII letters.
    strWide(min: number, max: number): string {
      const tiers = [8, 8, 16, 64];
      const cap = Math.max(min, Math.min(max, tiers[Math.floor(next() * tiers.length)]));
      const len = min + Math.floor(next() * (cap - min + 1));
      const pools = [
        "abcdefghijklmnopqrstuvwxyz", "ABCDEFGHIJKLMNOPQRSTUVWXYZ", "0123456789", " \t\n\r",
        "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~", "éüñßøçÅ", "日本語中文", "😀🎉𝒳𐍈",
      ];
      let out = "";
      for (let i = 0; i < len; i++) {
        const pool = Array.from(next() < 0.6 ? pools[0] : pools[Math.floor(next() * pools.length)]);
        out += pool[Math.floor(next() * pool.length)];
      }
      return out;
    },
    // Whole milliseconds across a day, a month, a present-day epoch and a `Date`'s whole reach.
    numWide(lo: number, hi: number): number {
      return Number(this.intWide(BigInt(lo), BigInt(hi), [86400000n, 2592000000n, 1700000000000n, BigInt(hi)]));
    },
    decimalWide(): { coefficient: bigint, scale: number } {
      const scale = Math.floor(next() * 9);
      return { coefficient: this.intWide(-1000000000000000000n, 1000000000000000000n, [1000000000000n, 1000000000000000000n]), scale };
    },
    bytes(min: number, max: number): Uint8Array {
      const tiers = [4, 16, 64];
      const cap = Math.max(min, Math.min(max, tiers[Math.floor(next() * tiers.length)]));
      const out = new Uint8Array(min + Math.floor(next() * (cap - min + 1)));
      for (let i = 0; i < out.length; i++) out[i] = next() < 0.15 ? (next() < 0.5 ? 0 : 255) : Math.floor(next() * 256);
      return out;
    },
    // #4121: a member of a `Matches` pattern, or a real calendar date for `IsoDate`.
    matches(spec: string): string { return __matchDraw(__matchSpec(spec), next); },
    bool(): boolean { return next() < 0.5; },
    pick(thunks: Array<() => any>): any { return thunks[Math.floor(next() * thunks.length)](); },
  };
}
// #4121: drawing from a `Matches` pattern's walk. The walk tree is the checker's parse of the
// pattern; the compiled patterns check every draw, so a walk the engine disagrees with fails
// loudly instead of drawing a wrong value.
type __MatchNode = ["e"] | ["c", number[]] | ["s", __MatchNode[]] | ["a", __MatchNode[]] | ["r", __MatchNode, number, number];
type __MatchSpec = { t: __MatchNode | null, p: [string, string] | null, o: Array<[string, string]>, iso: boolean, min: number, max: number };
type __MatchRun = { spec: __MatchSpec, own: RegExp | null, others: RegExp[], lo: number, hi: number, text: string };
const __matchRuns = new Map<string, __MatchRun>();
function __matchSpec(json: string): __MatchRun {
  let run = __matchRuns.get(json);
  if (run === undefined) {
    const spec = JSON.parse(json) as __MatchSpec;
    const re = (pair: [string, string]) => new RegExp("^(?:" + pair[0] + ")$", "u" + pair[1]);
    const [tlo, thi] = spec.t === null ? [10, 10] : __matchLens(spec.t);
    const text = [spec.p, ...spec.o].filter((q): q is [string, string] => q !== null).map((q) => "Matches(\"" + q[0] + "\")").concat(spec.iso ? ["IsoDate"] : []).join(" and ");
    run = { spec, own: spec.p === null ? null : re(spec.p), others: spec.o.map(re), lo: Math.max(tlo, spec.min), hi: Math.min(thi, spec.max < 0 ? Infinity : spec.max), text };
    __matchRuns.set(json, run);
  }
  return run;
}
function __matchLens(n: __MatchNode): [number, number] {
  switch (n[0]) {
    case "e": return [0, 0];
    case "c": return [1, 1];
    case "s": {
      let lo = 0, hi = 0;
      for (const c of n[1]) { const [a, b] = __matchLens(c); lo += a; hi += b; }
      return [lo, hi];
    }
    case "a": {
      const ls = n[1].map(__matchLens);
      return [Math.min(...ls.map((l) => l[0])), Math.max(...ls.map((l) => l[1]))];
    }
    case "r": {
      const [a, b] = __matchLens(n[1]);
      const max = n[3] < 0 ? Infinity : n[3];
      return [a * n[2], b === 0 || max === 0 ? 0 : b * max];
    }
  }
}
function __matchCp(ranges: number[], next: () => number): string {
  const pairs: Array<[number, number]> = [];
  for (let i = 0; i < ranges.length; i += 2) pairs.push([ranges[i], ranges[i + 1]]);
  const ascii = pairs.map(([a, b]): [number, number] => [Math.max(a, 0x20), Math.min(b, 0x7e)]).filter(([a, b]) => a <= b);
  const astral = pairs.map(([a, b]): [number, number] => [Math.max(a, 0x10000), b]).filter(([a, b]) => a <= b);
  let pool = pairs;
  if (ascii.length > 0 && next() < 0.85) pool = ascii;
  else if (astral.length > 0 && next() < 0.3) pool = astral;
  let total = 0;
  for (const [a, b] of pool) total += b - a + 1;
  let k = Math.floor(next() * total);
  for (const [a, b] of pool) {
    if (k < b - a + 1) return String.fromCodePoint(a + k);
    k -= b - a + 1;
  }
  return String.fromCodePoint(pool[0][0]);
}
function __matchPick(lo: number, hi: number, next: () => number): number {
  return lo + Math.floor(next() * (hi - lo + 1));
}
// A member of `n` that is exactly `t` code points long, or `null` when there is none.
function __matchGen(n: __MatchNode, t: number, next: () => number): string | null {
  switch (n[0]) {
    case "e": return t === 0 ? "" : null;
    case "c": return t === 1 ? __matchCp(n[1], next) : null;
    case "s": return __matchSeq(n[1], t, next);
    case "a": {
      const fit = n[1].filter((c) => { const [a, b] = __matchLens(c); return a <= t && t <= b; });
      let start = Math.floor(next() * fit.length);
      for (let i = 0; i < fit.length; i++) {
        const out = __matchGen(fit[(start + i) % fit.length], t, next);
        if (out !== null) return out;
      }
      return null;
    }
    case "r": {
      const [a, b] = __matchLens(n[1]);
      const max = n[3] < 0 ? Infinity : n[3];
      let kLo = b === 0 ? n[2] : Math.max(n[2], Math.ceil(t / b));
      if (t > 0) kLo = Math.max(kLo, 1);
      let kHi = a === 0 ? Math.min(max, Math.max(kLo, t) + 2) : Math.min(max, Math.floor(t / a));
      if (kLo > kHi) return null;
      const k = __matchPick(kLo, Math.min(kHi, kLo + 24), next);
      return __matchSeq(Array.from({ length: k }, () => n[1]), t, next);
    }
  }
}
function __matchSeq(items: __MatchNode[], t: number, next: () => number): string | null {
  const ls = items.map(__matchLens);
  const restLo = new Array<number>(items.length + 1).fill(0);
  const restHi = new Array<number>(items.length + 1).fill(0);
  for (let i = items.length - 1; i >= 0; i--) { restLo[i] = restLo[i + 1] + ls[i][0]; restHi[i] = restHi[i + 1] + ls[i][1]; }
  let rem = t, out = "";
  for (let i = 0; i < items.length; i++) {
    const lo = Math.max(ls[i][0], rem - restHi[i + 1]);
    const hi = Math.min(ls[i][1], rem - restLo[i + 1]);
    if (lo > hi) return null;
    const len = __matchPick(lo, Math.min(hi, lo + 24), next);
    const piece = __matchGen(items[i], len, next);
    if (piece === null) return null;
    out += piece;
    rem -= len;
  }
  return rem === 0 ? out : null;
}
function __matchValid(run: __MatchRun, s: string): boolean {
  const len = Array.from(s).length;
  if (len < run.spec.min || (run.spec.max >= 0 && len > run.spec.max)) return false;
  if (run.own !== null && !run.own.test(s)) return false;
  if (!run.others.every((re) => re.test(s))) return false;
  return !run.spec.iso || __matchIsDate(s);
}
function __matchIsDate(s: string): boolean {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  if (m === null) return false;
  const y = Number(m[1]), mo = Number(m[2]), d = Number(m[3]);
  return mo >= 1 && mo <= 12 && d >= 1 && d <= __matchDays(y, mo);
}
function __matchDays(y: number, mo: number): number {
  if (mo === 2) return (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0 ? 29 : 28;
  return [4, 6, 9, 11].includes(mo) ? 30 : 31;
}
// A real date: mostly the present era, with leap days, month ends and the first and last years.
function __matchDate(next: () => number): string {
  const r = next();
  const edge = [0, 1900, 2000, 2024, 2028, 9999];
  const y = r < 0.6 ? 1970 + Math.floor(next() * 131) : r < 0.85 ? Math.floor(next() * 10000) : edge[Math.floor(next() * edge.length)];
  const mo = 1 + Math.floor(next() * 12);
  const last = __matchDays(y, mo);
  const d = next() < 0.3 ? last : next() < 0.15 ? 1 : 1 + Math.floor(next() * last);
  return String(y).padStart(4, "0") + "-" + String(mo).padStart(2, "0") + "-" + String(d).padStart(2, "0");
}
function __matchDraw(run: __MatchRun, next: () => number): string {
  for (let attempt = 0; attempt < 200; attempt++) {
    let s: string | null;
    if (run.spec.t === null) s = __matchDate(next);
    else {
      const cap = Math.min(run.hi, run.lo + 24);
      const r = next();
      const t = r < 0.1 ? run.lo : r < 0.2 ? cap : __matchPick(run.lo, cap, next);
      s = __matchGen(run.spec.t, t, next);
    }
    if (s === null) continue;
    if (run.own !== null && !run.own.test(s)) {
      throw new Error("bynk: the generator disagrees with " + run.text.split(" and ")[0] + " on " + JSON.stringify(s));
    }
    if (__matchValid(run, s)) return s;
  }
  throw new Error("bynk: no value satisfied " + run.text + " in 200 draws; pin one with a `case`");
}
// The shortest (`which` 0) or longest (`which` 1) member, drawn without chance.
export function __bynkMatchBoundary(json: string, which: number): string {
  const run = __matchSpec(json);
  if (run.spec.t === null) return which === 0 ? "0000-01-01" : "9999-12-31";
  const next = () => (which === 0 ? 0 : 0.999999);
  const t = which === 0 ? run.lo : Math.min(run.hi, run.lo + 8);
  for (const target of [t, ...Array.from({ length: 8 }, (_, i) => run.lo + i)]) {
    const s = __matchGen(run.spec.t, target, next);
    if (s !== null && __matchValid(run, s)) return s;
  }
  return __matchDraw(run, () => 0.5);
}
// Candidates shorter or earlier than `v`, each still a member.
export function __bynkShrinkMatch(v: string, json: string): string[] {
  const run = __matchSpec(json);
  const out: string[] = [];
  const add = (s: string) => { if (s !== v && !out.includes(s) && __matchValid(run, s)) out.push(s); };
  if (run.spec.t === null) {
    add("0000-01-01");
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(v);
    if (m !== null) {
      add(m[1] + "-01-01");
      add(m[1] + "-" + m[2] + "-01");
      add(String(Math.floor(Number(m[1]) / 2)).padStart(4, "0") + "-" + m[2] + "-" + m[3]);
    }
    return out;
  }
  const cps = Array.from(v);
  for (let i = cps.length - 1; i >= 0; i--) add(cps.filter((_, j) => j !== i).join(""));
  for (let i = 0; i < cps.length; i++) {
    for (const c of ["0", "a", "A"]) if (c < cps[i]) add(cps.map((x, j) => (j === i ? c : x)).join(""));
  }
  return out.sort((a, b) => Array.from(a).length - Array.from(b).length);
}
export function __bynkShrinkInt(v: bigint, floor: bigint): bigint[] {
  const out: bigint[] = [];
  if (v === floor) return out;
  out.push(floor);
  let cur = v;
  for (let i = 0; i < 64; i++) {
    const mid = floor + (cur - floor) / 2n;
    if (mid === cur || mid === floor) break;
    out.push(mid);
    cur = mid;
  }
  if (v > floor) out.push(v - 1n);
  return out;
}
// Strings shrink by code point, so an astral character is never cut in half.
export function __bynkShrinkString(v: string, min: number): string[] {
  const out: string[] = [];
  const cps = Array.from(v);
  if (cps.length > min) {
    out.push(cps.slice(0, min).join(""));
    if (cps.length - 1 > min) out.push(cps.slice(0, cps.length - 1).join(""));
  }
  return out;
}
// #4199: a number (`Float`, `Duration`, `Instant`) shrinks toward `floor`.
export function __bynkShrinkNum(v: number, floor: number): number[] {
  const out: number[] = [];
  if (Object.is(v, floor)) return out;
  out.push(floor);
  const whole = Math.trunc(v);
  if (whole !== v && Math.abs(whole - floor) < Math.abs(v - floor)) out.push(whole);
  // A huge magnitude first drops by orders of magnitude, so it reaches an ordinary size within
  // the shrink budget.
  for (const f of [1e100, 1e50, 1e10, 1e3]) {
    const scaled = floor + (v - floor) / f;
    if (Math.abs(v - floor) > f && scaled !== v) out.push(scaled);
  }
  let cur = v;
  for (let i = 0; i < 64; i++) {
    const mid = floor + (cur - floor) / 2;
    if (mid === cur || mid === floor) break;
    out.push(mid);
    cur = mid;
  }
  return out;
}
// A `Duration` or `Instant` is whole milliseconds, so it shrinks through the integer shrinker.
export function __bynkShrinkMillis(v: number): number[] {
  return __bynkShrinkInt(BigInt(v), 0n).map(Number);
}
export function __bynkShrinkDecimal(v: { coefficient: bigint, scale: number }): Array<{ coefficient: bigint, scale: number }> {
  const out: Array<{ coefficient: bigint, scale: number }> = [];
  if (v.coefficient === 0n && v.scale === 0) return out;
  out.push({ coefficient: 0n, scale: 0 });
  for (const c of __bynkShrinkInt(v.coefficient, 0n)) out.push({ coefficient: c, scale: v.scale });
  if (v.scale > 0 && v.coefficient % 10n === 0n) out.push({ coefficient: v.coefficient / 10n, scale: v.scale - 1 });
  return out;
}
export function __bynkShrinkBytes(v: Uint8Array): Uint8Array[] {
  const out: Uint8Array[] = [];
  if (v.length === 0) return out;
  out.push(new Uint8Array(0));
  if (v.length > 1) out.push(v.slice(0, v.length - 1));
  if (v.some((b) => b !== 0)) out.push(new Uint8Array(v.length));
  return out;
}
export function __bynkIsFailure(e: any): e is Error {
  // A shrinkable property/contract failure: an `expect` assertion, or a v0.115
  // function-contract guard violation thrown from the attacked function. The
  // `e is Error` predicate narrows the caught value so `e.message` type-checks.
  // An invariant or transition violation (#2272) shrinks the same way: it is a
  // property of the input, so the smallest input that still violates it is the
  // one to show.
  return e instanceof ExpectationError || (!!e && e.name === "BynkContractError") || (!!e && e.invariantViolation !== undefined);
}
export async function __bynkShrink(gens: any[], where: ((v: any[]) => boolean) | null, body: (v: any[]) => Promise<void>, vals: any[]): Promise<any[]> {
  let current = vals.slice();
  let improved = true;
  let budget = 400;
  while (improved && budget > 0) {
    improved = false;
    for (let i = 0; i < gens.length; i++) {
      const cands = gens[i].shrink(current[i]);
      for (const c of cands) {
        if (--budget <= 0) break;
        const trial = current.slice();
        trial[i] = c;
        if (where && !where(trial)) continue;
        let failed = false;
        try { await body(trial); } catch (e) { failed = __bynkIsFailure(e); }
        if (failed) { current = trial; improved = true; break; }
      }
    }
  }
  return current;
}
export async function __bynkRunProperty(spec: { seed: number, cases: number, gens: any[], where: ((v: any[]) => boolean) | null, body: (v: any[]) => Promise<void>, name: string, location: string, file: string }): Promise<{ pass: boolean, error?: { message: string, location: string, violation?: { kind: string, agent: string, name: string, location: string } }, kind?: string, seed?: string, counterexample?: string }> {
  const rng = __bynkRng(spec.seed);
  const maxAttempts = spec.cases * 25 + 50;
  let ran = 0, attempts = 0;
  while (ran < spec.cases && attempts < maxAttempts) {
    const bi = attempts;
    attempts++;
    const vals = spec.gens.map((g) => (bi < g.boundaries.length ? g.boundaries[bi] : g.gen(rng)));
    if (spec.where && !spec.where(vals)) continue;
    ran++;
    try {
      await spec.body(vals);
    } catch (e) {
      if (!__bynkIsFailure(e)) {
        // A throw that is not an assertion is as seed-dependent as one that is,
        // so it reports the seed too; there is nothing shrunk to show.
        return { pass: false, error: { message: String(e), location: spec.location }, kind: "property", seed: "0x" + (__bynkSeed >>> 0).toString(16) };
      }
      const shrunk = await __bynkShrink(spec.gens, spec.where, spec.body, vals);
      // Report the run's *root* seed — each property derives its own from it via
      // `__bynkMix`, so `--seed <root>` reproduces the whole run byte-for-byte.
      const seedHex = "0x" + (__bynkSeed >>> 0).toString(16);
      const shown = spec.gens.map((g, i) => `${g.name} = ${g.show(shrunk[i])}`).join(", ");
      let detail = e.message;
      // The shrunk run's failing `expect` carries its own `path:line:col`; report
      // that so the failure is navigable, not the bare file path of the property.
      let at = (e as any).location ?? spec.location;
      // The violation the shrunk run reports, so the declaration named is the one the
      // counterexample actually violates (#2272).
      let violation = __bynkViolationOf(e);
      try { await spec.body(shrunk); } catch (e2) { if (__bynkIsFailure(e2)) { detail = (e2 as any).message; at = (e2 as any).location ?? spec.location; violation = __bynkViolationOf(e2); } }
      const firstLine = String(detail).split("\n")[0];
      const message = `property failed after ${ran} cases (seed ${seedHex})\n  shrunk counterexample:  ${shown}\n  ${firstLine}\n  reproduce: bynk test ${spec.file} --seed ${seedHex}`;
      return { pass: false, error: { message, location: at, violation }, kind: "property", seed: seedHex, counterexample: shown };
    }
  }
  return { pass: true };
}
