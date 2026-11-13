---
title: Run a model from a Worker
---
**Goal:** call a Workers AI model from a handler without an API token, generate
text, embed text, run any other model, stream a reply to a client, and test it
without reaching the network.

`Ai` is **experimental**: it carries no compatibility promise yet. Opt in in
`bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.Ai"]
```

## Use it

`Ai` is the Worker's one Workers AI binding, so it is used bare (`given Ai`) and
has no named instances. A Worker that reaches it declares the `[ai]` binding;
one that does not declares nothing, and there is nothing for `bynk deploy` to
create. Compare it with the
[REST adapter](/book/guides/effects-and-capabilities/call-workers-ai-through-an-adapter/),
which needs an account id and an API token held in secrets: the binding needs
neither.

Workers AI runs on your Cloudflare account, so a call may incur charges. What
applies is on Cloudflare's [Workers AI pricing](https://developers.cloudflare.com/workers-ai/platform/pricing/)
page, which this guide does not repeat. Put a [`RateLimit`](/book/guides/effects-and-capabilities/rate-limit-a-route/)
in front of any handler an anonymous caller can reach.

## Generate text

`generate` takes the conversation so far and the settings the chat models share,
and returns the model's text and, when the platform reports it, the tokens used:

```bynk,fragment
context shop.assistant

consumes bynk
consumes bynk.cloudflare { Ai, AiError, ChatMessage, ChatRole, GenerateRequest }

service ask from http {
	on POST("/ask") (question: String) -> Effect[HttpResult[String]] by Visitor given Ai {
		let request = GenerateRequest {
			messages: [
				ChatMessage {
					role: ChatRole.System,
					content: "Answer in one sentence.",
				},
				ChatMessage {
					role: ChatRole.User,
					content: question,
				},
			],
			maxTokens: Some(128),
		}
		let reply <- Ai.generate("@cf/meta/llama-3.2-1b-instruct", request)
		match reply {
			Ok(generation) => Ok(generation.text),
			Err(ModelNotFound) => NotFound,
			Err(_) => ServerError("the assistant is unavailable"),
		}
	}
}
```

The model id is a plain string, so a model Cloudflare ships tomorrow works the
day it appears. The text is untrusted input like any other: do not act on it as
if you had written it. An empty `messages` is a fault; a request a model refuses
is an `AiError`.

## Embed text

`embed` takes a batch of texts and returns one vector per text, in order, all of
the same length:

```bynk,fragment
service index from http {
	on POST("/vectors") (a: String, b: String) -> Effect[HttpResult[Int]] by Visitor given Ai {
		let reply <- Ai.embed("@cf/baai/bge-base-en-v1.5", [a, b])
		match reply {
			Ok(vectors) => Ok(vectors.length()),
			Err(_) => ServerError("embeddings are unavailable"),
		}
	}
}
```

Embed with one model, and one pooling, for the life of an index: vectors from
different models, or different pooling, are not comparable. `embed` uses the
platform's default pooling, so that stays true by construction. It does not check
how many texts one call may carry, because the limit is the model's: a call over
it is `InvalidInput`.

## Run any other model

Translation, classification, summarisation and the rest each have an input shape
of their own, so `run` takes the model's JSON input as text and returns its JSON
output as text. Build the input with `Json.encode`, and read the reply in a
second step with `Json.decode`, which gives a matchable error rather than a fault
when a model's output is not the shape you expected:

```bynk,fragment
type TranslateIn = { text: String, source_lang: String, target_lang: String }
type TranslateOut = { translated_text: String }

service translate from http {
	on POST("/translate") (text: String) -> Effect[HttpResult[String]] by Visitor given Ai {
		let input = Json.encode(TranslateIn { text: text, source_lang: "en", target_lang: "fr" })
		let reply <- Ai.run("@cf/meta/m2m100-1.2b", input)
		match reply {
			Ok(json) => match Json.decode[TranslateOut](json) {
				Ok(out) => Ok(out.translated_text),
				Err(_) => ServerError("the translation was not in the expected shape"),
			},
			Err(_) => ServerError("the translator is unavailable"),
		}
	}
}
```

