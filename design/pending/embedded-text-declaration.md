---
level: minor
probes: emit_diagnostics, diagnostic_coverage
changelog: A context or commons declares a project file's text with `embed name = "path"`, a `String` constant that is never served; declarations are checked, and a build refuses them with `bynk.embed.not_yet_emitted` until they can be emitted (embedded text, slice 1)
---

## ADR: embedded-text-in-the-language
title: A project file's text is declared in the language with `embed`, apart from `asset`
summary: `embed name = "path"` in a context or commons; the name is a `String` holding a file from `embeds/`, never served

**Context.** Markup that is mostly static had to be written as string literals:
a literal cannot span lines, and an `asset` exposes only its `.url`. The page
shell of bynk-lang/compat-board ended up as one long hand-escaped literal
(#2387). #1883 [DECISION E] deferred reading a file into a `String` as a
separate feature with its own questions: size limits, whether it leads to
templating, and escape safety.

**Decision.**

- **A separate declaration, not a field on `Asset` (#2387 [DECISION A]).**
  Everything declared with `asset` is public by construction: it lives in
  `assets/`, is shipped as a file and served. A template is the case that most
  wants to stay private, so "served to clients" and "read by code" are two
  declarations. The two differ in security (public or private), folder, output
  (a file or inlined text) and type (`Asset` or `String`).
- **A fixed folder, `embeds/` (B).** Embedded text is compiled into the Worker
  and so lands in the bundle and its source map. A declaration can name only a
  file in `embeds/` (`bynk.embed.outside_embeds_dir`), so it can never pull
  `.env` or a source file into deployed code. Path rules otherwise match
  assets (`bynk.embed.unsafe_path`).
- **A plain `String` (D).** An embed's value has nothing to carry beyond its
  text, so it composes with every `String` method and needs no new built-in
  type name.
- **Verbatim, with a seam for templates (F).** `embed` promises nothing about
  the text; a later `template` declaration would parse holes and check their
  types and escapes. Different contracts are different declarations, so
  `embed` never starts to interpret `{{ }}`. `template` is not reserved now.
- **`embed` is a contextual keyword (H)**, in `RESERVED_CONTEXTUAL` beside
  `asset`, so it stays usable as an identifier.
- **One value namespace.** An embed shares the namespace of functions, assets,
  types and events (`bynk.resolve.name_conflict`), and a unit sees its own
  embeds and those of each commons it `uses`, one level, as with assets. A
  package dependency may not declare one yet, as for an asset.
- **Checked before it is built.** This slice checks declarations (placement,
  path, names, the name typed `String`); reading the `embeds/` folder, the
  content rules (UTF-8, line endings, size caps) and emission are the next
  slice. Until then every build refuses a declaration with
  `bynk.embed.not_yet_emitted`, rather than emitting a module whose uses of the
  name are undefined. Checking (`bynk check`, the editor) is unaffected.

**Consequences.** The `embeds/` folder is not read yet, so whether a declared
file exists is not yet checked. The editor does not yet index embeds (no hover
or go-to-definition on an embed name; completion and semantic tokens for the
keyword come with the tooling slice), and an embed is not yet nameable inside a
`suite` case body. #2387's slices 2 and 3 add emission with the content rules,
and the compat-board adoption with the reference page and guide.
