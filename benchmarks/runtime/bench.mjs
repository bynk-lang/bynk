// Run-time cost of compiled Bynk against hand-written TypeScript twins.
//
//   node bench.mjs --bynk <path to bynk> [--out file.json | --record <dir>] [--runs 5]
//                  [--requests 1000] [--startups 15] [--sizes-only]
//
// Normally run through `cargo xtask bench-runtime`, which builds a release
// `bynk` and installs the dependencies first. See README.md for the method.
//
// Everything runs locally under workerd (through Miniflare); nothing is
// deployed. Bundle sizes are deterministic. Start-up time and request latency
// are measured, and are reported with their spread, because they are noisy.

import { build } from "esbuild";
import { Log, LogLevel, Miniflare } from "miniflare";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { cpus, platform, release, tmpdir, totalmem } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "../..");

// ---------------------------------------------------------------- arguments

const args = process.argv.slice(2);
function flag(name, fallback) {
  const i = args.indexOf(`--${name}`);
  return i === -1 ? fallback : args[i + 1];
}
const BYNK = resolve(flag("bynk", join(REPO, "target/release/bynk")));
const RECORD_DIR = flag("record", null); // writes <dir>/v<version>.json
let OUT = flag("out", null);
const RUNS = Number(flag("runs", 5)); // independent Miniflare instances per variant
const REQUESTS = Number(flag("requests", 1000)); // timed requests per route per run
const WARMUP = Number(flag("warmup", 200)); // untimed requests before each timed batch
const STARTUPS = Number(flag("startups", 15)); // fresh instances for the start-up figure
const SIZES_ONLY = args.includes("--sizes-only");
if (SIZES_ONLY && flag("record", null)) {
  // A sizes-only run has no latency or start-up figures; recording it would
  // replace the full record for this version with a partial one.
  console.error("bench.mjs: --sizes-only cannot be combined with --record");
  process.exit(2);
}

// ------------------------------------------------------------------ examples
//
// `ctx` is the worker directory `bynk compile --target workers` emits for the
// example's context. `durable` names the Durable Object binding and class, which
// the Bynk build and the twin share. `routes` are the timed requests.

const JSON_HEADERS = { "content-type": "application/json" };
const post = (path, body) => [path, { method: "POST", headers: JSON_HEADERS, body: JSON.stringify(body) }];

// A client may make ten requests in a window before it is limited, so the
// rotating-clients route uses enough clients that none is limited during a run.
const CLIENTS = Math.ceil((WARMUP + REQUESTS) / 10);

const SEED_ORDERS = 100;
const SEED_LINES = 500;

const EXAMPLES = {
  "hello-world": {
    ctx: "hello-web",
    twin: "hello-world.ts",
    durable: null,
    seed: null,
    routes: [
      { name: "GET /", req: () => ["/"], exact: true, status: 200 },
      { name: "GET /hello/:name", req: (i) => [`/hello/user${i % 50}`], exact: true, status: 200 },
      { name: "GET /hello/:name (rejected: 41 characters)", req: () => ["/hello/" + "x".repeat(41)], exact: true, status: 400 },
    ],
  },
  orders: {
    ctx: "orders",
    twin: "orders.ts",
    durable: { BOOK: { className: "Book", useSQLite: true } },
    // 100 orders and 500 lines, so the joins and the grouping have work to do.
    seed: async (fetchOne, lineCount = SEED_LINES) => {
      const created = async (...request) => {
        const status = await fetchOne(...request);
        if (status !== 201) throw new Error(`seeding: ${request[0]} answered ${status}, expected 201`);
      };
      for (let o = 0; o < SEED_ORDERS; o++) await created(...post("/orders", { id: `o${o}`, customer: `customer${o % 10}` }));
      for (let l = 0; l < lineCount; l++) {
        await created(...post("/lines", { id: `l${l}`, orderId: `o${l % SEED_ORDERS}`, sku: `sku${l % 7}`, qty: 1 + (l % 5) }));
      }
    },
    routes: [
      { name: "GET /orders/:id/lines (5 lines, indexed)", req: (i) => [`/orders/o${i % SEED_ORDERS}/lines`], exact: true, status: 200 },
      // Row order differs between the two, so the bodies are compared sorted.
      { name: "GET /totals (group 500 lines)", req: () => ["/totals"], exact: "sorted", status: 200 },
      { name: "GET /sales (join 500 lines to 100 orders)", req: () => ["/sales"], exact: "sorted", status: 200 },
      { name: "POST /lines (accepted)", req: (i) => post("/lines", { id: `n${i}`, orderId: `o${i % SEED_ORDERS}`, sku: "sku0", qty: 1 }), exact: false, status: 201 },
      { name: "POST /lines (rejected: qty 0)", req: (i) => post("/lines", { id: `r${i}`, orderId: "o0", sku: "sku0", qty: 0 }), exact: false, status: 400 },
    ],
  },
  "rate-limiter": {
    ctx: "ratelimit",
    twin: "rate-limiter.ts",
    durable: { LIMITER: { className: "Limiter", useSQLite: true } },
    seed: null,
    routes: [
      // Clients in rotation: each is its own Durable Object, so after the first
      // lap this is a warm object per request.
      { name: `GET /check/:client (${CLIENTS} clients in rotation, all allowed)`, req: (i) => [`/check/client${i % CLIENTS}`], exact: false, status: 200 },
      // One client over its limit: every request after the tenth is a 429.
      { name: "GET /check/:client (one client, limited)", req: () => ["/check/hot"], exact: false, status: 429, prime: 10 },
    ],
  },
};

