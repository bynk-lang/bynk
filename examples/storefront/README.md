# Storefront

A small `from http` API and the typed front end that calls it, end to end.

What it shows:

- **`bynk client`** — `[build.client]` in `bynk.toml` writes `web/src/api.ts`, a typed
  TypeScript module with one function per route, the server's refinements applied in
  the client, and `Option` as `T | null`.
- **A discriminated result** — every call returns the route's own answers plus a
  `CallFailure`; `web/src/storefront.ts` handles each with a `switch` on `kind`.
- **A drift gate** — `bynk client --check` fails when the committed client is stale.

The `web/` front end has no framework, so the same functions work in SvelteKit, Next.js
or a plain page; see the Book's
[Call your API from a front end](https://bynk-lang.org/book/guides/entry-points/call-from-a-front-end/).
The system test builds the Worker, type-checks `web/` under `tsc --strict`, and runs
`web/src/storefront.ts` against the real Worker in process.

```sh
bynk dev           # serve the API on http://localhost:8787
bynk client        # regenerate web/src/api.ts
```
