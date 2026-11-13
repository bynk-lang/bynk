---
title: Call Workers AI through an adapter
---
**Goal:** run a Workers AI model over Cloudflare's REST API, for code that must
run outside a Worker or that you want to keep portable.

Inside a Worker, prefer the in-Worker binding: [`Ai`](/book/guides/effects-and-capabilities/run-a-model-from-a-worker/)
needs no account id and no API token, and has typed `generate` and `embed` and a
streamed reply. This guide is the portable route, and the [migration](#moving-to-the-ai-binding)
below shows how to move a Worker's consumers to the binding.

## What is and is not reachable

A hand-written adapter's binding receives only its `given` dependencies, not the
Worker's `env`, so it cannot read the Worker's `[ai]` binding (that is what
first-party `Ai` is for). What an adapter *can* do is call Workers AI's REST endpoint through the portable
[`bynk.Fetch`](/book/reference/bynk-capabilities/) capability, authenticated with
a token held in [`bynk.Secrets`](/book/reference/bynk-capabilities/). That is an
ordinary [library adapter](/book/reference/adapters/): no new binding kind, and
it stays portable because it consumes only `bynk`.

The trade-offs are real: each call is an HTTPS round trip to the Cloudflare API
rather than an in-Worker binding, it is subject to the API's rate limits, and it
needs an account id and an API token with Workers AI permission.

## 1. Declare the adapter

The capability is the contract your code sees. Keep it narrow and typed: a prompt
in, the model's text out. The account id and token are read from secrets by the
binding, never passed by callers.

```bynk,fragment
adapter workers_ai {
	binding "./workers_ai.binding.ts"
	consumes bynk { Fetch, Secrets }
	exports capability { Ai }
	exports transparent { AiError }

	type AiError = enum { Unavailable, BadReply, InvalidModel }

	capability Ai {
		fn complete(model: String, prompt: String) -> Effect[Result[String, AiError]]
	}

	provides Ai = WorkersAi given Fetch, Secrets
}
```

## 2. Write the binding

The binding implements `Ai` against the generated interface, so `tsc --strict`
checks it. It reads `CF_ACCOUNT_ID` and `CF_API_TOKEN` through `Secrets`, sends
the request through `Fetch`, and builds results with the emitted constructors
(`Ok`, `Err`, `Some`, `None`, `Method.Post`). Model payloads differ per model; the
text-generation shape is shown. The model id is part of the URL path that carries
your API token, so the binding accepts only a plain `@cf/vendor/name` (or `@hf/…`)
id and refuses anything with `..`, `?`, `#` or extra slashes, rather than trusting
the caller.

```typescript
// workers_ai.binding.ts
import type { Ai } from "./workers_ai.js";
import { AiError } from "./workers_ai.js";
import { Method, type Fetch, type Secrets } from "./bynk.js";
import { Ok, Err, Some, None, type Result } from "./runtime.js";

const MODEL = /^@(cf|hf)\/[a-z0-9_-]+\/[a-z0-9_-][a-z0-9._-]*$/;

export class WorkersAi implements Ai {
  constructor(private deps: { Fetch: Fetch; Secrets: Secrets }) {}

  async complete(model: string, prompt: string): Promise<Result<string, AiError>> {
    // The model id goes into an authenticated URL path: accept only a plain id.
    if (!MODEL.test(model)) return Err(AiError.InvalidModel);

    const account = await this.deps.Secrets.get("CF_ACCOUNT_ID");
    const token = await this.deps.Secrets.get("CF_API_TOKEN");
    if (account.tag !== "Some" || token.tag !== "Some") return Err(AiError.Unavailable);

    const res = await this.deps.Fetch.send({
      method: Method.Post,
      url: `https://api.cloudflare.com/client/v4/accounts/${account.value}/ai/run/${model}`,
      contentType: Some("application/json"),
      authorization: Some(`Bearer ${token.value}`),
      headers: new Map(),
      body: Some(JSON.stringify({ prompt })),
    });
    if (res.tag !== "Ok" || res.value.status !== 200) return Err(AiError.Unavailable);

    try {
      const reply = JSON.parse(res.value.body);
      return typeof reply?.result?.response === "string"
        ? Ok(reply.result.response)
        : Err(AiError.BadReply);
    } catch {
      return Err(AiError.BadReply);
    }
  }
}
```

Supply the two secrets as for any other
([Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/)):
`bynk deploy` finds literal `Secrets.get("…")` names in the source, but not names
read inside a binding, so pass them with `--secrets-file` or `--secret` explicitly.

## 3. Consume it

Consumers see an ordinary capability. Treat the model's text as untrusted input
like any other external value.

```bynk,fragment
context assistant {
	consumes workers_ai { Ai, AiError }

	service summarise {
		on call(text: String) -> Effect[String] given Ai {
			let reply <- Ai.complete("@cf/meta/llama-3.1-8b-instruct", "Summarise: \(text)")
			match reply {
				Ok(summary) => summary,
				Err(_) => "(no summary available)",
			}
		}
	}
}
```

## Test it without the network

A `stub` replaces the capability at the seam, so a test never calls the API:

```bynk,fragment
stub Ai.complete(_, _) returns Ok("a short summary")
```

## Without TypeScript

If you would rather not write a binding, the same call fits in a Bynk
[provider](/book/guides/effects-and-capabilities/compose-a-provider/) over
`Fetch` and `Secrets`. This version validates the model id, returns the raw response body for a `200`
and leaves decoding to the caller:

```bynk
context assistant.ai

