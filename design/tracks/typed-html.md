# Typed HTML: templates checked at compile time, safe by construction

- **Status:** Settling in the draft PR that adds this doc. Spine issue: #3927,
  which carries the live slice status.
- **Realises:** the HTTP security review's finding on HTML escaping. A page a
  Bynk service serves is built by string replacement today, and escaping is the
  author's job. This track makes escaping the compiler's job, by context. It
  builds on #3924 (`Html.escapeText` and `Html.escapeAttr`, the building block)
  and #3923 (the HTML response headers).
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md). It qualifies on all
  three axes (§1.2).
- **Governing records:**
  [ADR 0608](../decisions/0608-embedded-text-in-the-language.md) and
  [ADR 0609](../decisions/0609-embedded-text-content-rules.md) (embedded text;
  0608 F reserves a separate `template` declaration for parsed files),
  [ADR 0143](../decisions/0143-raw-http-body.md) (`Raw`, and the stance that
  Bynk does not template HTML, which this track revises),
  [ADR 0164](../decisions/0164-http-security-headers.md) (`security { }`),
  [ADR 0659](../decisions/0659-url-commons.md) (`bynk.url` and its `Url`
  record), [ADR 0075](../decisions/0075-string-interpolation.md) (`\(…)`
  holes), [ADR 0739](../decisions/0739-multiline-string-language-tags.md)
  (value-neutral language tags). Four ADRs land up front (§12).

## 1. The theme

### 1.1 What this track settles

Bynk serves HTML pages. The guide *Serve a page from an HTML file* reads a page
shell with `embed`, fills its `{{markers}}` with `String.replace`, and returns
`Raw(…, "text/html; charset=utf-8")`. Nothing is escaped for the author. One
forgotten escape around request data is a cross-site scripting (XSS) hole. Even
a correct escape is wrong in some positions: a URL attribute (`javascript:`), an
unquoted attribute, `<script>`, `<style>`, an event handler attribute.

An embedded file is a compile-time constant. The compiler can tokenise its
HTML while building and know the context of every placeholder. This track
turns that into a guarantee:

1. **`Html` is a type, not a `String`.** Data reaches markup only through
   escaping chosen by context. Markup is inserted raw only through
   `Html.trusted`, which is explicit and searchable.
2. **Templates are checked.** A `template` declaration reads a file from
   `embeds/`, finds its placeholders, and checks them against a declared
   record: every placeholder known, every field used, each field's type
   accepted by every context it appears in.
3. **Fragments are checked the same way.** An `html"…"` literal is tokenised at
   compile time, and each hole is typed by its context.
4. **URLs have a type.** `Href` is the only thing a URL attribute takes, and its
   constructor refuses dangerous schemes.
5. **Pages have a response.** `Page(html)` sends `text/html; charset=utf-8` and
   gets #3923's HTML defaults.

**End state when the track retires:** data can only reach markup escaped for
its context; markup can only be inserted deliberately; placeholder mistakes are
compile errors; and the serve-a-page guide needs no hand-written escaping and
no "leftover `{{`" test.

### 1.2 Why a track (the ADR 0076 trigger)

- **Multi-increment.** The tokeniser and context model, the `Html` type and
  `template`, `Href`, the `html"…"` literal, `Page`, and the book. Each is
  reviewable on its own, in a forced order.
