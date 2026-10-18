# Bynk Playground

The in-browser REPL for Bynk (in-browser track, slice 4 — [ADR 0140](../design/decisions/0140-repl-execution-and-sandbox.md)).
Type Bynk, press **Run**, see it execute — with **no install and no server**: the
compiler runs in the browser as wasm, and the compiled JavaScript runs in a sandbox.

A fully static, client-side app. It deploys to two origins:

- **`playground.bynk-lang.org`** — the app: the editor + the `bynk_compile` wasm.
- **`sandbox.bynk-lang.org`** — the execution document: a sandboxed `<iframe>`
  wrapping a Web Worker. Untrusted code runs **only** here (the safety boundary).

## Layout

| Path | What |
|---|---|
| `src/app.ts` | The app: editor (CodeMirror 6), compile, diagnostics, run, deep-link, Share, examples picker. |
| `src/examples.ts` | The examples gallery — curated, **runnable** in-process snippets (each verified to compile + run); the header picker loads them. |
| `src/sandbox.ts` | The execution document: links the JS graph to blob-URL modules, runs it in a Worker under a wall-clock timeout, posts results back. |
| live diagnostics | A CodeMirror linter (in `src/app.ts`) calls `bynk_analyze` (debounced, on-type) → inline squiggles + gutter. Non-bailing — type errors in a context show live, not only on Run. |
| hover + completion | `src/app.ts` calls `bynk_hover` (the inferred type at the cursor) and `bynk_complete` (context-aware candidates). |
| `src/positions.ts` | UTF-16 (CodeMirror) ↔ UTF-8 byte (`bynk-wasm`) offset mapping, applied at every wasm call; unit-tested by `test/positions.test.mjs` (`npm test`). |
| `src/deeplink.ts` | The shared snippet format: `#base64url(deflate-raw(utf8(source)))`. |
| `src/highlight.ts` | CodeMirror Bynk highlighting (stream-based; see *Highlighting* below). |
| `src/shared.ts` | Origin config + the postMessage protocol. |
| `scripts/build-wasm.sh` | `cargo build --target wasm32 -p bynk-wasm` + `wasm-bindgen` → `src/vendor/`. |
| `scripts/build-grammar.sh` | `tree-sitter build --wasm` → the web-tree-sitter grammar (needs emcc/docker, and `npm install` in `tree-sitter-bynk/` for its CLI). |
| `build.mjs` / `serve.mjs` | esbuild build into `dist/` / a two-port local static server. |

## Build

```sh
cd playground
npm install
npm run build:wasm        # needs the wasm32 target + wasm-bindgen-cli at the Cargo.lock version
npm run build             # esbuild → dist/  (production origins by default)
```

`build:wasm` defaults to a debug wasm (~18 MB, fast to build). For deploy use a small
artefact:

```sh
npm run build:wasm -- --release   # release + wasm-opt -Oz if available
```

## Verify locally

The app and sandbox must be **different origins**. Build for localhost and serve on
two ports:

```sh
BYNK_APP_ORIGIN=http://localhost:8080 BYNK_SANDBOX_ORIGIN=http://localhost:8081 node build.mjs
node serve.mjs        # app → http://localhost:8080 , sandbox → http://localhost:8081
```

Open `http://localhost:8080`, then check:

1. The starter program is shown; **Run** (or ⌘/Ctrl-Enter) logs a line and prints a value.
2. Break the program → an error diagnostic appears with a line:col, and it does not run.
3. A `consumes bynk.cloudflare { … }` program shows *not runnable in-browser* (the platform lock).
4. **Share** puts a link in the address bar — a short `?s=<id>` link from the share service, or a self-contained `#…` link if the service is unavailable; reloading that URL restores the source.
5. An infinite loop (`fn` that never returns) is terminated after the wall-clock budget.

> The core logic (wasm compile, the emitted graph running, the blob-URL linker, the
> deep-link round-trip) is verified in Node; the browser DOM/iframe/Worker flow is
> verified by the steps above.

## Share service (`share/`) — written in Bynk

