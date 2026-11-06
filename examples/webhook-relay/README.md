# Webhook relay

Accept a signed webhook only when its signature proves it came from the trusted
sender, then forward the event to a configured upstream URL. The HMAC check is
generated for you — there is no app-written crypto.

What it shows:

- **A `Signature` actor** — `auth = Signature(secret = "WEBHOOK_SECRET", header = "X-Signature", timestamp = "X-Timestamp", tolerance = 300)`.
  Before the handler runs, the boundary recomputes HMAC-SHA256 over the raw body
  (constant-time, WebCrypto), rejects a mismatch or a stale timestamp with `401`,
  and only then parses the body from the same bytes.
- **Authenticity, not a principal** — a `Signature` actor has no identity, so the
  binder is omitted (`by Webhook`).
- **Outbound HTTP from a handler** — the verified event is re-encoded with
  `Json.encode` and POSTed onward with `Fetch.send`.
- **Configuration via `Secrets`** — the upstream URL is read from
  `Secrets.get("RELAY_TARGET_URL")` rather than hard-coded.

## Layout

```text
webhook-relay/
├── bynk.toml
├── src/
│   └── relay.bynk       # context relay — the HTTP service
└── tests/
    └── relay.bynk       # tests targeting the relay context
```

## Test

Every step of the handler runs on a platform capability, and under `bynk test`
each one is a deterministic test double: `Secrets` holds no values, `Logger`
records without printing, and `Fetch` never reaches the network. A case that
forwards supplies the secret and the upstream's answer with `stub`, then
observes the calls:

```bynk,ignore
case "a configured target forwards the event and logs it" {
  stub Secrets.get("RELAY_TARGET_URL") returns Some("https://upstream.test/hook")
  stub Fetch.send(_) returns Ok(Response { status: 202, headers: Map.empty(), body: "" })
  let r <- api.POST("/hooks/event", Event { id: "evt_1", kind: "order.created" }) by Webhook
  expect r is Ok(_)
  expect Fetch.send called once with req.url == "https://upstream.test/hook"
}
```

```sh
bynk test .
```

```text
relay:
  ✓ an unconfigured relay target is a server error
  ✓ a configured target forwards the event and logs it
  ✓ an upstream failure is a server error

3 passed, 0 failed.
```

At this tier the actor is given, not verified, so the HMAC check itself runs
only on a real request — exercise it under `bynk dev`, below.

## Run it

```sh
# this service reads two values — supply local ones through the passthrough
bynk dev \
  -- --var WEBHOOK_SECRET:dev-secret \
     --var RELAY_TARGET_URL:https://httpbin.org/post
```

From anywhere inside the project, `bynk dev` compiles, picks the `relay` worker,
and serves it on `http://localhost:8787` in local mode. Then:

```sh
# a request with no / wrong X-Signature is rejected at the boundary
curl -XPOST localhost:8787/hooks/event -d '{"id":"evt_1","kind":"order.created"}'
# (HTTP 401)

# with a valid HMAC-SHA256 of the body (and a fresh X-Timestamp) it forwards
curl -XPOST localhost:8787/hooks/event \
  -H "X-Timestamp: $(date +%s)" -H "X-Signature: sha256=<hmac>" \
  -d '{"id":"evt_1","kind":"order.created"}'
# "relayed"
```

*Under the hood,* `bynk dev` compiles to `out/workers/relay/` and runs `wrangler
dev` there. **Deploy** with `npx wrangler deploy`; set the real secrets with
`npx wrangler secret put WEBHOOK_SECRET` and `… RELAY_TARGET_URL`.
