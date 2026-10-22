//! Strip-only TypeScript → JavaScript for Bynk's first-class JS artefact (the
//! in-browser track, slice 1 — ADR 0137).
//!
//! `bynkc` emits TypeScript. A JS artefact is *emit-then-strip*: the same emitter
//! output with type annotations erased and nothing else changed. Because the
//! emitter is **strip-only** (ADR 0136 — every emitted `.ts` is erasable by pure
//! type-stripping), the transform here is total and lossless for runtime
//! behaviour: it never has to lower a type-directed construct (parameter
//! property, `enum`, `namespace`), only delete type syntax.
//!
//! The engine is [`oxc`] — a pure-Rust TS parser, type-erasing transform, and
//! codegen — so neither `bynkc --emit js` nor the in-browser compile path has any
//! Node/`tsc` dependency, and the crate compiles to `wasm32` for the playground.
//!
//! The transform is configured for **pure type-stripping**, matching Node's
//! `stripTypeScriptTypes` (the slice-0 strip oracle): `only_remove_type_imports`
//! keeps every *value* import even when unused, eliding only `import type` and
//! `type` specifiers — TypeScript's import-elision-by-usage is deliberately off,
//! so stripping is a syntactic erase, not a semantics-aware rewrite.

use std::fmt;
use std::path::Path;

use oxc::allocator::Allocator;
use oxc::codegen::Codegen;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{TransformOptions, Transformer, TypeScriptOptions};

/// A failure to strip TypeScript to JavaScript. For input produced by the Bynk
/// emitter this should never occur — the emitter only emits valid, strip-only
/// TypeScript (ADR 0136) — so a `StripError` indicates an emitter or toolchain
/// bug rather than user error.
#[derive(Debug, Clone)]
pub struct StripError {
    /// The file being stripped (for diagnostics).
    pub filename: String,
    /// What went wrong (parse or transform diagnostics).
    pub message: String,
}

impl fmt::Display for StripError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to strip types from {}: {}",
            self.filename, self.message
        )
    }
}

impl std::error::Error for StripError {}

/// Strip TypeScript types from `source`, returning equivalent JavaScript.
///
/// `filename` selects the source flavour (`.ts`/`.tsx`/`.mts`) and labels
/// diagnostics; it does not have to exist on disk. Value imports are preserved
/// verbatim (see the module docs); only type syntax is erased.
pub fn strip_types(source: &str, filename: &str) -> Result<String, StripError> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(filename).unwrap_or_else(|_| SourceType::ts());

    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        return Err(StripError {
            filename: filename.to_string(),
            message: format!("parse error: {}", join_diagnostics(&parsed.diagnostics)),
        });
    }

    let mut program = parsed.program;
    // `with_enum_eval(true)`: the transformer *panics* on an `enum` without it.
    // Strip-only emitter output never contains one (ADR 0136), but evaluating
    // enums keeps this a total function — graceful transform, never a panic — if
    // a non-strip-only source is ever handed in.
    let scoping = SemanticBuilder::new()
        .with_enum_eval(true)
        .build(&program)
        .semantic
        .into_scoping();

    let options = TransformOptions {
        typescript: TypeScriptOptions {
            // Pure type-stripping: keep every value import (even unused),
            // erase only `import type` / `type` specifiers. Matches Node's
            // strip-only mode rather than TypeScript's usage-based elision.
            only_remove_type_imports: true,
            ..TypeScriptOptions::default()
        },
        ..TransformOptions::default()
    };

    let ret = Transformer::new(&allocator, Path::new(filename), &options)
        .build_with_scoping(scoping, &mut program);
    if !ret.diagnostics.is_empty() {
        return Err(StripError {
            filename: filename.to_string(),
            message: format!("transform error: {}", join_diagnostics(&ret.diagnostics)),
        });
    }

    Ok(Codegen::new().build(&program).code)
}