Short share links (`?s=<id>`) are backed by a **Bynk program** (`playground/share/`)
compiled by `bynkc` to a Cloudflare Worker + KV — dogfooding: the playground that
compiles Bynk has a backend *written* in Bynk. `POST /api/snippets` stores the source
under a random id; `GET /api/snippets/:id` returns it. The `Source` refined type
bounds the body, so oversized/empty payloads are rejected at the boundary (`400`).
Stored snippets expire 30 days after creation (`Kv.putTtl`, issue #398) — a share
link is for showing someone a program, not permanent hosting, so retention is
bounded rather than unbounded KV storage/cost.

The browser calls it **same-origin** (`/api/*` on the app origin) — Bynk's `from http`
emits no CORS headers, so cross-origin would not work; same-origin routing avoids CORS
entirely. If the service is unavailable, Share falls back to the self-contained
`#hash` link, so the playground works without it.

Build + run + verify locally:

```sh
# Compile the Bynk service to a JS Worker:
bynkc compile share --target workers --platform cloudflare --emit js -o /tmp/share-js
# Run it (set wrangler.toml main=index.js + a dummy local KV id):
cd /tmp/share-js/workers/snippets && npx wrangler@4 dev --port 8799 --local
# serve.mjs proxies /api/* → http://localhost:8799 (override with BYNK_SHARE_WORKER),
# so the app at :8080 reaches it same-origin.
curl -X POST :8080/api/snippets -d '{"source":"context x.y\n"}'   # → {"id":"…"}
```

## Deploy (Cloudflare Pages — maintainer ops)

The deploy is CI-automated by `.github/workflows/deploy-playground.yml` — it builds `dist/` (release wasm + grammar + esbuild, with the production origins as the default) and uploads it to two Cloudflare Pages projects with `wrangler pages deploy`. It runs after a **green CI run** on `main` (via `workflow_run`, deploying only when `playground/**`, `bynk-wasm/**`, `tree-sitter-bynk/**`, or the workflow itself changed) and on manual `workflow_dispatch` (Actions tab → "Deploy the playground (Cloudflare Pages)" → Run workflow). The only thing a maintainer does is the one-time account-side setup below — that part cannot be automated from the repo.

**Security note — the app and the sandbox MUST be two distinct origins.** The sandbox origin is the safety boundary defined by [`ADR 0140`](../design/decisions/0140-repl-execution-and-sandbox.md): untrusted snippet code executes only on the opaque sandbox origin and can never reach the app origin's storage. Never collapse them to one project or one domain — doing so dissolves the boundary.

### One-time setup

1. Create two Cloudflare **Pages** projects of type **Direct Upload**: `bynk-playground` (the app) and `bynk-playground-sandbox` (the sandbox). These names are exactly what the workflow's `--project-name` flags target — keep them in sync if you rename either.
2. Attach custom domains: `playground.bynk-lang.org` → `bynk-playground`; `sandbox.bynk-lang.org` → `bynk-playground-sandbox`. Cloudflare's custom-domain flow creates the DNS records for you when the zone is Cloudflare-managed.
3. Create a Cloudflare API token scoped to **Account → Cloudflare Pages → Edit** — nothing broader. Add it as the GitHub repo secret `CLOUDFLARE_API_TOKEN`, and add your account id as `CLOUDFLARE_ACCOUNT_ID`.
4. Trigger the first deploy — push to `main` (the deploy follows its green CI run), or run the workflow manually.

### Green-skip before the secrets exist

Until both secrets are present, the workflow still builds `dist/` and simply skips the upload — it reports a notice rather than failing. So it is green from the very first push, and becomes a real deploy the moment `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` are added.

### Post-deploy verification

Once both origins serve, re-run the checks in this README's "Verify locally" section against the production origins. Confirm the sandbox iframe loads from `https://sandbox.bynk-lang.org` — that is how you know the origin split is real and not just configured.

### Short share links (deferred / optional)

For short `/api/*` share links, deploy the `share/` Bynk Worker: compile with `--emit js`, then `wrangler deploy` with a real KV namespace bound as `KV`, and add a Cloudflare route so `playground.bynk-lang.org/api/*` reaches it — same-origin, so no CORS. Without it, Share falls back to the self-contained `#hash` links, so this is not required for the playground to work.

## Highlighting

Highlighting is **web-tree-sitter** (in-browser track Q4): `tree-sitter-bynk` compiled
to wasm by `scripts/build-grammar.sh`, parsed and queried (`queries/highlights.scm`)
in `src/tshighlight.ts`, with captures rendered as CodeMirror decorations — the same
grammar the editor and CLI use. The grammar wasm build needs **`emcc` or a running
docker daemon**; run `npm run build:grammar` to produce `src/vendor/{tree-sitter,
tree-sitter-bynk}.wasm`.

The editor starts on the lightweight CodeMirror **stream highlighter**
(`src/highlight.ts`) and **swaps to web-tree-sitter once its wasm loads** (via a
Compartment). If the grammar wasm is absent or fails to load, the stream highlighter
stays — so the build (and the deploy) degrade gracefully without it.