// A worker that does nothing: the floor under every start-up figure, which
// includes spawning workerd and loading a script.
const EMPTY_WORKER = 'export default { fetch() { return new Response("ok"); } };\n';

// ------------------------------------------------------------------- helpers

function median(xs) {
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}
function percentile(sorted, p) {
  return sorted[Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)];
}
function roundSpread(s) {
  return Object.fromEntries(Object.entries(s).map(([k, v]) => [k, round(v)]));
}
const spread = (xs) => ({ median: median(xs), min: Math.min(...xs), max: Math.max(...xs) });
const round = (x, d = 3) => Math.round(x * 10 ** d) / 10 ** d;
const lines = (path) => readFileSync(path, "utf8").split("\n").length - 1;

function walk(dir, pred, acc = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, pred, acc);
    else if (pred(p)) acc.push(p);
  }
  return acc;
}

const BUNDLE_OPTIONS = {
  bundle: true,
  format: "esm",
  platform: "neutral",
  target: "es2022",
  mainFields: ["module", "main"],
  conditions: ["workerd", "worker", "browser"],
  external: ["cloudflare:*", "node:*"],
  write: false,
  logLevel: "error",
  legalComments: "none",
};

async function bundle(entry) {
  const plain = (await build({ ...BUNDLE_OPTIONS, entryPoints: [entry], minify: false })).outputFiles[0].text;
  const min = (await build({ ...BUNDLE_OPTIONS, entryPoints: [entry], minify: true })).outputFiles[0].text;
  const gz = gzipSync(Buffer.from(min), { level: 9 }).length;
  return {
    script: min,
    sizes: { bundled: Buffer.byteLength(plain), minified: Buffer.byteLength(min), minifiedGzip: gz },
  };
}

function miniflare(script, durable) {
  return new Miniflare({
    modules: true,
    script,
    compatibilityDate: "2026-07-01",
    compatibilityFlags: ["global_fetch_strictly_public"],
    ...(durable ? { durableObjects: durable } : {}),
    // The workers log through `console`; keep the benchmark's output readable.
    log: new Log(LogLevel.NONE),
    handleRuntimeStdio(stdout, stderr) {
      stdout.resume();
      stderr.resume();
    },
  });
}

async function timedFetch(mf, path, init) {
  const t0 = process.hrtime.bigint();
  const res = await mf.dispatchFetch(`http://bench.local${path}`, init);
  await res.arrayBuffer();
  return { ms: Number(process.hrtime.bigint() - t0) / 1e6, status: res.status };
}

// -------------------------------------------------------------------- setup

const workdir = mkdtempSync(join(tmpdir(), "bynk-bench-runtime-"));
process.on("exit", () => rmSync(workdir, { recursive: true, force: true }));

function compileBynk(name) {
  const out = join(workdir, `out-${name}`);
  execFileSync(BYNK, ["compile", join(REPO, "examples", name), "--target", "workers", "-o", out, "--quiet"], { stdio: ["ignore", "ignore", "inherit"] });
  return out;
}

function bynkVersion() {
  const text = execFileSync(BYNK, ["--version"], { encoding: "utf8" }).trim();
  return text.split(/\s+/).pop();
}

