# Typed HTML — templates checked at compile time, safe by construction

- **Status:** Settled (27 November 2026). The maintainer ruled on the four
  points the draft left open on 27 November 2026: the `template` declaration,
  the escape hatch's name (`Html.unsafe`), its use in dependencies, and a
  hash-only CSP. Every other decision was accepted in review as recommended
  (§17). The change is additive and enters as experimental (§14). Spine
  issue: #3927, which carries the live slice status.
- **Realises:** the HTTP security review's finding on HTML escaping. A page a
  Bynk service serves is built by string replacement today, and escaping is the
  author's job. This track makes escaping the compiler's job, by context. It
  builds on [ADR 0993](../decisions/0993-html-escape-commons.md) (#3924:
  `bynk.html`'s `escapeText` and `escapeAttr`) and
  [ADR 0954](../decisions/0954-html-response-security-headers.md) (#3923: the
  HTML response headers), both shipped.
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md), run per
  [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md). It
  qualifies on all three axes (§1.2).
- **Governing records:**
  [ADR 0608](../decisions/0608-embedded-text-in-the-language.md) and
  [ADR 0609](../decisions/0609-embedded-text-content-rules.md) (embedded text;
  0608 F reserves a separate `template` declaration for parsed files),
  [ADR 0143](../decisions/0143-raw-http-body.md) (`Raw`, and the stance that
  Bynk does not template HTML, which this track revises),
  [ADR 0164](../decisions/0164-http-security-headers.md) and
  [ADR 0954](../decisions/0954-html-response-security-headers.md)
  (`security { }`; 0954 D leaves a policy built from the page to this track),
  [ADR 0964](../decisions/0964-response-headers-on-http-result.md) (a handler
  cannot set the security headers),
  [ADR 0993](../decisions/0993-html-escape-commons.md) (`bynk.html`),
  [ADR 0659](../decisions/0659-url-commons.md) (`bynk.url` and its `Url`
  record), [ADR 0075](../decisions/0075-string-interpolation.md) (`\(…)`
  holes), [ADR 0739](../decisions/0739-multiline-string-language-tags.md)
  (value-neutral language tags),
  [ADR 1038](../decisions/1038-opaque-secret.md) (the opaque `Secret`),
  [ADR 0719](../decisions/0719-snapshot-testing.md) (snapshots),
  [ADR 1078](../decisions/1078-httpresult-is-extensible.md) (`HttpResult` is
  extensible). Related tracks and rulings:
  [`pass-through-http.md`](pass-through-http.md) (#3752: `Forward`, and the
  fill-not-overwrite rule for HTML headers, D9), the #911 ruling of
  27 November 2026 (a message code is scoped to the unit that declares it),
  and #3519 (HTMLRewriter, not started). Five ADRs land up front (§16).
- **Code on `main`:** cited by symbol, never by line, as of `24c0475e`. The
  book's reference pages are cited from `site/src/content/docs/reference/`.

## 1. The theme

### 1.1 What this track settles

Bynk serves HTML pages. The guide *Serve a page from an HTML file* reads a page
shell with `embed`, fills its `{{markers}}` with `String.replace` and
`escapeText`/`escapeAttr`, and returns `Raw(…, "text/html; charset=utf-8")`.
The escapes are right only where the author put them. One forgotten escape
around request data is a cross-site scripting (XSS) hole, and even a correct
escape is wrong in some positions: a URL attribute (`javascript:`), an
unquoted attribute, `<script>`, `<style>`, an event handler attribute.

An embedded file is a compile-time constant. The compiler can tokenise its
HTML while building and know the context of every placeholder. This track
turns that into a guarantee:

1. **`Html` is a type, not a `String`.** Data reaches markup only through the
   escape its context needs. Markup is inserted raw only through
   `Html.unsafe(markup, reason)`, which is explicit, has a literal reason, and
   is listed per package (§5.6).
2. **Templates are checked.** A `template` declaration reads a file from
   `embeds/`, finds its placeholders, and checks them against a declared
   record: every placeholder known, every field used, each field's type
   accepted by every context it appears in.
3. **Fragments are checked the same way.** An `html"…"` literal is tokenised at
   compile time, and each hole is typed by its context.
4. **URLs have a type.** `Href` is the only thing a URL attribute takes as a
   whole value, and its constructor refuses every scheme but `http`, `https`,
   `mailto` and `tel` (§5.4).
5. **Pages have a response.** `Page(html)` sends `text/html; charset=utf-8` and
   gets ADR 0954's HTML defaults.
6. **No run-time parser.** A template compiles to string concatenation and
   escape calls (§8).

**End state when the track retires:** data can only reach markup escaped for
its context; markup can only be inserted deliberately and visibly; a secret
can never reach a page without an explicit `reveal()`; placeholder mistakes are
compile errors, shown in the editor at the template line; and the serve-a-page
guide needs no hand-written escaping and no "leftover `{{`" test.

### 1.2 Why a track (the ADR 0076 trigger)

- **Multi-increment.** The tokeniser and context model, the `Html` type and
  `template`, `Href`, the `html"…"` literal, `Page`, tooling, and the book.
  Each is reviewable on its own, in a forced order.
- **Surface not yet settled** when the spine opened: placeholder syntax, the
  declaration form, what counts as a URL, how the escape hatch is gated, and
  the relation to HTMLRewriter (#3519). All are now settled (§17).
- **Security boundary.** XSS. A context missed, or an escape hatch that is too
  easy, gives false confidence, which is worse than none.

## 2. What's true today

### 2.1 Pages are strings

- An embed is a `String`; `reference/embeds.md` says "`\(` and `{{ }}` mean
  nothing to the language, so substituting into a template is ordinary
  `String` code, and nothing is escaped for you", and recommends a test that no
  `{{` is left.
- `bynk.html` (ADR 0993, `bynk-std/lib/core/bynk.html.bynk`, registered as
  `HTML_UNIT` and `BYNK_HTML_SRC` in `bynk-std/src/lib.rs`, id block 13)
  declares two pure, bare functions and no types: `escapeText` (`&`, `<`, `>`,
  `&` first) and `escapeAttr` (those plus `"`, `'` and a backtick). Their doc
  comments say what neither covers: unquoted and URL attributes, `<script>`,
  `<style>`. ADR 0993 kept them bare rather than `Html.escapeText` because an
  `Html.` prefix "would need a placeholder `Html` type before the typed-HTML
  track defines the real one". That type is this track's.
- `book/guides/entry-points/serve-a-page.md` now calls those functions, but
  still inserts `content` unescaped because "`content` is markup you built".
  Nothing in the types records which strings are markup.
- The guide fills `href="{{style}}"` with `style.url`. That is safe only because
  an asset's URL is a compiler-made path (`reference/assets.md`). Nothing stops
  a request value taking its place.
- A page is served as `Raw(Bytes.fromUtf8(…), "text/html; charset=utf-8")`. The
  content type is an unvalidated `String`.

### 2.2 The records this track revises

- ADR 0608 F: "a later `template` declaration would parse holes and check their
  types and escapes. Different contracts are different declarations, so
  `embed` never starts to interpret `{{ }}`." The seam is reserved in prose;
  `template` is not a keyword (`bynk-syntax/src/keywords.rs`; the word appears
  only in the message-entry docs, `MessageEntry` in `bynk-syntax/src/ast.rs`).
- `reference/http.md`, *Raw responses*: "Bynk serves *bytes with a
  content-type*; it does **not** template HTML (that is the frontend tier)."
  And under *What is deliberately excluded*: "a generated
  `Content-Security-Policy` for scripts and styles, which needs knowledge of
  the page's own markup that Bynk does not have", and "A policy built from the
  page itself (nonces, hashes) is for a typed-HTML feature, not this section."
  ADR 0954 D says the same.

### 2.3 The machinery a template needs

- **Reading the text.** A build reads every declared embed's text through
  `read_declared_embeds` (`bynk-check/src/embeds.rs`). The editor's analysis,
  `analyse_project_inner` (`bynk-check/src/analysis.rs`), lists `embeds/`
  through `bynk_project::assets::list_embed_files` and checks existence with
  `phase_embed_files`; it does not read the text. A template must be read in
  both, or the editor cannot show template errors.
- **Size caps.** `MAX_EMBED_BYTES` (256 KiB) and `MAX_EMBEDS_TOTAL_BYTES`
  (1 MiB) in `bynk-check/src/embeds.rs`, ADR 0609 C, `bynk.embed.too_large`.
- **Emitting the text.** `emit_project` (`bynk-emit/src/emitter/project.rs`)
  emits each `CommonsItem::Embed` as a `TsDecl::ConstDecl` typed `string`, from
  `embed_text` (`emit_model.rs`). A template is emitted as a function instead
  (§8).
- **Holes.** `interpolable` in `bynk-check/src/checker.rs` admits `String`,
  `Int`, `Float`, `Bool`, `Decimal`, `Duration`, `Instant`, a refinement of one,
  and `DecodeErrorKind`; anything else, opaque types included, is
  `bynk.types.interpolation_non_scalar` (raised in `type_of`, the
  `ExprKind::InterpStr` arm). So an opaque `Html` can never become a `String`
  by accident through a plain string, and nor can a `Secret`.
- **Secrets.** `Secret`, `Authorization` and `ApiKeyValue` are opaque in
  `bynk-std/lib/core/bynk.bynk` (ADR 1038), recognised by `SecretKind`,
  `is_secret_type` and `type_carries_secret` in
  `bynk-check/src/context_checks.rs`. They have no string form, are not
  `Codable`, and are refused at a boundary (`bynk.types.secret_at_boundary`).
  `reveal()` is the one door to a `Secret`'s text.
- **Language tags are value-neutral.** `"""html` already highlights as HTML,
  but the tag is not part of the value (ADR 0739; `multiline_opener` in
  `bynk-syntax/src/lexer.rs`). It cannot change type without changing every
  program that uses it.
- **String prefixes.** The only prefix today is raw, `r"…"` and `r#"…"#`
  (`string_open`, `lex_string`, `lex_multiline` in `bynk-syntax/src/lexer.rs`).
- **Built-in type names are reserved.** A name in `BUILTIN_TYPE_NAMES`
  (`bynk-syntax/src/keywords.rs`) may not be a user's `type`
  (`bynk.resolve.reserved_builtin_type`). ADR 0659 B kept `Url` out of the
  built-ins for this reason. `Html` as a built-in would break every project's
  own `type Html`.
- **`Url` is taken.** `bynk.url` declares `Url` as a record. A second `Url` in
  `bynk.html` would clash in any unit that uses both.
- **`Asset` is a built-in** (`BUILTIN_TYPE_NAMES`), emitted as
  `export const name = { url }` by `emit_project`, and is not data: it may not
  be a record field or cross a boundary (`reference/assets.md`). A template's
  fields record cannot carry one.
- **Static methods.** A commons may declare `fn T.name(…)` and callers write
  `T.name(…)` (`reference/types.md`, as `Sorted.from(xs)`), so `Html.join` and
  `Html.unsafe` need no new mechanism.
- **Literal-only precedents.** D1 SQL must be a literal so injection cannot be
  assembled at run time (ADR 0666 B), and a `const` bound to a literal counts
  (ADR 0762). A template is the same idea applied to markup.
- **Parser precedents.** The YAML subset is a hand-written parser held correct
  by a conformance corpus and a differential test against a dev-only library
  (ADR 0789). The Postgres check chose the pure-Rust `sqlparser` partly because
  it builds for `wasm32` (ADR 0816 B).
- **Percent-encoding.** `s.percentEncode()` (`PercentEncode` in
  `bynk-check/src/kernel_op.rs`, `__bynkPercentEncode` in
  `bynk-emit/runtime/src/url.ts`) keeps `A–Z a–z 0–9 - . _ ~` and encodes every
  other UTF-8 byte (`reference/types.md`).
- **The experimental gate.** `StabilityPolicy` (`bynk-check/src/stability.rs`)
  reads the root project's `[unstable] allow` (`read_unstable_config`,
  `bynk-project/src/paths.rs`); a use of an experimental path is
  `bynk.stability.experimental`. Language-provided items are listed in
  `BUILTIN_EXPERIMENTAL`, and items admitted by another item's gate in
  `BUILTIN_GATED` (`bynk-std/src/lib.rs`), as the API keys track did.

