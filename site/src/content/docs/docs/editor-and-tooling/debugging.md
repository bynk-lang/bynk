---
title: Debug in VS Code
---
**Goal:** set a breakpoint in a `.bynk` file, press **Debug**, and step through
your code as it runs — under Node (tests) or `workerd` (the dev server).

The `vscode-bynk` extension delegates to VS Code's built-in JavaScript debugger:
it compiles your project, starts the V8 inspector, and attaches automatically.
Breakpoints set in `.bynk` sources resolve through the emitted source maps to the
exact statement, so you debug *Bynk*, not the generated TypeScript.

## Prerequisites

- The `vscode-bynk` extension installed (see [Set up editor
  support](/docs/editor-and-tooling/editor-support/)).
- `bynkc` and `bynk` on your `PATH` (or set `bynk.compilerPath` / `bynk.bynkPath`).
- **Node ≥ 22.6** for the test path (the emitted `.ts` runs under Node's
  type-stripping). For the dev-server path, the same toolchain
  [`bynk dev`](/docs/bynk-cli/#bynk-dev) needs (`wrangler`).

## Debug a test

1. Open a `.bynk` test file and click in the gutter to set a breakpoint — on a
   test-body statement, or in the production code the test exercises.
2. Run or debug it from the editor: each test shows a **`▷ Run Test | Debug Test`**
   CodeLens (and the native run/debug glyphs in the gutter) — click **Debug Test**.
   You can also use the **Testing** view's **Debug** action or profile.
3. The extension shells `bynkc test --inspect`, attaches, and execution pauses at
   your breakpoint. Step, inspect variables, and continue as usual.

The whole suite runs under the debugger; your breakpoint decides where it stops.
The Bynk runtime and generated glue are skip-stepped, so stepping stays inside
your code.

## Debug the dev server

1. Set a breakpoint in a `.bynk` handler (e.g. a `service`/`agent` request
   handler).
2. Create a `launch.json` (Run → Add Configuration → **Bynk**) — or use the
   snippet **Bynk: Debug dev server**:

   ```json
   {
     "type": "bynk",
     "request": "launch",
     "name": "Bynk: Debug dev server",
     "mode": "dev"
   }
   ```

3. Press **F5**. The extension runs `bynk dev --inspect`, attaches to
   `wrangler`'s inspector, and your handler **pauses on a real request** — send
   one (e.g. `curl http://127.0.0.1:8787/`) and step through it.

For a multi-context project, `bynk dev --inspect` serves every context and gives
each its own inspector port, counting up from `"port"` (default `9229`) in the
order the contexts are listed — so the launch config attaches to the first, and a
second context's inspector is on `9230`. Set `"context": "<name>"` to serve and
attach to one alone; that context then takes `"port"` itself.

## Values read in Bynk

When you inspect a value, the debugger shows it in Bynk's vocabulary rather than
its emitted shape — under **both** the test (Node) and dev-server (workerd) paths:

| You wrote | The debugger shows |
|---|---|
| `Ok(42)` | `Ok(42)` (not `{tag: "Ok", value: 42}`) |
| `Some("hi")` / `None` | `Some("hi")` / `None` |
| a sum variant | `BadRequest("…")` / `NotFound` |
| nested | `Ok(Some(42))` |

This applies to hovers, the Variables pane, and Watch. Turn it off with
**`bynk.debug.semanticValues: false`** to see the raw emitted shape.

> **A note on deeply-nested values.** On the **dev-server (workerd)** path the
> rendering is reconstructed from the debugger's value preview, which elides deep
> nesting — a nested value may show one level (`Ok(…)`) with the inner value one
> expand away (it renders in Bynk vocabulary too when you open it). The **test**
> path renders nesting fully inline.

## Values beside the code

While the session is paused, each `let` binding and parameter that is in scope at
the stop shows its current value at the end of its line, in the same Bynk
rendering (`let doubled = n + n   doubled = Ok(42)`). The language server supplies
the bindings through `textDocument/inlineValue`, naming each by the variable the
compiler emitted for it, and the debugger resolves the values, so any editor that
speaks LSP and DAP gets them. In VS Code the setting is **`debug.inlineValues`**
(`auto` by default).

Only bindings the stop can see are listed: a `let` further down the function is
not shown until it runs, and bindings of other functions are left alone. A `let`
that re-binds a name already in scope (`let total = total + 1`) is emitted under a
throwaway name that cannot be known from the source, so it shows no inline value;
its value is still in the Variables pane. Test cases (`suite` files) show no
inline values yet; use the Variables pane there. Inline values follow the last saved and
analysed source, like the other editor features.

## The handler frame in Bynk

Stopped inside a handler, the **Variables** pane groups the frame in Bynk's terms
rather than as a flat list of emitted locals: the capabilities a handler consumes
appear together under **`Capabilities`**, and an agent's state under **`State`** — both
sorted to the top, still expandable, with your own bindings and request parameters
listed below. Same toggle (`bynk.debug.semanticValues`).

The **Call Stack** reads in Bynk too: a handler frame is named for its **Bynk
operation** — `GET "/"`, `bump(amount)` — rather than the emitted function (`http_GET`),
while toolchain and runtime frames stay greyed out (skip-stepped). Clicking a frame
still navigates to its `.bynk` line.

The **compiler temporaries** the lowering spills (`__`-prefixed bindings from `?`/`match`)
are filtered out, so stepping shows your bindings rather than the lowering's. Turn the
whole thing off (`bynk.debug.semanticValues: false`) to see them and the raw shapes.

> Surfacing the `by` actor as part of the frame is a planned addition.

## How it works

Per ADR 0104 (the debug-launch model) this is *glue, not a Debug Adapter*: the
extension contributes a `bynk` debug type whose
configuration provider starts the inspector by shelling the
[`--inspect` CLIs](/docs/bynk-cli/) and hands off to VS Code's
JavaScript debugger (`pwa-node`). The source maps — `.bynk` files referenced by
absolute path so an editor breakpoint resolves to the same source the debugger
loads — do the breakpoint relocation. You can also run the CLIs directly and
attach any JavaScript debugger by hand; the one-click flow just automates that.

## Troubleshooting

- **The breakpoint shows as unbound (hollow).** Make sure the project is the
  open workspace folder — the debugger anchors source resolution there. Rebuild
  if the emitted output is stale.
- **`node` not found / wrong version.** The test path needs Node ≥ 22.6 for
  TypeScript type-stripping; check with `bynk doctor`.
- **Nothing happens on the dev path.** Confirm `wrangler` is available
  (`bynk doctor --only deploy`) and that a request actually reaches the handler.
