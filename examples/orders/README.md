# Orders

Record orders and their line items in one place, then report across them with
**in-store joins, grouping, and aggregates** — no database, just an agent's
`store` maps and the `Query[T]` vocabulary.

What it shows:

- **Two storage `Map`s in one agent** — `orders` and `lines` live in the same
  instance (a Cloudflare Durable Object keyed by sales `channel`), so a query can
  join across them with no cross-instance fan-out.
- **`Query[T]` over storage** — `joinOn` (equi-join), `leftJoin` (orphan
  detection), `groupBy` + `sum` (per-order totals), and `filter` + `count`/`sum`
  aggregates. A join has no pair type to name: each matched `(line, order)` is
  projected through an `into` combiner into a named record (`Sale`, `OrderTotal`).
- **An `@indexed` field** — `lines` is `@indexed(by: orderId)`, so an equality
  filter on that field (`linesFor`, `unitsFor`) routes through a posting list
  rather than scanning every entry.
- **Tests with no harness** — the agent uses no platform capability (a `Map`
  needs no clock), so `bynk test .` constructs it by key, drives its handlers,
  and asserts on the scalar aggregates directly.

> **Size.** Each entry of an agent's collections is stored under its own key and a
> handler writes only what it changed, but every call still loads the whole state
> (reads are not lazy yet). The HTTP service here routes every
> order to the single `Book("default")` instance for brevity, which is the shape to
> avoid in production: key the agent by customer, order or channel, or keep a large
> table in D1. The Book's agents reference
> ("Size and cost of an agent's state") has the detail.

## Layout

```text
orders/
├── bynk.toml
├── src/
│   └── orders.bynk     # context orders — the Map-backed agent + HTTP service
└── tests/
    └── orders.bynk     # tests targeting the agent (joins / groups / aggregates)
```

## Check and test

```sh
bynk check src
bynk test .
```

```text
orders:
  ✓ joins, groups, and orphan detection
  ✓ an empty book aggregates to zero

2 passed, 0 failed.
```

The aggregate handlers return scalars, so each assertion exercises a real join,
group, or sum over storage — no platform binding required.

## Run it

```sh
bynk dev
```

From anywhere inside the project, `bynk dev` compiles, picks the `orders` worker,
and serves it on `http://localhost:8787` in local mode — the Durable Object is
simulated, with nothing to provision first. Then:

```sh
curl -XPOST localhost:8787/orders -d '{"id":"o1","customer":"Alice"}'
# {"id":"o1","customer":"Alice"}  (HTTP 201)

curl -XPOST localhost:8787/lines -d '{"id":"l1","orderId":"o1","sku":"apple","qty":3}'
curl -XPOST localhost:8787/lines -d '{"id":"l2","orderId":"o1","sku":"pear","qty":2}'

curl localhost:8787/sales
# [{"customer":"Alice","sku":"apple","qty":3},{"customer":"Alice","sku":"pear","qty":2}]

curl localhost:8787/totals
# [{"orderId":"o1","units":5}]

curl localhost:8787/orders/o1/lines
# [{"id":"l1","orderId":"o1","sku":"apple","qty":3}, ...]
```

*Under the hood,* `bynk dev` compiles to `out/workers/orders/` and runs `wrangler
dev` there. **Deploy** with `npx wrangler deploy` from that directory; the
generated `wrangler.toml` carries the Durable Object binding.
