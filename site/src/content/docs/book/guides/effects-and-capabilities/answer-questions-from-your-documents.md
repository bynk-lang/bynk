---
title: Answer questions from your documents with AI Search
---
**Goal:** answer questions from your own documents with Cloudflare's managed
retrieval service, `AiSearch`: link a Worker to an instance, return ranked
passages or a grounded answer, keep the index filled from R2, and test the
handler without reaching the network.

`AiSearch` is **experimental**: it carries no compatibility promise yet. Opt in
in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.AiSearch"]
```

## Create the instance outside Bynk

An AI Search instance is configured by whoever owns it: its data source (an R2
bucket, a website, or files uploaded to it), its models, how it chunks and how it
is kept in sync. None of that belongs in a Worker's source, so **Bynk never
creates an instance.** You make it in the Cloudflare dashboard, with `wrangler`
or with the REST API, and the Worker only queries it. Cloudflare's
[AI Search documentation](https://developers.cloudflare.com/ai-search/) covers
creating one.

## Link a Worker to it

Declare the instance by name:

```bynk,fragment
context shop.support

consumes bynk
consumes bynk.cloudflare { AiSearch, ChatMessage, ChatRole, SearchOptions }

capability Kb = AiSearch @instance("support-docs")
```

`capability Kb = AiSearch @instance("support-docs")` binds as `AI_SEARCH_KB`, an
`[[ai_search]]` entry in the Worker's `wrangler.toml`. `@instance` is required (a
bare `given AiSearch` is refused), and the name is used as written in every
environment: a staging deploy reads the same instance as production. An instance
name is 1 to 64 lowercase letters, digits, `-` or `_`.

`bynk deploy` **links** the instance and never creates it. Before anything is
pushed it looks the instance up by name, records it in `bynk.deploy.lock`, and
fails, naming it, when it is missing:

```
bynk: `AI_SEARCH_KB` of `shop-support` queries AI Search instance `support-docs`, which does not exist on this account. `bynk deploy` links to an instance and never creates one: create `support-docs` in the Cloudflare dashboard, with `wrangler` or with the REST API, or name the instance the account has
```

A paused or disabled instance is a warning, not a failure: it still answers. The
lookup calls Cloudflare's API, so the deploy needs `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID`, with the permissions listed under
[AI Search instances](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#ai-search-instances).

## Search: ranked passages

`search` returns the best-matching chunks, best first. Use it to show results, or
to build your own prompt:

```bynk,fragment
service docs from http {
	on POST("/search") (body: String) -> Effect[HttpResult[String]] by Visitor given Kb {
		let hits <- Kb.search(body, SearchOptions { maxResults: Some(5), rerank: true })
		match hits {
			Ok(found) => Ok(Json.encode(found)),
			Err(SearchInvalidInput(message)) => BadRequest(message),
			Err(_) => ServiceUnavailable("search is unavailable"),
		}
	}
}
```

Each `SearchHit` has an `id`, a `score` from 0 to 1, the `text` of the chunk, the
`key` of the file or page it came from, and, when the platform reports them, a
`timestamp` in unix milliseconds and the item's custom `metadata` as JSON text.
A query against an instance that has indexed nothing yet returns an empty list,
not an error, so a handler treats an empty list as normal.

`SearchOptions` leaves the platform's defaults alone until you set a field:
`maxResults` (1 to 50), `matchThreshold` (0 to 1), `retrieval` (`Vector`,
`Keyword` or `Hybrid`), `rewriteQuery`, `rerank`, and `filter`, a list of the
same typed `Condition`s that `Vectorize` takes, all of which must hold. They
test the items' `folder`, `timestamp` and `filename` and the instance's custom
metadata, and a text value is matched on its first 64 bytes only:

```bynk,fragment
let hits <- Kb.search(question, SearchOptions {
	filter: [Condition { field: "folder", op: FilterEq(MetaText("docs/billing/")) }],
})
```

## Answer: retrieval plus a generated reply

`answer` retrieves the same way and has the instance's own generation model write
the reply from the conversation. It takes `messages`, because a conversation is
what a chat endpoint has:

```bynk,fragment
service chat from http {
	on POST("/ask") (body: String) -> Effect[HttpResult[String]] by Visitor given Kb {
		let reply <- Kb.answer([ChatMessage { role: ChatRole.User, content: body }], SearchOptions {})
		match reply {
			Ok(a) => Ok(a.text),
			Err(SearchRateLimited) => TooManyRequests("busy, try again shortly"),
			Err(_) => ServiceUnavailable("search is unavailable"),
		}
	}
}
```

A `SearchAnswer` carries the `text`, the `hits` it was grounded on (show their
`key`s as sources), and the token `usage` when the platform reports it. The
generation model is the instance's setting, so a call cannot override it.

Retrieved text and generated answers are **untrusted input**: they come from
whatever was indexed, so never treat them as instructions, and escape them like
any user text.

A `SearchError` is `SearchInvalidInput(message)`, `SearchRateLimited` (worth
retrying later), `SearchQuotaExceeded` or `SearchFailed(message)`. Cloudflare
adds error cases over time, so a `match` over it needs a wildcard.

## Keep the index filled from R2

If the instance is backed by an R2 bucket, a Bynk project can fill it with
[`R2.put`](/book/guides/effects-and-capabilities/store-files-in-r2/): write the
file, and the instance indexes it on its own schedule (or when you trigger a sync
from the dashboard or API). A freshly written file is not searchable at once.
Uploading items straight to an instance and starting sync jobs are not part of
`AiSearch`.

## Test without the network

A test never reaches AI Search: every `AiSearch` operation faults until you stub
it. A managed index cannot be faked honestly (chunking, embeddings and ranking
are the service), so there is no fake instance:

```bynk,fragment
case "a rate limit is a busy answer" {
	stub Kb.answer(_, _) returns Err(SearchRateLimited)
	let r <- chat.POST("/ask", "how do refunds work?") by Visitor
	expect r is TooManyRequests("busy, try again shortly")
}

