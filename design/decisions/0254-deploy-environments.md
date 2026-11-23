# 0254 — Environment selection at deploy time: driver-synthesised per-environment config, not emitter-curated

- **Status:** Accepted (v0.220.1)

**Context.** The deploy-state ledger (`bynk.deploy.lock`) was built env-keyed
from slice 0 on (`DeployLock.environments: BTreeMap<String, Environment>`),
but nothing above it was real: every read/write site hardcoded the literal
`"default"`, `bynk deploy` had no `--env` flag, and `bynk deploy -- --env
staging` already silently half-worked — pushing under Wrangler's `staging`
environment while provisioning KV/queues/secrets and recording the ledger
against `"default"` regardless.

Confirmed against Cloudflare's own documentation: bindings (`kv_namespaces`,
`queues.consumers`, `durable_objects`, `services`) are **non-inheritable**
into a named environment — a bare `--env staging` against the flat,
environment-agnostic config `emit_wrangler_toml` writes would deploy with
**zero** bindings. `emit_wrangler_toml` cannot fix this itself: it runs once,
at compile time, before any `--env` is known — there is no `environment`
concept anywhere in `bynk-syntax`/`bynk-check` — so the fix cannot live in the
emitter.

Two other resource kinds needed the same scrutiny. Queues reconcile by their
bare user-given name, account-wide (ADR 0194 D2); two environments sharing an
account would create-or-reuse the same physical queue. Service Binding
targets are not automatically environment-scoped either — Cloudflare
auto-suffixes the *deploying* Worker's own name to `<name>-<env>` under
`--env`, but not what a `[[services]] service = "..."` binding points at, so
an unqualified binding would target the wrong, or a nonexistent, Worker. This
directly touches the multi-context deploy ordering slice 2 shipped (ADR
0193).

**Decision.**

**D1 — a real `--env NAME` flag (default `"default"`), threaded everywhere the
ledger key was hardcoded.** `DeployOptions` gains `environment: String`; every
production call site that read or wrote `bynk.deploy.lock` (`derive_plan`,
`recorded_kv`, `deploy_one`'s KV/queue/deployed-state bookkeeping,
`absent_dependencies`, `contract_skews`) takes it as a real parameter.
Omitting `--env` reproduces prior behaviour byte-for-byte.

**D2 — the driver synthesises a `[env.<name>]` config block at deploy time;
the emitter is unchanged.** For any environment other than `"default"`, a new
function parses the already-generated `wrangler.toml` generically (via
`toml::Table`, not the narrow read-only `WranglerConfig`/`ServiceBinding`/
`QueueConsumer` structs the plan already used, which drop fields — e.g.
`ServiceBinding` has no `binding`, `Migration` has no `new_classes` — that
must be copied byte-for-byte), builds a separate `{ env: { <name>: … } }`
table, and appends it as freshly-serialised text. The original top-level
bytes are never touched — they continue to serve the plain, no-`--env`
`bynk deploy` — and TOML string-escaping is the `toml` crate's job, not a
hand-rolled duplicate of the emitter's own escaping. This generalises the seam
`materialise_kv_id` already occupies (patching generated config just before
Wrangler runs) rather than inventing a new one.

> **Amended by the back-half paydown, slice 1C.2 (#4039).** The block is now rendered by
> `bynk-emit` (`wrangler::environment::render`), which the deploy's `ApplyWorker::environment_block`
> calls, in place of the driver's own function. It is still a function of the generated
> `wrangler.toml` as the deploy has materialised it, still appended after a newline, still written
> through `toml::to_string_pretty`, so every byte of the block is unchanged. What it does with each
> top-level key is that key's environment rule, `WranglerKey::environment_rule` in
> `bynk-project`, one exhaustive `match` over every table the emitter writes. The names a deploy
> gives a resource in an environment are `bynk_project::environment`'s, for the plan and the push
> alike. The emitter's *build* output is unchanged: the block is still made at deploy time, because
> the environment and the ids it holds are not known before.

**D3 — queue names and Service Binding targets are environment-qualified
(`<name>-<env>`); KV needs no qualification.** KV is safe by construction: a
driver-minted id, already separated by the ledger's per-environment key, with
no account-wide name to collide on. The wrangler-facing queue name and every
Service Binding target get the same `<name>-<env>` suffix Cloudflare itself
applies to a deployed Worker's own name — for queues this is bynk's own
convention (closing the account-sharing collision above); for Service
Bindings it is not a free choice, since it must match Cloudflare's own naming
exactly to resolve. The source-level logical name (`on queue "n"`, a
consumed context's dotted name) remains the identity everywhere else — the
plan's display, the manifest, cross-context binding resolution.

**D4 — only `wrangler deploy` and `wrangler secret put`/`list` receive
Wrangler's own `--env` argument.** `whoami`, `kv namespace create`, and
`queues create` are account-level operations Wrangler does not scope by
environment; passing `--env` to them would be a no-op at best.

**D5 — `--env`/`--environment` inside the `--` passthrough is a hard
pre-flight error, checked first, before any other work.** Once `--env` is a
real driver-curated concept, `bynk deploy --env staging -- --env production`
would otherwise forward both to `wrangler deploy`, leaving Wrangler's own
last-wins parsing to decide silently which one actually deploys while the
ledger records the driver's choice regardless — the same silently-half-right
footgun class this ADR exists to close, re-entered through the one seam that
was supposed to close it. Rejected outright rather than de-duplicated:
picking a winner between two explicit, conflicting user inputs is exactly the
ambiguity this ADR removes, not reproduces.

**D6 — account/credential selection is unchanged.** `deploy` gains no
`--account-id` flag; `CLOUDFLARE_API_TOKEN`/`wrangler login`'s own account
switching covers a `staging`/`production`-different-account setup, as it does
today. This slice does not close the pre-flight's "is *some* account
authenticated" gap into "is the *right* account for this environment" — see
Consequences.

**D7 — `bynk dev -- --remote` gets the same `--env`, for the same reason
(PR review).** `materialise_deploy_state` — the KV-placeholder-fill
`--remote` shares with `deploy` (§3 of the track doc names this seam
explicitly) — read the ledger's `"default"` section unconditionally. Before
`--env` existed every real deploy recorded into `"default"` regardless, so
that always matched; once a project can be deployed *only* under
`bynk deploy --env staging`, reading `"default"` misreports a provisioned
project as never deployed. `dev` gains `--env NAME` (default `"default"`),
used **only** to select the ledger section `--remote` reads — never forwarded
to `wrangler dev` itself, since `dev` curates no Wrangler-side environment
config and never provisions. The same passthrough-conflict guard (D5) applies
when `--remote` is present, since a `-- --env`/`-- --environment` would
otherwise pick a different Wrangler-side environment than the one `dev`
materialises the KV id for.

**Consequences.** A non-default `--env` costs one extra parse-and-serialise
pass per context at deploy time — negligible next to the network round-trips
around it. Two environments now provision, record, and reconcile
independently, including when they share one Cloudflare account. The
provisioning-state model's original promise — the schema was keyed by
environment from slice 0 so this slice would be additive, not a migration —
held.

- **S1 (deferred).** The pre-flight's account-blindness (D6): a user pointed
  at the wrong Cloudflare account for a given environment still gets a
  pre-flight pass and a deploy against the wrong account. Unchanged from
  today's single-environment behaviour, not newly introduced, not closed
  here either.
- **S2 (deferred).** Slice 5 (reconciliation maturity + orphan reporting) is
  the track's last remaining slice.
