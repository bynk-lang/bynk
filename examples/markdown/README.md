# Markdown

Render a small, safe subset of Markdown to HTML — the job a package registry
does for every package README. A pure library, plus an HTTP endpoint that
serves the result.

What it shows:

- **Text processing in a `commons`.** `render` folds the document line by line
  through a `Doc` record that holds the open block; `inline` splits spans on
  their delimiters. Both are plain functions, tested without a platform.
- **Escaping first, then markup.** Every character of the source is escaped
  before any markup is added, so the output holds only tags the renderer wrote.
  Links are limited to `http`, `https` and site-relative URLs (no
  `javascript:`), and a quote cannot break out of an `href`. The tests carry the
  hostile inputs.
- **A refined type at the boundary.** `Source` bounds the request body, so an
  oversized document is rejected with `400` before the handler runs.
- **A non-JSON response.** `Raw(bytes, "text/html; charset=utf-8")`.

The subset:

| Block | Written as |
|---|---|
| heading | `# ` to `###### ` |
| paragraph | consecutive lines; a blank line ends it |
| list | lines starting `- ` or `* ` |
| quote | a line starting `> ` |
| code | a fence of three backticks, closed by another (or the end of the document) |

Inside blocks: `` `code` ``, `**strong**`, `*emphasis*` and `[text](url)`.
There is no raw HTML, nesting, ordered lists or images; a link's URL ends at
its first `)`.

## Layout

```text
markdown/
├── bynk.toml
├── src/
│   ├── inline.bynk    # commons inline — escape, links, strong, emphasis, code spans
│   ├── blocks.bynk    # commons blocks — the line-by-line renderer
│   └── preview.bynk   # context preview — the HTTP endpoint
└── tests/
    ├── inline.bynk    # spans, links, and the hostile inputs
    ├── blocks.bynk    # every block, and how they end each other
    └── preview.bynk   # the endpoint
```

## Check and test

```sh
bynkc check .
bynkc test .
```

## Run it

```sh
bynk dev
```

```sh
curl -XPOST localhost:8787/render -d '{"markdown":"# Hi\n\nA *tiny* [library](https://bynk-lang.org)."}'
# <h1>Hi</h1>
# <p>A <em>tiny</em> <a href="https://bynk-lang.org">library</a>.</p>
```
