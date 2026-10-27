# 0633 — The Book a chat agent searches comes from the installed toolchain

- **Status:** Accepted (v0.422)

**Context.** `bynk_search_book` (#2463, DECISION B) must return Book text that
matches the `bynk` the user runs. The live site may describe features the
toolchain lacks and needs the network; bundling the Book in the `.vsix` ties it
to the extension's version instead. A file installed beside the binary does not
work: the extension downloads `bynk` as a single verified file
(`SHA256SUMS` covers the raw `bynk-<target>` assets), so a sibling file would
never arrive for a downloaded toolchain.

**Decision.** The driver embeds the Book and searches it. `bynk/build.rs`
includes `site/public/llms-full.txt` (the whole Book in sidebar order, generated
by `site/scripts/build-llms-full.mjs`) and the new `bynk book` subcommand prints
it under a first line naming the toolchain version; `bynk book --search <query>
[--format json]` returns the best-matching sections, each with its page link and
the Book version. The ranking lives in `bynk::book::search`, so `bynk mcp`
reuses it rather than ranking a second time. The release workflow generates the
bundle before building; a build without it embeds nothing, and `bynk book` then
says "no Book bundled in this build" and exits non-zero, so a source build never
silently returns nothing. `cargo install --path bynk` gets the bundle only when
`site/public/llms-full.txt` was generated first; one from a crate tarball (no
`site/`) never does.

The extension runs `bynk book --search`. When that is unavailable (no bundle, or
an older `bynk`) it fetches the hosted `https://bynk-lang.org/llms-full.txt` and
ranks it with a small TypeScript copy of the same scoring in `lmRender.ts`, and
labels the result with the Book's version, the installed toolchain's, and a
mismatch warning. **Two rankings therefore exist** (Rust for the toolchain, TS
for the fallback); their tests pin the same behaviours, and the fallback
is the one to retire if the hosted route ever goes. The tool is desktop-only: it
needs the CLI, and a browser's fetch of the hosted file depends on the site's
CORS policy, which is not established.

**Consequences.** Release binaries grow by the Book's size (the bundle is about
2 MB), and Book answers are exact for the version shipped. The generated bundle
stays out of version control (it is gitignored), so Book edits cause no churn in
`bynk/`. The vscode.dev leg of DECISION D (Book search on the web) is deferred
until the site's CORS policy is settled or the wasm module carries the Book.
