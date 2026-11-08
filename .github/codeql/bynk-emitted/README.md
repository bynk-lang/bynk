# CodeQL over the emitted TypeScript

Runs SAST over the TypeScript that `bynk compile --target workers` **emits**,
not over the repo's own sources (#255). A flaw in a codegen template lives only
in the output, so `codeql.yml` never sees it.

- [`scripts/codeql-emitted.sh`](../../../scripts/codeql-emitted.sh) does the
  work and runs locally.
- [`codeql-emitted.yml`](../../workflows/codeql-emitted.yml) runs it nightly
  (08:20 UTC) and on manual dispatch.
- It is **out of band on purpose**. It has no PR, merge-queue or push trigger,
  is not part of `CI green`, and is not run by any release job.

## Why custom models

CodeQL's stock JavaScript/TypeScript models know Express and Node, not a
Cloudflare module Worker. Over Bynk's output they find **no user input at all**.
The #255 experiment got zero results from 89 queries, and a scan with only the
stock models would pass whatever the emitter produced.

[`BynkEmitted.qll`](BynkEmitted.qll) adds two models, and only two:

| model | what it is |
|---|---|
| `WorkerFetchRequest` | the `request` parameter of `export default { fetch(request, env, ctx) }`, as a remote source (and as a redirect source) |
| `ResponseLocationHeader` | `new Response(_, { headers: { location: v } })` (how `httpResultToResponse` writes the 3xx `HttpResult`s), or `headers.set/append("location", v)`, as a URL-redirect sink |

The canaries showed which other models are **not** needed:

- **No `Fetch.send` sink.** `FetchProvider` ends in the global
  `fetch(url, …)`, already a `js/request-forgery` sink. Default call resolution
  follows the DI wiring (`compose.ts` → `deps.Fetch.send`) to it.
- **No `Result`/`Option` flow summaries.** Default taint tracking already
  follows the emitted `{ tag, value }` codecs and their switch-unwraps
  (`ssrf-option` and `redirect-result` cross them).
- **No KV model.** CodeQL does not follow taint across storage, so a
  `Kv.put` → `Kv.get` round trip would need `Kv.get` modelled as a source.
  - The only KV-to-sink flow in the corpus is `link-shortener` redirecting to a
    URL a user stored, which is that example's purpose. A `Kv.get` source would
    flag it permanently.
  - Stored-data (second-order) flows are therefore out of scope here.

**Why QL, not models-as-data.** A data-extension row names code by npm package,
or by a `file:` path that another file imports. The emitted runtime is reached
only through relative imports, and the Worker entry (`workers/<ctx>/index.ts`)
is imported by nothing. So no row can name it.

**Why the two queries are copied.** A `.qll` extension only affects queries
that import it, so [`queries/`](queries/) holds `js/request-forgery` and
`js/server-side-unvalidated-url-redirection` re-run with the models imported:

- `bynk/emitted-request-forgery`
- `bynk/emitted-url-redirection`

The stock suite still runs alongside them, but **only these two queries see
user input**. The other stock taint queries (code injection, XSS, path
injection, …) do not import the models, so over the emitted code they are as
blind as in the #255 experiment. Only the stock queries that match on code
shape alone (ReDoS, insecure randomness, …) add anything. To cover another
vulnerability class, copy its query into `queries/` with `import BynkEmitted`
and add a canary for it.

## Canaries

[`canaries/`](canaries/) holds deliberately vulnerable Bynk projects. Each must
be flagged by the query named in [`canaries/expected.tsv`](canaries/expected.tsv),
at a location inside its own emitted tree. If one is not flagged, the models
have stopped seeing the runtime: the script exits 1 and the nightly opens an
issue.

| canary | bug | crosses |
|---|---|---|
| `ssrf-body` | body field → `Fetch.send` URL | DI-wired `Fetch` |
| `ssrf-option` | `Option[String]` body field → `Fetch.send` URL | the `Option` codec |
| `redirect-query` | `?next=` → `Found` | the query decoder, `httpResultToResponse` |
| `redirect-result` | body field → `Result` → `SeeOther` | the `Result` codec |

Canary results are stripped before upload (`results-examples.sarif`), so the
deliberate bugs never become open alerts. When a canary is not flagged, nothing
is uploaded, so blind models can't mark real alerts as fixed.

## Running it locally

```sh
scripts/codeql-emitted.sh /tmp/emitted   # needs `bynk` and `codeql` on PATH (or BYNK=/CODEQL=)
cat /tmp/emitted/summary.md
```

The examples-only SARIF is uploaded with the category `emitted-ts`.

- Its paths (`examples/<name>/workers/…`) are relative to the compiled output,
  not the repository. A code-scanning alert therefore points at generated code
  that isn't in the tree.
- To read an alert, run the script and look under `stage/`.
- The fix goes in the emitter or runtime template that produced the code, never
  in the output.

## Adding a canary

1. Add a project under `canaries/<name>/` (a `bynk.toml` + `src/`).
2. Add a row to `canaries/expected.tsv`.
3. Run the script.
4. If the new canary is not flagged, the model is missing a piece. Add it to
   `BynkEmitted.qll`, as narrowly as the canary needs.