A model that answers with binary data (an image, say) is an `AiFailed`: `run`
carries JSON replies only.

## Stream a reply to the client

`runStream` is `run` with the reply streamed. Each element is one event of the
model's own stream, as the JSON text of that event, so serve it with `Streaming`
and the client sees what Workers AI sends:

```bynk,fragment
service chat from http {
	on GET("/story") () -> Effect[HttpResult[String]] by Visitor given Ai {
		let started <- Ai.runStream("@cf/meta/llama-3.2-1b-instruct", "{\"prompt\": \"Tell me a story\"}")
		match started {
			Ok(events) => Streaming(events),
			Err(_) => ServerError("the model is unavailable"),
		}
	}
}
```

A refusal before the stream starts (an unknown model, a usage limit) is the
`Err`, so the handler can answer with an ordinary response. A failure after the
first event faults the stream and aborts the response, as a failing producer does
for any stream. A handler that wants text rather than events maps the stream and
decodes each event. The shape of an event is the model's: for a chat model, a
run on a real account showed each event as a JSON object with a `response` text
delta (and a padding field `p`), the last JSON event also carrying the
cumulative `usage`, and the stream ending with `data: [DONE]`. That is observed,
not documented by Cloudflare, so check the model's page and do not build on
fields it does not name. A stream is read once and cannot cross a service boundary, so the handler
that calls `runStream` is the one that serves it.

## Answer the errors

An `AiError` is a platform refusal a handler can act on:

- `ModelNotFound`: the id is unknown or the model has been retired.
- `InvalidInput(message)`: the platform refused the request as malformed or too
  large.
- `QuotaExceeded`: a usage limit of the account, or a model its plan does not
  include. Back off, alert, change the plan.
- `Overloaded`: temporary capacity. Worth a retry.
- `BadReply(message)`: `generate` or `embed` got an answer that is not the shape
  they promise.
- `AiFailed(message)`: any other refusal, with the platform's message.

Cloudflare adds error codes over time, so a `match` over `AiError` needs a
wildcard.

## Test without the network

A test never reaches Workers AI: every `Ai` operation faults until you stub it,
and there is no fake model, since a made-up reply would make a test pass for the
wrong reason:

```bynk,fragment
case "a bad reply is a value the handler matches" {
	stub Ai.generate(_, _) returns Err(BadReply("generate: the reply has no text `response`"))
	let r <- ask.POST("/ask", "why?") by Visitor
	expect r is ServerError("the assistant is unavailable")
}

case "a streamed reply is re-served" {
	stub Ai.runStream(_, _) returns Ok(Stream.of(["{\"response\":\"once\"}", "{\"response\":\" upon\"}"]))
	let r <- chat.GET("/story") by Visitor
	match r {
		Streaming(body) => {
			let events <- body.collect()
			expect events.length() == 2
		}
		_ => expect false
	}
}
```

## Run it locally

Workers AI has no local simulation, so under `bynk dev` a call runs the model on
Cloudflare, against your account. `bynk dev` does not block it: the first call in
a session prints a one-time warning that links to Cloudflare's pricing page, and
the call goes ahead. Once you have read it, silence the warning in `bynk.toml`:

```toml
[dev]
ai = "allow"
```

See [Acknowledging a remote-only binding](/docs/manifest/#acknowledging-a-remote-only-binding).
Read the setting at the start of a session: a change takes effect the next time
`bynk dev` starts.

## See also

- [`Ai` in the Cloudflare capabilities reference](/book/reference/bynk-capabilities/#workers-ai-ai)
- [Call Workers AI through an adapter](/book/guides/effects-and-capabilities/call-workers-ai-through-an-adapter/), the REST route for code that must run outside a Worker
- [Rate-limit a route](/book/guides/effects-and-capabilities/rate-limit-a-route/)