### 2.4 The response side

- `applySecurityHeaders` (`bynk-emit/runtime/src/http.ts`) stamps `nosniff`,
  HSTS, and, on a `text/html` response, `htmlCsp` (default
  `frame-ancestors 'none'`) and `htmlReferrerPolicy` from the per-service
  `SecurityPolicy` (built by `html_security_values` and
  `build_security_services`, `bynk-emit/src/emitter/workers_entry/http_policy.rs`;
  checked by `validate_html_security_fields`,
  `bynk-check/src/context_checks.rs`). It uses `set`, so it overwrites.
- A handler cannot set those headers: `RESERVED`
  (`bynk-check/src/checker/response_headers.rs`) and `RESERVED_RESPONSE_HEADERS`
  (`http.ts`) include `content-security-policy` and `referrer-policy`, and
  `withHeader` refuses them (`bynk.http.reserved_response_header`, ADR 0964 C).
- **Fill, not overwrite** is designed, not built: `pass-through-http.md` D9
  stamps the HTML headers on a `Forward(…)` response "only where the upstream
  sent none". It applies to upstream responses only. `PassThrough`,
  `Forwarded` and `Upstream` are not on `main` yet (slices 1 and 2 of that
  track are: rest segments, ADR 1097; header enumeration, ADR 1081).
- `HTTP_VARIANTS` (`bynk-syntax/src/ast.rs`, payloads in
  `HttpVariantPayload`) holds `Raw`, `RawCreated`, `RawTagged`, `RawStreamed`,
  `Partial` and `RawError`; the runtime mirror is `HTTP_STATUS` (`http.ts`).
  `HttpResult` is extensible (ADR 1078, `HTTP_RESULT_EXTENSIBLE`), so a new
  variant breaks no exhaustive `match`.
- `Raw` carries a weak `ETag` up to `RAW_WEAK_ETAG_MAX_BYTES` (1 MiB), in
  `baseHttpResultToResponse`, when `response_opts`
  (`bynk-emit/src/emitter/workers_entry/http.rs`) sets `weakEtag`.
- Streaming responses exist: `Streaming` (server-sent events over
  `Stream[String]`), `RawStreamed(Stream[Bytes], contentType)` and
  `PartialContent`.
- Snapshots: `expect v matches snapshot "name"` compares `Json.encode`'s output,
  canonicalised (`snapshot_check` in `bynk-emit/src/emitter/lower_stmt.rs`,
  `__bynkSnapshotCheck` in `bynk-emit/runtime/src/testing/snapshot.ts`); a
  value that is not `Codable` is `bynk.test.snapshot_not_codable`.
- Outbound email: `EmailMessage` in `bynk-std/lib/cloudflare/bynk.cloudflare.bynk`
  has `html: Option[String]`, as do `InboundEmail` and `EmailReply`.

## 3. Goals and non-goals

**Goals**

- A value reaches markup only escaped for its context, or through
  `Html.unsafe`.
- Every placeholder in a template is known, every field is used, and each
  field's type is one its contexts accept. Mistakes are compile errors, in the
  editor too, at the template line.
- A context the model does not understand is refused at compile time, never
  guessed.
- A URL attribute takes only a value whose scheme was checked; an attribute
  that loads code takes only the service's own assets.
- A secret kind is refused in every context.
- A page response carries the right content type and ADR 0954's defaults.
- No run-time HTML parsing: a template compiles to string concatenation.

**Non-goals**

- Sanitising third-party HTML. Markup a project receives and passes to
  `Html.unsafe` is outside the guarantee, and the docs say so.
- A component model, client-side rendering or hydration.
- CSS as a typed value (`Css`). Placeholders in `style` are refused.
- Script placeholders other than JSON data blocks (slice 7).
- Streaming HTML (§8.3).
- Templates in package dependencies, as for embeds today. A dependency may
  export functions that return `Html`.
- `srcset`, placeholders in foreign content (SVG, MathML) and dynamic
  attribute names in the first version. Each is refused, and can be admitted
  later without breaking anything.
