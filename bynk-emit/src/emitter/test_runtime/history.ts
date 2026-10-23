type __BynkHistoryHandler = { tag: string, gens: Array<{ boundaries: any[], gen: (rng: any) => any, shrink: (v: any) => any[], show: (v: any) => string }> };
type __BynkHistorySpec = { seed: number, cases: number, maxLen: number, handlers: __BynkHistoryHandler[], drive: (seq: any[]) => Promise<any[]>, body: (run: any[]) => Promise<void>, name: string, location: string, file: string };
function __bynkGenHistory(rng: any, spec: __BynkHistorySpec): Array<{ h: number, args: any[] }> {
  const len = Math.floor(rng.next() * (spec.maxLen + 1));
  const seq: Array<{ h: number, args: any[] }> = [];
  for (let i = 0; i < len; i++) {
    const h = spec.handlers.length > 0 ? Math.floor(rng.next() * spec.handlers.length) : 0;
    const args = spec.handlers[h] ? spec.handlers[h].gens.map((g) => g.gen(rng)) : [];
    seq.push({ h, args });
  }
  return seq;
}
async function __bynkHistoryStillFails(spec: __BynkHistorySpec, seq: Array<{ h: number, args: any[] }>): Promise<boolean> {
  let run: any[];
  try { run = await spec.drive(seq); } catch { return false; }
  try { await spec.body(run); return false; } catch (e) { return __bynkIsFailure(e); }
}
async function __bynkShrinkHistory(spec: __BynkHistorySpec, seq: Array<{ h: number, args: any[] }>): Promise<Array<{ h: number, args: any[] }>> {
  let cur = seq.slice();
  let budget = 300;
  // Delta-debug the sequence: drop a step, re-drive, keep the reduction only if it
  // still reproduces the failure (so the printed counterexample stays reachable).
  let improved = true;
  while (improved && budget > 0) {
    improved = false;
    for (let i = 0; i < cur.length && budget > 0; i++) {
      budget--;
      const trial = cur.slice(0, i).concat(cur.slice(i + 1));
      if (await __bynkHistoryStillFails(spec, trial)) { cur = trial; improved = true; break; }
    }
  }
  // Then shrink each surviving step's arguments with the value shrinker.
  improved = true;
  while (improved && budget > 0) {
    improved = false;
    for (let i = 0; i < cur.length; i++) {
      const step = cur[i];
      const gens = spec.handlers[step.h] ? spec.handlers[step.h].gens : [];
      for (let j = 0; j < gens.length; j++) {
        const cands = gens[j].shrink(step.args[j]);
        for (const c of cands) {
          if (--budget <= 0) break;
          const nargs = step.args.slice(); nargs[j] = c;
          const trial = cur.slice(); trial[i] = { h: step.h, args: nargs };
          if (await __bynkHistoryStillFails(spec, trial)) { cur = trial; improved = true; break; }
        }
        if (budget <= 0) break;
      }
    }
  }
  return cur;
}
function __bynkShowHistory(spec: __BynkHistorySpec, seq: Array<{ h: number, args: any[] }>): string {
  return "[" + seq.map((st) => {
    const h = spec.handlers[st.h];
    if (!h) return "?";
    const args = st.args.map((a: any, j: number) => h.gens[j] ? h.gens[j].show(a) : __bynkShow(a)).join(", ");
    return h.tag + "(" + args + ")";
  }).join(", ") + "]";
}
async function __bynkRunHistory(spec: __BynkHistorySpec): Promise<{ pass: boolean, error?: { message: string, location: string }, kind?: string, seed?: string, counterexample?: string }> {
  const rng = __bynkRng(spec.seed);
  for (let c = 0; c < spec.cases; c++) {
    const seq = __bynkGenHistory(rng, spec);
    let run: any[];
    try { run = await spec.drive(seq); } catch (e) {
      return { pass: false, error: { message: String(e), location: spec.location }, kind: "history", seed: "0x" + (__bynkSeed >>> 0).toString(16) };
    }
    try {
      await spec.body(run);
    } catch (e) {
      if (!__bynkIsFailure(e)) {
        return { pass: false, error: { message: String(e), location: spec.location }, kind: "history", seed: "0x" + (__bynkSeed >>> 0).toString(16) };
      }
      const shrunk = await __bynkShrinkHistory(spec, seq);
      const seedHex = "0x" + (__bynkSeed >>> 0).toString(16);
      const shown = __bynkShowHistory(spec, shrunk);
      let detail = (e as any).message;
      let at = (e as any).location ?? spec.location;
      try { const __r2 = await spec.drive(shrunk); await spec.body(__r2); } catch (e2) { if (__bynkIsFailure(e2)) { detail = (e2 as any).message; at = (e2 as any).location ?? spec.location; } }
      const firstLine = String(detail).split("\n")[0];
      const message = `history property failed after ${c + 1} runs (seed ${seedHex})\n  shrunk sequence:  ${shown}\n  ${firstLine}\n  reproduce: bynkc test ${spec.file} --seed ${seedHex}`;
      return { pass: false, error: { message, location: at }, kind: "history", seed: seedHex, counterexample: shown };
    }
  }
  return { pass: true };
}
