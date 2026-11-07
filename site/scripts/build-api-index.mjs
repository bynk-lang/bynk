// @ts-check
//! Write the top-level landing page for the generated Rust API docs.
//!
//! `cargo doc --no-deps --workspace` produces one `target/doc/<crate>/index.html`
//! per crate but NO workspace-level landing (documentation track follow-on). After
//! the CI deploy copies that tree into `site/public/docs/api/`, this script writes
//! an `index.html` there: a curated list of the workspace crates, each linking to
//! its rustdoc, with the description read straight from its `Cargo.toml` (the single
//! source of truth, so the landing can't drift from the crates).
//!
//!   node scripts/build-api-index.mjs <api-dir>
//!
//! <api-dir> is the directory holding the copied rustdoc (…/public/docs/api). The
//! script is a no-op-safe generator: it only reads Cargo manifests and writes the
//! one index.html. Run in the deploy workflow, after cargo doc + copy.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.join(HERE, "../.."); // site/scripts → repo root

const apiDir = process.argv[2];
if (!apiDir) {
  console.error("usage: node scripts/build-api-index.mjs <api-dir>");
  process.exit(2);
}

/** The workspace member dirs, from the root Cargo.toml `members = [ … ]` array. */
function workspaceMembers() {
  const toml = fs.readFileSync(path.join(REPO, "Cargo.toml"), "utf8");
  const block = /members\s*=\s*\[([^\]]*)\]/s.exec(toml);
  if (!block) throw new Error("could not find [workspace] members in Cargo.toml");
  return [...block[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

/** A crate's package name, description, and optional `[[bin]]` name, from its Cargo.toml. */
function crateMeta(dir) {
  const toml = fs.readFileSync(path.join(REPO, dir, "Cargo.toml"), "utf8");
  const name = /^\s*name\s*=\s*"([^"]+)"/m.exec(toml)?.[1] ?? dir;
  const description = /^\s*description\s*=\s*"([^"]+)"/m.exec(toml)?.[1] ?? "";
  const bin = /\[\[bin\]\][\s\S]*?name\s*=\s*"([^"]+)"/.exec(toml)?.[1];
  return { name, description, bin };
}

/** The rustdoc output dir for a crate: `<name>` underscored, or its bin name
 *  (a bin-only crate like bynk-lsp documents under its binary, bynk-lsp). */
function docDirFor(meta) {
  const candidates = [meta.name, meta.bin].filter(Boolean).map((n) => n.replace(/-/g, "_"));
  return candidates.find((d) => fs.existsSync(path.join(apiDir, d, "index.html"))) ?? null;
}

function escapeHtml(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

const crates = [];
for (const dir of workspaceMembers()) {
  const meta = crateMeta(dir);
  const docDir = docDirFor(meta);
  if (!docDir) {
    console.warn(`build-api-index: no rustdoc output for ${meta.name} — skipping`);
    continue;
  }
  crates.push({ ...meta, docDir });
}
if (crates.length === 0) throw new Error("build-api-index: no crate docs found under " + apiDir);

const cards = crates
  .map(
    (c) => `      <li>
        <a href="./${c.docDir}/index.html"><code>${escapeHtml(c.name)}</code></a>
        <p>${escapeHtml(c.description)}</p>
      </li>`,
  )
  .join("\n");

const html = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Bynk — Rust crate API docs</title>
    <meta name="robots" content="index,follow" />
    <style>
      :root { color-scheme: light dark; --fg: #1b1b1f; --muted: #55555f; --bg: #fff; --accent: #4f46e5; --card: #f6f6f8; }
      @media (prefers-color-scheme: dark) { :root { --fg: #e6e6ea; --muted: #a0a0ad; --bg: #17171b; --accent: #a5a1ff; --card: #202028; } }
      * { box-sizing: border-box; }
      body { margin: 0; padding: 2.5rem 1.25rem; background: var(--bg); color: var(--fg);
        font: 16px/1.6 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }
      main { max-width: 60rem; margin: 0 auto; }
      h1 { font-size: 1.9rem; margin: 0 0 .35rem; }
      .lede { color: var(--muted); margin: 0 0 2rem; max-width: 44rem; }
      .lede a { color: var(--accent); }
      ul { list-style: none; margin: 0; padding: 0; display: grid; gap: .75rem;
        grid-template-columns: repeat(auto-fill, minmax(17rem, 1fr)); }
      li { background: var(--card); border-radius: .6rem; padding: 1rem 1.1rem; }
      li a { color: var(--accent); text-decoration: none; font-size: 1.05rem; }
      li a:hover { text-decoration: underline; }
      li code { font-family: ui-monospace, "SF Mono", Menlo, monospace; }
      li p { margin: .4rem 0 0; color: var(--muted); font-size: .9rem; }
      footer { margin-top: 2.5rem; color: var(--muted); font-size: .85rem; }
      footer a { color: var(--accent); }
    </style>
  </head>
  <body>
    <main>
      <h1>Bynk — Rust crate API docs</h1>
      <p class="lede">
        Generated <code>rustdoc</code> for the Bynk workspace crates — the compiler
        internals behind the <a href="/docs/">toolchain</a>. Most users compile Bynk
        through the <code>bynk</code> CLI rather than depending on
        these crates directly. Back to the <a href="/docs/">Developer Documentation</a>.
      </p>
      <ul>
${cards}
      </ul>
      <footer>Generated from the workspace at build time · <a href="https://github.com/bynk-lang/bynk">github.com/bynk-lang/bynk</a></footer>
    </main>
  </body>
</html>
`;

fs.writeFileSync(path.join(apiDir, "index.html"), html);
console.log(`wrote ${path.join(apiDir, "index.html")} — ${crates.length} crates listed.`);