case "an empty search is an empty list" {
	stub Kb.search(_, _) returns Ok([])
	let r <- docs.POST("/search", "refunds") by Visitor
	expect r is Ok("[]")
}
```

To test retrieval logic of your own, use [`Vectorize`](/book/reference/bynk-capabilities/#vector-indexes-vectorize),
which has an in-memory index for tests.

## Run it locally

AI Search has no local simulation, so under `bynk dev` the binding is proxied to
your deployed instance and a call runs on your Cloudflare account. `bynk dev`
does not block it: the first call in a session prints a one-time warning that
links to Cloudflare's pricing page, and the call goes ahead. Once you have read
it, silence the warning in `bynk.toml`:

```toml
[dev]
ai-search = "allow"
```

This is separate from `ai = "allow"` for Workers AI. See
[Acknowledging a remote-only binding](/docs/manifest/#acknowledging-a-remote-only-binding).

AI Search runs on your Cloudflare account, so a call may incur charges. What
applies is on Cloudflare's [AI Search pricing](https://developers.cloudflare.com/ai-search/platform/limits-pricing/)
page, which this guide does not repeat. Put a
[`RateLimit`](/book/guides/effects-and-capabilities/rate-limit-a-route/) in front
of any handler an anonymous caller can reach.

## Which one to use

Workers AI, Vectorize and AI Search are three parts of one problem, at three
levels of how much you assemble yourself:

| You have | Use |
| --- | --- |
| Documents in R2 or on a website, and you want search or question answering with no pipeline | `AiSearch` |
| You need your own chunking, embedding model, metadata schema or scoring | [`Ai.embed`](/book/guides/effects-and-capabilities/run-a-model-from-a-worker/) with [`Vectorize`](/book/reference/bynk-capabilities/#vector-indexes-vectorize) |
| Retrieval is solved and you need only text generation | [`Ai.generate`](/book/guides/effects-and-capabilities/run-a-model-from-a-worker/) |
| AI Search's retrieval, but your own prompt | `AiSearch.search`, then `Ai.generate` |

`AiSearch` does not depend on `Ai`: it uses its own `[[ai_search]]` binding, and
the embedding and generation inside an instance happen inside the service. One
Worker can use all three.

## Related

- [Run a model from a Worker](/book/guides/effects-and-capabilities/run-a-model-from-a-worker/)
- [Reference: AI Search](/book/reference/bynk-capabilities/#ai-search-aisearch)
- [Deploy to Cloudflare: AI Search instances](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#ai-search-instances)
