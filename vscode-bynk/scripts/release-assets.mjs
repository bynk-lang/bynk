// Download assets of the compiler release `bynkServerVersion` pins, verified
// against the release's SHA256SUMS and cached per pin in `.contract-cache/`.
// The extension needs nothing from the compiler's repository but these: the
// editor contract (check-contract.mjs), the web assets (fetch-web-assets.mjs)
// and the raw binaries (fetch-toolchain.mjs).

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseSums, releaseTarget } from "./contract-lib.mjs";

export const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

export const pin = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")).bynkServerVersion;

const releases = `https://github.com/bynk-lang/bynk/releases/download/${pin}`;
const cache = path.join(root, ".contract-cache", pin);

let table;
/** The release's SHA256SUMS as `{ name: hash }`. */
async function sums() {
  if (table) return table;
  const res = await fetch(`${releases}/SHA256SUMS`);
  if (!res.ok) throw new Error(`cannot fetch ${releases}/SHA256SUMS: HTTP ${res.status}`);
  return (table = parseSums(await res.text()));
}

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

/** Download release asset `name` (verified, cached) and return its local path. */
export async function asset(name) {
  const hash = (await sums())[name];
  if (!hash) throw new Error(`${name} is not in ${pin}'s SHA256SUMS (a pin that predates it?)`);
  const file = path.join(cache, name);
  if (fs.existsSync(file) && sha256(fs.readFileSync(file)) === hash) return file;
  const res = await fetch(`${releases}/${name}`);
  if (!res.ok) throw new Error(`cannot fetch ${releases}/${name}: HTTP ${res.status}`);
  const bytes = Buffer.from(await res.arrayBuffer());
  if (sha256(bytes) !== hash) throw new Error(`${name}: checksum does not match ${pin}'s SHA256SUMS`);
  fs.mkdirSync(cache, { recursive: true });
  fs.writeFileSync(file, bytes, { mode: 0o755 });
  return file;
}

/** The release target triple of this machine (`.exe` suffix on Windows). */
export function target() {
  const triple = releaseTarget(process.platform, process.arch);
  if (!triple) throw new Error(`no prebuilt Bynk binaries for ${process.platform}/${process.arch}`);
  return triple;
}
