//! v0.28 (ADR 0057): semantic tokens over the project binding index.
//!
//! The one `index_queries` function that builds `lsp_types` values, so it stays
//! with the transport; the rest of the queries live in
//! [`bynk_ide::index_queries`] and are re-exported by `crate::index_queries`.

use std::path::Path;

use bynk_check::index::{ProjectIndex, SymbolKind};
use bynk_syntax::span::Span;

// -- v0.28 (ADR 0057): semantic tokens --

/// The frozen semantic-tokens legend. **Array order is the wire encoding**
/// (clients index into these arrays): entries are append-only, never
/// reordered — pinned by the legend-stability test. Token types: standard
/// where faithful (`type`, `function`), custom for the Bynk-distinctive
/// kinds (`capability`, `service`, `agent`, `provider`). The names live in
/// `bynk_syntax::semantic_tokens`, shared with the editor contract.
pub fn semantic_tokens_legend() -> tower_lsp::lsp_types::SemanticTokensLegend {
    use bynk_syntax::semantic_tokens::{TOKEN_MODIFIERS, TOKEN_TYPES};
    use tower_lsp::lsp_types::{SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};
    SemanticTokensLegend {
        token_types: TOKEN_TYPES
            .iter()
            .map(|n| SemanticTokenType::new(n))
            .collect(),
        token_modifiers: TOKEN_MODIFIERS
            .iter()
            .map(|n| SemanticTokenModifier::new(n))
            .collect(),
    }
}

/// Legend indices/bits — must mirror [`semantic_tokens_legend`]'s order.
fn token_type_index(kind: SymbolKind) -> u32 {
    match kind {
        SymbolKind::Type => 0,
        SymbolKind::Fn => 1,
        SymbolKind::Capability => 2,
        SymbolKind::Service => 3,
        SymbolKind::Agent => 4,
        SymbolKind::Provider => 5,
        // 6 is `variable` (locals; TOK_LOCAL); methods append at 7.
        SymbolKind::Method => 7,
        // v0.36 slice 2: ops reuse `method` (7); fields append `property` at 8.
        SymbolKind::CapabilityOp => 7,
        SymbolKind::Field => 8,
        // v0.45: actors append `actor` at 9.
        SymbolKind::Actor => 9,
        // #304: agent handlers reuse `method` (7), same as capability ops.
        SymbolKind::Handler => 7,
        // message-bundles slice 1: `messages` bundles append `messages` at 11.
        SymbolKind::Messages => 11,
        // #1883: asset names append `asset` at 13.
        SymbolKind::Asset => 13,
        // #2387: embed names append `embed` at 14.
        SymbolKind::Embed => 14,
        // #2617: constant names append `constant` at 16.
        SymbolKind::Const => 16,
    }
}

/// Legend index of the `variable` token type (locals; ADR 0064).
const TOK_LOCAL: u32 = 6;

/// Legend index of the `decorator` token type (handler annotations; ADR 0163).
pub const TOK_DECORATOR: u32 = 10;

/// Legend index of the `interface` token type (constraint names in a bound).
pub const TOK_INTERFACE: u32 = 15;

/// Legend index of the `keyword` token type (#2618: `private` in modifier
/// position, a contextual keyword the editor grammars may not know).
pub const TOK_KEYWORD: u32 = 17;

const MOD_DECLARATION: u32 = 1 << 0;
const MOD_REFINED: u32 = 1 << 1;
const MOD_OPAQUE: u32 = 1 << 2;
const MOD_PLATFORM_NATIVE: u32 = 1 << 3;

fn modifier_bits(m: bynk_check::index::SymbolModifiers) -> u32 {
    (if m.refined { MOD_REFINED } else { 0 })
        | (if m.opaque { MOD_OPAQUE } else { 0 })
        | (if m.platform_native {
            MOD_PLATFORM_NATIVE
        } else {
            0
        })
}

