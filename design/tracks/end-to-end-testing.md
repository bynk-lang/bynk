# End-to-end testing in the language — a project's whole-system scenarios as `system` cases

- **Status:** Settled, ready to slice. Spine issue
  [#2133](https://github.com/bynk-lang/bynk/issues/2133). This doc lands via
  the settling PR [#2140](https://github.com/bynk-lang/bynk/pull/2140), which
  references it (*"Part of #2133"*, never `Closes`). Every question in §3 is
  **settled**, each in the direction the draft leaned. Slice 0 is two bugs found while drafting
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
  `bynk-emit/src/project/tests_emit.rs:1004`. Settled (§3.0): the line moves,
  and end-to-end testing is the language's.

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
There are two exceptions: cases a real deployment can't honour are refused
up front (§3.7), and assertions on absolute instants don't promote (§3.4).

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
- [x] **Surface not yet settled (at settling time; all now settled in §3).** Where a test's configuration lives (§3.1).
  Whether the harness may own time where `stub` is refused (§3.4). How a case
  waits for asynchronous effects, given
  [ADR 0155](../decisions/0155-history-properties-are-runner-only.md) D2's
  refusal of an `eventually` keyword (§3.6). Whether "against a real runtime"
  is a tier or a run (§3.5). None has a precedent that settles it.
- [ ] **Security/safety boundary.** Not for slices 0–4. They are test-build-only
  ([ADR 0147](../decisions/0147-structural-test-ness-and-flat-paths.md) strips
  every `suite`) and sign with fixed test secrets. Slice 5 has a narrower one
  (§6): a runner that signs real tokens with a deployment's secret.

## 3. Design questions

All nine were settled in the settling review on PR
[#2140](https://github.com/bynk-lang/bynk/pull/2140). Each section records
what was decided, then the evidence and alternatives behind it.

### 3.0 — Should end-to-end testing move into the language at all? — SETTLED: yes

**Decision.** End-to-end testing is the language's. ADR 0207's "e2e (Postman,
external)" line is superseded, and every slice (0–5) proceeds. The
non-determinism a remote run brings is contained, not avoided:

- the run is opt-in (`--against`, §3.5);
- retries are reported per case (§3.6);
- the in-process `system` tier stays the default and the gate.

ADR 0431's determinism guarantee holds for every run without `--against`.

**Why.** ADR 0207 drew the line deliberately: the `system` tier proves the
deployable Worker in process, and the real ceremony against a deployed
instance is external. Three arguments moved it:

1. **Promotion.** ADR 0153's commitment is one body across tiers. An e2e
   scenario written outside the language loses the types, the signer, the
   decoded `HttpResult` and the route checking the lower tiers give.
2. **Most of it is already built.** §1.1 shows the `system` tier covers
   everything in the shop's script except configuration, the webhook signer,
   cron and queue, time, and the transport. The first four are gaps the
   `system` ADRs named as follow-ons anyway.
3. **Every project otherwise hand-rolls the same thing.** That means token
   signing, HMAC over `<ts>.<body>`, port guessing and readiness loops, and the
   shop's script shows each is easy to get subtly wrong.

The argument against was that a remote run brings flakiness, environment and
secret handling into `bynk test`. The decision answers it with containment
rather than exclusion.

### 3.1 — Where a test's configuration lives — SETTLED: in source, `env { … }` (proposal #2134)

**Decision.**

- **Syntax.** An `env { NAME = "value" … }` clause in a suite or case body.
  `env` is contextual in that position only.
- **Scope.** One map applies to every participant. A case-level `env` layers
  over the suite's, name by name.
- **Values.** Values are `String` literals, matching what a Worker's `env`
  holds.
- **Tiers.** It is legal at every tier. At `system`, entries go into each
  participant's `env`. At `unit`/`integration`, they are what the `Secrets`
  double answers for that name (`Some(value)`; other names stay `None`).
- **Not a `stub`.** It sets the environment and replaces no seam, so it is
  legal at `system`, where `stub` is refused.
- **Test secrets.** An entry naming an actor's declared secret replaces the
  fixed test secret for both signing and verification. This is what lets
  slice 5 sign tokens a real `bynk dev` accepts. In process and under
  `--against dev`, where the runner configures the Workers, this is the
  signing secret. Under `--against <name>`, the deployed Workers' secrets are
  not the runner's to set, so such an entry is refused (§3.7) and the signing
  secret comes from the environment (§3.8).
- **Manifest.** A `[test.env]` table is not added. If slice 5 finds it needs a
  project-wide default, that is a re-settle of this section, not a ride-along.

**Evidence.**

- Today the harness `env` holds only bindings and call keys
  (`emit_integration_harness`, `bynk-emit/src/project/tests_emit.rs:1699-1834`).
- The Bearer secret is set in `process.env` as the fixed `"__bynk_test_secret"`
  (`:1515-1540`).
- `SecretsProvider.get` reads `env[name]`, then `process.env`
  (`bynk-std/lib/core/bindings/bynk-cloudflare.ts:139-157`).
- `bynk.toml` has no `[test]` table (`bynk-project/src/paths.rs:138-153`).

**Alternatives.**

- *Manifest only* separates the configuration from the cases that depend on it
  and gives no per-case override.
- *Per-context maps* (`env shop.payments { … }`) are more faithful to Workers,
  but no project yet needs two contexts to see different values under one
  name. This is re-openable on the first one that does.

### 3.2 — Signing `Signature` routes, and the failure principals — SETTLED: reserved principals (proposal #2135)

**Decision.**

- **Plain calls are signed.** At `system`, the driver signs every call to a
  route whose actor is a `Signature` actor, using the actor's declared
  `secret`, `header` and `timestamp`. No `by` is needed, because the actor has
  no identity. `unit`/`integration` are unchanged.
- **Three reserved principals cover the failure cases.** Each yields
  `Rejected(Unauthorized)` on a `401`, decoded as in ADR 0212 D2:
  - `by Nobody`, relaxed to `Signature` routes, sends no signature headers.
  - `by Forged` signs with a wrong secret.
  - `by Stale` signs correctly with a timestamp `tolerance + 60` seconds in the
    past. It is refused on an actor that declares no `timestamp`.
- **Gating.** `Forged` and `Stale` are legal only on a `Signature` route at
  `system`.
- **What is signed** is exactly the bytes sent: serialised JSON on the typed
  path, and the raw string for `Wire(…)`.
- **The secret** is the suite's `env` entry for the actor's `secret` (§3.1),
  else a fixed test secret set as the Bearer path sets its own.

**Evidence.**

- A `Signature` actor declares everything a signer needs (`SignatureSeam`,
  `bynk-check/src/actors.rs:449-480`).
- The verifier is `verifySignatureHmacSha256`
  (`bynk-emit/runtime/src/auth.ts:92`), called from the Worker entry
  (`workers_entry.rs:2294-2400`).
- ADR 0212 refused `by Nobody` on these routes only because no driver existed.

**Alternative.** A single `by Signed(secret = …, at = …)` with explicit options
was rejected, because each case would have to restate what the actor already
declares. It is re-openable if a test needs one specific timestamp.

**Until slice 2 lands**, #2132 refuses the call at `system` (ADR 0212 D1:
"meaning is only given where it is implemented").

### 3.3 — Driving `cron` and `queue` through the Worker — SETTLED: the handler's own result (proposal #2136)

**Decision.**

- **`queue` is admitted to `system`** as a serialisation edge, because the
  platform serialises the message.
- **`cron` is not admitted on its own**, because a scheduled event carries only
  a string and a time. A cron address is legal in a suite that already
  qualifies (consumed contexts, or an `http` service).
- **Drivers.** `svc.schedule(…)` and `q.message(…)` at `system` lower to drivers
  that call the participant's real `worker.scheduled` / `worker.queue` with
  platform-shaped events. The drivers settle the execution context (ADR 0478).
- **The address yields the handler's own result**, so `expect r is Ok(_)` reads
  the same at every tier:
  - **Queue:** the batch double records `ack()` / `retry()`, which yields
    `Ack` / `Retry`.
  - **Cron:** the test build's entry stores the handler's `Result` on the
    execution-context double the harness already supplies, and the driver reads
    it back. Production entries do not change.
- **Throwing handlers** map as the platform would: `Retry` for a queue, and the
  decoded failure for cron. They never crash the test.
- **The scheduled time** stays a required argument when the handler declares
  one. §3.4's clock does not default it.

**Evidence.**

- `system_needs_wire` is at `bynk-check/src/test_suites.rs:549-568`.
- The unit-tier addresses are checked at
  `bynk-check/src/checker/calls.rs:3320-3403` and lowered at
  `lower.rs:2602-2633`.
- The entries are `emit_scheduled_handler` and `emit_queue_handler`, at
  `workers_entry.rs:1562` and `:1638`.

**Alternative.** Yielding `()` at `system` and refusing an `expect` on it was
rejected because it breaks promotion.

**Landed with slice 3 (#2136)**, which retired #2131's refusal.

### 3.4 — May the harness own time at `system`? — SETTLED: yes, for `Clock` only (proposal #2137)

**Decision.**

- **One harness clock per case.** Every participant's `Clock` reads it at
  `system`, and the `Clock` double reads it at `unit`/`integration`.
- **It starts at the epoch**, as ADR 0431's double does, so a promoted case
  reads the same `Clock.now` at every tier.
- **`advance Clock by <Duration>`** is the only way to move it. It is a
  statement in a case body, legal at every tier, and its operand must be a
  non-negative `Duration`.
- **This is the principled exception to `bynk.stub.at_system`, not a loophole.**
  A `stub` overrides one operation of one context's seam. The harness clock is
  part of the environment the harness provides to every participant, as the
  in-memory KV and Durable Object namespaces of ADR 0467 are. `stub Clock.now`
  stays refused at `system`.
- **It governs the `Clock` capability only.** The runtime's own seams (JWT
  `exp`, the `Signature` tolerance, Durable Object alarms) keep wall-clock
  time.
- **The injection point is emitted only in the test build's participant
  modules.** A fixture asserts the deploy output is byte-identical.

**Evidence.**

- At `system`, `Clock.now` is the real `Date.now()`
  (`bynk-cloudflare.ts:9-13`).
- At `unit`/`integration`, the double reads the epoch (ADR 0431 §1).
- The refusal is at `bynk-check/src/test_suites.rs:681-694`.

**It does not promote to a remote run.** In process, the clock starts at the
epoch. Under `--against`, the Workers read wall-clock time and the runner can't
reach them. A case that asserts on a `Clock`-derived value (a `createdAt`, an
expiry computed from `Clock.now`) can therefore pass in process and fail
remotely, and `--settle` retries can't fix that. The rules:

- **`advance Clock` is refused remotely** (§3.7). Which `expect`s depend on the
  clock can't be decided exactly, so the rest is a documented limit, not a
  refusal.
- **Checker warning.** The warning is `bynk.test.remote_clock_dependent`. It
  fires only in the remote check mode that `bynk test --against` asks for
  (§3.7), so an in-process run gets none.
  - It fires on an `expect` that compares an `Instant`-typed value whose
    instants are not all *run-derived*.
  - An instant is run-derived if it comes from an address's result in the
    same case, or from such a value shifted by a `Duration` (`created + 1h`).
  - A literal instant, or one built from a constant
    (`Instant.fromEpochMillis(0)`), is not run-derived.
- **Docs.** They say plainly that a case meant to run remotely asserts on
  relations ("expires after it was created"), not on absolute instants.

The shop's scenarios are unaffected, because none asserts on an instant.

**Priority.** This stays the track's lowest-priority slice. The shop's sweeper
takes its scheduled time as a parameter, so slice 3 alone tests "an hour from
now".

### 3.5 — A real runtime: a tier or a run? — SETTLED: a runner flag (proposal #2138)

**Decision.** Where the Workers run is a property of the run, not of the case.
There is no `as e2e` tier.

- **`bynk test --against dev`:**
  - starts `bynk dev` with the suites' configuration;
  - waits for readiness;
  - runs the `system` suites unchanged against it;
  - stops it.

  Suites with different `env` (§3.1) get one `bynk dev` per distinct
  configuration, run in turn.
- **`bynk test --against <name>`** runs against a target named in a
  `[test.targets]` table in `bynk.toml`, which gives each Worker's base URL and
  the names of the secrets to read from the environment (§3.8).
- **Readiness.** `bynk dev` gains `--ready-json <file>`. It writes
  `{ "<worker>": "<url>" }` once every context has answered a probe. The runner
  uses it, and `bynk-lang/bynk-dev` can become a thin wrapper over
  `--against dev`.
- **Transport.** The drivers route through one transport function. In process
  it is today's `__h.env.<binding>.fetch`; remote, it is
  `fetch(baseUrl[binding] + path)`.

**Evidence.**

- `TestArgs` (`bynk-driver/src/test_runner.rs:77-121`) has no transport or tier
  flag.
- Each system driver builds a concrete `Request` (`tests_emit.rs:1255`) and
  calls `__h.env.<binding>.fetch` (`sysdrive_driver`, `:7032-7107`). Only that
  hop is in process.
- `bynk dev` prints only a human table to stderr (`serving_report`,
  `bynk/src/dev.rs:991`).

**Alternative.** A fourth tier was rejected, because it would put the same
body in two places and mark as a property of the case what is a choice of the
run, like `--target`.

### 3.6 — Waiting for asynchronous effects against a real runtime — SETTLED: the runner retries `expect` (proposal #2138)

**Decision.** There is no new syntax. Under `--against`, a failing `expect` is
retried until it passes or a deadline expires (`--settle <Duration>`, default
`5s` remotely and `0` in process).

**The retry rule is static.** Before each `expect`, the runner re-runs the
addresses made since the last *unsafe* one: an HTTP address whose method is
not safe per RFC 9110 §9.2.1 (anything but `GET`, `HEAD`, `OPTIONS`), or a
`schedule`/`message` address. The emitter knows each address's method, so it
marks the replayable segment at compile time and the runner never guesses.
If the segment has no address, the `expect` is not retried.

**Reporting.** Each retry is reported per case (`--format json` gains
`retries`), so an ordering bug shows up as a retry count rather than hiding
behind a pass.

**Why this is not ADR 0155's `eventually`.** ADR 0155 D2 rejected
`eventually`/`always` *keywords*, a temporal surface over generated
histories. This is a runner policy for a real system's latency. It adds no
language surface and makes no claim about history, and it is a no-op in
process, where ADR 0478 and 0467 already settle every send and the fan-out.

**Alternatives.**

- An explicit `eventually within … { }` block was rejected, because it puts
  latency into the case body and re-opens 0155's question.
- Settling on the harness side through a test-only drain endpoint was rejected,
  because it needs a new endpoint in what is deployed.

Prior art: Playwright's auto-retrying assertions.

### 3.7 — What a remote run cannot honour — SETTLED: the checker refuses, with an explicit skip (proposal #2138)

**Decision.** When a remote run is requested, the checker refuses a case that:

- reads an agent directly;
- addresses an `on call` service, which is internal and, once released, signed
  per pair (#2004, #2012);
- uses `advance Clock` (§3.4);
- uses `svc.event(…)` ([ADR 0495](../decisions/0495-test-event-address.md));
- **against a named target only:** sets an `env` entry (§3.1) that the target
  doesn't declare with the same value.

On that last point: a deployed target is already running with its own
variables, so the runner can't apply a suite's `env`. A target declares the
non-secret values it was deployed with
(`vars = { PAYMENTS_PROCESSOR = "fake" }` under `[test.targets.<name>]`).

- A case runs only if its non-secret `env` and the target's `vars` agree on
  every name either one mentions.
- A refusal covers three situations:
  - the case sets a name the target doesn't declare;
  - the target declares a name the case doesn't set;
  - the two give different values.
- The second refusal is what makes the guarantee hold when a suite relies on
  the code's default instead of pinning a value. In process, an unset name
  reads as `None`, so a target that declares that name is a different
  environment, and the case wouldn't promote unchanged anyway.
- An `env` entry naming a secret is always refused against a named target. That
  secret comes from the environment instead (§3.8).

This keeps a suite written for the fake processor from running against a
preview deployed with the real one, which is the "takes real payments"
accident in §6. Under `--against dev` the runner starts the Workers itself, so
it applies `env` as in process and none of this applies.

The diagnostics are a `bynk.test.remote_unsupported_*` family.
`--skip-unsupported` skips such cases instead, naming each one and why. Without
it, a suite can't silently shrink to nothing remotely.

**Per-run isolation.** `run.id` is a `String` readable in case bodies. In
process it is a fixed value, so existing goldens don't move. A remote run gets
a fresh one, so `orderIdFrom("o1-\(run.id)")` doesn't collide between runs.
Fresh state per case is not promised remotely, and the docs say so.

**Alternative.** Silently prefixing path parameters was rejected, because it
breaks on ids carried inside bodies.

### 3.8 — Keeping a remote run away from production — SETTLED: named targets only (proposal #2138)

**Decision.**

- `--against` accepts only `dev` or a name under `[test.targets]`. A bare URL
  is refused.
- A target marked `production = true` is refused.
- For a named target, secrets are read only from the environment, by the
  names the target lists, never from `bynk.toml` and never from a suite's
  `env` (§3.1, §3.7). Under `--against dev` the runner sets the Workers'
  secrets itself, from `env` or the fixed test secrets, as in process.
- A target's non-secret deploy values are declared as `vars` and checked
  against each case's `env` (§3.7).
- The runner prints the target and each Worker's URL before the first request.

See §6.

## 4. Candidate slice decomposition

Settled with this doc. Slices 1–4 are independent and can land in
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
  "e2e is external" line. It lands with slice 5, the first slice that crosses
  that line. Slices 0–4 stay within the `system` tier's existing scope.
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
  - for a named target, secrets come from the environment only;
  - a case's `env` must agree with the target's declared `vars`, so a suite written
    for a fake provider can't run against a real one (§3.7);
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
- [x] Slice 3: #2136 (`cron` and `queue` at `system`)
- [ ] Slice 4: #2137 (harness clock)
- [ ] Slice 5: #2138 (run against a real runtime)

## 8. Done when

- [x] §3.0–§3.8 are settled under review (#2140).
- [ ] The ADRs in §5 have landed with their slices.
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
