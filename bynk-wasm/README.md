# bynk-wasm

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **[Bynk](https://github.com/bynk-lang/bynk) compiler compiled to a wasm
module**, backing the in-browser [playground](https://github.com/bynk-lang/playground)
(served at [playground.bynk-lang.dev](https://playground.bynk-lang.dev)) and the
VS Code web extension.

> **Not a published crate.** `bynk-wasm` is `publish = false` — an internal build
> artefact, not on crates.io or docs.rs. It is built by `scripts/build-wasm.sh`,
> never via `cargo install`, and each release ships it in `bynk-web-<tag>.tar.gz`
> (`scripts/package-web-assets.sh`), which the playground builds from.

## What it does

A single entry point — `bynk_compile` (wasm) / `compile` (native) — takes
in-memory Bynk source and returns a runnable **JavaScript module graph** plus
diagnostics, with **no filesystem and no `tsc`**:

```text
source ─▶ bynk_emit::compile_in_memory  ─▶ ProjectOutput (TypeScript)
       ─▶ bynk_driver::strip::strip_project_to_js ─▶ ProjectOutput (JavaScript)
       ─▶ { files: [{ path, kind, contents, map? }], diagnostics }
```

The returned graph is the complete set the browser links: the user module,
the runtime modules it reaches (`__bynk_runtime/<module>.js`, ADR 0615 — only the
modules the program imports, not the whole runtime), the `bynk-browser.js` binding,
and `compose.js`. Each file carries a `kind`: `"user"` (modules authored from the
source, e.g. `hello/world.js`), `"wiring"` (`compose.js`) or `"support"`
(`__bynk_runtime/*.js`, `bynk-browser.js`, injected first-party units such as `bynk.js`),
so a consumer can show only the user's code while still linking the whole graph
(TS: `kind: "user" | "wiring" | "support"` on `EmittedFile`). A `"user"` file also
carries `map` (`map?: string` on `EmittedFile`, omitted for the other kinds): a source-map
v3 JSON document from that module's JavaScript lines back to the Bynk source, composed from
the emitter's own declaration and handler anchors (ADR 0569), so a host can highlight the JS
a Bynk line compiled to, and back, from the result alone. `sources` names the module by its
logical path (`app/demo.bynk`) and there is no `sourcesContent`; a few hundred bytes per module.
Three companion
entries back the playground's editor: `bynk_analyze` returns diagnostics only,
for live on-type checking; `bynk_hover` returns the inferred type at an offset;
`bynk_complete` returns completion candidates at an offset; and `bynk_format`
returns the source in house style (`{ok, formatted}` / `{ok:false, error}`). Every entry
compiles for the `browser` platform.

### Entry handler signatures

`bynk_compile` also reports each `service`'s `on call(…)` handler as `entries`
(empty on failure), so a host can build a run's arguments without parsing the
source. It is derived from the checker's resolved types, and only the program's
own contexts are listed (never injected first-party units). A named type's fields are resolved in the scope of the unit that declares it.
Within one parameter each record/sum is expanded once (its first occurrence)
and nested at most 6 deep (a wide DAG would otherwise grow exponentially);
later occurrences and anything deeper are reported by name, so look a name up
at its first occurrence. The field is additive:
the rest of the result is unchanged.

```ts
interface CompileResult {
  ok: boolean;
  files: EmittedFile[];
  diagnostics: Diagnostic[];
  entries: Entry[]; // new
}
interface Entry {
  unit: string;       // the context, e.g. "app.demo"
  service: string;
  params: EntryParam[];
  returns: string;    // the declared return type, e.g. "Effect[String]"
}
interface EntryParam {
  name: string;
  type: string;           // as declared, e.g. "Slug", "List[Point]"
  base: string | null;    // what it is, refines or wraps ("String" for Slug); null for records, sums, collections
  refined: boolean;       // a named refined type: a `base` value its `T.of` accepts
  shape: Shape;
}
type Shape =
  | { kind: "base"; base: string }                     // Int, String, Bool, Float, Duration, Instant, Bytes, Decimal
  | { kind: "refined"; name: string; base: string }
  | { kind: "opaque"; name: string; base: string }
  | { kind: "record"; name: string; fields: { name: string; shape: Shape }[] }
  | { kind: "sum"; name: string; variants: { name: string; fields: { name: string; shape: Shape }[] }[] }
  | { kind: "list" | "set" | "option"; element: Shape }
  | { kind: "map"; key: Shape; value: Shape }
  | { kind: "result"; ok: Shape; err: Shape }
  | { kind: "unit" }
  | { kind: "named"; name: string }                    // a record/sum already expanded earlier in this parameter, or nested deeper than 6 levels: not expanded again
  | { kind: "recursive"; name: string }                // a type already being described above it
  | { kind: "other"; display: string };               // a generic record instance, a function, ...: no value to build
```

Each diagnostic (in `bynk_compile`, `bynk_analyze` and `bynk_analyze_project`)
carries, besides `category` and `message`, the registry's one-line `summary`
(`null` only for an unregistered code) and an `href`: the curated Book
page when the code has one (what the LSP offers as `codeDescription`), otherwise
the diagnostic index section for its category. `bynk_explain(code)` returns
`{ code, summary, blurb?, example?, href }` for any code — the same curated table
`bynk explain` prints — so a consumer keeps no copy of either table. TS:
`summary: string | null; href: string` on `Diagnostic`;
`bynk_explain(code: string): string` (JSON).

`bynk_reference()` returns the standard library as the compiler knows it, in one
call (JSON, built once per page load, ~45 KB), so a reference browser needs no
completion probes and keeps no copy of the tables. It reads the same sources as
completion and hover: the kernel method tables, the built-in statics and the
embedded first-party `.bynk` surface (whose `---` doc blocks become `doc`).
`experimental` marks items that need `[unstable] allow` (e.g. `bynk.crypto`);
`deprecated` carries the replacement (the `bynk.list` free functions with a
`List` method form). Kernel methods and statics carry a signature but no `doc`.
TS:

```ts
interface Member {
  name: string;
  signature: string;
  doc?: string;          // Markdown, from the `---` block
  experimental?: true;   // absent when stable
  deprecated?: string;   // what to use instead
}
interface Reference {
  types: { name: string; statics: Member[]; methods: Member[] }[];
  capabilities: { name: string; unit: string; doc?: string; experimental: boolean; operations: Member[] }[];
  modules: { name: string; doc?: string; experimental: boolean; functions: Member[] }[];
}
// bynk_reference(): string  (JSON of Reference)
```

`Effect[Result[T, E]]` is its own `types` entry (it has extra combinators over
plain `Effect`).

The pipeline reuses the on-disk compile path wholesale (first-party injection,
the per-platform binding, the strip-only emitter), so the in-browser result
matches the CLI's.

## Build

The crate builds two ways:

- **`cdylib`** — the `wasm32` output in the release's browser artefacts.
- **`rlib`** — lets the native test build link the same logic, so the compile
  path is verified without a browser.

It is built by `scripts/build-wasm.sh` rather than installed on its own. Its
exports are the playground's interface: ci.yml's `web-assets` job checks the
tarball still exports what the playground calls.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
