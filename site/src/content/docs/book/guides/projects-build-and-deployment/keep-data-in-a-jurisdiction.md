---
title: Keep a project's data in one jurisdiction
---
**Goal:** keep a context's stored data inside the EU (or a FedRAMP boundary), so a data-residency
promise holds without hand-written Cloudflare code.

Cloudflare pins a resource to a jurisdiction when it is **created**, and cannot move it afterwards.
Bynk creates your resources, so you state the jurisdiction in the source and the deploy applies it.

## Agents

Put `@jurisdiction("eu")` before `agent` (the values are `"eu"`, `"fedramp"` and `"us"`, the same for agents, `R2` and `D1`):

```bynk,fragment
@jurisdiction("eu")
agent Learner {
	key id: String
	store total: Cell[Int]

	on call add(n: Int) -> Effect[Int] {
		let _ <- total.update((t) => t + n)
		total
	}
}
```

Every instance's state now lives in the EU. Decide this **before the first deploy**: adding, changing
or dropping it later mints different ids, which orphans the existing instances, so the deploy asks you
to confirm it by name (`--confirm-jurisdiction shop.Learner`) and `--yes` never stands in. If you
already deployed an agent, create a new agent class in the jurisdiction and move its data across
yourself.

Locally, workerd does not implement jurisdictions, so the Worker uses the unrestricted namespace and
warns once; a deployed Worker is restricted.

## R2 buckets

An [`R2` instance](/reference/bynk-capabilities/#object-storage-r2) takes the same annotation:

```bynk,fragment
capability Archives = R2 @jurisdiction("eu")
```

`bynk deploy` creates the bucket with `--jurisdiction eu`. Changing it makes a new bucket and leaves the
old one, with its objects, where it is.

## D1 databases

A [`D1` instance](/reference/bynk-capabilities/#sql-databases-d1) takes it too:

```bynk,fragment
capability Orders = D1 @jurisdiction("eu")
```

`bynk deploy` creates the database in the `eu` jurisdiction through Cloudflare's API and records the jurisdiction
in `bynk.deploy.lock`. A database's jurisdiction is fixed when it is made, so adding or changing the
annotation on a database that is already provisioned is refused, and the plan never recreates it: it holds
data. Keep the annotation the database was made with, or create a new `D1` capability, move the data
across and remove the old one.

A database that is already on the account but not in the lock (a lost lock, or one made by hand) was not
created by this deploy, and Cloudflare does not say which jurisdiction it has. With `@jurisdiction` on its
capability, `bynk deploy` refuses to adopt it rather than record a jurisdiction it did not apply: check the
database, then remove the annotation to adopt it as it is, or delete it so the next deploy creates it in
the jurisdiction.

## What to check yourself

A jurisdiction is invisible to a query and to `bynk test`, so nothing in a test run proves it. Bynk
cannot show it either. For an agent, Cloudflare reports it from inside the Durable Object, as
`ctx.id.jurisdiction`, so a check on a deployed Worker has to read it there. For an `R2` bucket or a `D1`
database, look at the resource in the Cloudflare dashboard. See [Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/)
for how the plan reports a change.

Two things the annotation does not cover. A Worker's implicit idempotency database (the one
`Idempotency.claim` uses) is always created with no jurisdiction, so a project that pins every `D1` of its
own still stores idempotency keys outside it. And a database adopted by name keeps whatever jurisdiction it
was made with.
