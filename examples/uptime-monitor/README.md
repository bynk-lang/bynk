# Uptime monitor

A scheduled job that pings a set of URLs every five minutes, records each one's
status in KV, and serves the latest result over HTTP. It puts **cron**, **`Fetch`**,
**`Kv`**, and **`Logger`** together in one small context.

What it shows:

- **A cron entry point** — `service checks from cron { on schedule("*/5 * * * *") (at: Int) … }`.
  Cron has no ambient clock, so the schedule-aligned instant arrives as the `at`
  parameter (epoch-ms).
- **Outbound HTTP** — `Fetch.send(Request { method: Get, … })` returns a
  `Result[Response, FetchError]`; a network failure is a value, not an exception.
  The request sends no extra headers (`headers: Map.empty()`).
- **Capabilities live on handlers** — the effectful fetch/store work stays in the
  cron handler (a free function can't hold `given`, and `Request` can only be
  built where it's used); the pure health policy (`isHealthy`) and key helper
  (`statusKey`) are factored into `commons status`.
- **A read-side HTTP route** — `GET /status/:name` reads the stored JSON back
  through `Json.decode[Status]`.

## Layout

```text
uptime-monitor/
├── bynk.toml
├── src/
│   ├── status.bynk     # commons status — isHealthy + statusKey
│   └── monitor.bynk    # context monitor — cron service + HTTP service
└── tests/
    └── status.bynk     # unit tests for the health policy + key helper
```

## Check and test

```sh
bynk check src
bynk test .
```

```text
status:
  ✓ a 2xx code is healthy
  ✓ a 3xx code is still healthy
  ✓ a 4xx/5xx code is unhealthy
  ✓ code 0 (the request never completed) is unhealthy
  ✓ the KV key is namespaced

5 passed, 0 failed.
```

The health policy and key helper live in `commons status` so they are unit-tested
without `Fetch`/`Kv`. The cron and HTTP handlers consume those platform
capabilities; under `bynk test` `Kv` is in memory and `Fetch` must be stubbed.
Run the schedule locally (below) to exercise the whole path.

## Run it

```sh
bynk dev
```

From anywhere inside the project, `bynk dev` compiles, picks the `monitor`
worker, and serves it on `http://localhost:8787` in local mode — KV is
simulated, with nothing to provision first.

Trigger the schedule locally (wrangler exposes a scheduled endpoint in dev):

```sh
curl "localhost:8787/__scheduled?cron=*/5+*+*+*+*"

curl localhost:8787/status/example
# {"name":"example","ok":true,"code":200,"at":...}
```

*Under the hood,* `bynk dev` compiles to `out/workers/monitor/` and runs
`wrangler dev` there. The generated `wrangler.toml` already carries `crons =
["*/5 * * * *"]`. **Deploy** with `npx wrangler deploy` (create the KV namespace
first: `npx wrangler kv namespace create KV`, then paste the id into
`wrangler.toml`). To watch more sites, add another `Fetch`/`Kv` block in the cron
handler.