- **Surface not yet settled.** Placeholder syntax, the declaration form, what
  counts as a URL, how `trusted` is gated, and the relation to HTMLRewriter
  (#3519) are open (§13).
- **Security boundary.** XSS. A context missed, or an escape hatch that is too
  easy, gives false confidence, which is worse than none.

## 2. What's true today

### 2.1 Pages are strings

- An embed is a `String` and its text "has no interpretation: `\(` and `{{ }}`
  in the file mean nothing to the language"
  (`site/src/content/docs/book/spec/static-semantics.md:3437`). The reference
  says "nothing is escaped for you" and recommends a test that no `{{` is left
  (`site/src/content/docs/book/reference/embeds.md:58-65`).
- The guide hand-writes `escape` as `&`, `<`, `>` and `"` only
  (`site/src/content/docs/book/guides/entry-points/serve-a-page.md:47-52`). It
  misses `'`, so it is unsafe in a single-quoted attribute. #3924 replaces it
  with two library functions.
- The guide inserts `content` unescaped because "`content` is markup you built"
  (`serve-a-page.md:62-64`). Nothing in the types records which strings are
  markup.
- The guide fills `href="{{style}}"` with `style.url`
  (`serve-a-page.md:56`). That is safe only because an asset's URL is a
  compiler-made path (`site/src/content/docs/book/reference/assets.md:82`).
  Nothing stops a request value taking its place.
- A page is served as `Raw(Bytes.fromUtf8(…), "text/html; charset=utf-8")`
  (`serve-a-page.md:77`). The content type is an unvalidated `String`
  (`site/src/content/docs/book/reference/http.md:844-846`).

### 2.2 The records this track revises

- ADR 0608 F: "a later `template` declaration would parse holes and check their
  types and escapes. Different contracts are different declarations, so
  `embed` never starts to interpret `{{ }}`." The seam exists and is reserved
  in prose; `template` is not yet a keyword.
- `http.md:820-825` (and ADR 0143's context): "Bynk serves *bytes with a
  content-type*; it does **not** template HTML (that is the frontend tier)."
  `http.md:840-842` holds "a `404` with an HTML error page" out of scope, yet
  `RawError` already serves one (`http.md:291-296`), and #3923 cites
  `RawError(NotFound, html, "text/html")` as current practice.
- `http.md:1355-1361`: CSP and `X-Frame-Options` are excluded because Bynk does
  not template HTML; "If Bynk ever serves HTML, the `security { }` section is
  where they would land." #3923 lands them.

### 2.3 The machinery a template needs

- **Reading the text.** A build reads every declared embed's text
  (`bynk-driver/src/project.rs:3131-3139`, through
  `bynk_check::embeds::read_declared_embeds`). The editor's analysis only lists
  `embeds/` to check that files exist; it does not read them
  (`bynk-check/src/analysis.rs:804-806`). A template must be read in both, or
  the editor cannot show template errors.
- **Emitting the text.** An embed is emitted as `export const name: string`
  (`bynk-emit/src/emitter.rs:499-519`). A template is emitted as a function
  instead (§6).
- **Holes.** An interpolation hole takes `String`, `Int`, `Float`, `Bool`,
  `Decimal` or a refinement of one; an opaque type is refused
  (`bynk.types.interpolation_non_scalar`,
  `site/src/content/docs/book/spec/static-semantics.md:558-566`). So an opaque
  `Html` can never be turned into a `String` by accident through a plain
  string.
- **Language tags are value-neutral.** `"""html` already highlights as HTML,
  but "the tag is **not part of the value**"
  (`site/src/content/docs/book/spec/lexical-grammar.md:146-158`; ADR 0739). It
  cannot change type without changing every program that uses it.
- **Built-in type names are reserved.** A name in `BUILTIN_TYPE_NAMES` may not
  be a user's `type` (`bynk.resolve.reserved_builtin_type`,
  `bynk-syntax/src/keywords.rs:306-322`). ADR 0659 B kept `Url` out of the
  built-ins for this reason. `Html` as a built-in would break every project's
  own `type Html`.
- **`Url` is taken.** `bynk.url` declares `Url` as a record
  (`site/src/content/docs/book/reference/bynk-capabilities.md:2542-2556`). A
  second `Url` in `bynk.html` would clash in any unit that uses both.
- **`Asset` is not data.** It may not be a record field or cross a boundary
  (`assets.md:116-118`). A template's fields record cannot carry one.
- **Literal-only precedents.** D1 SQL must be a literal so injection cannot be
  assembled at run time (ADR 0666 B), and a `const` bound to a literal counts
  (ADR 0762). A template is the same idea applied to markup.
- **Parser precedents.** The YAML subset is a hand-written parser held correct
  by a conformance corpus and a differential test against a dev-only library
  (ADR 0789). The Postgres check chose the pure-Rust `sqlparser` over a C
  library partly because it builds for `wasm32` (ADR 0816 B).

### 2.4 The response side

- `applySecurityHeaders` stamps `nosniff` and HSTS only
  (`bynk-emit/runtime/src/http.ts:1660-1668`). #3923 extends it to stamp
  `frame-ancestors 'none'` and `Referrer-Policy` on `text/html` responses.
- `Raw` carries a weak `ETag` on an eligible `GET` up to 1 MiB
  (`http.ts:317-323`). `RawError` sets only its content type
  (`http.ts:279-284`).
- `HTTP_VARIANTS` is the compiler's registry of `HttpResult` variants
  (`bynk-syntax/src/ast.rs:2096`), mirrored by `HTTP_STATUS` in the runtime
  (`http.ts:27-65`).
- Snapshots exist: `expect v matches snapshot "name"` compares a codable value's
  JSON (`site/src/content/docs/book/reference/testing.md:917-922`).

## 3. Goals and non-goals

**Goals**

- A value reaches markup only escaped for its context, or through
  `Html.trusted`.
- Every placeholder in a template is known, every field is used, and each
  field's type is one its contexts accept. Mistakes are compile errors, in the
  editor too.
- A context the model does not understand is refused at compile time, never
  guessed.
- A URL attribute takes only a value whose scheme was checked; an attribute
  that loads code takes only the service's own assets.
- A page response carries the right content type and #3923's defaults.
- No run-time HTML parsing: a template compiles to string concatenation.

**Non-goals**

- Sanitising third-party HTML. Markup a project receives and marks trusted is
  outside the guarantee, and the docs say so.
- A component model, client-side rendering or hydration.
- CSS as a typed value (`Css`). Placeholders in `style` are refused.
- Script placeholders other than JSON data blocks (§13, Q3).
- Templates in package dependencies, as for embeds today.
- `srcset`, foreign content (SVG, MathML) and dynamic attribute names in the
  first version. Each is refused, and can be added later without breaking
  anything.

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
  asset `style` and is filled at compile time. It is not a field (§5.2).
  A placeholder in a code-loading attribute names an asset in scope; anywhere
  else it names a field. A name that is both a field and an asset in scope is
  refused wherever it appears (`bynk.template.ambiguous_placeholder`), and an
  asset name in an ordinary context is an unknown placeholder, with a note
  naming the asset. So renaming an asset, or adding a field, can never
  silently change what a placeholder binds to: it is a compile error instead.
- `\(title)` appears twice, both times as element text, so `String` is
  accepted. `\(home)` is a URL attribute and takes `Href`. `\(content)` is
  element text and takes `Html` as is.
- A misspelt `\(titel)` is `bynk.template.unknown_placeholder`, with the
  nearest field named. A field no placeholder uses is
  `bynk.template.unused_field`. Diagnostic codes here are indicative.

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
commons that #3924 adds, and known to the compiler by its resolved identity (as
ADR 0666 B keys the D1 check on `bynk.cloudflare`'s `D1`). A unit opts in with
`uses bynk.html`. No user name becomes reserved.

| Operation | Type | Note |
|---|---|---|
| `html"…"`, `html"""…"""` | `Html` | checked fragment (§5.3) |
| `t.fill(fields)` | `Html` | `t: Template[F]`, `fields: F` |
| `Html.text(s)` | `String -> Html` | escaped as element text |
| `Html.join(xs)` | `List[Html] -> Html` | concatenation |
| `Html.empty` | `Html` | |
| `Html.trusted(s, reason)` | `(String, String) -> Html` | the one raw door (Q5) |
| `h.toString()` | `String` | markup out, for tests and other bodies (Q10) |
| `Html.escapeText`, `Html.escapeAttr` | `String -> String` | #3924, unchanged |

`Html` is not data at a boundary. It may be a record field (a fields record
needs it), but a type containing it is refused by the codec, as a request
parameter, by `Json.decode` and in agent `store` fields. Markup from outside
enters only through `Html.trusted`. To send markup as JSON, send
`h.toString()`.

### 5.2 Contexts

The compiler tokenises a template or fragment as a browser does, and records
the tokeniser state at each placeholder. That state is the context:

| Context | Accepts | Inserted as |
|---|---|---|
| Element text (data state) | `String`, `Int`, `Float`, `Bool`, `Decimal`, a refinement of one; `Html` | text escaped with `escapeText`; `Html` as is |
| Text of `<title>`, `<textarea>` (RCDATA) | the scalars above | escaped with `escapeText`; `Html` refused |
| Quoted attribute value, ordinary attribute | the scalars above | escaped with `escapeAttr` |
| Quoted URL attribute, whole value | `Href` | escaped with `escapeAttr` |
| Quoted URL attribute, after a fixed relative or safe-scheme prefix | the scalars above | `percentEncode()`, then `escapeAttr` |
| Code-loading attribute | an `Asset` (a name in scope; an `Asset` expression in a fragment) | its `.url`, filled at compile time in a template |
| `<script type="application/json">` whole body | a codable value | JSON, with `<`, `>`, `&` as `\u003c`, `\u003e`, `\u0026` (slice 6) |
| Anything else | nothing: refused | |

URL attributes are `href`, `src`, `action`, `formaction`, `poster`, `cite`,
`ping`, `background`, `xlink:href` and
the rest of the list Go's `html/template` uses for the same purpose.
Code-loading attributes are `<script src>`, `<link href>` unless `rel` is
literally a navigational value (`canonical`, `alternate`, `icon`, `prev`,
`next`, `author`, `license`), `<base href>`, `<object data>`,
`<embed src>`, and SVG's `<script href>` and `<script xlink:href>`. The SVG
entries cannot be reached while placeholders inside `<svg>` are refused; they
are listed so that admitting foreign content later cannot open them by
default. They decide what code a page runs, so only code the service
ships may fill them.

Refused, each with its own diagnostic and a note naming the fix:

- a placeholder in `<script>`, `<style>` or other raw text, or in an `on*`,
  `style` or `srcdoc` attribute;
- an unquoted attribute value;
- a placeholder in a tag name, an attribute name, a comment, a doctype or a
  CDATA section;
- a placeholder in `<meta content>` when the element has `http-equiv`;
- a placeholder inside `<svg>` or `<math>`, or an `<svg>` or `<math>`
  subtree left open at the end of a template or fragment (§5.3);
- `srcset` and `imagesrcset`.

A name used in several places must satisfy every context it appears in. A
field used both as element text and as a whole URL is refused, naming both
places.

### 5.3 Fragment rules

An `html"…"` literal starts in element text and must end there: not inside a
tag, an attribute, a comment, or raw or RCDATA text. So an `Html` value can
never leave a parent page in another state. Element balance is not required:
`html"<ul>"` is a valid fragment, as the example in §4.2 needs.

Foreign content is the exception, because it is tree-builder state, not
tokeniser state. Inside `<svg>` or `<math>` the tree builder does not switch
the tokeniser to raw text or RCDATA for `<script>`, `<style>` or `<title>`, it
reads `<![CDATA[`, and SVG's `<script href>` loads code. A fragment checked
from the data state cannot see that a parent opened `<svg>`, so
`Html.join([html"<svg>", html"<script href=\"\(h)\"></script>", html"</svg>"])`
would pass a rule about tokeniser state alone. So:

- every `<svg>` or `<math>` a template or fragment opens must be closed in the
  same template or fragment, and an `</svg>` or `</math>` with no opener in it
  is refused;
- inside such a subtree, the checker follows the foreign-content rules above
  to find its end, and refuses any placeholder;
- in the first version, a subtree may not contain `<foreignObject>`,
  `<annotation-xml>`, `<desc>` or `<title>` in SVG, or any HTML element that
  makes the tree builder leave foreign content (`<p>`, `<div>`, `<b>` and the
  rest of the WHATWG list), so that counting open and close tags matches what
  the tree builder does.

An `Html` value can therefore never end inside foreign content, and composition
cannot place one there. Static inline icons (`<svg>…</svg>` with no
placeholders) keep working.

A template file follows the same rules.

### 5.4 `Href`

`Href` is an opaque type over `String` in `bynk.html`:

- `Href.of(s) -> Option[Href]` accepts a relative reference, or an absolute URL
  whose scheme is `http`, `https`, `mailto` or `tel`, compared without regard
  to case. It refuses any ASCII control character or space anywhere (browsers
  strip them, so `java\tscript:` runs), and any other scheme, `javascript:`,
  `data:`, `vbscript:` and `file:` among them.
- `Href.asset(a)` is an asset's URL.
- `Href.fromUrl(u)` takes `bynk.url`'s `Url` record and applies the same scheme
  rule.
- `Href.root` is `/`.

A partial URL such as `href="/orders/\(id)"` is common. After a literal prefix
that is relative (starts with `/` but not `//`, or with `./`, `?` or `#`), or
that is `http://` or `https://` followed by a host and then `/`, `?` or `#`, a
scalar is accepted and percent-encoded with the existing `percentEncode()`
(ADR 0659 A). That encodes `/`, `:`, `@` and `%`, so a value cannot turn `/`
into `//evil.example`, and the host is always the author's: a bare
`https://\(x)` or `https://example.com\(x)` is refused, since the value would
choose or extend the host.

`percentEncode()` keeps `.`, so a value of `.` or `..` that fills a whole path
segment is still a dot segment: `/orders/\(id)` with `id` = `..` resolves to
`/`, and `/orders/\(id)/cancel` to `/cancel`. Encoding cannot fix this, since
the URL standard also reads `%2e` as a dot. It stays on the same origin, so it
is not XSS, but it can point a link or form at another of the service's paths.
It is a known limit (§10); a field typed as `Int` or a refinement that excludes
dots cannot produce it.

## 6. Emission

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
- `html"…"` lowers the same way, inline.
- `Html.trusted` lowers to the identity.
- The escape functions are #3924's, emitted only when used.
- A template's text counts against the embed size caps (ADR 0609 C).
- The editor's analysis reads template files, not only lists them
  (§2.3), so template errors show while editing. The caps bound the cost.

## 7. Responses

- **`Page(html)`**: a 200 with `content-type: text/html; charset=utf-8` and the
  body as UTF-8. It is `ETag`-eligible on the same terms as `Raw`, and `@cache`
  applies. It is one new `HTTP_VARIANTS` row and payload shape, following ADR
  0143's pattern for `Raw`.
- **`PageError(status, html)`**: an error status with an HTML body, under the
  same `ErrorStatus` rule as `RawError` (`http.md:311-316`).
- **Security headers.** Both are `text/html`, so #3923's defaults apply with
  no extra code. `nosniff` already applies to every response.
- **Tests.** A unit-tier case matches `Page(h)` and reads `h.toString()`. At the
  system tier a non-JSON 200 already reads as `Raw` (#3050); `Page` reads the
  same way there.
- **Streaming HTML** is not part of this track (Q7).

## 8. CSP and security headers

#3923 makes `text/html` responses unframeable by default, sets a referrer
policy, and adds an opt-in, verbatim `csp`. This track adds nothing to that in
its core slices.

A template and a fragment are fully known at compile time, so the compiler
knows the exact text of every static inline `<script>` and `<style>` a service
can produce. A later, opt-in slice (Q8) can compute a hash-based policy from
them. It cannot see markup inserted with `Html.trusted`, which such a policy
would then block. That is the right failure for a security header, but it
would surprise people, so it is never the default.

## 9. Risks

- **The tokeniser differs from browsers.** A mis-classified context is a
  silent hole. Mitigated by implementing the WHATWG tokeniser states the model
  needs, refusing everything else, and testing against the `html5lib-tests`
  tokeniser corpus and a differential test against `html5ever` as a dev
  dependency, as ADR 0789 did for YAML.
- **`Html.trusted` becomes the easy path.** Mitigated by the required reason
  literal, a hint when its argument is a literal (write `html"…"` instead), and
  docs that teach templates first.
- **The refusals are too strict for real pages.** Measured on the guide and on
  the known adopter (bynk-lang/compat-board, cited by ADR 0608) before slice 1
  is accepted. Each refusal names its fix.
- **Two `Html`s.** #3924 introduces `Html.escapeText` before `Html` is a type.
  The commons must be shaped so that the later type keeps those calls working.
- **A breaking change for `"""html` users.** Avoided: `"""html` stays a
  `String` (ADR 0739). Only the new `html"` prefix is typed.
- **Template text in the editor.** Reading template files in analysis adds IO
  to every check. Bounded by the size caps; only declared files are read.

## 10. Threat model

- **Assets:** the integrity of pages a service serves; users' sessions and data
  in the browser.
- **Adversary:** anyone who can put text into data a page renders: a form
  field, a stored record, a query parameter, a header.
- **Where verification happens:** at compile time for every template and
  fragment (context and type of each placeholder); at run time in the escape
  functions (property-tested in #3924) and in `Href.of`'s scheme check.

| Context | Attack | Handled by |
|---|---|---|
| Element text | `<script>` in a value | `escapeText` |
| `<title>`, `<textarea>` | `</title><script>` | `escapeText`; `Html` refused |
| Quoted attribute | breaking out with `"` or `'` | `escapeAttr` (covers both, and `` ` ``) |
| Unquoted attribute | breaking out with a space | refused |
| URL attribute | `javascript:`, `data:`, `java\tscript:` | `Href` only; `Href.of` refuses |
| Partial URL | `//evil.example`, a host chosen by the value | safe prefix, with a fixed host when absolute, required; value percent-encoded (`.` and `..` segments are a known limit) |
| Code-loading attribute | a script from the attacker's origin | `Asset` only |
| `<script>`, `on*` | code injection | refused |
| `<style>`, `style` | CSS injection, exfiltration | refused |
| JSON data block | `</script>` in a string | `<`, `>`, `&` escaped as `\u` sequences |
| Comment, tag or attribute name | structure injection | refused |
| `<meta http-equiv>` | refresh to `javascript:` | refused |
| `srcdoc` | a nested document | refused |
| SVG, MathML | different parsing rules; SVG `<script href>` | a placeholder inside, or a subtree left open across fragments, refused |

**Known limits, stated in the docs:**

- Markup passed to `Html.trusted` is outside the guarantee. The track does not
  sanitise HTML.
- DOM-based XSS in the page's own scripts (an asset reading `location.hash`) is
  outside what the compiler sees.
- An attacker-chosen `id` or `name` value can clobber DOM globals. Escaping
  does not prevent it; it is noted, not solved.
- `Href` allows any `https` host. That is not XSS, but a link to a hostile site
  is possible when a value chooses the host.
- In a partial URL, a value of `.` or `..` filling a whole path segment moves
  the URL to another path on the same origin (§5.4). Not XSS; typing the field
  as `Int` or a refinement without dots avoids it.

## 11. Slices

| # | Slice | Depends on |
|---|---|---|
| 0 | Settle this doc; land the four ADRs (§12) | — |
| 1 | `Html` in `bynk.html`; the `template` declaration and `fill`; placeholder recognition, including `\(` that is not a placeholder (Q1); the tokeniser and the context model for element text, RCDATA and quoted attributes; every refusal, including foreign-content balance (§5.3); the editor reads template text | 0, #3924 |
| 2 | `Href`, URL and code-loading attributes, partial URLs | 1 |
| 3 | `html"…"` and `html"""…"""` literals; `Html.join`, `Html.empty`, `Html.text`, `Html.trusted`, `toString` | 1 |
| 4 | `Page` and `PageError`, with #3923's defaults | 1, #3923 |
| 5 | The book: the serve-a-page guide, the embeds reference, and `http.md`'s "does not template HTML" passages rewritten around `Html` | 2, 3, 4 |
| 6 | JSON data blocks (`<script type="application/json">`, `application/ld+json`) | 3 |
| 7 | Opt-in hash-based CSP from templates, if Q8 is accepted | 4, #3923 |

Slices 6 and 7 are not needed for the end state and may be cut.

## 12. Decisions (ADRs to land up front)

- **`Html` is not a `String`.** Markup and text are different types, declared
  in `bynk.html`. The only bridges are escaping by context (automatic) and
  `Html.trusted` (explicit). `Html` does not cross a boundary. This revises
  ADR 0143's "Bynk does not template HTML" for HTML responses.
- **The context model.** The contexts of §5.2, what each accepts, and that
  anything else is refused at compile time. Includes the tokeniser decision
  (hand-written, WHATWG states, corpus and differential tests) and the fragment
  rule (§5.3).
- **The `trusted` rule.** `Html.trusted(markup, reason)` with a literal reason;
  permitted everywhere, including dependencies; no warning.
- **Templates are their own declaration.** `template name: Fields = "path"`,
  honouring ADR 0608 F; `\(name)` placeholders; a declared nominal fields
  record. `embed` stays verbatim.

Recorded in their slices: `Href`'s scheme rule (slice 2), `Page` and
`PageError` (slice 4), JSON data blocks (slice 6), CSP hashes (slice 7).

## 13. Open questions — proposed answers

Proposed for review, not settled. Each answer is open until the PR is marked
ready.

**Q1. Placeholder syntax in templates.**
Proposed: `\(name)`, a bare name, not an expression.
It reads like a hole in an `html"…"` literal, so one rule covers both. `{{` is
used by Vue, Alpine, Angular and Handlebars, which may share the file; `\(` is
rare in HTML and appears mainly in inline scripts, where placeholders are
refused anyway. A bare name keeps the file free of Bynk expressions.
Only `\(` followed by a name and `)` is a placeholder, and only when its
backslash does not follow another backslash. Any other `\(` is literal text,
and the file has no other escapes, so MathJax's `\(x^2\)`, a regex such as
`/\(/` in an inline script, and `\\(` all pass through unchanged. A literal
`\(name)` in text or an attribute is written with a character reference,
`&#92;(name)`. In raw text a placeholder-shaped `\(name)` is refused like any
placeholder there, with a note to rewrite it.
Rejected: `{{name}}`, which authors use today; migration is a find-and-replace
either way, and the clash is permanent.

**Q2. The HTML parser.**
Proposed: a hand-written tokeniser in the checker, implementing the WHATWG
tokeniser states the context model needs and refusing the rest; tested against
the `html5lib-tests` corpus and differentially against `html5ever` as a dev
dependency only.
The compiler needs the tokeniser state at each placeholder, not a tree. A tree
builder's error recovery is what the model should refuse, not reproduce. It
must build for `wasm32` (ADR 0816 B's concern). ADR 0789 is the precedent.
Rejected: `html5ever` as a compiler dependency (a tree builder and its
dependencies for a fraction of its use); and a restricted subset grammar of our
own, which would refuse valid pages a browser parses fine.

**Q3. Which contexts, and what each takes.**
Proposed: the table in §5.2. Element text takes scalars (escaped) or `Html`;
quoted attributes take scalars; URL attributes take `Href`; code-loading
attributes take only an `Asset`; `style` and `<style>` are refused (no `Css`
type); `<script>` and `on*` are refused, except a whole-body JSON data block
taking a codable value (slice 6); unquoted attributes are refused.
Each accepted context has one escape with one right answer. CSS and JavaScript
have their own nested contexts; typing them is a track of its own, and the
safe answer is an asset. Code-loading attributes are split from URL attributes
because they decide what code runs.
Rejected: a `Css` type, and Go's choice of letting any URL fill `<script src>`.

**Q4. The URL type.**
Proposed: an opaque `Href` in `bynk.html`, built by `Href.of(s) ->
Option[Href]` (relative, `http`, `https`, `mailto`, `tel`; no control
characters or spaces), `Href.asset(a)`, `Href.fromUrl(u)` and `Href.root`. No
`data:` in the first version. No route builder: Bynk has none.
Opaque, so `Href.of` is the only door and no literal admission applies.
`Url` is taken by `bynk.url`'s record (ADR 0659 B); a second `Url` would clash
in any unit using both, and the name `Href` says where it goes.
Rejected: a refined `String` (widens back to `String` and invites a regex
scheme check that misses `java\tscript:`); and reusing `bynk.url`'s `Url`
record, which allows any scheme.

**Q5. `Html.trusted(s)`.**
Proposed: `Html.trusted(markup, reason)`, where `reason` must be a string
literal; no `bynk.html.trusted_dynamic` warning; a hint when `markup` is a
literal; permitted in dependencies.
Every real use is dynamic (rendered Markdown, CMS content), so a warning would
fire on every use and teach people to ignore it. A literal reason is visible
in review and in hover, and searchable. A Markdown renderer is the obvious
dependency, and it cannot exist if dependencies may not call `trusted`.
Rejected: a warning on run-time arguments; refusing `trusted` in dependencies.

**Q6. Composition.**
Proposed: `Html.join(List[Html])`, `Html.empty`, `Html.text(s)`, and plain
`if`/`else` and `match` over `Html` values. `fill` takes a declared nominal
record, `t.fill(PageFields { … })`. No partial fill: compose a layout template
with an `Html` field instead.
Bynk has no anonymous record literals and no named arguments in calls, so a
nominal record is the form the language already has. Declaring it makes the
template's contract readable in Bynk without opening the HTML, and a change
to either side is a compile error on the other.
Rejected: a record type inferred from the file, which makes the HTML file
silently change a Bynk API; and partial fills, which need a type per subset of
fields.

**Q7. Responses.**
Proposed: `Page(html)` (200, `text/html; charset=utf-8`, `ETag`-eligible,
`@cache`, #3923's defaults) and `PageError(status, html)` with `RawError`'s
status rule. No streaming HTML in this track.
A new variant keeps the one-variant-one-shape model of ADR 0143.
`RawError` cannot imply its content type without overloading its arity.
Streaming HTML needs `Stream[Html]` and fits better with #3519; `RawStreamed`
already serves a streamed body.
Rejected: `RawError(status, html)` with an implied content type.

**Q8. CSP integration.**
Proposed: yes, as opt-in slice 7 only: a service-wide hash-based policy, the
union of the hashes of every static inline script and style its templates and
fragments contain, offered as a computed form of #3923's `csp`. Never the
default.
The union is sound: it allows only scripts the author wrote. It blocks
anything inserted with `Html.trusted` and all static `on*` handlers unless
`'unsafe-hashes'` is added; both would surprise as a default.
Rejected: per-response policies, which need to track which fragments reach
which response through run-time composition.

**Q9. HTMLRewriter (#3519).**
Proposed: stays separate. It shares `Html` only as the type of inserted
content: a rewriter's `append` or `replace` takes `Html`, and an attribute
setter takes `String` and escapes it.
A rewriter transforms markup the service did not build, so it has different
questions (selectors, streaming, effects). Sharing the type keeps inserted
content under this track's guarantee.
Rejected: folding a rewriter into this track, which would delay every slice
behind #3519's open questions.

**Q10. Testing.**
Proposed: `h.toString()` for assertions, and
`expect page.toString() matches snapshot "home"` using the existing snapshot
mechanism. No HTML-specific snapshot form.
`toString()` is the name Bynk already uses for an explicit conversion
(`Duration`, `Instant`, the numbers). `Html` does not cross boundaries, so it
is not codable, and its string is the snapshot.
Rejected: `h.text()`, which reads as the DOM's `textContent`, the text with
tags stripped.
