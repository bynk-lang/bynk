# 0609 — An embedded file is UTF-8 text with LF endings, size-capped, and emitted as a plain constant

- **Status:** Accepted (v0.402)

**Context.** Slice 1 of #2387 declared `embed` and checked it but refused to
build it. Building one needs answers the checker never had to give: how the
folder reaches the disk-free compiler, what counts as text, what keeps a build
reproducible, how big an embedded file may be, and how the value reaches the
code and a test.

**Decision.**

- **The folder is read the way `assets/` is (#2387 [DECISION B]).** The driver
  and the test harnesses list `embeds/` into `CompileOptions::embed_files`, an
  `AssetFolder` over `embeds/` with a lazy reader (`EmbedFolder`), so
  `bynk-emit` stays disk-free and the build reads only the files a declaration
  names. The same refusals apply: regular files only, symbolic links not
  followed, a read that resolves outside the folder refused.
- **Text means UTF-8 with no NUL (C).** Anything else is `bynk.embed.invalid_encoding`
  or `bynk.embed.binary_content`; binary content belongs in `assets/`. Invalid
  UTF-8 is judged before NUL.
- **The build is reproducible across checkouts (C).** A leading byte-order mark
  is dropped, and `\r\n` and a bare `\r` become `\n`, so `autocrlf` cannot
  change the constant. Everything else, including a trailing newline, is kept
  verbatim. Nothing in the text is interpreted (F).
- **Size caps (C).** 256 KiB per file and 1 MiB across the files a project's
  declarations name, each file counted once (`bynk.embed.too_large`). The text
  lands in the Worker bundle and counts against the platform's script size
  limits. Raising a cap later is compatible; lowering one is not.
- **A plain exported constant (D).** `export const shell: string = "…";` in
  the declaring module, imported by name like a function wherever a unit
  `uses` it. It is not emitted conditionally: a bundler's tree-shaking drops an
  unreferenced one, which keeps the emitter free of a reachability analysis.
- **A single file cannot build one.** It has no project root to find `embeds/`
  under (`bynk.embed.needs_project`, as for an asset); checking one is
  unaffected. The slice 1 build gate `bynk.embed.not_yet_emitted` is gone.
- **A unit-tier case names its target's embeds.** The privileged view carries
  the target's own embeds and its `uses`d commons', as it does functions, so a
  case can assert on the text. A `system` suite does not: an embed is code, not
  something a Worker serves.
- **`bynk dev` watches `embeds/`**, so editing a template rebuilds the code
  that embeds it.

**Consequences.** Hover, completion and semantic tokens for `embed`, and the
reference page and guide, follow in #2387's next slices. Whether embedded text
should feed the contract hash is not decided here and is unchanged: nothing in
this slice touches it.
