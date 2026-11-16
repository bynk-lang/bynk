# `bynk dev` request traces — what one request actually did

- **Status:** Accepted direction. The maintainer accepted the proposal with Decisions A–D as recommended and made it a track (Decision E) on [#3580](https://github.com/bynk-lang/bynk/issues/3580), which is the spine, in [this comment](https://github.com/bynk-lang/bynk/issues/3580#issuecomment-6901057222). No language surface.
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md), run GitHub-native per [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md). It qualifies on two axes: it spans several increments, and the cross-Worker identity is unsettled until slice 2.
- **Consumer:** [vscode-bynk#143](https://github.com/bynk-lang/vscode-bynk/issues/143) overlays a request's path on its handler's sequence diagram. That work is not part of this track.
- **Governing records:** [ADR 0152](../decisions/0152-observation-is-auto-recorded-at-the-capability-seam.md) (production carries no test cost; the model for dev-only recording), [ADR 0103](../decisions/0103-source-map-contract.md) (spans), [ADR 0622](../decisions/0622-dev-worker-logs-frames-in-the-driver.md) (`bynk dev` log events and frame rewriting) and [ADR 0505](../decisions/0505-bynk-dev-event-stream.md) (the event list).

## 1. The theme

Nothing in the editor or the CLI shows what one request did. `bynk dev --format json` emits `serving`, `rebuilding`, `rebuild_ok`, `rebuild_failed`, `stopped` and `log`, and nothing per request. Only the toolchain knows which generated call came from which `.bynk` expression, so only the toolchain can record the path exactly.

The track adds one event, `request`, per request a dev Worker serves: its context, route, status and duration, and an ordered list of steps. Each step has a kind, target, duration, outcome and the `.bynk` span that made the call.

## 2. Decisions (settled)

- **A. Where steps are recorded.** At the runtime choke points, with the call-site spans passed in by the emitter. There are no per-statement hooks. The recorder wraps cross-context calls (`callService`), agent calls (`__makeAgent`) and the deps (as the test observation proxy does). Recording only observes; it never changes control flow.
- **B. How the trace leaves the Worker.** As one structured stdout line with a `__bynk_trace:` prefix. `pump_output` in `bynk/src/dev.rs` parses it and re-emits it as `DevEvent::Request`, as it already does for `log`. No new port or endpoint.
- **C. Opt-in and dev-only.** `--trace-events` implies `--format json`. The recording code is emitted only for a dev build that asked for it. Deploy output stays byte-identical, and a golden pins that. Without the flag, dev output is unchanged.
- **D. What a step may carry.** Kinds, targets, spans, timings and outcomes. No argument values. A later `--trace-values` is a separate slice, if wanted.

## 3. Architecture

- **The runtime module.** A new runtime module, `trace.ts`, holds the recorder. ADR 0615 emits a runtime module only when emitted code reaches it, so a build that never imports it is unchanged, including every existing runtime module. The existing `boundary.ts` and `agent.ts` are not edited. The emitter wraps their calls in the dev build instead.
- **The request scope.** A request-scoped recorder, found through `AsyncLocalStorage` (the dev build adds the `nodejs_als` compatibility flag through the ordinary `compatibility_flags` path, so it is merged and de-duplicated with the project's own flags, usually `nodejs_compat`, which already provides it; a deploy build never adds it). The generated `fetch` moves into an inner function, as L7's `__bynkFetch` does, and is wrapped in a function that opens the record, runs the dispatch and flushes when the response is ready, whether it returned or faulted. Work that continues after that, through `waitUntil` or a streamed response body, is out of the record: a step still unsettled at the flush reports `pending`, and a step begun afterwards is dropped.
- **A step.** `step(kind, target, span, thunk)` times the thunk, records its outcome (`ok` or `fault`) and rethrows. The emitter wraps three sites: a cross-context call, an agent method call, and a capability op (through the deps wrapper).
- **Bounds.** At most N steps per request (the first N are kept), then `truncated: true`.
- **The flag is documented, and proven inert.** The `--trace-events` docs name the `nodejs_als` flag. The behavioural fixture (slice 1c) runs its "off" side without the flag, so it also shows that the flag itself changes no outcome.
- **The event shape** is documented in the `bynk dev` JSON reference (`site/src/content/docs/docs/bynk-cli.md`), so a consumer can be built against it.

## 4. Slices

| Slice | Scope | State |
|---|---|---|
| 1a | Emitter and runtime: the `trace.ts` module, the dev-only build option, the wrapped sites, the fetch wrapper, the `__bynk_trace:` line. A golden pins deploy output. | tracked on #3580 |
| 1b | Driver and CLI: `--trace-events`, `DevEvent::Request`, `pump_output` parsing, the build option set by `bynk dev`. | tracked on #3580 |
| 1c | Docs: the JSON reference, the run-locally guide, ADR 0505's event list. A behavioural fixture runs the same suite with recording on and off and compares the outcomes. It runs under real `wrangler dev`/workerd in CI, not only under Node, so `AsyncLocalStorage` is proven there before the track is called done. If it misbehaves under workerd, the work stops and is reported rather than worked around. | tracked on #3580 |
| 2 | Cross-Worker stitching: a request id on each record, carried to the callee on an internal header, so each callee's `request` event names its parent. The caller wraps the Service Binding it hands `callService` (`__traceBinding`), so `boundary.ts` stays untouched; the call step's `request` is the id the callee's record takes. | tracked on #3580 |

Slice 1 covers the handler, capability ops, agent calls and the reply or fault, within one Worker. Slice state lives on the spine, not here.

## 5. Risks

- **A dev build that behaves differently from a deploy build.** Recording observes and never changes control flow. The behavioural fixture runs one suite with recording on and off and compares the outcomes.
- **Overhead on a busy dev server.** Opt-in, and the step count is bounded.
- **Data in logs.** No values (Decision D).

## 6. Retirement

The last slice's PR is the retirement PR: it removes this doc, appends the closing summary to [`../archive/retired-tracks.md`](../archive/retired-tracks.md), and carries `Closes #3580`.
