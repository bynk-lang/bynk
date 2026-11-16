---
title: Search by meaning with Vectorize and Workers AI
---
**Goal:** store documents as vectors in a Vectorize index, find the ones nearest
to a question, narrow the search by a property, and test the flow without the
network.

`Vectorize` is **experimental**: it carries no compatibility promise yet. Opt in
in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.Vectorize"]
```

An index costs money to query and to keep on your Cloudflare account. What applies
is on Cloudflare's [Vectorize pricing](https://developers.cloudflare.com/vectorize/platform/pricing/)
page, which this guide does not repeat. Put a
[`RateLimit`](/book/guides/effects-and-capabilities/rate-limit-a-route/) in front of
any handler an anonymous caller can reach.

## Declare the index

An index is state your project owns, so it is a named instance, like a `D1`
database. Its dimensions and metric cannot change once it exists, so you write
them where the code that depends on them lives:

```bynk,fragment
context shop.search

consumes bynk
consumes bynk.cloudflare { Ai, Vectorize, Vector, VectorOf, VectorError, QueryOptions, Condition, FilterOp, MetaValue, MetadataScope }

capability Docs = Vectorize @dimensions(768) @metric("cosine") @indexed(["category:string", "year:number"])
```

`@dimensions` is the length of every vector (Cloudflare accepts 32 to 1536) and
`@metric` is `"cosine"`, `"euclidean"` or `"dot-product"`. Nothing ties them to an
embedding model: `Ai.embed` with `@cf/baai/bge-base-en-v1.5` gives 768 numbers and
pairs with `cosine`, and a vector of the wrong length is a `DimensionMismatch`
returned as a value. `@indexed` lists the properties a query may filter on, at
most ten, each `name:type` with the type `string`, `number` or `boolean`. A dot
nests (`author.name:string`).

Embed with one model, and one pooling, for the life of an index: vectors from
different models, or `mean` and `cls` pooling of one model, are not comparable.

## Store documents

Embed the text with [`Ai`](/book/guides/effects-and-capabilities/run-a-model-from-a-worker/),
then write the vector with its metadata. `upsert` takes metadata as JSON text, so
write it with `Json.encode`:

```bynk,fragment
type Meta = { category: String, year: Int }

