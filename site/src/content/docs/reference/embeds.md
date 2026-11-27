---
title: Embedded text
---
A context or a commons reads a project file's text with an `embed`
declaration. The file's contents become a `String` constant compiled into the
code; unlike an [asset](/reference/assets/) it is never served. See the
[grammar for `embed_decl`](/reference/grammar/#rule-embed_decl) and
[§5.12a of the specification](/reference/spec/static-semantics/#embeds).

## Declaring an embed

```bynk,fragment
commons app.page

---
The page shell.
---
embed shell = "embeds/page.html"

fn page(title: String, content: String) -> String {
	shell.replace("{{title}}", title).replace("{{content}}", content)
}
```

`embeds/page.html` is an ordinary file, so an editor gives it the highlighting,
formatting and diffs of its own language. `shell` is a `String`; `{{title}}`
and `{{content}}` are the author's convention, not syntax the language knows.

- **Path:** relative to the project root, and inside the project's `embeds/`
  folder. Anything else is `bynk.embed.outside_embeds_dir`; a path that is
  empty, absolute or contains `..` is `bynk.embed.unsafe_path`. The file must
  exist as a regular file (`bynk.embed.missing_file`) and be readable
  (`bynk.embed.unreadable_file`); symbolic links are not followed. Only the
  files a declaration names are read.
- **Name:** a value of type `String`, in the same namespace as functions,
  assets, types and events, so it may not repeat an embed
  (`bynk.embed.duplicate`) or reuse one of those names
  (`bynk.resolve.name_conflict`) in the same unit.
- **Where:** in a context or a commons, never an adapter
  (`bynk.embed.outside_context_or_commons`). A unit sees its own embeds and
  those of each commons it `uses`, like functions.
- **A dependency** may not declare one yet
  (`bynk.package.unsupported_in_dependency`), as for an asset.

## What the text is

The value is the file's contents verbatim, with three exceptions that keep a
build the same on every checkout:

- A leading byte-order mark is dropped.
- `\r\n` and a bare `\r` become `\n`.
- Nothing else changes; a trailing newline is kept.

The file must be UTF-8 text: invalid UTF-8 is `bynk.embed.invalid_encoding`
and a NUL byte is `bynk.embed.binary_content`. A binary file belongs in
`assets/`.

The text is not interpreted. `\(` and `{{ }}` mean nothing to an `embed`, so
substituting into one is ordinary `String` code, and nothing is escaped for
you. A [`template`](#templates) is the declaration that parses its file and
escapes each placeholder for you. With a plain embed, escape every value you
substitute into markup, with
[`escapeText` and `escapeAttr`](/reference/bynk-capabilities/#html-escaping-bynkhtml) from
`bynk.html`, and check a rendered page for a placeholder you forgot to fill:

```bynk,fragment
expect page("Home", "hi").contains("{{") == false
```

## Templates

A `template` declaration reads an HTML file from `embeds/` as an `embed` does,
and then parses it: each `\(name)` placeholder in the file names a field of a
declared record, and the compiler escapes each value for where it sits. It is
experimental, behind `bynk.html.typed` in `[unstable] allow`, and needs
`uses bynk.html`, whose `Html` type it makes.

```html
<!doctype html>
<html lang="en">
<head>
	<title>\(title)</title>
</head>
<body>
	<h1>\(title)</h1>
	<img alt="\(caption)" src="/logo.svg">
	\(content)
</body>
</html>
```

```bynk,ignore
commons app.page

uses bynk.html

type PageFields = {
	title: String,
	caption: String,
	content: Html,
}

template shell: PageFields = "embeds/page.html"

template footer = "embeds/footer.html"

fn render(title: String) -> Html {
	shell.fill(PageFields { title: title, caption: "Our logo", content: footer })
}
```

- **The value.** `shell.fill(fields)` takes the whole fields record and is
  `Html`; a template with fields is used only that way
  (`bynk.template.fill_required`). A template with no fields type, like
  `footer`, is an `Html` value as it stands, and a placeholder in it is
  `bynk.template.placeholder_without_fields`. `h.toString()` gives the markup
  back as a `String`. The fields type must be a non-generic record
  (`bynk.template.fields_not_record`), and the unit must `uses bynk.html`
  (`bynk.template.needs_bynk_html`).
- **Placeholders.** Only `\(` followed by a name and `)` is a placeholder,
  and only when its backslash does not follow another backslash. Any other
  `\(` is text, so MathJax's `\(x^2\)` and `\\(` pass through unchanged; write
  a literal `\(name)` as `&#92;(name)`. Every placeholder must name a field
  (`bynk.template.unknown_placeholder`, with the nearest field suggested), and
  every field must be used (`bynk.template.unused_field`). An asset's name is
  not a field, and a name that is both is `bynk.template.ambiguous_placeholder`.
- **Contexts.** Each placeholder's context decides what it takes and how it is
  escaped:

  | Where | Takes | Becomes |
  | --- | --- | --- |
  | Element text | a scalar (`String`, `Int`, `Float`, `Bool`, `Decimal`, `Duration`, `Instant`, or a refinement of one), or `Html` | the scalar escaped with `escapeText`; `Html` as is |
  | Text of `<title>` or `<textarea>` | a scalar | escaped with `escapeText`; `Html` is `bynk.template.html_in_rcdata` |
  | A quoted attribute value, either quote | a scalar | escaped with `escapeAttr` |
  | An unquoted attribute value | nothing | `bynk.template.unquoted_attribute`: quote it |
  | Inside `<script>` | nothing | `bynk.template.placeholder_in_script` |
  | Inside `<style>`, or a `style` attribute | nothing | `bynk.template.placeholder_in_style` |
  | An `on*` handler, `srcdoc`, `<meta content>` beside `http-equiv`, or an attribute that decides how the element is read (`<script type>`, `<link rel>`, `<iframe sandbox>`) | nothing | `bynk.template.placeholder_in_code_attribute` |
  | A tag or attribute name, a comment, a doctype, or anywhere inside `<svg>` or `<math>` | nothing | `bynk.template.placeholder_position` |
  | Inside `<xmp>`, `<iframe>`, `<noembed>`, `<noframes>`, `<noscript>`, or after `<plaintext>` | nothing | `bynk.template.placeholder_in_raw_text` |

  A URL attribute (`href`, `src`, `action` and the rest) takes no placeholder
  yet (`bynk.template.unsupported_markup`); write the URL literally. A field
  whose type a context does not take, such as an `Option`, is
  `bynk.template.context_type`, and a secret (`Secret`, `Authorization`,
  `ApiKeyValue`) is `bynk.template.secret_placeholder` in every context.
- **Markup the compiler does not model** is refused, never guessed
  (`bynk.template.unsupported_markup`): a repeated attribute, a `<` in an
  attribute name, an unclosed comment, a `</script` that is not the end tag,
  a template that ends inside a tag or an element whose text is not markup.
  Every `<svg>` and `<math>` a template opens must close in it, and may not
  hold an integration point such as `<foreignObject>`
  (`bynk.template.foreign_unbalanced`); a `<noscript>` must close in it too,
  and a `<select>` holds only `<option>`, `<optgroup>` and `<hr>`.
- **Errors at the template's line.** A placeholder or markup error points at
  the line and column in the template file, with a related location at the
  declaration, in the terminal and in the editor.
- **Not data.** `Html`, and any type that holds one, has no codec and is
  refused as a request parameter and in an agent's `store`
  (`bynk.html.at_boundary`): a template's escaping would be skipped by markup
  from outside.

A template compiles to a function of its fields that joins the file's text
with each escaped value: the page is built from strings, with no HTML parser
at run time. Its text counts against the same size caps as an embed's.

## Size

An embedded file may be at most 256 KiB, and the files a project's
declarations name at most 1 MiB in all, each file counted once
(`bynk.embed.too_large`). The text is part of the Worker's code, so an embed is
for markup and prose, not data.

## What it becomes

An embed is an exported `string` constant of the declaring module, imported by
name like a function wherever a unit `uses` it. A bundler drops one that
nothing references. `bynk dev` rebuilds when a file in `embeds/` changes.

A unit-tier `suite` case can name its target's embeds, and those of the commons
the target `uses`, to assert on the text. A `system` suite cannot: an embed is
code, not something a Worker serves.

A single file (`bynk compile page.bynk`) has no project to find `embeds/` in,
so building one that declares an embed is `bynk.embed.needs_project`. Checking
it is unaffected.

## Embed or asset

A suite's test input is a [`fixture`](/reference/fixtures/), which is never deployed.

| | `asset` | `embed` |
| --- | --- | --- |
| Folder | `assets/` | `embeds/` |
| Public | yes, served by the platform | no, part of the code |
| Value | an `Asset`, with a `.url` | a `String` (a `template`: `Html`) |
| Content | any file | UTF-8 text only |

To link a stylesheet from a page whose shell is an embed, substitute the
asset's `.url` into the shell; see
[Serve a page from an HTML file](/book/guides/entry-points/serve-a-page/).
