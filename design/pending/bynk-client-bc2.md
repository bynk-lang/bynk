---
level: minor
changelog: `bynk client` gains `--check` (a CI drift gate), `--layout dir` (four modules) and `[build.client]` in `bynk.toml`, which `bynk build` writes and `bynk client` takes its flags from (#3508 slice BC2)
probes: ts_writes
---

## ADR: bynk-client-drift-layout-and-manifest
title: `bynk client`: the drift check, the directory layout and `[build.client]`
summary: `--check` compares a written client with the generated one, `--layout dir` splits it in four, and `[build.client]` lets `bynk build` write it and `bynk client` default its flags

**Context.** BC1 of #3508 shipped `bynk client`, which writes a typed TypeScript client for a project's `from http` services. A committed generated file goes stale when the API changes, and a team wants CI to say so, to split the module when one file is unwieldy, and to have `bynk build` keep it written. The proposal's Decision A fixed all three; this increment is slice BC2.

**Decision.**

- *`--check`.* Generates the client in memory, compares it with the file (or, for a directory layout, each of the four files) at `--out`, and exits non-zero with `client is stale: run `bynk client --out PATH`` and, per file, the first differing line and how many differ. A missing file is stale. It writes nothing. The header line naming the compiler's version, and line endings, are ignored, so a `bynk` upgrade or a Windows checkout alone is not drift. `--check` with no path to compare is a command error.
- *`--layout file|dir`.* `file` (default) is BC1's single module. `dir` writes `index.ts` (the client, and `export *` of the types and the runtime), `types.ts` (the public and wire types, and each route's response and query types), `codecs.ts` (the shared codecs and the `T | null` conversions) and `runtime.ts` (the inlined runtime and `WIRE_VERSION`). Each file imports exactly the names it uses from the others, types marked `type`, found by scanning the code with comments and string literals removed, so a build with `verbatimModuleSyntax`, `isolatedModules` and `noUnusedLocals` passes. Imports end in `.js` (`"./types.js"`), which TypeScript resolves to the `.ts` files under `bundler` and under `node16`/`nodenext` resolution alike, so one output serves Vite, SvelteKit, Next.js and strict Node ESM (the maintainer's call on #3508; the system tier type-checks the layout under both). `--layout dir` needs `--out DIR`.
- *`[build.client]`.* `out` (required; relative to the project root; unlike `[build] out` it may leave the project, because a front end often lives beside the Bynk project), `layout` (`file` or `dir`) and `service` (a list). The manifest schema describes the table. `bynk build` writes the client when the table is present and does nothing when it is not, so a project without it builds byte-identically; a file whose text is already right is not rewritten, so a watcher sees no change.
- *Flags and the manifest.* `bynk client` takes `--out`, `--layout` and `--service` from `[build.client]` when the flags are left out, and a flag overrides the table, so `bynk client --check` runs with no arguments in CI. The proposal names the table only for `bynk build`; reading it in the command too keeps one description of where the client goes.
- *Stable output.* The output was already byte-identical between runs; the compiler-version header is the only per-release difference and `--check` and the goldens ignore it.

**Consequences.** A front-end repository gains a drift gate that costs one command, a directory layout that is the stepping stone to a package, and an opt-in build step. No existing project changes: the manifest key is new, and the client's text for the single-file layout is unchanged except that the conversion functions are now exported (the directory layout imports them). `[build.client]` is the first nested table under `[build]`.