/// Semantic tokens for `path`, delta-encoded over the frozen legend —
/// a pure read of the cached index's two sources: `symbols` (user
/// defs+refs; a def site carries `declaration`) and `foreign_refs`
/// (first-party references). `range` (byte offsets into `text`, the
/// analysed snapshot) filters to overlapping tokens for the `…/range`
/// request; `None` is the full document.
pub fn semantic_tokens(
    index: &ProjectIndex,
    local_tokens: &[(Span, bool)],
    syntax_tokens: &[(Span, u32)],
    path: &Path,
    text: &str,
    range: Option<Span>,
) -> Vec<tower_lsp::lsp_types::SemanticToken> {
    let in_scope = |span: Span| {
        span.end <= text.len() && range.is_none_or(|r| span.end > r.start && span.start < r.end)
    };
    let mut raw: Vec<(Span, u32, u32)> = Vec::new();
    for (key, entry) in &index.symbols {
        let ty = token_type_index(key.kind);
        let mods = modifier_bits(entry.modifiers);
        if let Some(def) = &entry.def
            && def.path == path
            && in_scope(def.span)
        {
            raw.push((def.span, ty, mods | MOD_DECLARATION));
        }
        for site in &entry.refs {
            if site.path == path && in_scope(site.span) {
                raw.push((site.span, ty, mods));
            }
        }
    }
    for fr in &index.foreign_refs {
        if fr.site.path == path && in_scope(fr.site.span) {
            raw.push((
                fr.site.span,
                token_type_index(fr.kind),
                modifier_bits(fr.modifiers),
            ));
        }
    }
    // v0.31 (ADR 0064): local bindings + their uses (precomputed by the caller
    // via `locals_nav`, so this stays free of that dependency for the
    // `#[path]`-include tests). Disjoint from the index tokens — locals are
    // never top-level symbols — so they merge into the same sorted stream.
    for &(span, is_decl) in local_tokens {
        if in_scope(span) {
            raw.push((span, TOK_LOCAL, if is_decl { MOD_DECLARATION } else { 0 }));
        }
    }
    // v0.140 (ADR 0163): handler-annotation name + argument-label spans (precomputed
    // by the caller from the parsed unit, keeping this a parse-free index read).
    // Disjoint from the index/local tokens — annotations are neither symbols nor
    // locals — so they merge into the same sorted stream.
    // Constrained generics slice 0: constraint names ride the same list, as
    // `interface` (the caller classifies each span). #2618: so does each
    // `private` modifier, as `keyword`.
    for &(span, ty) in syntax_tokens {
        if in_scope(span) {
            raw.push((span, ty, 0));
        }
    }
    // Name segments never overlap (the index invariant), so a position
    // sort fully determines the protocol's relative encoding.
    raw.sort_by_key(|(span, _, _)| (span.start, span.end));
    let mut data = Vec::with_capacity(raw.len());
    // One line index for the whole snapshot: a `semanticTokens/full` request
    // emits a position per token, so scanning from byte 0 each time is O(n²)
    // (#732). Build it once and binary-search per token instead.
    let positions = crate::position::PositionMap::new(text);
    let (mut prev_line, mut prev_start) = (0u32, 0u32);
    for (span, token_type, token_modifiers_bitset) in raw {
        let pos = positions.position(span.start);
        let delta_line = pos.line - prev_line;
        let delta_start = if delta_line == 0 {
            pos.character - prev_start
        } else {
            pos.character
        };
        data.push(tower_lsp::lsp_types::SemanticToken {
            delta_line,
            delta_start,
            // The protocol counts in the negotiated encoding (UTF-16, as
            // positions are) — not bytes.
            length: text[span.range()].encode_utf16().count() as u32,
            token_type,
            token_modifiers_bitset,
        });
        prev_line = pos.line;
        prev_start = pos.character;
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_check::index::{SiteRef, SymbolEntry, SymbolKey};
    use std::path::PathBuf;

    fn site(path: &str, start: usize, end: usize) -> SiteRef {
        SiteRef {
            path: PathBuf::from(path),
            span: Span::new(start, end),
        }
    }

    fn key(unit: &str, kind: SymbolKind, name: &str) -> SymbolKey {
        SymbolKey {
            unit: unit.into(),
            kind,
            name: name.into(),
        }
    }

    fn index_with(entries: Vec<(SymbolKey, SiteRef, Vec<SiteRef>)>) -> ProjectIndex {
        let mut index = ProjectIndex::default();
        for (k, def, refs) in entries {
            index.symbols.insert(
                k,
                SymbolEntry {
                    def: Some(def),
                    refs,
                    ..Default::default()
                },
            );
        }
        index
    }

    // -- v0.28 (ADR 0057): semantic tokens --

    /// The legend's array order IS the wire encoding: this test freezes it.
    /// New entries APPEND — a failure here means a silent recolour of every
    /// client; never fix it by reordering.
    #[test]
    fn legend_is_frozen() {
        let legend = semantic_tokens_legend();
        let types: Vec<&str> = legend.token_types.iter().map(|t| t.as_str()).collect();
        assert_eq!(
            types,
            [
                "type",
                "function",
                "capability",
                "service",
                "agent",
                "provider",
                "variable",  // v0.31 (ADR 0064): locals — appended, never reordered
                "method",    // v0.36 (ADR 0069): instance methods — appended
                "property",  // v0.36 (ADR 0069, slice 2): record fields — appended
                "actor",     // v0.45: actor declarations — appended
                "decorator", // v0.140 (ADR 0163): handler annotations — appended
                "messages",  // message-bundles slice 1 (#859): messages bundles — appended
                "comment",   // #1888: `--|` doc lines — appended
                "asset",     // #1883: static asset names — appended
                "embed",     // #2387: embedded-text names — appended
                "interface", // constrained generics slice 0: constraint names — appended
                "constant",  // #2617: named constants — appended
                "keyword",   // #2618: contextual `private` modifier — appended
            ]
        );
        let modifiers: Vec<&str> = legend.token_modifiers.iter().map(|m| m.as_str()).collect();
        assert_eq!(
            modifiers,
            [
                "declaration",
                "refined",
                "opaque",
                "platformNative",
                "documentation"
            ]
        );
    }

    /// #2203: a one-line doc gets no semantic token, as a `---` block gets
    /// none, so the editor grammar's Markdown highlighting shows through; the
    /// legend keeps its `comment` type and `documentation` modifier.
    #[test]
    fn doc_lines_get_no_semantic_token() {
        let text = "commons m\n\n  --- A doc.\n// a comment\ntype T = Int\n";
        let tokens = semantic_tokens(
            &ProjectIndex::default(),
            &[],
            &[],
            Path::new("a.bynk"),
            text,
            None,
        );
        assert!(tokens.is_empty(), "{tokens:?}");
        let legend = semantic_tokens_legend();
        assert_eq!(legend.token_types[12].as_str(), "comment");
        assert_eq!(legend.token_modifiers[4].as_str(), "documentation");
    }

    #[test]
    fn tokens_are_delta_encoded_with_modifier_bitsets() {
        // text:  line 0: "type Age = Int"   (def `Age` at 5..8, refined)
        //        line 1: "fn f(a: Age) ..." (ref `Age` at 23..26)
        let text = "type Age = Int\nfn f(a: Age) -> Age {}\n";
        let mut index = index_with(vec![(
            key("shop", SymbolKind::Type, "Age"),
            site("a.bynk", 5, 8),
            vec![site("a.bynk", 23, 26), site("a.bynk", 31, 34)],
        )]);
        index
            .symbols
            .get_mut(&key("shop", SymbolKind::Type, "Age"))
            .unwrap()
            .modifiers = bynk_check::index::SymbolModifiers {
            refined: true,
            ..Default::default()
        };
        let tokens = semantic_tokens(&index, &[], &[], Path::new("a.bynk"), text, None);
        assert_eq!(tokens.len(), 3);
        // Def: line 0 char 5, length 3, type `type` (0), declaration|refined.
        assert_eq!(
            (
                tokens[0].delta_line,
                tokens[0].delta_start,
                tokens[0].length
            ),
            (0, 5, 3)
        );
        assert_eq!(tokens[0].token_type, 0);
        assert_eq!(tokens[0].token_modifiers_bitset, 0b0011);
        // First ref: next line, char 8 (absolute — line changed), refined only.
        assert_eq!(
            (
                tokens[1].delta_line,
                tokens[1].delta_start,
                tokens[1].length
            ),
            (1, 8, 3)
        );
        assert_eq!(tokens[1].token_modifiers_bitset, 0b0010);
        // Second ref: same line, char delta from the previous token's start.
        assert_eq!(
            (
                tokens[2].delta_line,
                tokens[2].delta_start,
                tokens[2].length
            ),
            (0, 8, 3)
        );
    }

    #[test]
    fn foreign_refs_emit_tokens_and_range_filters() {
        let text = "given Kv {\n  Kv.get(k)\n}\n";
        let mut index = ProjectIndex::default();
        index.foreign_refs.push(bynk_check::index::ForeignRef {
            site: site("a.bynk", 6, 8),
            kind: SymbolKind::Capability,
            modifiers: bynk_check::index::SymbolModifiers {
                platform_native: true,
                ..Default::default()
            },
        });
        index.foreign_refs.push(bynk_check::index::ForeignRef {
            site: site("a.bynk", 13, 15),
            kind: SymbolKind::Capability,
            modifiers: bynk_check::index::SymbolModifiers {
                platform_native: true,
                ..Default::default()
            },
        });
        let all = semantic_tokens(&index, &[], &[], Path::new("a.bynk"), text, None);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].token_type, 2); // capability
        assert_eq!(all[0].token_modifiers_bitset, 0b1000); // platformNative
        // Range covering only line 0 keeps only the first token.
        let ranged = semantic_tokens(
            &index,
            &[],
            &[],
            Path::new("a.bynk"),
            text,
            Some(Span::new(0, 10)),
        );
        assert_eq!(ranged.len(), 1);
        // Other files and empty indexes yield nothing.
        assert!(semantic_tokens(&index, &[], &[], Path::new("b.bynk"), text, None).is_empty());
        assert!(
            semantic_tokens(
                &ProjectIndex::default(),
                &[],
                &[],
                Path::new("a.bynk"),
                text,
                None
            )
            .is_empty()
        );
    }
}
