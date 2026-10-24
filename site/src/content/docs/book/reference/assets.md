---
title: Static assets
---
A context or a commons ships a static file, such as a stylesheet, script or
icon, with an `asset` declaration. The platform serves the file beside the
Worker, at a content-hashed URL, without running the Worker. See the
[grammar for `asset_decl`](/book/reference/grammar/#rule-asset_decl) and
[§5.12 of the specification](/book/spec/static-semantics/#assets).

## Declaring an asset

```bynk,fragment
commons compat.page

---
The board's stylesheet.
---
asset style = "assets/board.css"

fn link(a: Asset) -> String {
  "<link rel=\"stylesheet\" href=\"\(a.url)\">"
}

fn head() -> String {
  link(style)
}
```

- **Path:** relative to the project root, and inside the project's `assets/`
  folder. Anything else is `bynk.asset.outside_assets_dir`; a path that is
  absolute or contains `..` is `bynk.asset.unsafe_path`. The file must exist
  as a regular file (`bynk.asset.missing_file`) and be readable
  (`bynk.asset.unreadable_file`); symbolic links are not followed. Only the
  files a declaration names are read, so other files in `assets/` cost a
  build nothing.
- **Name:** a value of the built-in type `Asset`. Its one field, `.url`, is a
  `String`: the URL the file is served at. The name is in the same namespace
  as functions, so it may not repeat an asset (`bynk.asset.duplicate`), or a
  function, type, event, service, agent or provider
  (`bynk.resolve.name_conflict`) in the same unit.
- **Where:** in a context or a commons, never an adapter
  (`bynk.asset.outside_context_or_commons`). A unit sees its own assets and
  those of each commons it `uses`, like functions.

## Fixed paths

Some files are fetched by name by clients that never read a page: a browser
asks for `/favicon.ico`, a crawler for `/robots.txt`. Serve those at a fixed
path with `at`:

```bynk,fragment
context compat.board

asset favicon = "assets/favicon.ico" at "/favicon.ico"
asset robots = "assets/robots.txt" at "/robots.txt"
```

Its `.url` is then that path. A fixed path names one file: it starts with `/`,
does not end with `/`, is not under `/_assets/` or `/_bynk/`, and is not a file
the platform reads as configuration (`/_headers`, `/_redirects`,
`/.assetsignore`) (`bynk.asset.invalid_at_path`). One Worker serves one file at
a path, and one path cannot be a folder of another (`bynk.asset.path_conflict`).

The platform answers a fixed path before the Worker runs, for every method, so
a route at exactly that path is unreachable (`bynk.asset.shadows_route`), and a
route with a parameter that matches it (`/:page`) no longer sees it (a warning,
`bynk.asset.may_shadow_route`). A fixed path is not content-hashed, so it gets
the platform's default caching rather than the year-long one.

To read a file's text in code instead of serving it, see
[Embedded text](/book/reference/embeds/).

## Public by construction

An asset is served without running the Worker, so it is public whatever the
context's actors say. Put nothing in `assets/` that should not be published;
that is why assets come only from that folder.

## The URL

`.url` is `/_assets/<stem>.<hash>.<ext>`: the file's name with a hash of its
content inserted before the extension, for example
`/_assets/board.3bf3a8d5236ad2db.css`. Changing the file changes the URL, so
the platform caches every asset for a year (`Cache-Control: public,
max-age=31536000, immutable`) and a page always links the current version.

## What ships where

On the Workers target, each context's Worker serves the content-hashed files
of every unit it reaches through `uses` or `consumes`, directly or indirectly,
so a URL handed over by a used commons or a consumed context's service still
resolves on this Worker. Fixed-path files are served only by the Worker whose
own `uses` reach them. No handler may use the `/_assets/` prefix
(`bynk.http.reserved_prefix`). Its `wrangler.toml` gains an `[assets]` block:

```toml
[assets]
directory = "./assets"
run_worker_first = false
not_found_handling = "none"
html_handling = "none"
```

A request for an asset is answered by the platform; any other request reaches
the Worker as before. A Bundle build (`bynkc compile` without
`--target workers`) ships no files, so it warns that each asset is not served
(`bynk.asset.not_served`). `bynk dev` rebuilds when a file in `assets/` changes,
and `bynk deploy` records each Worker's served files in `bynk.deploy.lock`.

A suite can name its target's assets and fetch one with `api.GET(style)`,
which answers with the file as the platform would; see
[Testing](/book/reference/testing/#assets).

An `Asset` is not data: it may not appear in a record field, a boundary or an
agent `store` field (`bynk.types.asset_at_boundary`). Store or send its `.url`
instead.

A single file (`bynkc compile page.bynk`) has no project and no Worker, so
building one that declares an asset is `bynk.asset.needs_project`. Checking it
is unaffected.