- Rich-text messages (markup inside a message bundle's text) (§9.1).

## 4. The surface

### 4.1 A template

The file keeps its own language. A placeholder is `\(name)`:

```html
<!doctype html>
<html lang="en">
<head>
	<meta charset="utf-8">
	<title>\(title)</title>
	<link rel="stylesheet" href="\(style)">
</head>
<body>
	<h1>\(title)</h1>
	<a href="\(home)">Home</a>
	\(content)
</body>
</html>
```

The code declares the fields and the template:

```bynk
commons app.page

uses bynk.html

asset style = "assets/page.css"

---
What the page shell needs.
---
type PageFields = {
	title: String,
	home: Href,
	content: Html,
}

template shell: PageFields = "embeds/page.html"

fn render(title: String, content: Html) -> Html {
	shell.fill(PageFields { title: title, home: Href.root, content: content })
}
```

- `\(style)` sits in a stylesheet link, which loads code, so it names the
  asset `style` and is filled at compile time. It is not a field (§5.2). A
  placeholder in a code-loading attribute names an asset in scope; anywhere
  else it names a field. A name that is both a field and an asset in scope is
  refused wherever it appears (`bynk.template.ambiguous_placeholder`), and an
  asset name in an ordinary context is an unknown placeholder, with a note
  naming the asset. So renaming an asset, or adding a field, can never
  silently change what a placeholder binds to: it is a compile error instead.
- `\(title)` appears twice, both times in text, so `String` is accepted.
  `\(home)` is a whole URL attribute and takes `Href`. `\(content)` is element
  text and takes `Html` as is.
- A misspelt `\(titel)` is `bynk.template.unknown_placeholder`, with the
  nearest field named. A field no placeholder uses is
  `bynk.template.unused_field`. Codes are listed in §16.1.

A template with no fields type has value `Html`, and any placeholder in it is
an error:

```bynk
template footer = "embeds/footer.html"
```

### 4.2 Fragments in code

```bynk
fn row(item: Item) -> Html {
	html"<li><a href=\"\(item.link)\">\(item.name)</a></li>"
}

fn list(items: List[Item]) -> Html {
	if items.isEmpty() {
		html"<p>Nothing yet.</p>"
	} else {
		Html.join([html"<ul>", Html.join(items.map(row)), html"</ul>"])
	}
}
```

The holes of `html"…"` are expressions, as in a string. Each is typed by the
context it lands in. A multi-line fragment is `html"""`:

```bynk
fn card(title: String, body: Html) -> Html {
	html"""
	<section class="card">
		<h2>\(title)</h2>
		\(body)
	</section>
	"""
}
```

`html"""` is a prefix, not a language tag: `"""html` keeps meaning a plain
`String` with HTML highlighting (ADR 0739), and the two never coincide.

### 4.3 Serving it

```bynk
service web from http {
	on GET("/") () -> Effect[HttpResult[()]] by Visitor {
		Page(render("Home", html"<p>Hello</p>"))
	}
}
```

## 5. The type and escaping model

### 5.1 `Html` and its companions

`Html` is an opaque type over `String`, declared in the first-party `bynk.html`
commons (ADR 0993's unit, not a built-in), and known to the compiler by its
resolved identity, as ADR 0666 B keys the D1 check on `bynk.cloudflare`'s `D1`.
A unit opts in with `uses bynk.html`. No user name becomes reserved.

| Operation | Type | Note |
|---|---|---|
| `html"…"`, `html"""…"""` | `Html` | checked fragment (§5.3) |
| `t.fill(fields)` | `Html` | `t: Template[F]`, `fields: F` |
| `Html.text(s)` | `String -> Html` | escaped as element text |
| `Html.join(xs)` | `List[Html] -> Html` | concatenation |
| `Html.empty` | `Html` | |
| `Html.unsafe(markup, reason)` | `(String, String) -> Html` | the one raw door (§5.6) |
| `h.toString()` | `String` | markup out, for tests and other bodies |
| `escapeText`, `escapeAttr` | `String -> String` | ADR 0993, unchanged and still bare |

`Html` is not data at a boundary. It may be a record field (a fields record
needs it), but a type containing it is refused by the codec, as a request
parameter, by `Json.decode` and in agent `store` fields
(`bynk.html.at_boundary`). Markup from outside enters only through
`Html.unsafe`. To send markup as JSON, send `h.toString()`.

### 5.2 Contexts, one by one

The compiler tokenises a template or fragment as a browser does, and records
the tokeniser state at each placeholder. That state, plus the element and
attribute names around it, is the context. Every context has exactly one
treatment and one fixture that pins it (slice named):

| # | Context | Accepts | Output | Fixture (slice) |
|---|---|---|---|---|
| C1 | Element text (data state) | a scalar (`String`, `Int`, `Float`, `Bool`, `Decimal`, `Duration`, `Instant`, a refinement of one); `Html` | scalar: `escapeText`; `Html`: as is | `\(t)` with `t = "<script>alert(1)</script>"` renders `&lt;script&gt;…`; an `Html` field is inserted verbatim (1) |
| C2 | Text of `<title>`, `<textarea>` (RCDATA) | a scalar | `escapeText`; `Html` refused (`bynk.template.html_in_rcdata`) | `</title><script>` in a value renders inert; an `Html` field there is refused (1) |
| C3 | Double- or single-quoted value of an ordinary attribute | a scalar | `escapeAttr` (both quotes and backtick) | a value of `"' onmouseover=x '"` stays inside `alt="…"` and `alt='…'` (1) |
| C4 | Unquoted attribute value | nothing | refused: `bynk.template.unquoted_attribute`, fix adds quotes | `<img alt=\(a)>` (1) |
| C5 | Quoted URL attribute, whole value | `Href` | `escapeAttr` of the checked string | `href="\(h)"` with a `String` field is refused; with `Href.of("javascript:x")`, `None` (2) |
| C6 | Quoted URL attribute, after a safe literal prefix (§5.4) | a scalar | `percentEncode()`, then `escapeAttr` | `/orders/\(id)` with `id = "a/../b?x#y"` renders `/orders/a%2F..%2Fb%3Fx%23y`; `https://\(x)` refused (2) |
| C7 | Code-loading attribute | an `Asset` (a name in scope in a template; an `Asset` expression in a fragment) | its `.url`, filled at compile time in a template | `<script src="\(app)">` with an asset compiles to its hashed path; with a field, refused (2) |
| C8 | `<script>` body (any type but JSON data) | nothing | refused: `bynk.template.placeholder_in_script` | (1) |
| C9 | `<script type="application/json">` or `application/ld+json`, whole body | a value of a `Codable` type | JSON, with `<`, `>`, `&`, U+2028 and U+2029 as `<`, `>`, `&`, ` `, ` ` | a string field of `</script><script>x` stays one JSON string; a `Secret` is refused (not `Codable`) (7) |
| C10 | `<style>` body, `style` attribute | nothing | refused: `bynk.template.placeholder_in_style` | (1) |
| C11 | `on*` attribute, `srcdoc`, `<meta content>` beside `http-equiv` | nothing | refused: `bynk.template.placeholder_in_code_attribute` | (1) |
| C12 | `srcset`, `imagesrcset` | nothing in this track | refused: `bynk.template.srcset` | (2) |
| C13 | Tag name, attribute name, comment, doctype, CDATA, inside `<svg>`/`<math>` | nothing | refused: `bynk.template.placeholder_position` | (1) |

So script and style take no placeholder at all, with one exception that needs
an explicit typed value: a JSON data block (C9) whose whole body is one
`Codable` value, which is data, not code, and is not executed.

**URL attributes** (C5, C6): `href`, `src`, `action`, `formaction`, `poster`,
`cite`, `ping`, `background`, `longdesc`, `manifest`, `codebase`, `data` (except
on `<object>`), `icon`, `usemap`, `xlink:href`, and the rest of the list Go's
`html/template` uses for the same purpose. `formaction` on `<button>` and
`<input>` is an ordinary URL attribute and takes `Href`: it decides where a form
goes, not what code runs, and `Href` already refuses `javascript:`. `ping`
takes one `Href` (a space-separated list is not built).

**Code-loading attributes** (C7): `<script src>`, `<link href>` unless `rel` is
literally a navigational value (`canonical`, `alternate`, `icon`, `prev`,
`next`, `author`, `license`), `<object data>`, `<embed src>`,
`<iframe src>` when it has no `sandbox` attribute, and SVG's `<script href>` and
`<script xlink:href>`. They decide what code a page runs, so only code the
service ships may fill them. `<base href>` takes no placeholder at all: it
would re-root every relative URL on the page, and no asset is a base.

The SVG entries cannot be reached while placeholders inside `<svg>` are
refused; they are listed so that admitting foreign content later cannot open
them by default.

A name used in several places must satisfy every context it appears in. A
field used both as element text and as a whole URL is refused, naming both
places (`bynk.template.conflicting_contexts`).

### 5.3 Fragment rules

An `html"…"` literal starts in element text and must end there: not inside a
tag, an attribute, a comment, or raw or RCDATA text
(`bynk.template.fragment_unclosed_state`). So an `Html` value can never leave a
parent page in another state. Element balance is not required: `html"<ul>"` is
a valid fragment, as the example in §4.2 needs.

Foreign content is the exception, because it is tree-builder state, not
tokeniser state. Inside `<svg>` or `<math>` the tree builder does not switch
the tokeniser to raw text or RCDATA for `<script>`, `<style>` or `<title>`, it
reads `<![CDATA[`, and SVG's `<script href>` loads code. A fragment checked
from the data state cannot see that a parent opened `<svg>`, so
`Html.join([html"<svg>", html"<script href=\"\(h)\"></script>", html"</svg>"])`
would pass a rule about tokeniser state alone. So:

- every `<svg>` or `<math>` a template or fragment opens must be closed in the
  same template or fragment, and an `</svg>` or `</math>` with no opener in it
  is refused (`bynk.template.foreign_unbalanced`);
- inside such a subtree, the checker follows the foreign-content rules above
  to find its end, and refuses any placeholder;
- in the first version, a subtree may not contain `<foreignObject>`,
  `<annotation-xml>`, `<desc>` or `<title>` in SVG, or any HTML element that
  makes the tree builder leave foreign content (`<p>`, `<div>`, `<b>` and the
  rest of the WHATWG list), so that counting open and close tags matches what
  the tree builder does.

An `Html` value can therefore never end inside foreign content, and composition
cannot place one there. Static inline icons (`<svg>…</svg>` with no
placeholders) keep working. A template file follows the same rules.

### 5.4 `Href` and URL safety

`Href` is an opaque type over `String` in `bynk.html`.

**Constructors.**

- `Href.of(s) -> Option[Href]`.
- `Href.asset(a)`: an asset's URL.
- `Href.fromUrl(u) -> Option[Href]`: `bynk.url`'s `Url` record, formatted and
  then checked by the same rule as `Href.of`.
- `Href.root`: `/`.

There is no route builder, because Bynk has none.

**What `Href.of` accepts.** It checks the string the browser will see. Every
`Href` is attribute-escaped on output, so a character reference in the value
(`&#106;avascript:`) reaches the browser as literal text, never decoded into a
scheme.

1. Any ASCII control character (U+0000–U+001F, U+007F), space or backslash
   anywhere is refused. Browsers strip tabs and newlines, so `java\tscript:`
   would run. They also read `\` as `/` in special URLs, so `/\evil.example`
   would be a network-path reference.
2. A string with a scheme (WHATWG: an ASCII letter, then letters, digits, `+`,
   `-` or `.`, then `:` before any `/`, `?` or `#`) is accepted only for
   `http`, `https`, `mailto` and `tel`, compared without regard to case.
   `javascript:`, `data:`, `vbscript:`, `blob:`, `file:` and every other
   scheme are refused. `data:` is refused even for images in this track.
3. A string without a scheme is a relative reference, and is accepted unless
   it starts with `//` (a network-path reference that chooses the host). Write
   the scheme instead.

**Percent-encoded input.** `Href.of` neither decodes nor re-encodes. A
percent-encoded scheme is not a scheme: `javascript%3Aalert(1)` and
`%6Aavascript:alert(1)` have no `:` in scheme position, so they are relative
paths on the service's own origin, which is how the browser reads them too. A
value that is already percent-encoded passes through unchanged and is not
double-encoded.

**Partial URLs** (C6). A placeholder after a literal prefix in a URL attribute
is common (`href="/orders/\(id)"`). It takes a scalar, which is
percent-encoded with `percentEncode()` (ADR 0659 A) and then attribute-escaped.
The literal prefix must be one of:

- a relative path starting with `/` but not `//` or `/\`, or with `./`, `?` or
  `#`;
- `http://` or `https://`, a host, and then `/`, `?` or `#`.

`percentEncode()` encodes `/`, `\`, `:`, `@`, `?`, `#` and `%`, so a value can
neither turn `/` into `//evil.example`, nor add a path, query or fragment the
author did not write, nor choose the host. A bare `https://\(x)` or
`https://example.com\(x)` is refused (`bynk.template.url_prefix`), since the
value would choose or extend the host. Any other prefix, including one with a
scheme other than `http(s)`, is refused the same way. A prefix that is itself a
placeholder (`\(base)/x`) is C5 followed by literal text, and the whole value
must then be an `Href` plus literal text that adds no scheme.

`percentEncode()` keeps `.`, so a value of `.` or `..` that fills a whole path
segment is still a dot segment: `/orders/\(id)` with `id = ".."` resolves to
`/`. Encoding cannot fix this, since the URL standard also reads `%2e` as a
dot. It stays on the same origin, so it is not XSS, but it can point a link or
form at another of the service's paths. It is a known limit (§13); a field
typed as `Int`, or a refinement that excludes dots, cannot produce it.

**`srcset`, `formaction`, `style`.** `formaction` is covered as a URL attribute
(§5.2). `srcset` and `imagesrcset` are refused in this track: each is a list of
URL-and-descriptor pairs with its own grammar, where a comma in a value changes
the parse. A later increment can admit a `List[SrcsetEntry]` of `Href`s without
breaking anything. `style` is refused outright (C10), because `url(…)` inside
CSS is a second URL context with its own escaping, and CSS can exfiltrate data
without any URL.

### 5.5 Secrets never reach a page

`Secret`, `Authorization` and `ApiKeyValue` (every `SecretKind`) are refused in
every context, including C1, with their own code
(`bynk.template.secret_placeholder`) and a note naming ADR 1038. This already
follows from the table, since none is a scalar, `Html`, `Href`, `Asset` or
`Codable`; the dedicated code makes the error say why, rather than reporting a
type mismatch. A record that carries a secret (`type_carries_secret`) cannot be
a template's fields type at all.

The secret kinds have no path into `Html` other than `reveal()`: `Html.text`
and `Html.unsafe` take a `String`, and a `Secret` has no string form. So a
secret reaches a page only through `Html.text(s.reveal())` or
`Html.unsafe(s.reveal(), …)`, each of which writes the audited door in the
source. This track adds no rule on `reveal()` itself.

### 5.6 The escape hatch: `Html.unsafe`

`Html.unsafe(markup, reason)` is the only way raw markup enters an `Html`
value (ruled 27 November 2026).

- **`reason` must be a string literal**, not a `const`, so the reason is at the
  call site, in review and in hover (`bynk.html.unsafe_reason_not_literal`). An
  empty reason is refused.
- **Greppable.** The name is one token, `Html.unsafe`, and no other API reaches
  it: there is no `Html` constructor, no implicit conversion, and `toString()`
  goes only the other way. Emission lowers it to the identity.
- **A hint when `markup` is a string literal**: write `html"…"` instead, which
  is checked (`bynk.html.unsafe_literal`, a hint, machine-applicable when the
  literal is a valid fragment).
- **No warning on run-time arguments.** Every real use is dynamic (rendered
  Markdown, CMS content), so a warning would fire on every use and teach people
  to ignore it.
- **Dependencies may call it, and are listed** (ruled 27 November 2026).
  `bynk check --unsafe-html` lists every `Html.unsafe` call site the build
  compiles, grouped by package (the project's own first), with the file, line
  and reason. The language server shows the same list as a code lens on the
  `[dependencies]` table. A Markdown renderer is the obvious dependency, and it
  cannot exist if dependencies may not call it. The listing makes the trust a
  project places in its dependencies visible without an opt-in per package.
- **The experimental gate.** `Html.unsafe` belongs to the track's gate
  (§14), like every other `Html` operation. A dependency that uses it compiles
  only in a project that allows the gate, because `StabilityPolicy` is the root
  project's.

### 5.7 What makes the guarantee hold

The guarantee rests on three things, each with its own test (§11):

1. the tokeniser agrees with browsers on the state at every placeholder;
2. each escape is complete for its context;
3. nothing else produces `Html`: the only constructors are the checked
   literal, `fill`, `Html.text`, `Html.join`, `Html.empty` and `Html.unsafe`.

## 6. The tokeniser

A hand-written tokeniser in the checker implements the WHATWG tokeniser states
the context model needs (data, RCDATA, RAWTEXT, script data, tag open, the
attribute states, comment, doctype, CDATA in foreign content) and refuses
everything else. It needs the state at each placeholder, not a tree, and a tree
builder's error recovery is what the model should refuse, not reproduce. It
must build for `wasm32` (ADR 0816 B's concern), and ADR 0789 is the precedent
for a hand-written parser held to a corpus.

It also refuses inputs a browser would accept only through error recovery where
the recovery changes a context: a `<` in an attribute name, an attribute
repeated on one element (the browser keeps the first), an unclosed comment, a
`</script` inside a script that is not the end tag.

**Placeholder recognition** (Q1). Only `\(` followed by a name and `)` is a
placeholder, and only when its backslash does not follow another backslash. Any
other `\(` is literal text, and the file has no other escapes, so MathJax's
`\(x^2\)`, a regex such as `/\(/` in an inline script, and `\\(` all pass
through unchanged. A literal `\(name)` in text or an attribute is written with
a character reference, `&#92;(name)`. In raw text a placeholder-shaped
`\(name)` is refused like any placeholder there, with a note to rewrite it.

## 7. Responses

- **`Page(html)`**: a 200 with `content-type: text/html; charset=utf-8` and the
  body as UTF-8. It is `ETag`-eligible on the same terms as `Raw`
  (`RAW_WEAK_ETAG_MAX_BYTES`, `response_opts`), and `@cache` applies. It is one
  new `HTTP_VARIANTS` row and `HttpVariantPayload` shape, with an
  `HTTP_STATUS` row, following ADR 0143's pattern for `Raw`. `HttpResult` is
  extensible (ADR 1078), so no exhaustive `match` breaks.
- **`PageError(status, html)`**: an error status with an HTML body, under the
  same `ErrorStatus` rule as `RawError`.
- **Name resolution.** As `pass-through-http.md` D8 does for `Forward`, the new
  variant names resolve by expected type, so a program with its own `Page`
  variant keeps compiling.
- **Security headers.** Both are `text/html`, so ADR 0954's defaults apply with
  no extra code. `nosniff` already applies to every response.
- **Tests.** A unit-tier case matches `Page(h)` and reads `h.toString()`. At the
  system tier a non-JSON 200 already reads as `Raw` (#3050); `Page` reads the
  same way there.

## 8. Emission and performance

### 8.1 String building, no run-time parser

A template compiles to a function of its fields. The tokeniser runs once, in
the compiler. The emitted code is literal chunks and escape calls:

```ts
export function shell(f: PageFields): Html {
	return ("<!doctype html>…<title>" + escapeText(f.title) + "</title>…" +
		"<link rel=\"stylesheet\" href=\"/_assets/page.3bf3a8d5.css\">…") as Html;
}
```

- `Html` and `Href` are branded `string` types in TypeScript, so they cost
  nothing at run time.
- `html"…"` lowers the same way, inline. `Html.unsafe` lowers to the identity,
  `Html.join` to `Array.prototype.join("")`.
- The escape functions are ADR 0993's, emitted only when used. A C6 placeholder
  lowers to `escapeAttr(__bynkPercentEncode(…))`; a C7 placeholder in a
  template is a literal, so the chunk around it folds into one string.
- A template's text counts against the embed size caps (ADR 0609 C).
- The runtime gains no HTML code: no tokeniser, no parser, no sanitiser. The
  emitted bundle for a template-using service grows by the escape functions
  and the template functions only.

### 8.2 The editor reads templates

`analyse_project_inner` reads declared template files, not only lists them
(§2.3), so template errors show while editing. The caps bound the cost, and
only files a `template` declaration names are read. The tokeniser's result is
cached per file content, as embed text is.

### 8.3 Streaming

Streaming HTML is out of scope. A page is built whole, then sent; the size caps
and the 1 MiB `ETag` bound keep that cheap. A streamed page needs
`Stream[Html]` and a decision about flushing that belongs with HTMLRewriter
(#3519), and `RawStreamed` already serves a streamed body for anyone who needs
one now. `Stream[Html]` can be added later without breaking anything, because
`Page` takes `Html`, not a stream.

## 9. Composition with other features

### 9.1 Message bundles (#911)

The #911 ruling (27 November 2026) scopes a message code to the unit that
declares it: `message(code)` and `render` resolve only against the same unit's
bundles. A template follows from that with no new rule:

- **A template never names a message code.** A placeholder is a field name
  (or, in C7, an asset). There is no `\(msg:code)` form.
- **A template renders codes from its own unit's bundles**, because the
  function that fills it is in that unit: `shell.fill(PageFields { title:
  render(tag, message("page.title")), … })` resolves `"page.title"` in the unit
  that declares `shell`. A template another unit exports is filled with
  strings that unit's caller rendered from the caller's own bundles, as
  `examples/greeter-i18n` does with `greet(…)`.
- **A rendered message is a `String`,** so it is escaped for its context like
  any other. A translator's `<b>` in a message shows as text. Markup inside a
  message is out of scope (§3); the way to it is a template per locale or
  `Html.unsafe` with a reason, not a new rule.

### 9.2 Pass-through responses (#3752)

- **A `Forward` is never wrapped in a template.** `Forwarded` is opaque and has
  no body accessor (`pass-through-http.md` D8), and `Html` has no constructor
  from it, so no code can read an upstream body into `Html` or a `Page`. This
  is structural, not a check.
- **A pass-through route may answer locally with a page.** `Respond(Page(h))`
  and `Respond(PageError(status, h))` are ordinary `HttpResult`s and are
  shaped as on any route.
- **The computed CSP (§10) is never stamped on a `Forward`.** The upstream's
  markup is not a template, so hashes from this service's templates would block
  its scripts. A `Forward` gets the D9 rule unchanged: ADR 0954's header
  values, filled only where the upstream sent none.

### 9.3 HTMLRewriter (#3519)

The two stay separate and do not overlap. A rewriter transforms markup the
service did not build, as it streams, so it has different questions
(selectors, streaming, effects). If it lands:

- it operates on a response stream (`RawStreamed`, or a `Forward`'s body), never
  on a `Page`: a page the service builds is composed before it is sent;
- it shares `Html` only as the type of inserted content: an `append`,
  `prepend` or `replace` takes `Html`, and an attribute setter takes a scalar
  (escaped as C3) or an `Href` for a URL attribute;
- a rewritten response is not a template, so §9.2's CSP rule applies to it.

Folding a rewriter into this track would delay every slice behind #3519's open
questions.

### 9.4 Email

`EmailMessage.html`, `InboundEmail.html` and `EmailReply.html` stay
`Option[String]` in this track. Once `Html` is promoted out of experimental,
`EmailMessage.html` and `EmailReply.html` should take `Option[Html]`, so an
email body built from a template gets the same escaping and cannot take raw
markup by accident. That is a breaking change to a stable type, so it is a
follow-on with its own proposal and a `breaking:` line, not a slice
(§14). `InboundEmail.html` is markup from outside and stays a `String`; a
handler that shows it goes through `Html.unsafe` with a reason.

## 10. CSP and security headers

ADR 0954 makes `text/html` responses unframeable by default, sets a referrer
policy, and adds an opt-in, verbatim `csp`. The core slices add nothing to
that: a `Page` gets those headers because it is `text/html`.

**Templates emit no nonces** (ruled 27 November 2026). A nonce is per
response, so it would make every page uncacheable (`ETag`, `@cache`, the edge
cache), and it would thread request state into `fill`.

**A computed, hash-based policy, opt-in only** (slice 8). A template and a
fragment are fully known at compile time, so the compiler knows the exact text
of every static inline `<script>` and `<style>` a service can produce. A
service opts in with `security { csp: computed }`. The policy is
`script-src 'self' <hashes>; style-src 'self' <hashes>; object-src 'none';
base-uri 'none'`, where `<hashes>` is the union of the SHA-256 hashes of every
static inline script and style in the templates and fragments the service's
units compile, and `frame-ancestors` is appended per ADR 0954 D.

- **Precedence.** `csp: computed` and `csp: "…"` are one field, so a service has
  one or the other, never a merge. An explicit string always wins because it
  is the only policy there is.
- **Fill, not overwrite.** On the service's own responses the policy is the
  service's and is set, as today (a handler cannot set the header, ADR 0964 C).
  On a `Forward` the computed policy is never used, and D9's fill rule applies
  to ADR 0954's values only (§9.2). So the fill-not-overwrite rule from
  `pass-through-http.md` holds unchanged, and a computed policy can never
  overwrite an upstream's own.
- **What it blocks.** Anything inserted with `Html.unsafe`, and every static
  `on*` handler unless the author adds `'unsafe-hashes'` by writing the policy
  by hand. That is the right failure for a security header, but it would
  surprise people, so it is never the default, and the `--unsafe-html` listing
  names the call sites it will block.
- **Rejected:** per-response policies, which need to track which fragments
  reach which response through run-time composition; nonces (above).

## 11. Testing

### 11.1 How a test asserts on rendered HTML

By string and by snapshot, not structurally:

- `h.toString()` gives the markup, for `expect h.toString() == "…"` and for
  `contains`.
- `expect page.toString() matches snapshot "home"` uses the existing snapshot
  mechanism (ADR 0719). `Html` is not `Codable`, so `toString()` is the snapshot
  and `bynk.test.snapshot_not_codable` gains a note naming it.
- A unit-tier case matches `Page(h)` on a handler's result.

A structural assertion (a selector query over the rendered page) would need an
HTML parser in the test runtime, the one thing §8 keeps out, and the output is
deterministic text, so a snapshot pins it exactly. It can be added later as a
test-only library without changing `Html`.

### 11.2 Tests for the guarantee

- **Per context.** Each of C1–C13 has the positive or negative fixture named in
  §5.2, plus a negative fixture for every code in §16.1.
- **Tokeniser conformance.** The `html5lib-tests` tokeniser corpus, run in the
  checker's tests over every state the model implements, with a test that
  every state it does not implement is refused.
- **Tokeniser differential.** A `proptest` property (the workspace already
  depends on `proptest`) generates HTML-like inputs with placeholders, and
  checks that the state the tokeniser records at each placeholder matches the
  state `html5ever`, a dev-dependency only, is in at the same offset; and that
  every input the tokeniser accepts, filled with adversarial values, parses in
  `html5ever` with each value as exactly one text or attribute-value token.
- **Fuzzing.** A new `cargo-fuzz` target beside `fuzz/fuzz_targets/compile.rs`
  and `parse.rs` feeds arbitrary bytes to the template checker: no panic, and
  the differential above on every input it accepts.
- **Escaper properties,** in `bynk.html`'s own suite, as Bynk `property` tests
  over generated `String`s: `escapeText(s)` contains no `<` or `>`, and every `&`
  in it starts one of the escapes it writes; `escapeAttr(s)` also contains no
  `"`, `'` or backtick; decoding the five references gives `s` back.
- **`Href` properties.** For every generated string, `Href.of(s)` is `Some` only
  if the WHATWG URL parser (the `url` crate, dev-only, run on the same corpus
  in the Rust tests) reads either no scheme or one of the four; every string
  with a control character, space or backslash is `None`; and a corpus of
  known `javascript:` bypasses (case, entities, encodings, whitespace) is
  `None` or, for percent-encoded schemes, relative.

## 12. Tooling

- **Grammar.** `template` is a contextual keyword at item position only, so an
  identifier named `template` elsewhere is unaffected. `html"` and `html"""` are
  a string prefix in `string_open` and `lex_multiline`, like `r"`. Both are
  added to `reference/grammar.md`.
- **tree-sitter-bynk.** A `template_declaration` node, and an `html_string`
  node whose body is injected as `html` in `queries/injections.scm`, with the
  holes as Bynk, as the tagged strings are today.
- **Formatter (`bynk-fmt`).** A `template` declaration formats as an `embed`
  does. The formatter never reflows the markup inside `html"…"`; it formats
  the hole expressions only.
- **vscode-bynk.** The TextMate grammar (`syntaxes/bynk.tmLanguage.json`)
  gains `template` and an `html"`/`html"""` rule embedding `text.html.basic`
  with `\(…)` holes, as `"""html` does today. Template files are ordinary
  `.html` files; the extension adds an injection grammar that scopes
  `\(name)` in HTML files and relies on the language server's semantic tokens
  to colour only real placeholders in declared templates.
- **The site and `site-theme`.** `site/src/grammars/bynk.tmLanguage.json` is a
  copy of the extension's grammar and changes with it. `@bynk-lang/site-theme`
  needs a token colour for the placeholder scope, released before the book
  slice uses it, and the blog picks it up with its next theme bump.
- **Language server.**
  - The server opens declared template files as well as `.bynk` files, and
    reports template diagnostics at the template line and column, with a
    related location at the `template` declaration (and at the field, for a
    type error).
  - Hover on a placeholder in a template file or a fragment shows its field,
    type, context and treatment (for example "URL attribute: takes `Href`").
    Hover on a fields-record field lists the placeholders and contexts that use
    it. Hover on `Html.unsafe` shows its reason.
  - Completion after `\(` in a template file offers the fields and, in a
    code-loading attribute, the assets in scope; inside an `html"…"` hole it
    is ordinary expression completion, filtered to types the context accepts.
  - Go to definition from a placeholder to its field or asset; rename of a
    field renames its placeholders.
  - Quick fixes: add quotes to an unquoted attribute; wrap a `String` in
    `Href.of(…)` with a `match` in C5; replace `Html.unsafe("<literal>", …)`
    with `html"…"`.

## 13. Threat model

- **Assets:** the integrity of pages a service serves; users' sessions and data
  in the browser.
- **Adversary:** anyone who can put text into data a page renders: a form
  field, a stored record, a query parameter, a header, a message from an
  upstream.
- **Where verification happens:** at compile time for every template and
  fragment (the context and type of each placeholder); at run time only in the
  escape functions and `Href.of`'s scheme check (both property-tested, §11.2).

| Context | Attack | Handled by |
|---|---|---|
| Element text | `<script>` in a value | `escapeText` |
| `<title>`, `<textarea>` | `</title><script>` | `escapeText`; `Html` refused |
| Quoted attribute | breaking out with `"`, `'` or a backtick | `escapeAttr` |
| Unquoted attribute | breaking out with a space | refused |
| URL attribute | `javascript:`, `data:`, `java\tscript:`, `&#106;avascript:` | `Href` only; `Href.of` refuses; output is attribute-escaped |
| Partial URL | `//evil.example`, `/\evil.example`, a host chosen by the value | safe prefix required, with a fixed host when absolute; value percent-encoded (`.` and `..` segments are a known limit) |
| Code-loading attribute | a script from the attacker's origin | `Asset` only |
| `<base href>` | re-rooting every relative URL | no placeholder |
| `<script>`, `on*` | code injection | refused |
| `<style>`, `style` | CSS injection, exfiltration | refused |
| JSON data block | `</script>` in a string | `<`, `>`, `&`, U+2028, U+2029 escaped as `\u` sequences |
| Comment, tag or attribute name | structure injection | refused |
| `<meta http-equiv>` | refresh to `javascript:` | refused |
| `srcdoc` | a nested document | refused |
| SVG, MathML | different parsing rules; SVG `<script href>` | a placeholder inside, or a subtree left open across fragments, refused |
| Secrets | a token rendered into a page | every secret kind refused; only `reveal()` reaches text |
| Upstream HTML | a proxied page treated as trusted | `Forwarded` cannot become `Html` |

**Known limits, stated in the docs:**

- Markup passed to `Html.unsafe` is outside the guarantee. The track does not
  sanitise HTML.
- DOM-based XSS in the page's own scripts (an asset reading `location.hash`) is
  outside what the compiler sees.
- An attacker-chosen `id` or `name` value can clobber DOM globals. Escaping
  does not prevent it; it is noted, not solved.
- `Href` allows any `http(s)` host. That is not XSS, but a link to a hostile
  site is possible when a value chooses the whole URL.
- In a partial URL, a value of `.` or `..` filling a whole path segment moves
  the URL to another path on the same origin (§5.4).
- `Html.unsafe` in a dependency is trusted once the project allows the gate;
  the listing (§5.6) makes it visible, not safe.

## 14. The 1.0 position

**Additive and experimental, with no Foundations behaviour changed.** Every new
form is an error or means nothing today, so no program that compiles now
changes meaning:

- **`template name: F = "path"`** is a parse error today: no item starts with
  an identifier. `template` becomes a contextual keyword at item position only;
  `template` as a binding, field or function name elsewhere keeps working.
  This is the one grammar touch, and it is flagged here for the 1.0 grammar
  freeze.
- **`html"…"` and `html"""`** lex today as the identifier `html` directly
  followed by a string, which is a parse error in every position (Bynk has no
  juxtaposition). So the prefix changes only programs that do not compile. An
  identifier `html` followed by a space and a string is unchanged, and
  `"""html` keeps its ADR 0739 meaning.
- **`Html`, `Href` and `Template[F]`** are new names in `bynk.html`, reached
  only with `uses bynk.html`. They are not built-ins, so no user `type Html` or
  `type Href` is broken. A unit that already `uses bynk.html` and declares its
  own `Html` would see a clash; the corpus survey in slice 1 checks for one,
  and none is known.
- **`Page` and `PageError`** are new `HttpResult` variants. `HttpResult` is
  extensible (ADR 1078), and the names resolve by expected type (§7), so a
  user's `Page` variant keeps compiling.
- **`security { csp: computed }`** is a new value for an existing field; every
  string value means what it means today.
- **Emission** changes only for code that could not compile before.
  `escapeText` and `escapeAttr` keep their names, types and behaviour.
- **ADR 0143 and `http.md`** change in what they say, not in what any program
  does: the "does not template HTML" stance is revised for `Page` (D1).

**Every new form enters as experimental.** One gate, `bynk.html.typed`, is
added to `BUILTIN_EXPERIMENTAL`, and `BUILTIN_GATED` maps `bynk.html.Html`,
`bynk.html.Href`, `bynk.html.Template`, `template`, `html"…"`, `Page`,
`PageError` and `csp: computed` to it, as the API keys track did for its types.
None joins the frozen surface until promoted. Because the change is additive
and experimental, it does not have to land before 1.0, and the slices can run
either side of it.

**One breaking follow-on, outside the track:** `EmailMessage.html` and
`EmailReply.html` move to `Option[Html]` only after promotion (§9.4). If
promotion comes before 1.0, it lands before 1.0 with a `breaking:` line;
otherwise it waits for the next breaking window.

## 15. Slices

| # | Slice | Depends on | Done when |
|---|---|---|---|
| 0 | Settle this doc; land D1–D5 (§16) | — | D1–D5 have merged with numbers, the spine's checklist links them, and this doc's status reads Settled. |
| 1 | `Html` in `bynk.html`; the `template` declaration and `fill`; placeholder recognition (§6); the tokeniser and contexts C1–C4, C8, C10, C11, C13; every refusal, foreign-content balance (§5.3), the secret refusal (§5.5); `Html` refused at a boundary; the editor reads template text; the gate | 0 | A template with C1, C2 and C3 placeholders fills and renders at unit and workerd tiers with the escapes of §5.2; each of C1–C4, C8, C10, C11 and C13 has its fixture; every slice-1 code in §16.1 has a negative fixture, including `\(` that is not a placeholder, `&#92;(name)` and the three-fragment `<svg>` case; a `Secret` field is `bynk.template.secret_placeholder`; the `html5lib-tests` conformance test, the `proptest` differential and the fuzz target run in CI; the editor shows a template error at the template line; every new form needs `[unstable] allow`; the corpus survey for `Html` clashes is recorded in the PR. |
| 2 | `Href` and its constructors; URL and code-loading attributes (C5–C7, C12); partial URLs; `<base>` | 1 | Each of C5, C6, C7 and C12 has its fixture; the `Href` property tests and the bypass corpus of §11.2 pass; `/orders/\(id)` percent-encodes; `https://\(x)`, `//x` and `/\x` prefixes are refused; `<script src>` takes only an asset; `<link rel="canonical" href>` takes an `Href`. |
| 3 | `html"…"` and `html"""…"""` literals; `Html.join`, `empty`, `text`, `unsafe`, `toString`; the fragment rule; `bynk check --unsafe-html` | 1 | The §4.2 examples compile and render; a fragment ending inside a tag is `bynk.template.fragment_unclosed_state`; `Html.unsafe` with a non-literal reason is refused and with a literal markup gets the hint; the listing prints a dependency's call site under its package; the escaper property tests are in `bynk.html`'s suite. |
| 4 | `Page` and `PageError`, with ADR 0954's defaults; composition with `PassThrough` once it exists | 1, and #3752's `PassThrough` for the composition fixture | `Page(h)` answers 200 with `text/html; charset=utf-8`, `frame-ancestors 'none'` and a weak `ETag`; `PageError(NotFound, h)` answers 404; a user `Page` variant still compiles; once #3752 lands, `Respond(Page(h))` compiles on an `ANY` route and no expression turns a `Forwarded` into `Html` (a negative fixture). |
| 5 | Tooling: tree-sitter, formatter, vscode-bynk, the site grammar and `site-theme`, the language server (§12) | 1; 2 and 3 for the complete set | tree-sitter and the formatter accept every form in §4 and leave `html"…"` markup unchanged; vscode-bynk highlights `html"…"`, `template` and placeholders in a template file; the site grammar copy is updated and `site-theme` released with the placeholder colour; hover, completion, go to definition, rename and the quick fixes of §12 work in a template file and a fragment, each with an IDE test. |
| 6 | The book | 2, 3, 4, 5 | The serve-a-page guide uses `template`, `Href`, `Page` and no hand-written escaping or "leftover `{{`" test; the docs delta (§18) is applied. |
| 7 | JSON data blocks (C9) | 3 | C9's fixture passes, including a `</script>` string and U+2028; a non-`Codable` value is refused. |
| 8 | Opt-in hash-based CSP, `security { csp: computed }` (§10) | 4 | A service with `csp: computed` sends the hashes of its static inline scripts and styles and `frame-ancestors`; `csp: "…"` beside it is impossible by construction; a `Forward` response is never given the computed policy (a fixture once #3752 lands). |

Slices 7 and 8 are not needed for the end state and may be cut.

## 16. Decisions (ADRs to land up front)

- **D1. `Html` is not a `String`.** Markup and text are different types,
  declared in `bynk.html`. The only bridges are escaping by context
  (automatic) and `Html.unsafe` (explicit). `Html` does not cross a boundary,
  and `Forwarded` never becomes `Html`. This revises ADR 0143's "Bynk does not
  template HTML" for HTML responses, and the matching `http.md` passages.
- **D2. The context model.** The contexts C1–C13 of §5.2, what each accepts,
  and that anything else is refused at compile time; secret kinds refused in
  every context. Includes the tokeniser decision (hand-written, WHATWG states,
  corpus, differential and fuzz tests) and the fragment and foreign-content
  rules (§5.3).
- **D3. The `unsafe` rule.** `Html.unsafe(markup, reason)` with a literal,
  non-empty reason; permitted everywhere, including dependencies; listed per
  package by `bynk check --unsafe-html`; no warning on run-time arguments.
- **D4. Templates are their own declaration.** `template name: Fields = "path"`,
  honouring ADR 0608 F; `\(name)` placeholders recognised as in §6; a declared
  nominal fields record; asset names only in code-loading attributes. `embed`
  stays verbatim.
- **D5. `Href`.** An opaque type in `bynk.html`; `Href.of`'s scheme rule
  (`http`, `https`, `mailto`, `tel`; no control characters, spaces or
  backslashes; no `//` prefix); no decoding; the partial-URL rule.

Recorded in their slices: `Page` and `PageError` (slice 4), JSON data blocks
(slice 7), the computed CSP (slice 8).

### 16.1 Diagnostic codes

Named here so the ADRs can cite them; the slice that first raises a code
registers it with its explainer, and may rename it, saying so in its ADR.

| Code | Raised when | Slice |
|---|---|---|
| `bynk.template.unknown_placeholder` | a placeholder names no field (or an asset outside C7); the nearest name is suggested | 1 |
| `bynk.template.unused_field` | a field no placeholder uses | 1 |
| `bynk.template.ambiguous_placeholder` | a name is both a field and an asset in scope | 1 |
| `bynk.template.conflicting_contexts` | one field's contexts accept no common type | 1 |
| `bynk.template.context_type` | a field's type is not one its context accepts | 1 |
| `bynk.template.html_in_rcdata` | an `Html` value in `<title>` or `<textarea>` | 1 |
| `bynk.template.unquoted_attribute` | a placeholder in an unquoted attribute value | 1 |
| `bynk.template.placeholder_in_script` | a placeholder in `<script>` (outside C9) | 1 |
| `bynk.template.placeholder_in_style` | a placeholder in `<style>` or a `style` attribute | 1 |
| `bynk.template.placeholder_in_code_attribute` | a placeholder in `on*`, `srcdoc` or `<meta http-equiv>`'s `content` | 1 |
| `bynk.template.placeholder_position` | a placeholder in a tag or attribute name, a comment, a doctype, CDATA, `<base href>` or foreign content | 1, 2 |
| `bynk.template.foreign_unbalanced` | an `<svg>`/`<math>` left open, closed without an opener, or holding an integration point | 1 |
| `bynk.template.unsupported_markup` | markup the tokeniser does not model, or error recovery that changes a context (§6) | 1 |
| `bynk.template.secret_placeholder` | a secret kind, or a record carrying one, reaches a placeholder or a fields type | 1 |
| `bynk.html.at_boundary` | a type containing `Html` or `Href` crosses a boundary | 1 |
| `bynk.template.url_prefix` | a partial URL's literal prefix is not a safe one (§5.4) | 2 |
| `bynk.template.srcset` | a placeholder in `srcset` or `imagesrcset` | 2 |
| `bynk.template.code_loading_not_asset` | a code-loading attribute filled with anything but an asset | 2 |
| `bynk.template.fragment_unclosed_state` | an `html"…"` literal ends outside the data state | 3 |
| `bynk.html.unsafe_reason_not_literal` | `Html.unsafe`'s reason is not a non-empty string literal | 3 |
| `bynk.html.unsafe_literal` (hint) | `Html.unsafe`'s markup is a literal; use `html"…"` | 3 |
| `bynk.template.json_not_codable` | a C9 value is not `Codable` | 7 |
| `bynk.test.snapshot_not_codable` (reused, note added) | an `Html` value is snapshotted directly | 3 |
| `bynk.stability.experimental` (reused) | any form of §14 without the gate | 1 |

## 17. Decisions — the spine's questions and their rulings

The maintainer ruled on 27 November 2026 on the four points the draft left
open, each as recommended: the `template` declaration (Q11), the name
`Html.unsafe` (Q5), dependencies allowed and listed (Q5), and hash-only CSP
with no nonces (Q8). Every other answer was accepted in review as recommended.
The review on the settling PR (#3989) found five problems, each fixed in the
draft before this pass: foreign content across composition (§5.3), a literal
`\(` (§6), the partial-URL claims (§5.4), a garbled JSON row, and asset and
field names (§4.1).

1. **Q1. Placeholder syntax.** *Settled:* `\(name)`, a bare name, recognised as
   in §6. *Why:* it reads like a hole in an `html"…"` literal, so one rule
   covers both; `{{` is used by Vue, Alpine, Angular and Handlebars, which may
   share the file. *Rejected:* `{{name}}`, whose clash is permanent; migration
   is a find-and-replace either way.
2. **Q2. The parser.** *Settled:* a hand-written WHATWG tokeniser in the
   checker that refuses what it does not model; `html5lib-tests`, a `proptest`
   differential against `html5ever` (dev-only) and a fuzz target (§6, §11.2).
   *Rejected:* `html5ever` as a compiler dependency (a tree builder for a
   fraction of its use, and its recovery is what the model must refuse); a
   restricted grammar of our own, which would refuse valid pages.
3. **Q3. Contexts.** *Settled:* C1–C13 (§5.2): unquoted attributes refused,
   script and style refused except a whole-body JSON data block taking a
   `Codable` value, code-loading attributes split from URL attributes.
   *Rejected:* a `Css` type; Go's choice of letting any URL fill
   `<script src>`.
4. **Q4. The URL type.** *Settled:* an opaque `Href` (§5.4), allowing `http`,
   `https`, `mailto`, `tel` and relative references; `javascript:` and `data:`
   refused; no decoding; `srcset` refused; `formaction` covered. *Why:* opaque,
   so `Href.of` is the only door; `Url` is `bynk.url`'s record (ADR 0659 B).
   *Rejected:* a refined `String` (it widens back to `String` and invites a
   regex that misses `java\tscript:`); reusing `bynk.url`'s `Url`, which allows
   any scheme.
5. **Q5. The escape hatch.** *Ruled:* `Html.unsafe(markup, reason)`, literal
   reason; allowed in dependencies and listed per package. *Why:* the name
   tells a reviewer the compiler stops checking here, and greps with other
   `unsafe` audits; a Markdown renderer must be possible as a dependency.
   *Rejected:* `Html.trusted` (reads as an assertion of safety); a warning on
   run-time arguments; refusing dependencies unless opted in.
6. **Q6. Composition.** *Settled:* `Html.join`, `Html.empty`, `Html.text`,
   `if`/`else` and `match` over `Html`; `fill` takes a declared nominal record;
   no partial fill. *Why:* Bynk has no anonymous record literals or named
   arguments, and a declared record makes the template's contract readable in
   Bynk. *Rejected:* a record inferred from the file, which lets an HTML file
   silently change a Bynk API; partial fills.
7. **Q7. Responses.** *Settled:* `Page(html)` and `PageError(status, html)`; no
   streaming HTML (§8.3). *Rejected:* `RawError(status, html)` with an implied
   content type, which overloads its arity.
8. **Q8. CSP.** *Ruled:* hash-based only, opt-in as `csp: computed`, no nonces;
   never applied to a `Forward`; an explicit `csp` string always wins (§10).
   *Rejected:* nonces (uncacheable pages); per-response policies; dropping CSP
   from the track.
9. **Q9. HTMLRewriter.** *Settled:* separate, sharing only `Html` as inserted
   content, and never applied to a `Page` (§9.3). *Rejected:* folding it in.
10. **Q10. Testing.** *Settled:* `h.toString()` and snapshots; no structural
    assertions in this track; property, differential and fuzz tests for the
    tokeniser and the escapers (§11). *Rejected:* `h.text()`, which reads as
    the DOM's `textContent`; a run-time parser for structural matching.
11. **Q11. The declaration form.** *Ruled:* a `template` declaration, as ADR
    0608 F reserves. *Rejected:* `embed shell: Html`, which gives `embed` two
    contracts by annotation, the thing 0608 F was written to prevent.
12. **Q12. Where `Html` lives.** *Settled:* `bynk.html`, beside ADR 0993's
    escapes. *Rejected:* a built-in, which would break every project's own
    `type Html`.
13. **Q13. ADR 0143.** *Settled:* revised by D1 for HTML responses; `http.md`'s
    passages rewritten in slice 6.
14. **Q14. Names.** *Settled:* `Href`, `Page`, `PageError`, `Template[F]`.
15. **Q15. Secrets.** *Settled:* every secret kind refused in every context,
    with its own code; only `reveal()` reaches text (§5.5).
16. **Q16. Message bundles.** *Settled:* follows the #911 ruling; a template
    never names a code, and is filled with strings rendered in the filling
    unit (§9.1).
17. **Q17. Pass-through.** *Settled:* a `Forward` cannot become `Html`;
    `Respond(Page(…))` is allowed; the computed CSP is never stamped on a
    `Forward` (§9.2).
18. **Q18. Email.** *Settled:* a follow-on after promotion, not a slice
    (§9.4).
19. **Q19. The 1.0 position.** *Settled:* additive, one gate, `template` a
    contextual keyword (§14).

### 17.1 Decisions index

| | Question | Outcome |
|---|---|---|
| Q1 | `\(name)` placeholders, literal `\(` otherwise | Accepted in review: as recommended |
| Q2 | Hand-written tokeniser; corpus, differential and fuzz tests | Accepted in review: as recommended |
| Q3 | Contexts C1–C13; script and style refused except JSON data | Accepted in review: as recommended |
| Q4 | Opaque `Href`; four schemes; no decoding; `srcset` refused | Accepted in review: as recommended |
| Q5 | `Html.unsafe(markup, reason)`; dependencies allowed and listed | Ruled: as recommended |
| Q6 | `join`/`empty`/`text`; nominal fields record; no partial fill | Accepted in review: as recommended |
| Q7 | `Page` and `PageError`; no streaming HTML | Accepted in review: as recommended |
| Q8 | Hash-only opt-in CSP; no nonces; never on a `Forward` | Ruled: as recommended |
| Q9 | HTMLRewriter separate, sharing `Html` | Accepted in review: as recommended |
| Q10 | `toString()` and snapshots; no structural assertions | Accepted in review: as recommended |
| Q11 | A `template` declaration | Ruled: as recommended |
| Q12 | `Html` in `bynk.html` | Accepted in review: as recommended |
| Q13 | ADR 0143 revised by D1 | Accepted in review: as recommended |
| Q14 | `Href`, `Page`, `PageError`, `Template[F]` | Accepted in review: as recommended |
| Q15 | Secret kinds refused everywhere | Accepted in review: as recommended |
| Q16 | Templates and message bundles per the #911 ruling | Accepted in review: as recommended |
| Q17 | `Forward` never wrapped; `Respond(Page(…))` allowed | Accepted in review: as recommended |
| Q18 | Email after promotion | Accepted in review: as recommended |
| Q19 | Additive and experimental, one gate | Accepted in review: as recommended |

## 18. Docs delta

None of these pages is edited by this settling PR. Each is updated by the
slice named.

- `reference/embeds.md` (slices 1 and 6): `template` beside `embed`; the
  "nothing is escaped for you" paragraph and the "leftover `{{`" test point to
  templates.
- `book/guides/entry-points/serve-a-page.md` (slice 6): rewritten around
  `template`, `Href`, `Page` and `Html.unsafe`'s absence.
- `reference/http.md` (slices 4, 6 and 8): *Raw responses*' "does **not**
  template HTML" sentence; *What is deliberately excluded*'s generated-CSP item
  and "a typed-HTML feature, not this section"; `Page` and `PageError` beside
  `Raw` and `RawError`; `csp: computed` in the `security { }` section.
- `reference/bynk-capabilities.md`, `bynk.html` (slices 1, 2, 3): `Html`,
  `Href`, `Template[F]` and their operations; the doc comments of `escapeText`
  and `escapeAttr` gain a pointer to templates.
- `reference/assets.md` (slice 2): an asset fills a code-loading attribute.
- `reference/testing.md` (slice 3): asserting on a page, `toString()` and
  snapshots.
- `reference/spec/static-semantics.md` (slices 1–3): the template and fragment
  rules, the contexts, and every code in §16.1; the embed section's
  "has no interpretation" sentence gains its `template` counterpart.
- `reference/spec/lexical-grammar.md` and `reference/grammar.md` (slices 1 and
  3): the `html"` prefix beside `r"`, and `template` as a contextual keyword;
  the language-tag paragraph distinguishes `html"""` from `"""html`.
- `reference/keywords.md` (slice 1): `template` as contextual.
- `reference/upgrading.md` and the changelog (each slice): the new gate; no
  migration is needed.
- The generated diagnostic index picks up each code from the registry; each
  slice adds its codes' explain entries.
- A security concept page, `book/concepts/…/understand-html-safety.md`
  (slice 6): the guarantee, the contexts, the known limits of §13, and the
  `--unsafe-html` listing.
- vscode-bynk's README and changelog, and `site-theme`'s changelog (slice 5).
- `design/bynk-design-notes.md` (slice 0 ADRs): where it says Bynk does not
  serve markup, cite D1.
