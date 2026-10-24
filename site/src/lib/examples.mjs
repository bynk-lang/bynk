// Build-time helpers for Bynk by Example (documentation track, slice 4).
//
// The gallery's code is read straight from the real `examples/` projects and the
// `src/snippets/` corpus, so it can never drift from what the compiler gates. For
// runnable snippets we precompute a playground deep link here, at build time —
// `CompressionStream` / `btoa` / `TextEncoder` all exist in Node, so the link is a
// plain <a> with no client-side island.

import fs from "node:fs";
import path from "node:path";

// Locate the repo root by walking up from the build's working directory (the
// `site/` project) to the workspace marker. Robust to Astro bundling the lib —
// `import.meta.url` would point into a build chunk, not the source tree.
function findRepoRoot() {
  let dir = process.cwd();
  for (;;) {
    if (fs.existsSync(path.join(dir, "Cargo.toml")) && fs.existsSync(path.join(dir, "examples"))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) {
      throw new Error("examples.mjs: could not locate the repo root (Cargo.toml + examples/)");
    }
    dir = parent;
  }
}

const REPO_ROOT = findRepoRoot();

const PLAYGROUND_ORIGIN = "https://playground.bynk-lang.dev";
const GITHUB_TREE = "https://github.com/bynk-lang/bynk/tree/main";

/** Read a source file given a path relative to the repo root. */
export function readSource(relUnderRepo) {
  const abs = path.join(REPO_ROOT, relUnderRepo);
  return fs.readFileSync(abs, "utf8").replace(/\n+$/, "") + "\n";
}

/** GitHub link to a full example project directory. */
export function githubProjectLink(name) {
  return `${GITHUB_TREE}/examples/${name}`;
}

// --- The playground deep-link format (ADR 0140), ported verbatim from
// --- src/deeplink.ts in bynk-lang/playground so the link is byte-compatible with
// --- its decoder:
// ---   #<base64url( deflate-raw( utf8(source) ) )>

async function pipeThrough(bytes, stream) {
  const writer = stream.writable.getWriter();
  void writer.write(bytes);
  void writer.close();
  const buf = await new Response(stream.readable).arrayBuffer();
  return new Uint8Array(buf);
}

function toBase64Url(bytes) {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Encode source into a playground deep link (the full URL). */
export async function playgroundLink(source) {
  const utf8 = new TextEncoder().encode(source);
  const deflated = await pipeThrough(utf8, new CompressionStream("deflate-raw"));
  return `${PLAYGROUND_ORIGIN}/#${toBase64Url(deflated)}`;
}