/// Rewrite a compiled [`ProjectOutput`](bynk_emit::project::ProjectOutput) from
/// TypeScript into a JavaScript artefact (the in-browser track's first-class JS
/// output — ADR 0137). The emitter always produces TypeScript; a JS artefact is
/// that same output with types stripped, which is total because the emitter is
/// strip-only (ADR 0136).
///
/// Dispatches on [`Document`](bynk_emit::project::Document) directly (P7.6,
/// #1309) — no `CompiledFile` text intermediate. Every `Ts` document is
/// printed through the one printer that owns a character (R7.3), then
/// type-stripped and renamed to `.js` as its own `Document::Js`; `tsconfig.json`
/// is dropped (a TypeScript-compiler config with no role for a JS artefact);
/// `wrangler.toml`'s real tree gets its `main` entry patched *structurally*,
/// in place — no print-then-reparse (Decision E) — since the Workers
/// manifest names its entry module and a `main` left pointing at the
/// stripped `.ts` breaks `wrangler dev`/deploy on the emitted output. Source
/// maps and the debug sidecar are dropped — they map into the `.ts` the JS
/// replaces. Import specifiers are already `.js` (the default `ImportExt`),
/// so the renamed tree resolves as-is.
pub fn strip_project_to_js(
    out: bynk_emit::project::ProjectOutput,
) -> Result<bynk_emit::project::ProjectOutput, StripError> {
    use bynk_emit::project::{Artefacts, Document};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    let mut docs: BTreeMap<PathBuf, Document> = BTreeMap::new();
    for (path, doc) in out.artefacts.docs {
        match doc {
            Document::Ts(program) => {
                let printed = bynk_ts::print(&program, "", "", &path.to_string_lossy());
                let js = strip_types(&printed.text, &path.to_string_lossy())?;
                docs.insert(path.with_extension("js"), Document::Js(js));
            }
            Document::Toml(mut toml) => {
                if path.file_name().and_then(|n| n.to_str()) == Some("wrangler.toml")
                    && !toml.set_main("index.js")
                {
                    return Err(StripError {
                        filename: path.to_string_lossy().into_owned(),
                        message: "wrangler.toml has no root `main` key".to_string(),
                    });
                }
                docs.insert(path, Document::Toml(toml));
            }
            Document::Json(s) => {
                if path.file_name().and_then(|n| n.to_str()) == Some("tsconfig.json") {
                    continue;
                }
                docs.insert(path, Document::Json(s));
            }
            // Source maps/debug sidecars map into the `.ts` the JS replaces.
            Document::SourceMap(_) | Document::DebugSidecar(_) => {}
            // `bynk-emit` never produces this itself, but `strip_project_to_js`
            // is `pub` over a `pub` `Artefacts`, so an already-stripped
            // `Document::Js` reaching this function (e.g. run twice, or a
            // caller assembling `Artefacts` by hand) is a real, reachable
            // input — pass it through rather than panicking on it (review,
            // #1309/#1310: `unreachable!` here aborted the process on input
            // the public API permits).
            Document::Js(s) => {
                docs.insert(path, Document::Js(s));
            }
        }
    }
    // #1883, slice 0: static assets are served as-is by the platform; type
    // stripping has nothing to do to them.
    Ok(bynk_emit::project::ProjectOutput {
        artefacts: Artefacts {
            docs,
            assets: out.artefacts.assets,
            modules: out.artefacts.modules,
        },
        ..out
    })
}