// A synthetic single-context project of `groups` groups, each a refined type, a
// record, an agent with one stored cell, and an HTTP route that calls it. Used
// only for the growth measurement.
function syntheticProject(groups) {
  const dir = join(workdir, `growth-${groups}`);
  mkdirSync(join(dir, "src"), { recursive: true });
  writeFileSync(join(dir, "bynk.toml"), '[project]\nname = "growth"\nversion = "0.1.0"\n');
  let src = "context growth\n\n";
  for (let g = 0; g < groups; g++) {
    src += `type Name${g} = String where NonEmpty && MaxLength(40)\n`;
    src += `type Row${g} = {\n\tid: String,\n\tname: Name${g},\n\tcount: Int,\n}\n`;
    src += `agent Counter${g} {\n\tkey id: String\n\tstore total: Cell[Int]\n\ton call bump(delta: Int) -> Effect[Int] {\n\t\ttotal := delta\n\t\tdelta\n\t}\n}\n`;
    src += `service api${g} from http {\n\ton POST("/bump${g}/:id") (id: String, body: Row${g}) -> Effect[HttpResult[Int]] by Visitor {\n\t\tlet n <- Counter${g}(id).bump(body.count)\n\t\tCreated(n)\n\t}\n}\n\n`;
  }
  writeFileSync(join(dir, "src", "growth.bynk"), src);
  return dir;
}

// ---------------------------------------------------------------- measuring

async function measureSizes(name, ex, bynkOut) {
  const twinEntry = join(HERE, "twins", ex.twin);
  const bynkEntry = join(bynkOut, "workers", ex.ctx, "index.ts");
  const bynk = await bundle(bynkEntry);
  const twin = await bundle(twinEntry);
  const emitted = walk(bynkOut, (p) => p.endsWith(".ts"));
  return {
    bynk: {
      ...bynk.sizes,
      emittedTypeScriptLines: emitted.reduce((n, p) => n + lines(p), 0),
      runtimeTypeScriptLines: walk(join(bynkOut, "__bynk_runtime"), (p) => p.endsWith(".ts")).reduce((n, p) => n + lines(p), 0),
    },
    twin: { ...twin.sizes, sourceLines: lines(twinEntry) },
    scripts: { bynk: bynk.script, twin: twin.script },
  };
}

// Start-up: a fresh Miniflare instance, timed from construction to the first
// answered request. Variants are measured in alternation, so slow drift on the
// machine (thermal state, a busy neighbour) lands on all of them equally, and a
// couple of unrecorded rounds come first to warm the disk cache.
async function measureStartups(variants, probePath) {
  const samples = Object.fromEntries(Object.keys(variants).map((k) => [k, []]));
  for (let i = -2; i < STARTUPS; i++) {
    for (const [key, { script, durable }] of Object.entries(variants)) {
      const t0 = process.hrtime.bigint();
      const mf = miniflare(script, durable);
      const { status } = await timedFetch(mf, probePath);
      const ms = Number(process.hrtime.bigint() - t0) / 1e6;
      await mf.dispose();
      if (status >= 500) throw new Error(`start-up probe ${probePath} answered ${status} for ${key}`);
      if (i >= 0) samples[key].push(ms);
    }
  }
  return Object.fromEntries(Object.entries(samples).map(([k, v]) => [k, roundSpread(spread(v))]));
}

// Request latency: each run is a fresh instance, seeded, warmed, then timed one
// request at a time. Runs alternate between the variants (and swap which goes
// first), for the same reason as above. Per run we keep p50 and p99; across runs
// we report the median of those with the range.
async function measureRoutes(ex, scripts) {
  const acc = Object.fromEntries(Object.keys(scripts).map((k) => [k, ex.routes.map(() => ({ p50: [], p99: [] }))]));
  for (let run = 0; run < RUNS; run++) {
    const order = Object.keys(scripts);
    if (run % 2 === 1) order.reverse();
    for (const variant of order) {
      const mf = miniflare(scripts[variant], ex.durable);
      const fetchOne = async (path, init) => (await timedFetch(mf, path, init)).status;
      if (ex.seed) await ex.seed(fetchOne);
      for (let r = 0; r < ex.routes.length; r++) {
        const route = ex.routes[r];
        let counter = run * 1_000_000 + r * 100_000; // distinct ids for mutating routes
        for (let i = 0; i < WARMUP; i++) await timedFetch(mf, ...route.req(counter++));
        const times = [];
        for (let i = 0; i < REQUESTS; i++) {
          const { ms, status } = await timedFetch(mf, ...route.req(counter++));
          // Time only the path the route claims to time.
          if (status !== route.status) throw new Error(`${route.name} (${variant}): answered ${status}, expected ${route.status}`);
          times.push(ms);
        }
        times.sort((x, y) => x - y);
        acc[variant][r].p50.push(percentile(times, 50));
        acc[variant][r].p99.push(percentile(times, 99));
      }
      await mf.dispose();
    }
  }
  return Object.fromEntries(
    Object.entries(acc).map(([variant, perRoute]) => [
      variant,
      ex.routes.map((route, r) => ({ route: route.name, p50Ms: roundSpread(spread(perRoute[r].p50)), p99Ms: roundSpread(spread(perRoute[r].p99)) })),
    ]),
  );
}

