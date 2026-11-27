// TEMPORARY diagnostic for #4579: boot `wrangler dev` the way bynk-tests does (stdout and
// stderr to a file, `cmd /C npx` on Windows, its own dev registry, an inspector port), and
// record when the first log line, the first HTTP answer and the `Ready on` line each arrive.
import { spawn, execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";

const [, , label, countArg, ...extra] = process.argv;
const count = Number(countArg || 1);
const win = process.platform === "win32";
const root = fs.mkdtempSync(path.join(os.tmpdir(), `diag-${label}-`));

function project(i) {
  const dir = path.join(root, `w${i}`);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "wrangler.toml"),
    `name = "diag${i}"\nmain = "index.js"\ncompatibility_date = "2025-01-01"\n`);
  fs.writeFileSync(path.join(dir, "index.js"),
    'export default { fetch() { return new Response("ok"); } };\n');
  return dir;
}

async function boot(i) {
  const dir = project(i);
  const port = 18700 + i * 2, inspector = 18701 + i * 2;
  const log = path.join(dir, "wrangler.log");
  const fd = fs.openSync(log, "w");
  const env = { ...process.env, WRANGLER_SEND_METRICS: "false" };
  if (!extra.includes("--no-registry")) env.WRANGLER_REGISTRY_PATH = path.join(dir, "registry");
  const args = ["-y", "wrangler@4", "dev", "--port", String(port), "--inspector-port", String(inspector),
    ...extra.filter((a) => a !== "--no-registry")];
  const child = win
    ? spawn("cmd", ["/C", "npx", ...args], { cwd: dir, env, stdio: ["ignore", fd, fd] })
    : spawn("npx", args, { cwd: dir, env, stdio: ["ignore", fd, fd], detached: true });
  const t0 = Date.now();
  const at = {};
  let seen = 0;
  const lines = [];
  while (Date.now() - t0 < 200_000) {
    const text = fs.readFileSync(log, "utf8");
    const all = text.split("\n");
    for (; seen < all.length - 1; seen++) {
      const s = ((Date.now() - t0) / 1000).toFixed(1);
      lines.push(`+${s}s ${all[seen].slice(0, 180)}`);
      at.firstLine ??= s;
    }
    if (!at.served) {
      try { const r = await fetch(`http://127.0.0.1:${port}/`, { signal: AbortSignal.timeout(400) });
        if (r.ok) at.served = ((Date.now() - t0) / 1000).toFixed(1); } catch {}
    }
    if (!at.ready && text.includes(`Ready on http://localhost:${port}`)) at.ready = ((Date.now() - t0) / 1000).toFixed(1);
    if (at.ready && at.served) break;
    await new Promise((r) => setTimeout(r, 250));
  }
  if (win) { try { execSync(`taskkill /T /F /PID ${child.pid}`, { stdio: "ignore" }); } catch {} }
  else { try { process.kill(-child.pid, "SIGKILL"); } catch {} }
  return { i, ...at, lines };
}

const results = await Promise.all(Array.from({ length: count }, (_, i) => boot(i)));
for (const r of results) {
  console.log(`=== ${label}#${r.i}: first line ${r.firstLine}s, first HTTP 200 ${r.served ?? "never"}s, Ready line ${r.ready ?? "never"}s`);
}
if (count === 1 || process.env.DIAG_ALL_LINES) for (const r of results) console.log(r.lines.join("\n"));
else console.log(results[0].lines.join("\n"));
