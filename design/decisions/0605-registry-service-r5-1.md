# 0605 — Abuse limits for the first-party registry

- **Status:** Accepted (v0.400.1)

**Context.** Track §6 asks for per-token and per-IP rate limits and a
per-organisation storage quota. §11 Q2 settled the quota as a best-effort count
in an `OrgUsage` agent. Rate limiting (stdlib P3, #2342) provides
`RateLimit @limit @period`, counted per Cloudflare location. The accepted
proposal is #2390, with its limits revised on review: GitHub-hosted runners
share their outgoing addresses, so writes are limited per token, not per IP.

**Decision.**

- **A. Four limiters** (`src/registry/store/limits.bynk`), behind one `Limits`
  capability, so each route makes one call:

  | Limiter | Limit | Key | Routes |
  |---|---|---|---|
  | `PublishLimit` | 60 a minute | the token's sha256 | `PUT`, yank, unyank |
  | `FloodLimit` | 600 a minute | `CF-Connecting-IP` | the same: a flood guard only |
  | `AccountLimit` | 60 a minute | `CF-Connecting-IP` | every `/account/…` route but the exchange |
  | `ExchangeLimit` | 10 a minute | the verified token's `repository_id` | the trusted-publishing exchange |

  - **Order.** An IP limit is checked first, at no cost. The token limit is
    checked before the token is looked up and before admission. The exchange's
    limit is checked after the OIDC token is verified, before any agent is
    called.
  - **Rotating tokens.** `PublishLimit` is keyed by the bearer as sent,
    before it is checked, so made-up tokens each get a fresh allowance. The
    per-IP flood guard is what bounds them, and each is refused `401` before
    it costs a lookup.
  - **No repository id.** An exchange whose verified token has no
    `repository_id` is refused (`403 not_trusted`) before any limiter, so such
    tokens never share one allowance.
  - **Off the edge.** A request with no `CF-Connecting-IP`, which only happens
    off Cloudflare's edge, isn't IP-limited.
  - **The refusal** is `429 rate_limited`, in the protocol's JSON shape.
  - **Reads** aren't limited by the service. They're cached at the edge, and
    read abuse is a WAF matter for the operator.
- **B. A storage quota per organisation.**
  - **The count.** `OrgUsage(org)` holds the bytes of the organisation's
    uploads.
  - **The check.** The `PUT` route checks it after admission (when the size is
    known) and before storing. It adds to it only when the line commits fresh.
    A retry of a tree already published passes whatever the usage.
  - **The limit.** The default is 1 GiB. The `REGISTRY_QUOTAS` secret raises it
    per organisation, as `[{"org", "bytes"}]`, so nothing account-specific is in
    git (E3). A table that can't be read is the operator's `500`.
  - **The refusal.** Past the quota, the upload is `413 quota_exceeded`, naming
    the usage.
  - **Nothing is stored outside the count.** Before storing an archive, the
    route asks the package's index what an append would do
    (`PackageIndex.outlook`), and signs the line. Another tree for a published
    version (`409`), a withdrawn version (`410`) or a signing fault never
    reaches the bucket. `append` stays the authority, and only a `Fresh` append
    is counted.
  - **How exact it is.** Best-effort at the margin (Q2): concurrent publishes to
    different packages can overshoot by one upload each.
- **C. Two optional rows on the protocol page:** `413 quota_exceeded` and
  `429 rate_limited`. No conformance check is added, since limits are the
  operator's choice. The client already reports any refusal's `error` and
  `message`.
- **D. Proof.**
  - **Unit tests** (`tests/limits.bynk`): each limiter's refusal through its
    routes, the exchange keyed by repository, the quota's arithmetic, the
    raised quota, the retry, and an unreadable table.
  - **End to end:** `scripts/check-limits.sh`, run in CI, spends each counted
    limit under `bynk dev` and gets `413 quota_exceeded` from the real client.
    `scripts/check-trusted.sh` spends the exchange's limit. Every existing
    end-to-end script still passes under the real limits.

**Deviations from the accepted proposal.**

- **`REGISTRY_QUOTAS` is a JSON list of `{org, bytes}`, not a map.** That is
  the shape the service already decodes for `REGISTRY_TOKENS`.
- **`check-limits.sh` runs the real limits; there is no test build with
  smaller ones.** Spending 60, or 600, requests takes seconds locally.

**Consequences.**

- A leaked token, a flood from one address, or a looping CI job is bounded
  before it reaches the agents or admission.
- `bynk deploy` allocates four rate-limit namespace ids into `bynk.deploy.lock`
  at the first deploy (R6).
- `RawError` can't set `Retry-After`. A refusal's message says to try again
  within a minute.