consumes bynk { Fetch, Secrets }

capability Ai {
	fn run(model: String, body: String) -> Effect[Result[String, String]]
}

provides Ai = CloudflareAi given Fetch, Secrets {
	fn run(model: String, body: String) -> Effect[Result[String, String]] {
		let account <- Secrets.get("CF_ACCOUNT_ID")
		let token <- Secrets.get("CF_API_TOKEN")
		match account {
			None => Effect.pure(Err("missing CF_ACCOUNT_ID")),
			Some(a) => match token {
				None => Effect.pure(Err("missing CF_API_TOKEN")),
				Some(t) => if !model.matchesRegex("@(cf|hf)/[a-z0-9_-]+/[a-z0-9_-][a-z0-9._-]*") {
					Effect.pure(Err("invalid model id"))
				} else {
					let res <- Fetch.send(Request {
						method: Post,
						url: "https://api.cloudflare.com/client/v4/accounts/\(a)/ai/run/\(model)",
						contentType: Some("application/json"),
						authorization: Some("Bearer \(t)"),
						headers: Map.empty(),
						body: Some(body),
					})
					match res {
						Ok(r) => if r.status == 200 {
							Effect.pure(Ok(r.body))
						} else {
							Effect.pure(Err("HTTP \(r.status)"))
						},
						Err(_) => Effect.pure(Err("request failed")),
					}
				},
			},
		}
	}
}
```

## Moving to the `Ai` binding

If your Worker can use the first-party binding, move the adapter's consumers to
it and delete the adapter, its two secrets and the `--secrets-file` entries:

1. In `bynk.toml`, allow the experimental capability:
   `[unstable] allow = ["bynk.cloudflare.Ai"]`.
2. In each consumer, replace `consumes workers_ai { Ai, AiError }` with
   `consumes bynk.cloudflare { Ai, AiError }`. A context that consumes both for a
   while is refused with `bynk.consumes.capability_name_clash`, because each
   unit exports an `Ai`. Consume `bynk.cloudflare` without the braces instead, and
   write `given bynk.cloudflare.Ai` and `bynk.cloudflare.Ai.generate(…)` until the
   adapter is gone.
3. Replace each `Ai.complete(model, prompt)` with
   `Ai.generate(model, request)`, passing a `GenerateRequest` whose `messages`
   holds the prompt as a `User` message, and read `generation.text` where you
   read the string. A model that needs another input shape goes through
   `Ai.run(model, input)` with the JSON input as text.
4. Match the new `AiError` (`ModelNotFound`, `InvalidInput`, `QuotaExceeded`,
   `Overloaded`, `BadReply`, `AiFailed`) in place of the adapter's
   `Unavailable`, `BadReply` and `InvalidModel`, with a wildcard arm.
5. In tests, replace `stub Ai.complete(_, _) returns …` with
   `stub Ai.generate(_, _) returns …`.

The adapter stays supported for code that runs outside a Worker, where there is no
binding to read.

## See also

- [Run a model from a Worker](/book/guides/effects-and-capabilities/run-a-model-from-a-worker/)
- [Wrap a library as an adapter](/book/guides/effects-and-capabilities/wrap-a-library/)
- [Adapters reference](/book/reference/adapters/)
- [First-party `bynk` capabilities](/book/reference/bynk-capabilities/)
