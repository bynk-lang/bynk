---
level: minor
changelog: `bynk client`'s generated module exports `viaBinding`, which sends a client's calls through a Cloudflare Service Binding, with a verified recipe for SvelteKit and Next.js on Cloudflare (#3508 slice BC5)
probes: ts_writes
---

## ADR: bynk-client-service-binding
title: `bynk client`: calling a Bynk Worker through a Service Binding
summary: A Service Binding's `fetch` has `fetch`'s shape, so the client's `fetch` option is the seam and `viaBinding` calls the binding correctly, with no second transport option

**Context.** The proposal's Decision E sketched a `Transport` seam, `(request: Request) => Promise<Response>`, so a front end on Cloudflare (SvelteKit or Next.js on Workers) could call a Bynk Worker through a Service Binding: no public hop, no CORS, no egress. This is slice BC5 and server-side only, since a binding exists only in a Worker.

**Decision.**

- *No second option.* A binding's `env.API.fetch(input, init)` takes what `fetch` does, and the client already takes an injected `fetch`, so a `transport` option would be a second spelling of the same seam. The one trap is the detached method: `fetch: env.API.fetch` throws "Illegal invocation" (verified, and returned as a `Transport` failure). The generated module therefore exports `viaBinding(binding, baseUrl?)`, which returns `{ baseUrl, fetch }` calling the binding as a method, to spread into `createClient`. It is in the generated file for every client and a bundler drops it when unused. The coordinator agreed.
- *The URL.* Cloudflare's HTTP binding page requires "a valid and fully-qualified URL with a hostname" for a request built by hand. A binding ignores the host, so `baseUrl` defaults to the reserved `https://bynk.invalid`; the Worker sees it in `request.url`.
- *Authentication.* A binding call carries no `Origin` and no `Sec-Fetch-Site`, so a state-changing route under a signed cookie answers `403` unless the call sets `origin`, the option BC3 verified. The system test shows a binding call with no `Origin` and with one. The front end's server must authenticate its own caller.
- *Framework facts, verified from the shipped adapters* (their documentation sites were unreachable from this environment): `@sveltejs/adapter-cloudflare` 7 passes `platform.env` to the server; version 8 (Kit 3) instead exposes `env` through `cloudflare:workers`, which a prerenderable route cannot use. `@opennextjs/cloudflare` 1.x gives `getCloudflareContext().env`, with an async form for static routes and `initOpenNextCloudflareForDev()` for `next dev`. The binding is declared with `[[services]]`. The reference records these as read from the adapters at those versions.
- *Types.* The generated client type-checks with `@cloudflare/workers-types`' `Fetcher` passed to `viaBinding` when the DOM library is also loaded (a SvelteKit or Next.js project has it), and it does not under `@cloudflare/workers-types` alone, which declares the fetch classes without the `globalThis` members the client names. That limit predates BC5 and is documented.
- *Verified.* Two `wrangler dev` processes under `workerd`, the front end bound to the Bynk Worker by `[[services]]`, ran the client for a JSON route, a raw route, a server-sent event stream, `origin`, a forwarded `Cookie`, and the detached-method failure.

**Consequences.** A Cloudflare-hosted front end calls its Bynk Worker with the same typed client and one extra line. Nothing about the generated text of existing routes changes apart from the helper in the runtime, so the goldens are blessed.
