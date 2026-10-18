# End-to-end testing in the language — a project's whole-system scenarios as `system` cases

- **Status:** Settling. Spine issue
  [#2133](https://github.com/bynk-lang/bynk/issues/2133); this doc lands via a
  settling draft PR referencing it (*"Part of #2133"*, never `Closes`). Every
  question in §3 is **open**. Each records the options and the draft's leaning,
  and the settling review closes them. Slice 0 is two bugs found while drafting
  ([#2131](https://github.com/bynk-lang/bynk/issues/2131),
  [#2132](https://github.com/bynk-lang/bynk/issues/2132)). Slices 1–5 are open
  as increment proposals
  ([#2134](https://github.com/bynk-lang/bynk/issues/2134)–[#2138](https://github.com/bynk-lang/bynk/issues/2138)),
  sub-issues of the spine, none yet `accepted`.
- **Realises:** the testing track's "tier is a dial, not a kind" pillar
  ([ADR 0153](../decisions/0153-tier-is-a-dial-on-the-case-header.md)),
  extended to the rung it left outside the language. It also takes up the
  follow-ons the `system`-tier ADRs named and left open: queue-at-`system`
  ([ADR 0207](../decisions/0207-system-tier-http-boundary.md) D1), and a
  `Signature` signer (the note in
  [ADR 0489](../decisions/0489-test-principal-refinement-and-sum.md)).
- **Posture:** feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md), run GitHub-native per
  [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md). It
  qualifies on two axes, multi-increment and an unsettled surface (§2). The
  third, a safety boundary, applies only to slice 5.
- **What it reverses:** ADR 0207's tier ladder places "e2e (Postman, external)
  = real auth ceremony against a deployed instance" outside the language
  (0207 context, and D3's "real-token edge cases … are e2e's job"). The same
  line is repeated in the 0212 entry of
  [`../decisions/README.md`](../decisions/README.md) and the comment at
  `bynk-emit/src/project/tests_emit.rs:1004`. This track argues that line
  should move, and the settling review should treat that as its first question
  (§3.0).

## 1. The theme

A Bynk project's unit, integration and `system` tests are written in Bynk and
run by `bynkc test`. Its end-to-end tests are not.
[`bynk-lang/shop`](https://github.com/bynk-lang/shop) is the worked example:
four contexts wired by Service Bindings and events, with one adapter. Its
`e2e/scenarios.sh` is about 200 lines of Bash run against `bynk dev` by the
[`bynk-lang/bynk-dev`](https://github.com/bynk-lang/bynk-dev) action. It:

- signs HS256 bearer tokens, including an `admin` claim, with `openssl`;
- signs the payment processor's HMAC webhooks: correctly, forged, ten minutes
  stale, and the same notice twice;
- sends requests with `curl` and matches `"<status> <body>"` strings;
- polls the ledger until the events from orders have been counted;
- runs the stale-hold sweeper through Wrangler's `/__scheduled?cron=…&time=…`;
- learns each Worker's URL from `BYNK_DEV_URL_*` variables, after a warm-up loop
  that retries until the first Service Binding call succeeds.

None of it is type-checked against the routes it drives. Renaming a field
shows up as a failed string match, at run time, in CI.

**End state when the track retires:** the shop's `e2e/scenarios.sh` is
deleted. Its scenarios are `system` cases under `tests/`, type-checked against
the routes they call. They run in process under `bynk test`, and the same
bodies run against `bynk dev` or a deployed preview with one runner flag.

### 1.1 What is already true

Most of the script is within reach of the `system` tier already, which drives
each participant's real emitted Worker `fetch` across the real wire
([ADR 0207](../decisions/0207-system-tier-http-boundary.md) D2).

| The script does | The `system` tier on `main` |
| --- | --- |
| Signs a customer's and an admin's bearer token | ✅ `by Customer("alice")` signs the token (0207 D3). `by Admin("root")` also signs the claim the refinement needs ([0489](../decisions/0489-test-principal-refinement-and-sum.md)) |
| `401` without a token, `403` for a customer on an admin route | ✅ `by Nobody` gives `Rejected(Unauthorized)` ([0212](../decisions/0212-system-tier-no-credential.md)). The `403` comes from the real refinement seam |
| `400` naming `$.qty` for a bad body | ✅ `Wire("…")` sends a raw body ([0210](../decisions/0210-system-tier-wire-rejection.md)) |
| Calls across Service Bindings | ✅ Supported. An agent may now call a consumed context too (#1960, closed) |
| Events reach the ledger | ✅ Subscribers and the fan-out are stood up ([0467](../decisions/0467-system-harness-binds-the-worker-bindings.md)), sends are settled ([0478](../decisions/0478-test-harness-settles-sends.md)), and the emitted Worker type-checks (#2027, closed) |
| Chooses the fake payment processor with `PAYMENTS_PROCESSOR=fake` | ❌ A suite can't set a Worker variable, and `stub` is refused at `system` (`bynk.stub.at_system`) → §3.1, slice 1 |
| Sends the webhook signed, forged and stale | ❌ No signer for `Signature` routes, and the call isn't refused either (#2132) → §3.2, slice 2 |
| Runs the sweeper "an hour from now" | ❌ `cron` and `queue` can't be driven at `system`, and the address emits `/* unknown */` when the target also has an `http` service (#2131) → §3.3, slice 3 |
| Moves time on between calls | ❌ `Clock` is the real `Date.now()` at `system` → §3.4, slice 4 |
| Runs against `bynk dev` (workerd, Wrangler's bundle) | ❌ The tier runs only in an in-process Node harness → §3.5–§3.8, slice 5 |

## 2. Why a track (the ADR 0076 trigger)

- [x] **Multi-increment.** Slices 1–4 each close one gap in the `system` tier,
  have their own forks and fixtures, are useful alone, and can land in any
  order. Slice 5 is a different kind of change: a transport and a runner, not
  a tier feature. No single delete-on-merge proposal can carry all of it.
- [x] **Surface not yet settled.** Where a test's configuration lives (§3.1).
  Whether the harness may own time where `stub` is refused (§3.4). How a case
  waits for asynchronous effects, given
  [ADR 0155](../decisions/0155-history-properties-are-runner-only.md) D2's
  refusal of an `eventually` keyword (§3.6). Whether "against a real runtime"
  is a tier or a run (§3.5). None has a precedent that settles it.
- [ ] **Security/safety boundary.** Not for slices 0–4. They are test-build-only
  ([ADR 0147](../decisions/0147-structural-test-ness-and-flat-paths.md) strips
  every `suite`) and sign with fixed test secrets. Slice 5 has a narrower one
  (§6): a runner that signs real tokens with a deployment's secret.

## 3. Open design questions

### 3.0 — Should end-to-end testing move into the language at all? — OPEN (the gating question)

ADR 0207 drew the line deliberately: the `system` tier proves the deployable
Worker in process, and the real ceremony against a deployed instance is
external. Three arguments for moving the line:

1. **Promotion.** ADR 0153's commitment is one body across tiers. Today an e2e
   scenario has to be rewritten in another language, losing the types, the
   signer, the decoded `HttpResult` and the route checking the lower tiers
   give.
2. **Most of it is already built.** §1.1 shows the `system` tier covers
   everything in the shop's script except configuration, the webhook signer,
   cron and queue, time, and the transport. The first four are gaps the
   `system` ADRs named as follow-ons anyway.
3. **Every project otherwise hand-rolls the same thing.** That means token
   signing, HMAC over `<ts>.<body>`, port guessing and readiness loops, and the
   shop's script shows each is easy to get subtly wrong.

The argument against: a remote run brings flakiness, environment and
secret-handling into `bynk test`, which is today deterministic by construction
([ADR 0431](../decisions/0431-deterministic-platform-test-doubles.md)).

**Draft leaning:** move the line. Contain the non-determinism by keeping it
opt-in and separately reported (§3.5, §3.6), so the in-process tier stays the
default and the gate. Slices 0–4 are worth landing even if the review rejects
slice 5. They make the `system` tier honest and complete on its own terms.

### 3.1 — Where a test's configuration lives — OPEN (proposal #2134)

A `system` suite can't say what its participants' variables are. Today the
harness `env` holds only bindings and call keys
(`emit_integration_harness`, `tests_emit.rs:1699-1834`). The Bearer secret is
set in `process.env` as the fixed `"__bynk_test_secret"` (`:1515-1540`).
`SecretsProvider.get` reads `env[name]` and falls back to `process.env`
(`bynk-std/lib/core/bindings/bynk-cloudflare.ts:139-157`). `bynk.toml` has
no `[test]` table (`bynk-project/src/paths.rs:138-153`).

Options:

- **(a) In source:** an `env { NAME = "value" }` clause in a suite or case.
- **(b) In the manifest:** a `[test.env]` table.
- **(c) Both**, with source layered over manifest.

The draft leans to (a). The configuration is part of the scenario ("with the
fake processor, a charge over 1,000.00 is declined"), so it belongs next to the
case that depends on it. It also applies at every tier: at
`unit`/`integration` it is what the `Secrets` double answers. It is not a
`stub`, so it is legal at `system`.

Sub-questions for the review:

- one map for all participants, or one per context;
- whether an `env` entry for an actor's declared secret replaces the fixed
  test secret (the draft says yes, because slice 5 needs it);
- the keyword (`env`, `config`, `secrets`).

The manifest table is deferred to §3.5, where a remote run may need a
project-wide default.

### 3.2 — Signing `Signature` routes, and the failure principals — OPEN (proposal #2135)

A `Signature` actor declares everything a signer needs: `secret`, `header`,
`timestamp`, `tolerance` (`SignatureSeam`, `bynk-check/src/actors.rs:449-480`).
The verifier is `verifySignatureHmacSha256` (`bynk-emit/runtime/src/auth.ts:92`),
called from the Worker entry (`workers_entry.rs:2294-2400`).

The draft leans to:

- **The driver signs every call to such a route at `system`.** No `by` is
  needed, because the actor has no identity.
- **Three reserved principals for the failure cases:**
  - `by Nobody`, relaxed to `Signature` routes, sends no signature. ADR 0212
    refused it there only because no driver existed.
  - `by Forged` signs with a wrong secret.
  - `by Stale` signs with a timestamp `tolerance + 60` seconds old, and is
    refused on an actor with no `timestamp`.

  Each yields `Rejected(Unauthorized)` on a `401`, as in ADR 0212 D2.
- **What is signed** is exactly the bytes sent, so a `Wire` body is signed raw.

Open: reserved names versus a single `by Signed(secret = …, at = …)` with
explicit knobs. The knobs are more general, but every case would have to
restate what the actor already declares.

Slice 0's #2132 is the stopgap: refuse the call at `system` until the signer
lands, per ADR 0212 D1's "meaning is only given where it is implemented".

### 3.3 — Driving `cron` and `queue` through the Worker — OPEN (proposal #2136)

Today:

- `system_needs_wire` (`bynk-check/src/test_suites.rs:549-568`) keeps a
  cron-only or queue-only, single-context target out of `system`.
- The unit-tier addresses `svc.schedule("<expr>", at)` and `q.message(m)`
  (`bynk-check/src/checker/calls.rs:3320-3403`) lower in process
  (`lower.rs:2602-2633`).
- The Worker's `scheduled` and `queue` entries are emitted at
  `workers_entry.rs:1562` and `:1638`.

The draft leans to:

- **`queue` is admitted to `system` as a serialisation edge**, because a
  message is serialised by the platform. **`cron` is not**: a scheduled event
  carries only a string and a time. A cron address is still legal in a suite
  that already qualifies, which is the shop's case.
- **The addresses lower to drivers** that call `worker.scheduled` and
  `worker.queue` with platform-shaped events and settle the execution context
  (ADR 0478).

Open: **what the address yields**. Promotion wants the handler's own result,
so `expect r is Ok(_)` reads the same at every tier.

- For a queue, the batch double can record `ack()` / `retry()`.
- For cron, the Worker's `scheduled` returns `void`. The draft proposes the
  test build's entry store the handler's result on the execution-context
  double the harness already supplies. The alternative is to yield `()` at
  `system` and refuse an `expect` on it, which is simpler but breaks
  promotion.

Slice 0's #2131 is the stopgap: refuse the address at `system` until the
drivers land.

### 3.4 — May the harness own time at `system`? — OPEN (proposal #2137)

At `system`, `Clock.now` is the real `Date.now()` (`bynk-cloudflare.ts:9-13`).
At `unit`/`integration` the double reads the epoch (ADR 0431 §1). So a
time-dependent case changes meaning, and becomes non-deterministic, when it is
promoted. `stub Clock.now` is refused at `system`.

The draft leans to a harness clock:

- **It is not a `stub`.** It is part of the environment the harness provides,
  as the in-memory KV and Durable Object namespaces of ADR 0467 are.
- **It starts at the epoch**, matching the lower tiers.
- **It moves only on `advance Clock by <Duration>`**, which is also legal at
  `unit`/`integration`, where it moves the double.
- **It governs the `Clock` capability only.** The runtime's own seams (JWT
  `exp`, the `Signature` tolerance, Durable Object alarms) keep wall-clock
  time.

Open:

- whether "the harness's environment" is a principled exception to
  `bynk.stub.at_system` or a loophole in it;
- how a test build hands the clock to each participant's `compose` without the
  injection point reaching the deploy output.

**This is the track's lowest priority.** The shop doesn't need it, because its
sweeper takes the scheduled time as a parameter, so slice 3 alone tests "an
hour from now". It matters for handlers that read `Clock.now` between two
calls.

### 3.5 — A real runtime: a tier or a run? — OPEN (proposal #2138)

Today:

- The runner (`bynk-driver/src/test_runner.rs`, `TestArgs` at `:77-121`) has
  no transport or tier flag. It runs `tsc`, then
  `node out-js/tests/main.js` (`:400-475`).
- Each system driver builds a concrete
  ``new Request(`https://test${path}`, …)`` (`tests_emit.rs:1255`) and calls
  `__h.env.<binding>.fetch` (`sysdrive_driver`, `:7032-7107`). Only that hop
  is in process.

Options:

- **(a) A fourth tier, `as e2e`.** This marks the case.
- **(b) A runner flag over `system` cases**, `bynk test --against dev|<target>`.

The draft leans to (b). Where the Workers run is a property of the run, like
`--target`, not of the case, and a tier would put the same body in two places.
Under (b), `bynk test --against dev` starts `bynk dev` with the suites'
configuration, waits for it, runs the suites, and stops it. A named target in a
`[test.targets]` table runs against deployed Workers.

Open:

- the readiness signal: `bynk dev` prints only a human table to stderr
  (`serving_report`, `bynk/src/dev.rs:991`); the draft proposes
  `--ready-json <file>`;
- how suites with different `env` (§3.1) share one `bynk dev`: the draft
  proposes one `bynk dev` per distinct configuration;
- whether `bynk-lang/bynk-dev` becomes a thin wrapper over `--against dev`.

### 3.6 — Waiting for asynchronous effects against a real runtime — OPEN (proposal #2138)

In process, ADR 0478 settles `~>` sends, and ADR 0467's fan-out can be settled
to quiescence, so a case never waits. Against a real runtime it has to: the
shop polls `/totals` for up to twenty seconds.

ADR 0155 D2 rejected `eventually`/`always` keywords, for history properties
over generated runs. Is a bounded wait for a real system's effects a different
thing?

The draft leans to **no new keyword**. Under `--against`, the runner retries a
failing `expect` up to a deadline (`--settle 5s`, default `0` in process),
re-running the addresses between the last mutating call and the `expect`. A
case body therefore stays exactly the in-process one. Retries are reported per
case, so an ordering bug can't hide behind them.

Open: whether "re-run the reads since the last write" can be determined
statically from a case body, and the prior art (Playwright's auto-retrying
assertions are the nearest). This is the fork most likely to change under
review.

### 3.7 — What a remote run cannot honour — OPEN (proposal #2138)

These have no meaning against a real deployment:

- reading an agent directly;
- addressing an `on call` service, which is internal and, once released,
  signed per pair (#2004, #2012);
- `advance Clock` (§3.4);
- `svc.event(…)` ([ADR 0495](../decisions/0495-test-event-address.md));
- expecting fresh state per case.

Options:

- **(a)** The checker refuses such cases when a remote run is requested.
- **(b)** The runner skips them with a reason.
- **(c)** They are made inexpressible.

The draft leans to (a), with (b) behind an explicit `--skip-unsupported`.

For state, the draft proposes a `run.id` string in case bodies. In process it
is a fixed value, so existing goldens don't move. A remote run gets a fresh one,
so `orderIdFrom("o1-\(run.id)")` doesn't collide between runs. The alternative,
the runner silently prefixing path parameters, breaks on ids inside bodies.

### 3.8 — Keeping a remote run away from production — OPEN (proposal #2138)

See §6. The draft leans to:

- refusing any URL not named under `[test.targets]`;
- refusing a target marked `production = true`;
- reading secrets only from the environment, never from `bynk.toml`;
- printing the target and its URLs before the first request.

## 4. Candidate slice decomposition

Provisional until this doc settles. Slices 1–4 are independent and can land in
any order. Slice 5 depends on slice 1, because a remote run must give the real
Workers the secrets it signs with.

0. **Honesty fixes (bugs).** Make check and emit agree on what `system` can't
   do yet:
   - #2131: a cron or queue address at `system` passes check and emits
     `/* unknown */`.
   - #2132: a `Signature` route at `system` passes check and can only fail.

   Each is a diagnostic until its slice lands.
1. **Suite configuration** (#2134): §3.1.
2. **A `Signature` signer at `system`** (#2135): §3.2. It lifts #2132's
   stopgap.
3. **`cron` and `queue` at `system`** (#2136): §3.3. It lifts #2131's stopgap.
4. **A harness clock** (#2137): §3.4.
5. **Run against a real runtime** (#2138): §3.5–§3.8.

**Acceptance for the track:** port `bynk-lang/shop`'s scenarios. After slices
0–3, every scenario but the transport is a `system` case. After slice 5,
`e2e/scenarios.sh` and the `bynk-dev` step in its `e2e.yml` are deleted.

## 5. Front-loaded ADR candidates

These are load-bearing and hard to reverse, so they should land with, or just
before, the slice that first depends on them:

- **§3.0: end-to-end testing is the language's.** This supersedes ADR 0207's
  "e2e is external" line. It lands with the settling of this doc, or with
  slice 5 if the review prefers to defer it.
- **§3.1: where a test's configuration lives**, because the remote transport
  must read the same thing. It lands with slice 1.
- **§3.4: whether the harness may own time at `system`**, and how that squares
  with `bynk.stub.at_system`. It lands with slice 4.
- **§3.5/§3.7: tier versus run, and the rule for what a remote run refuses.**
  It lands with slice 5.

## 6. Threat model

**Slices 0–4:** none. They are test-build-only (ADR 0147). They sign with fixed
test secrets that exist only in the test output, and they reach nothing outside
the process.

**Slice 5:**

- **Assets:** a real deployment's signing secrets (`AUTH_JWT_SECRET`, webhook
  secrets) and its data.
- **Adversary:** mostly accident. Examples:
  - a CI job or a developer pointing `--against` at production;
  - a scenario that places real orders or takes real payments;
  - a secret committed to `bynk.toml` so that `--against` works.
- **Mitigations:**
  - targets are named, never free URLs;
  - `production = true` refuses;
  - secrets come from the environment only;
  - the target is printed before any request.
- **Where verification happens:** in the runner, before the first request. It
  is not delegated to the deployment.
- **Residual risk:** a mis-labelled target. The docs say a preview deploy is
  the intended target, and the shop's own README already warns against taking
  real money.

## 7. Slice status

- [ ] Slice 0: #2131, #2132
- [ ] Slice 1: #2134 (suite configuration)
- [ ] Slice 2: #2135 (`Signature` signer)
- [ ] Slice 3: #2136 (`cron` and `queue` at `system`)
- [ ] Slice 4: #2137 (harness clock)
- [ ] Slice 5: #2138 (run against a real runtime)

## 8. Done when

- [ ] §3.0–§3.8 are settled under review, and the ADRs in §5 have landed with
  their slices.
- [ ] Check and emit agree for every address and principal at `system`: no
  `/* unknown */`, and no case that passes check but can only fail.
- [ ] `bynk-lang/shop`'s scenarios are `system` cases. Its `e2e/scenarios.sh`
  is deleted, and its CI runs them in process and with `--against dev`.
- [ ] The testing reference covers `env`, the `Signature` principals, cron and
  queue at `system`, time in tests, and testing against a running system.
  ADR 0207's "e2e is external" line is marked superseded.
- [ ] **On retire:** remove this doc, append its closing summary to
  [`../archive/retired-tracks.md`](../archive/retired-tracks.md), and close
  #2133.