// How the cost of one write grows with the state behind it. Bynk loads and
// commits an agent's whole state on every call, so a write to a bigger `orders`
// book costs more; a hand-written object that writes one record does not. This
// times `POST /lines` against books of increasing size.
const SCALING_LINES = [0, 500, 2000];
const SCALING_REQUESTS = 300;
async function measureStateScaling(ex, scripts) {
  const out = [];
  for (const lines of SCALING_LINES) {
    const row = { lines };
    const acc = Object.fromEntries(Object.keys(scripts).map((k) => [k, []]));
    for (let run = 0; run < 3; run++) {
      const order = Object.keys(scripts);
      if (run % 2 === 1) order.reverse();
      for (const variant of order) {
        const mf = miniflare(scripts[variant], ex.durable);
        const fetchOne = async (path, init) => (await timedFetch(mf, path, init)).status;
        await ex.seed(fetchOne, lines);
        let n = run * 100_000;
        const write = () => timedFetch(mf, ...post("/lines", { id: `w${n}`, orderId: `o${n++ % SEED_ORDERS}`, sku: "sku0", qty: 1 }));
        for (let i = 0; i < 50; i++) await write();
        const times = [];
        for (let i = 0; i < SCALING_REQUESTS; i++) {
          const { ms, status } = await write();
          if (status !== 201) throw new Error(`write scaling (${variant}, ${lines} lines): POST /lines answered ${status}, expected 201`);
          times.push(ms);
        }
        times.sort((x, y) => x - y);
        acc[variant].push(percentile(times, 50));
        await mf.dispose();
      }
    }
    for (const [variant, p50s] of Object.entries(acc)) row[variant] = { p50Ms: roundSpread(spread(p50s)) };
    out.push(row);
  }
  return out;
}

// The twin must do the same work. Compare what each answers, route by route,
// on a fresh instance with the same seed; a mismatch means the comparison is not
// fair, so it stops the run.
async function conformance(name, ex, scripts) {
  const answers = {};
  for (const variant of ["bynk", "twin"]) {
    const mf = miniflare(scripts[variant], ex.durable);
    const fetchOne = async (path, init) => (await timedFetch(mf, path, init)).status;
    if (ex.seed) await ex.seed(fetchOne);
    answers[variant] = [];
    for (const route of ex.routes) {
      // A route that needs earlier requests first (a limiter that must be over its
      // limit) gets them, so the request compared is the one that is timed.
      for (let i = 0; i < (route.prime ?? 0); i++) await mf.dispatchFetch(`http://bench.local${route.req(i)[0]}`, route.req(i)[1]).then((r) => r.arrayBuffer());
      const [path, init] = route.req(7);
      const res = await mf.dispatchFetch(`http://bench.local${path}`, init);
      const text = await res.text();
      answers[variant].push({ status: res.status, body: text });
    }
    await mf.dispose();
  }
  ex.routes.forEach((route, i) => {
    const a = answers.bynk[i];
    const b = answers.twin[i];
    if (a.status !== route.status) throw new Error(`${name}: ${route.name}: Bynk answers ${a.status}, expected ${route.status}`);
    if (a.status !== b.status) throw new Error(`${name}: ${route.name}: Bynk answers ${a.status}, the twin ${b.status}`);
    const canon = (body) => (route.exact === "sorted" ? JSON.stringify(JSON.parse(body).map((row) => JSON.stringify(row)).sort()) : body);
    if (route.exact && canon(a.body) !== canon(b.body)) throw new Error(`${name}: ${route.name}: bodies differ\n  bynk: ${a.body}\n  twin: ${b.body}`);
  });
}

// ---------------------------------------------------------------------- main

