// Slice 2 (ADR 0104) CDP harness — driven by tests/debug_inspect.rs.
//
// Emulates what vscode-js-debug/pwa-node does for a `.bynk` breakpoint: launch the
// emitted test entry under `node --inspect-brk`, map the requested `.bynk` line to
// its generated `.ts` line through the source map, set a breakpoint there, and
// confirm it binds and pauses at the matching generated location. Prints "BIND OK"
// and exits 0 on success; exits 1 otherwise.
//
// Usage: node debug_attach.mjs <entry.ts> <file.ts.map> <bynk_line>

import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";

const [, , entry, mapPath, bynkLineArg] = process.argv;
const bynkLine = Number(bynkLineArg);

// Decode the map: generated line (1-based) -> source line (1-based).
const map = JSON.parse(readFileSync(mapPath, "utf8"));
const B = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const dec = (s) => {
  const o = []; let sh = 0, v = 0;
  for (const c of s) { const d = B.indexOf(c); v += (d & 31) << sh; if (d & 32) sh += 5; else { o.push(v & 1 ? -(v >> 1) : v >> 1); v = 0; sh = 0; } }
  return o;
};
const genToSrc = {};
{ let sl = 0; map.mappings.split(";").forEach((seg, i) => { if (!seg) return; sl += dec(seg)[2]; genToSrc[i + 1] = sl + 1; }); }

// The generated line whose source line is the requested `.bynk` line.
const genLine = Object.keys(genToSrc).map(Number).find((g) => genToSrc[g] === bynkLine);
if (!genLine) { console.error(`no generated line maps to ${map.file}:.bynk line ${bynkLine}`); process.exit(1); }
// Escape every regex metacharacter (backslashes included) before matching the
// generated file name as a URL suffix.
const escapeRegex = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const fileRegex = `${escapeRegex(map.file || "")}$`;
console.log(`[setup] breakpoint .bynk:${bynkLine} -> ${map.file}:${genLine}`);

const child = spawn("node", ["--experimental-strip-types", "--inspect-brk=0", entry],
  { stdio: ["ignore", "ignore", "pipe"] });
const wsUrl = await new Promise((res, rej) => {
  const t = setTimeout(() => rej(new Error("node never printed an inspector URL")), 10000);
  child.stderr.on("data", (b) => { const m = String(b).match(/ws:\/\/[^\s]+/); if (m) { clearTimeout(t); res(m[0]); } });
}).catch((e) => { console.error(e.message); process.exit(1); });

const ws = new WebSocket(wsUrl);
let nextId = 1; const pending = new Map();
const pausedQ = []; let pausedW = null;
const onPaused = (p) => { if (pausedW) { const w = pausedW; pausedW = null; w(p); } else pausedQ.push(p); };
const nextPaused = () => new Promise((res) => { if (pausedQ.length) res(pausedQ.shift()); else pausedW = res; });
let resolvedAt = null;
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); }
  else if (m.method === "Debugger.paused") onPaused(m.params);
  else if (m.method === "Debugger.breakpointResolved") resolvedAt = m.params.location.lineNumber + 1;
});
const send = (method, params = {}) => { const id = nextId++; ws.send(JSON.stringify({ id, method, params })); return new Promise((r) => pending.set(id, r)); };
await new Promise((r) => ws.addEventListener("open", r));

await send("Runtime.enable");
await send("Debugger.enable");
await send("Debugger.setBreakpointByUrl", { lineNumber: genLine - 1, urlRegex: fileRegex });

const initial = nextPaused();
await send("Runtime.runIfWaitingForDebugger");
await initial;                 // --inspect-brk entry pause
await send("Debugger.resume"); // run to the breakpoint

const hit = await Promise.race([
  nextPaused(),
  new Promise((_, rej) => setTimeout(() => rej(new Error("never paused at the breakpoint")), 10000)),
]).catch((e) => { console.error(e.message); process.exit(1); });

const stoppedGen = hit.callFrames[0].location.lineNumber + 1;
const stoppedSrc = genToSrc[stoppedGen];
console.log(`[bind] breakpointResolved at ${map.file}:${resolvedAt}`);
console.log(`[hit]  paused at ${map.file}:${stoppedGen} -> .bynk:${stoppedSrc}`);

await send("Debugger.resume").catch(() => {});
child.kill();

const ok = stoppedGen === genLine && stoppedSrc === bynkLine && hit.reason !== "exception";
if (ok) { console.log(`BIND OK: .bynk:${bynkLine} -> ${map.file}:${genLine} -> paused -> .bynk:${stoppedSrc}`); process.exit(0); }
console.error(`BIND FAIL: expected ${map.file}:${genLine}/.bynk:${bynkLine}, got ${map.file}:${stoppedGen}/.bynk:${stoppedSrc}`);
process.exit(1);