service index from http {
	on PUT("/doc") (id: String, text: String) -> Effect[HttpResult[String]] by Visitor given Ai, Docs {
		let embedded <- Ai.embed("@cf/baai/bge-base-en-v1.5", [text])
		match embedded {
			Ok([values, ..]) => {
				let written <- Docs.upsert([Vector {
					id: id,
					values: values,
					metadata: Some(Json.encode(Meta { category: "faq", year: 2026 })),
				}])
				match written {
					Ok(mutation) => Ok(mutation.id),
					Err(_) => ServerError("the index is unavailable"),
				}
			},
			_ => ServerError("embeddings are unavailable"),
		}
	}
}
```

`upsert` replaces a vector with the same id in full: it does not merge values or
metadata. `insert` keeps an existing vector and writes only new ids.

To let the compiler encode the metadata, use `upsertValue` with a `VectorOf`,
whose `metadata` is a value of your own type. The type parameter is inferred, and
the metadata is encoded exactly as `Json.encode` would write it:

```bynk,fragment
let written <- Docs.upsertValue([VectorOf {
	id: id,
	values: values,
	metadata: Some(Meta { category: "faq", year: 2026 }),
}])
```

Metadata is a JSON object on the platform, so the type must encode to one: a bare
`String` is a fault. Only writes are typed. Reads give the metadata back as text.

Keep the properties you filter on plain (`String`, `Int`, `Bool`), not `Option`: an
`Option` field is written as a tagged object (`{"kind":"Some","value":"x"}`), exactly
as `Json.encode` writes it, so a filter on `tag:string` would never match it.

## Find the nearest

`query` takes a vector and returns the nearest matches, best first. Its `filter` is
a list of typed conditions that must all hold, so a string from a request can never
become an operator:

```bynk,fragment
service search from http {
	on GET("/search") (text: String) -> Effect[HttpResult[String]] by Visitor given Ai, Docs {
		let embedded <- Ai.embed("@cf/baai/bge-base-en-v1.5", [text])
		match embedded {
			Ok([values, ..]) => {
				let hits <- Docs.query(values, QueryOptions {
					topK: 3,
					returnMetadata: MetadataScope.Indexed,
					filter: [Condition { field: "category", op: FilterEq(MetaText("faq")) }],
				})
				match hits {
					Ok(found) => Ok(Json.encode(found)),
					Err(VectorInvalidInput(message)) => BadRequest(message),
					Err(_) => ServerError("search is unavailable"),
				}
			},
			_ => ServerError("embeddings are unavailable"),
		}
	}
}
```

A `VectorMatch` carries the `score` (a larger cosine score is nearer; a smaller
euclidean distance is nearer), and the metadata as text when you ask for it. A
filter works only on a property in `@indexed`, and only on vectors written after
that property's metadata index existed.

## Read metadata in two steps

Decode what a query returns yourself, with `Json.decode`, instead of asking the
query to decode it. Metadata outlives the type that wrote it: after a release adds
a required field, every older vector fails to decode, and one stale vector would otherwise
fail every query that returns it. Decoding in a second step lets the handler skip,
repair or count it:

```bynk,fragment
match Json.decode[Meta](metadata) {
	Ok(meta) => Some(meta),
	Err(_) => None,
}
```

## Writes are not instant

An `upsert`, `insert` or `deleteByIds` returns a `Mutation { id }`, not `()`. The
platform processes it asynchronously: a write takes some seconds to be visible to
queries. A handler that writes and then queries in the same request will not see
its own write, and nothing in your code should depend on it.

Expect transient empty results too. Reads stay eventually consistent even after
the index reports a write as processed: for about a minute, repeating the same
query can alternate between the full result and an empty one. A test or a
handler that waits for a write and then queries once can still see `[]`, so
retry before concluding that a vector is missing. The same goes for a metadata
index: it is built asynchronously, and a filter on a property that is not yet
listed matches nothing.

## Answer the errors

A `VectorError` is a platform refusal a handler can act on:

- `DimensionMismatch(message)`: a vector of the wrong length.
- `VectorInvalidInput(message)`: a filter, `topK`, batch or metadata the platform refuses.
- `VectorRateLimited`: back off.
- `VectorFailed(message)`: any other refusal, with the platform's message.

Cloudflare adds error cases over time, so a `match` over `VectorError` needs a
wildcard. An index that does not exist is a fault, not a value: the deploy creates
it.

## Deploy it

`bynk deploy` creates the index with the dimensions and metric you declared,
creates the `@indexed` metadata indexes before it pushes the Worker, and never
recreates an index: a mismatch with an existing one fails the deploy, because
recreating would delete every vector. To use an index that already exists, name it
in `bynk.toml`. See [Vectorize indexes](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#vectorize-indexes).

## Test without the network

In a test each `Vectorize` is an in-memory index built from its declaration, so a
retrieval flow (store, filter, rank) runs without Cloudflare. It ranks exactly where
production ranks approximately, and a write is visible at once where production
takes seconds; those are its two differences, so do not take a passing test as proof
that your own writes are visible in production. It enforces the dimensions,
namespaces, the difference between `insert` and `upsert`, all eight filter operators
and the platform's limits as of the date its documentation gives. A filter on a
property that is not in `@indexed` faults, because production silently matches
nothing. A `stub` still replaces any operation:

```bynk,fragment
case "an unavailable index is a value the handler matches" {
	stub Docs.query(_, _) returns Err(VectorFailed("unavailable"))
	let r <- search.GET("/search", "why?") by Visitor
	expect r is ServerError("search is unavailable")
}
```

## Run it locally

Vectorize has no local simulation, so under `bynk dev` a call reaches a real index.
Name an index of your own for each capability in `bynk.toml`. `bynk dev` binds it,
never the one the deploy made, and checks it against your declaration on the first
call:

```toml
[dev.vectorize-index]
Docs = "docs-dev"

[dev]
vectorize = "allow"
```

The first call prints a one-time warning that links to Cloudflare's pricing page,
and goes ahead; `vectorize = "allow"` silences it. A capability with no entry is
unbound, and a call to it fails, naming the entry to add. See
[Binding a Vectorize index under `bynk dev`](/docs/manifest/#binding-a-vectorize-index-under-bynk-dev).

## See also

- [`Vectorize` in the Cloudflare capabilities reference](/book/reference/bynk-capabilities/#vector-indexes-vectorize)
- [Run a model from a Worker](/book/guides/effects-and-capabilities/run-a-model-from-a-worker/)
- [Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#vectorize-indexes)