const version = bynkVersion();
if (RECORD_DIR) OUT = join(REPO, RECORD_DIR, `v${version}.json`);
const report = {
  schema: 1,
  bynkVersion: version,
  recordedAt: new Date().toISOString().slice(0, 10),
  machine: {
    cpu: cpus()[0]?.model ?? "unknown",
    logicalCpus: cpus().length,
    memoryGiB: round(totalmem() / 2 ** 30, 1),
    os: `${platform()} ${release()}`,
    node: process.version,
  },
  tools: {
    miniflare: JSON.parse(readFileSync(join(HERE, "node_modules/miniflare/package.json"), "utf8")).version,
    workerd: JSON.parse(readFileSync(join(HERE, "node_modules/workerd/package.json"), "utf8")).version,
    esbuild: JSON.parse(readFileSync(join(HERE, "node_modules/esbuild/package.json"), "utf8")).version,
  },
  parameters: SIZES_ONLY ? { sizesOnly: true } : { runs: RUNS, requestsPerRoute: REQUESTS, warmupRequests: WARMUP, startupSamples: STARTUPS },
  examples: {},
  growth: [],
  writeScaling: [],
};


const scriptsByExample = {};
for (const [name, ex] of Object.entries(EXAMPLES)) {
  process.stderr.write(`${name}: compiling and bundling\n`);
  const bynkOut = compileBynk(name);
  const measured = await measureSizes(name, ex, bynkOut);
  const entry = { bynk: measured.bynk, twin: measured.twin };
  if (!SIZES_ONLY) {
    process.stderr.write(`${name}: checking the twin matches\n`);
    await conformance(name, ex, measured.scripts);
    const probe = ex.routes[0].req(0)[0];
    process.stderr.write(`${name}: measuring start-up\n`);
    const startups = await measureStartups(
      { bynk: { script: measured.scripts.bynk, durable: ex.durable }, twin: { script: measured.scripts.twin, durable: ex.durable }, floor: { script: EMPTY_WORKER, durable: null } },
      probe,
    );
    entry.bynk.startupMs = startups.bynk;
    entry.twin.startupMs = startups.twin;
    entry.emptyWorkerStartupMs = startups.floor;
    process.stderr.write(`${name}: measuring request latency\n`);
    const routes = await measureRoutes(ex, measured.scripts);
    entry.bynk.routes = routes.bynk;
    entry.twin.routes = routes.twin;
  }
  scriptsByExample[name] = measured.scripts;
  report.examples[name] = entry;
}

if (!SIZES_ONLY) {
  process.stderr.write("orders: measuring how a write scales with state size\n");
  report.writeScaling = await measureStateScaling(EXAMPLES.orders, scriptsByExample.orders);
}

// Growth: one context, K groups of (refined type, record, agent, route).
for (const groups of [1, 5, 10, 25]) {
  const dir = syntheticProject(groups);
  const out = join(workdir, `out-growth-${groups}`);
  execFileSync(BYNK, ["compile", dir, "--target", "workers", "-o", out, "--quiet"], { stdio: ["ignore", "ignore", "inherit"] });
  const { sizes } = await bundle(join(out, "workers", "growth", "index.ts"));
  report.growth.push({ groups, ...sizes });
}

// ------------------------------------------------------------------- output

const kb = (n) => (n / 1024).toFixed(1);
let md = `Bundle size (esbuild, minified, one worker):\n\n| Example | Bynk min | Bynk gzip | Twin min | Twin gzip |\n|---|---|---|---|---|\n`;
for (const [name, e] of Object.entries(report.examples)) {
  md += `| ${name} | ${kb(e.bynk.minified)} KiB | ${kb(e.bynk.minifiedGzip)} KiB | ${kb(e.twin.minified)} KiB | ${kb(e.twin.minifiedGzip)} KiB |\n`;
}
process.stdout.write(md + "\n");
if (!SIZES_ONLY) {
  for (const [name, e] of Object.entries(report.examples)) {
    process.stdout.write(`${name}: start-up median ${e.bynk.startupMs.median} ms (Bynk) vs ${e.twin.startupMs.median} ms (twin); empty worker ${e.emptyWorkerStartupMs.median} ms\n`);
    e.bynk.routes.forEach((r, i) => {
      const t = e.twin.routes[i];
      process.stdout.write(`  ${r.route}: p50 ${r.p50Ms.median} vs ${t.p50Ms.median} ms, p99 ${r.p99Ms.median} vs ${t.p99Ms.median} ms\n`);
    });
  }
}
if (!SIZES_ONLY) {
  for (const w of report.writeScaling) {
    process.stdout.write(`POST /lines with ${w.lines} lines stored: p50 ${w.bynk.p50Ms.median} ms (Bynk) vs ${w.twin.p50Ms.median} ms (twin)\n`);
  }
}
if (OUT) {
  mkdirSync(dirname(resolve(OUT)), { recursive: true });
  writeFileSync(OUT, JSON.stringify(report, null, 2) + "\n");
  process.stderr.write(`wrote ${OUT}\n`);
}