fn join_diagnostics(diags: &[oxc::diagnostics::OxcDiagnostic]) -> String {
    diags
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert the stripped JS contains `needle` and none of the `absent` strings.
    fn strip(src: &str) -> String {
        strip_types(src, "test.ts").expect("strip should succeed on valid strip-only TS")
    }

    #[test]
    fn erases_annotations_and_keeps_values() {
        let js = strip("export const add = (a: number, b: number): number => a + b;\n");
        assert!(js.contains("export const add"));
        assert!(!js.contains(": number"), "annotations erased:\n{js}");
    }

    #[test]
    fn removes_type_aliases_and_interfaces() {
        let js = strip(
            "export type Id = string & { readonly __brand: \"x\" };\n\
             export interface Logger { info(m: string): Promise<void>; }\n\
             export const v = 1;\n",
        );
        assert!(!js.contains("interface"), "interface erased:\n{js}");
        assert!(!js.contains("type Id"), "type alias erased:\n{js}");
        assert!(js.contains("export const v = 1"));
    }

    #[test]
    fn preserves_value_imports_drops_type_specifiers() {
        // `Ok`/`Err` are value imports and must survive even though they are
        // unused here; `type Result`/`import type` must go.
        let js = strip(
            "import { Ok, Err, type Result } from \"./runtime.js\";\n\
             import type { Foo } from \"./foo.js\";\n\
             export const x = 1;\n",
        );
        assert!(js.contains("Ok"), "value import Ok kept:\n{js}");
        assert!(js.contains("Err"), "value import Err kept:\n{js}");
        assert!(!js.contains("Result"), "type specifier dropped:\n{js}");
        assert!(!js.contains("Foo"), "import type dropped:\n{js}");
        assert!(
            !js.contains("./foo.js"),
            "type-only import line dropped:\n{js}"
        );
    }

    #[test]
    fn de_sugared_provider_constructor_strips() {
        // The shape the slice-0 emitter produces for a `given` provider.
        let js = strip(
            "export class P {\n\
             \x20 private deps: { Log: unknown };\n\
             \x20 constructor(deps: { Log: unknown }) { this.deps = deps; }\n\
             }\n",
        );
        assert!(js.contains("class P"));
        assert!(
            js.contains("constructor(deps)"),
            "ctor param keeps name:\n{js}"
        );
        assert!(
            js.contains("this.deps = deps"),
            "assignment preserved:\n{js}"
        );
        assert!(!js.contains(": { Log"), "field/param types erased:\n{js}");
    }

    #[test]
    fn as_casts_and_unique_symbol_erased() {
        let js = strip(
            "export const Tok: unique symbol = Symbol(\"T\");\n\
             export const id = (v: string) => v as string;\n",
        );
        assert!(!js.contains("unique symbol"), "unique symbol erased:\n{js}");
        assert!(!js.contains(" as string"), "as-cast erased:\n{js}");
        assert!(js.contains("Symbol(\"T\")"));
    }

    #[test]
    fn invalid_source_is_an_error_not_a_panic() {
        let err = strip_types("const = = =;", "bad.ts");
        assert!(err.is_err(), "malformed source is an error");
    }

    /// P7.4 (#1305)/P7.6 (#1309): `strip_project_to_js`'s `wrangler.toml`
    /// patch, exercised through the real function against a *real* compiled
    /// `Document::Toml` tree (`compile_in_memory`, `BuildTarget::Workers`) —
    /// not a hand-typed text fixture standing in for one, which is exactly
    /// the shortcut P7.4's own defect report warns against (a hand-built
    ///2-key snippet can't prove a whole-document reformat didn't happen).
    /// P7.6's own Decision E moved this from a text re-parse
    /// (`toml_edit`) to a structural, in-tree mutation
    /// (`TomlDocument::set_main`) — asserting the *printed* text still
    /// pins "nothing but `main` changed", the same discipline as before.
    #[test]
    fn strip_project_to_js_patches_wrangler_toml_main_structurally() {
        use bynk_emit::project::{BuildTarget, Document, compile_in_memory};

        let out = compile_in_memory(
            "context api {\n  service routes from http {\n    on GET(\"/\") () -> Effect[HttpResult[String]] by v: Visitor {\n      Ok(\"ok\")\n    }\n  }\n}\n",
            BuildTarget::Workers,
            Default::default(),
        )
        .unwrap_or_else(|f| {
            panic!(
                "workers-target fixture should compile: {:?}",
                f.flatten()
            )
        });
        let wrangler_path = out
            .artefacts
            .docs
            .keys()
            .find(|p| p.file_name().and_then(|n| n.to_str()) == Some("wrangler.toml"))
            .cloned()
            .expect("a Workers-target compile emits a wrangler.toml");
        let original_text = out.artefacts.docs[&wrangler_path].text();
        assert!(
            original_text.contains("main = \"index.ts\""),
            "expected the pre-strip manifest to point at the .ts entry:\n{original_text}"
        );

        let stripped = strip_project_to_js(out).expect("wrangler.toml patches cleanly");
        let patched = stripped
            .artefacts
            .docs
            .get(&wrangler_path)
            .unwrap_or_else(|| panic!("{} should survive stripping", wrangler_path.display()));
        assert!(
            matches!(patched, Document::Toml(_)),
            "wrangler.toml should stay a real tree through stripping, not become opaque text"
        );
        let expected = original_text.replacen("main = \"index.ts\"", "main = \"index.js\"", 1);
        assert_eq!(
            patched.text(),
            expected,
            "only `main` should differ from the pre-strip manifest"
        );
        // No leftover `.ts` index — `Document::Ts` entries are all renamed
        // to their own `.js` sibling.
        let index_ts = wrangler_path.with_file_name("index.ts");
        assert!(
            !stripped.artefacts.docs.contains_key(&index_ts),
            "the stripped output must not keep the pre-strip .ts path: {:?}",
            stripped.artefacts.docs.keys().collect::<Vec<_>>()
        );
    }

    /// #1883, slice 0: static assets are served as-is, so stripping a
    /// project's types must hand `Artefacts.assets` through byte-for-byte —
    /// `bynkc compile --emit js` strips before it writes, and an asset dropped
    /// here would be missing from that output. One is added to a real
    /// build's output by hand, so this tests stripping alone.
    #[test]
    fn strip_project_to_js_passes_static_assets_through_unchanged() {
        use bynk_emit::project::{BuildTarget, compile_in_memory, worker_assets_dir};

        let mut out = compile_in_memory(
            "commons only\n\nfn f() -> Int {\n  1\n}\n",
            BuildTarget::Workers,
            Default::default(),
        )
        .unwrap_or_else(|f| panic!("fixture should compile: {:?}", f.flatten()));
        let path = worker_assets_dir("only").join("_assets/icon.9b1d4e.png");
        let bytes = vec![0x89, b'P', b'N', b'G', 0xff, 0x00];
        out.artefacts.assets.insert(path.clone(), bytes.clone());

        let stripped = strip_project_to_js(out).expect("strips cleanly");
        assert_eq!(stripped.artefacts.assets.len(), 1);
        assert_eq!(stripped.artefacts.assets.get(&path), Some(&bytes));
    }
}
