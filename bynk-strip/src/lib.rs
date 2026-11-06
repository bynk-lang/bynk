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
use std::path::{Path, PathBuf};

use oxc::allocator::Allocator;
use oxc::codegen::{Codegen, CodegenOptions};
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
    strip_types_with_map(source, filename, false).map(|(js, _)| js)
}

/// [`strip_types`], optionally also returning the JS-to-input source map the
/// codegen records (generated JS position -> position in `source`).
fn strip_types_with_map(
    source: &str,
    filename: &str,
    want_map: bool,
) -> Result<(String, Option<oxc_sourcemap::SourceMap<'static>>), StripError> {
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

    if !want_map {
        return Ok((Codegen::new().build(&program).code, None));
    }
    let options = CodegenOptions {
        source_map_path: Some(PathBuf::from(filename)),
        ..CodegenOptions::default()
    };
    let ret = Codegen::new().with_options(options).build(&program);
    Ok((ret.code, ret.map.map(|m| m.into_owned())))
}

/// Compose a Bynk-to-TypeScript source map (the emitter's own, line-anchored
/// per ADR 0103) with the TypeScript-to-JavaScript map the strip codegen
/// records, giving a Bynk-to-JavaScript v3 map. No second mapping is
/// generated: each JS token takes the Bynk position the emitter already
/// assigned to its TypeScript line. `sourcesContent` is dropped (the consumer
/// holds the source). `None` when either map does not parse or nothing maps.
fn compose_maps(bynk_to_ts: &str, ts_to_js: &oxc_sourcemap::SourceMap<'_>) -> Option<String> {
    let first = oxc_sourcemap::SourceMap::from_json_string(bynk_to_ts).ok()?;
    // The emitter's map has one segment per TS line; the last wins per line.
    let mut by_line: std::collections::HashMap<u32, (u32, u32, u32)> = Default::default();
    for t in first.get_tokens() {
        by_line.insert(
            t.get_dst_line(),
            (
                t.get_source_id().unwrap_or(0),
                t.get_src_line(),
                t.get_src_col(),
            ),
        );
    }
    let mut builder = oxc_sourcemap::SourceMapBuilder::default();
    for name in first.get_sources() {
        builder.add_source_and_content(name, "");
    }
    let mut last: Option<(u32, u32, u32, u32)> = None;
    let mut any = false;
    for t in ts_to_js.get_tokens() {
        let Some(&(src, sl, sc)) = by_line.get(&t.get_src_line()) else {
            continue;
        };
        // Line-level maps: one token per JS line per Bynk position is enough.
        let key = (t.get_dst_line(), src, sl, sc);
        if last == Some(key) {
            continue;
        }
        last = Some(key);
        builder.add_token(t.get_dst_line(), t.get_dst_col(), sl, sc, Some(src), None);
        any = true;
    }
    if !any {
        return None;
    }
    let mut map = builder.into_sourcemap();
    // Without `sourcesContent`, as promised above.
    map.set_source_contents(Vec::new());
    Some(map.to_json_string())
}

/// [`strip_types`] for one emitted module, also composing the module's Bynk-to-JavaScript
/// source map: `bynk_map` is the emitter's Bynk-to-TypeScript v3 map for the same module
/// (`None` skips the map work entirely). Returns the JavaScript and, when `bynk_map` was
/// given and the two maps compose, the composed v3 map as JSON (#2705).
///
/// This is the text-level seam the project-level strip in `bynk-driver` calls per module;
/// it keeps `oxc`'s source-map types inside this crate.
pub fn strip_types_mapped(
    source: &str,
    filename: &str,
    bynk_map: Option<&str>,
) -> Result<(String, Option<String>), StripError> {
    let (js, ts_to_js) = strip_types_with_map(source, filename, bynk_map.is_some())?;
    let map = match (ts_to_js, bynk_map) {
        (Some(ts_to_js), Some(bynk)) => compose_maps(bynk, &ts_to_js),
        _ => None,
    };
    Ok((js, map))
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

    /// A v3 map whose one source is `a.bynk`, with the given
    /// `(generated line, source line)` segments (all at column 0).
    fn bynk_map(segs: &[(u32, u32)]) -> String {
        let mut b = oxc_sourcemap::SourceMapBuilder::default();
        b.add_source_and_content("a.bynk", "");
        for &(gl, sl) in segs {
            b.add_token(gl, 0, sl, 0, Some(0), None);
        }
        b.into_sourcemap().to_json_string()
    }

    /// A TS-to-JS map with one token per `(js line, ts line)`.
    fn ts_to_js(toks: &[(u32, u32)]) -> oxc_sourcemap::SourceMap<'static> {
        let mut b = oxc_sourcemap::SourceMapBuilder::default();
        b.add_source_and_content("a.ts", "");
        for &(jl, tl) in toks {
            b.add_token(jl, 0, tl, 0, Some(0), None);
        }
        b.into_sourcemap().into_owned()
    }

    fn composed_lines(json: &str) -> Vec<(u32, u32)> {
        let m = oxc_sourcemap::SourceMap::from_json_string(json).unwrap();
        m.get_tokens()
            .map(|t| (t.get_dst_line(), t.get_src_line()))
            .collect()
    }

    #[test]
    fn compose_drops_a_ts_line_with_no_emitter_segment() {
        // The emitter maps TS line 1 only; the JS token for TS line 0 is dropped.
        let out = compose_maps(&bynk_map(&[(1, 4)]), &ts_to_js(&[(0, 0), (1, 1)])).unwrap();
        assert_eq!(composed_lines(&out), vec![(1, 4)]);
    }

    #[test]
    fn compose_takes_the_last_segment_on_a_ts_line() {
        let out = compose_maps(&bynk_map(&[(0, 2), (0, 7)]), &ts_to_js(&[(0, 0)])).unwrap();
        assert_eq!(composed_lines(&out), vec![(0, 7)]);
    }

    #[test]
    fn compose_is_none_when_nothing_maps() {
        assert!(compose_maps(&bynk_map(&[(5, 1)]), &ts_to_js(&[(0, 0)])).is_none());
        assert!(compose_maps("not json", &ts_to_js(&[(0, 0)])).is_none());
    }

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
}
