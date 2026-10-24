---
title: Embedded text
---
A context or a commons reads a project file's text with an `embed`
declaration. The file's contents become a `String` constant compiled into the
code; unlike an [asset](/book/reference/assets/) it is never served. See the
[grammar for `embed_decl`](/book/reference/grammar/#rule-embed_decl) and
[§5.12a of the specification](/book/spec/static-semantics/#embeds).

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

The text is not interpreted. `\(` and `{{ }}` mean nothing to the language, so
substituting into a template is ordinary `String` code, and nothing is
escaped for you. Escape every value you substitute into markup, and check a
rendered page for a placeholder you forgot to fill:

```bynk,fragment
expect page("Home", "hi").contains("{{") == false
```

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

A single file (`bynkc compile page.bynk`) has no project to find `embeds/` in,
so building one that declares an embed is `bynk.embed.needs_project`. Checking
it is unaffected.

## Embed or asset

| | `asset` | `embed` |
| --- | --- | --- |
| Folder | `assets/` | `embeds/` |
| Public | yes, served by the platform | no, part of the code |
| Value | an `Asset`, with a `.url` | a `String` |
| Content | any file | UTF-8 text only |

To link a stylesheet from a page whose shell is an embed, substitute the
asset's `.url` into the shell; see
[Serve a page from an HTML file](/book/guides/entry-points/serve-a-page/).
