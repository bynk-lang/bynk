# Rate limiter

A fixed-window rate limiter: at most N requests per client per window. Each client
is an **agent** — on Cloudflare, one Durable Object per key — so limits are
strongly consistent and never bleed across clients.

What it shows:

- **The agent model** — `agent Limiter { key client: ClientId; state { … } }`.
  Two calls with the same `ClientId` address the same instance; different ids are
  independent. Compiles to a Durable Object on the `workers` target.
- **Zeroable state** — `windowStart` and `count` are `Int`s, so a never-seen
  client starts clean at `0/0` with no constructor to call.
- **A single `commit` per call** — the handler computes the next window and count
  once and commits one replacement; an over-budget request is *not* counted, so a
  client can't deepen its own hole.
- **An honest clock** — the agent has no ambient time; the HTTP handler reads
  `Clock.now()` and passes it in, which keeps the agent a pure function of its
  inputs.
- **A pure policy, factored out** — the whole windowing decision is `decide(…)`
  in `commons window`, a function of plain numbers. The agent is a thin shell that
  reads state, applies the decision, and commits; the policy is unit-tested
  directly, with no agent or clock.

> Within the budget the handler returns `200` with the verdict body (`allowed`,
> `remaining`, `resetAt`); once the window is exhausted it returns
> `429 TooManyRequests` with the reset time in the message.

## Layout

```text
rate-limiter/
├── bynk.toml
├── src/
│   ├── window.bynk        # commons window — the pure fixed-window policy
│   └── ratelimit.bynk     # context ratelimit — agent + HTTP service
└── tests/
    └── window.bynk        # unit tests for the policy
```

## Check and test

```sh
bynk check src
bynk test .
```

```text
window:
  ✓ the first request in a window is allowed and counted
  ✓ an over-limit request is denied and not counted
  ✓ a request after the window lapses opens a fresh window

3 passed, 0 failed.
```

The policy lives in `commons` so it is testable in isolation, with no clock at
all. The `ratelimit` context consumes the platform `Clock`, which under
`bynk test` reads the epoch unless a case stubs it.

## Run it

```sh
bynk dev
```

From anywhere inside the project, `bynk dev` compiles, picks the `ratelimit`
worker, and serves it on `http://localhost:8787` in local mode — the Durable
Object is simulated, with nothing to provision first. Then:

```sh
# 10 requests / 60s per client (the :client segment is the key)
curl localhost:8787/check/acme
# {"allowed":true,"remaining":9,"resetAt":...}

# hammer it past the limit and `allowed` flips to false, `remaining` to 0
for i in $(seq 1 12); do curl -s localhost:8787/check/acme; echo; done
```

*Under the hood,* `bynk dev` compiles to `out/workers/ratelimit/` and runs
`wrangler dev` there. **Deploy** with `npx wrangler deploy` — the Durable Object
migration is already in the generated `wrangler.toml`.
