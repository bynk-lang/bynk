//! Document-symbol tree for the LSP `textDocument/documentSymbol` request
//! (v1.1; LSP spec §3.7).
//!
//! Walks a single file's parsed AST and emits a hierarchical
//! [`DocumentSymbol`] tree that populates VS Code's Outline pane and
//! powers "Go to Symbol in File" (Cmd-Shift-O). Multi-file commons /
//! contexts each report only their own file's contents — joining across
//! files is `workspaceSymbol` territory, which is deferred.

use bynk_syntax::ast::*;
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use tower_lsp::lsp_types::{DocumentSymbol, Range, SymbolKind};

use crate::position::PositionMap;

/// Build the document-symbol tree for the given source text. Returns an
/// empty vector when the file cannot be parsed at all (no recognisable
/// header).
pub fn outline(source: &str) -> Vec<DocumentSymbol> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let Some(unit) = unit else {
        return Vec::new();
    };
    // One line index for the whole file: the walk emits two ranges per symbol,
    // so scanning from byte 0 each time is O(symbols × file size) (#732).
    let pm = PositionMap::new(source);
    match unit {
        SourceUnit::Commons(c) => vec![commons_symbol(&pm, &c)],
        SourceUnit::Context(c) => vec![context_symbol(&pm, &c)],
        SourceUnit::Suite(t) => vec![test_symbol(&pm, &t)],
        SourceUnit::Adapter(a) => vec![adapter_symbol(&pm, &a)],
    }
}

fn adapter_symbol(pm: &PositionMap, a: &AdapterDecl) -> DocumentSymbol {
    let children: Vec<DocumentSymbol> = a.items.iter().map(|item| item_symbol(pm, item)).collect();
    make_symbol(
        a.name.joined(),
        detail_from_doc(&a.documentation),
        SymbolKind::MODULE,
        pm.range(a.span),
        pm.range(a.name.span),
        children,
    )
}

fn test_symbol(pm: &PositionMap, t: &SuiteDecl) -> DocumentSymbol {
    let mut children: Vec<DocumentSymbol> = Vec::new();
    for p in &t.stubs {
        children.push(make_symbol(
            format!("stub {}.{}", p.capability.name, p.method.name),
            None,
            SymbolKind::INTERFACE,
            pm.range(p.span),
            pm.range(p.capability.span),
            Vec::new(),
        ));
    }
    for c in &t.cases {
        children.push(make_symbol(
            c.name.clone(),
            None,
            SymbolKind::FUNCTION,
            pm.range(c.span),
            pm.range(c.name_span),
            Vec::new(),
        ));
    }
    make_symbol(
        format!("test {}", t.target.joined()),
        detail_from_doc(&t.documentation),
        SymbolKind::MODULE,
        pm.range(t.span),
        pm.range(t.target.span),
        children,
    )
}

fn commons_symbol(pm: &PositionMap, c: &Commons) -> DocumentSymbol {
    let children: Vec<DocumentSymbol> = c.items.iter().map(|item| item_symbol(pm, item)).collect();
    make_symbol(
        c.name.joined(),
        detail_from_doc(&c.documentation),
        SymbolKind::MODULE,
        pm.range(c.span),
        pm.range(c.name.span),
        children,
    )
}

fn context_symbol(pm: &PositionMap, c: &Context) -> DocumentSymbol {
    let children: Vec<DocumentSymbol> = c.items.iter().map(|item| item_symbol(pm, item)).collect();
    make_symbol(
        c.name.joined(),
        detail_from_doc(&c.documentation),
        SymbolKind::MODULE,
        pm.range(c.span),
        pm.range(c.name.span),
        children,
    )
}

fn item_symbol(pm: &PositionMap, item: &CommonsItem) -> DocumentSymbol {
    match item {
        CommonsItem::Type(t) => type_symbol(pm, t),
        CommonsItem::Fn(f) => fn_symbol(pm, f),
        CommonsItem::Capability(c) => capability_symbol(pm, c),
        CommonsItem::Provider(p) => provider_symbol(pm, p),
        CommonsItem::Service(s) => service_symbol(pm, s),
        CommonsItem::Agent(a) => agent_symbol(pm, a),
        CommonsItem::Actor(a) => actor_symbol(pm, a),
        CommonsItem::Messages(m) => messages_symbol(pm, m),
        // Events track, slice 0 (spine #936): an `event` outlines exactly
        // like a `type` — same synthetic `TypeDecl` used elsewhere.
        CommonsItem::Event(e) => type_symbol(pm, &e.as_type_decl()),
        CommonsItem::Asset(a) => asset_symbol(pm, a),
        CommonsItem::Embed(e) => embed_symbol(pm, e),
    }
}

/// #1883, slice 1a: an `asset` outlines as a constant — a named value that
/// never changes, which is what its `.url` is.
fn asset_symbol(pm: &PositionMap, a: &AssetDecl) -> DocumentSymbol {
    make_symbol(
        a.name.name.clone(),
        detail_from_doc(&a.documentation),
        SymbolKind::CONSTANT,
        pm.range(a.span),
        pm.range(a.name.span),
        Vec::new(),
    )
}

fn embed_symbol(pm: &PositionMap, e: &EmbedDecl) -> DocumentSymbol {
    make_symbol(
        e.name.name.clone(),
        detail_from_doc(&e.documentation),
        SymbolKind::CONSTANT,
        pm.range(e.span),
        pm.range(e.name.span),
        Vec::new(),
    )
}

fn messages_symbol(pm: &PositionMap, m: &MessagesDecl) -> DocumentSymbol {
    make_symbol(
        format!("messages \"{}\"", m.tag),
        detail_from_doc(&m.documentation),
        SymbolKind::STRUCT,
        pm.range(m.span),
        pm.range(m.tag_span),
        Vec::new(),
    )
}

fn actor_symbol(pm: &PositionMap, a: &ActorDecl) -> DocumentSymbol {
    make_symbol(
        a.name.name.clone(),
        detail_from_doc(&a.documentation),
        SymbolKind::INTERFACE,
        pm.range(a.span),
        pm.range(a.name.span),
        Vec::new(),
    )
}

fn type_symbol(pm: &PositionMap, t: &TypeDecl) -> DocumentSymbol {
    let (kind, children) = match &t.body {
        TypeBody::Record(r) => (SymbolKind::STRUCT, record_field_symbols(pm, &r.fields)),
        TypeBody::Sum(s) => (SymbolKind::ENUM, variant_symbols(pm, &s.variants)),
        TypeBody::Opaque { .. } => (SymbolKind::CLASS, Vec::new()),
        TypeBody::Refined { .. } => (SymbolKind::TYPE_PARAMETER, Vec::new()),
    };
    make_symbol(
        t.name.name.clone(),
        detail_from_doc(&t.documentation),
        kind,
        pm.range(t.span),
        pm.range(t.name.span),
        children,
    )
}

fn record_field_symbols(pm: &PositionMap, fields: &[RecordField]) -> Vec<DocumentSymbol> {
    fields
        .iter()
        .map(|f| {
            make_symbol(
                f.name.name.clone(),
                None,
                SymbolKind::FIELD,
                pm.range(f.span),
                pm.range(f.name.span),
                Vec::new(),
            )
        })
        .collect()
}

fn variant_symbols(pm: &PositionMap, variants: &[Variant]) -> Vec<DocumentSymbol> {
    variants
        .iter()
        .map(|v| {
            make_symbol(
                v.name.name.clone(),
                None,
                SymbolKind::ENUM_MEMBER,
                pm.range(v.span),
                pm.range(v.name.span),
                Vec::new(),
            )
        })
        .collect()
}

fn fn_symbol(pm: &PositionMap, f: &FnDecl) -> DocumentSymbol {
    // Free functions are top-level Function symbols. Methods (whose
    // owning type lives in the same file) would normally nest under that
    // type, but the type-decl symbol is built independently — see the
    // commons/context walk. For v1.1, surface methods as top-level
    // siblings with a "TypeName.method" name; nesting can be added once
    // the walker reorders items.
    let kind = match &f.name {
        FnName::Free(_) => SymbolKind::FUNCTION,
        FnName::Method { .. } => SymbolKind::METHOD,
    };
    make_symbol(
        f.name.display(),
        detail_from_doc(&f.documentation),
        kind,
        pm.range(f.span),
        pm.range(f.name.ident().span),
        Vec::new(),
    )
}

fn capability_symbol(pm: &PositionMap, c: &CapabilityDecl) -> DocumentSymbol {
    let children = c
        .ops
        .iter()
        .map(|op| {
            make_symbol(
                op.name.name.clone(),
                detail_from_doc(&op.documentation),
                SymbolKind::METHOD,
                pm.range(op.span),
                pm.range(op.name.span),
                Vec::new(),
            )
        })
        .collect();
    make_symbol(
        c.name.name.clone(),
        detail_from_doc(&c.documentation),
        SymbolKind::INTERFACE,
        pm.range(c.span),
        pm.range(c.name.span),
        children,
    )
}

fn provider_symbol(pm: &PositionMap, p: &ProviderDecl) -> DocumentSymbol {
    let children = p
        .ops
        .iter()
        .map(|op| {
            make_symbol(
                op.name.name.clone(),
                None,
                SymbolKind::METHOD,
                pm.range(op.span),
                pm.range(op.name.span),
                Vec::new(),
            )
        })
        .collect();
    // The display name shows both the capability and provider names so
    // the outline disambiguates multiple `provides X = ...` blocks.
    let name = format!("{} = {}", p.capability.name, p.provider_name.name);
    make_symbol(
        name,
        detail_from_doc(&p.documentation),
        SymbolKind::OBJECT,
        pm.range(p.span),
        pm.range(p.provider_name.span),
        children,
    )
}

fn service_symbol(pm: &PositionMap, s: &ServiceDecl) -> DocumentSymbol {
    let children = s.handlers.iter().map(|h| handler_symbol(pm, h)).collect();
    make_symbol(
        s.name.name.clone(),
        detail_from_doc(&s.documentation),
        SymbolKind::CLASS,
        pm.range(s.span),
        pm.range(s.name.span),
        children,
    )
}

fn agent_symbol(pm: &PositionMap, a: &AgentDecl) -> DocumentSymbol {
    let mut children = Vec::new();
    // Key field — surface as a Property.
    children.push(make_symbol(
        a.key_name.name.clone(),
        Some("key".into()),
        SymbolKind::PROPERTY,
        pm.range(a.key_name.span),
        pm.range(a.key_name.span),
        Vec::new(),
    ));
    // Store fields.
    for field in &a.store_fields {
        children.push(make_symbol(
            field.name.name.clone(),
            Some("store".into()),
            SymbolKind::PROPERTY,
            pm.range(field.span),
            pm.range(field.name.span),
            Vec::new(),
        ));
    }
    // Handlers.
    for h in &a.handlers {
        children.push(handler_symbol(pm, h));
    }
    make_symbol(
        a.name.name.clone(),
        detail_from_doc(&a.documentation),
        SymbolKind::CLASS,
        pm.range(a.span),
        pm.range(a.name.span),
        children,
    )
}

fn handler_symbol(pm: &PositionMap, h: &Handler) -> DocumentSymbol {
    let name = match &h.method_name {
        Some(m) => format!("call {}", m.name),
        None => "call".to_string(),
    };
    let selection_span = h.method_name.as_ref().map(|m| m.span).unwrap_or(h.span);
    make_symbol(
        name,
        detail_from_doc(&h.documentation),
        SymbolKind::METHOD,
        pm.range(h.span),
        pm.range(selection_span),
        Vec::new(),
    )
}

fn detail_from_doc(doc: &Option<Doc>) -> Option<String> {
    doc.as_ref().and_then(|d| {
        let first = d
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())?
            .to_string();
        Some(first)
    })
}

#[allow(deprecated)] // `deprecated` and `tags` fields exist on DocumentSymbol.
fn make_symbol(
    name: String,
    detail: Option<String>,
    kind: SymbolKind,
    range: Range,
    selection_range: Range,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children: if children.is_empty() {
            None
        } else {
            Some(children)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline_of(src: &str) -> Vec<DocumentSymbol> {
        outline(src)
    }

    #[test]
    fn returns_empty_for_empty_input() {
        assert!(outline_of("").is_empty());
    }

    #[test]
    fn commons_with_types_and_fns_produces_module_with_children() {
        let src = "commons demo.x {\n\
                   type Money = Int where NonNegative\n\
                   fn double(n: Int) -> Int { n + n }\n\
                   }";
        let syms = outline_of(src);
        assert_eq!(syms.len(), 1);
        let module = &syms[0];
        assert_eq!(module.kind, SymbolKind::MODULE);
        assert_eq!(module.name, "demo.x");
        let children = module.children.as_ref().expect("children");
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].name, "Money");
        assert_eq!(children[0].kind, SymbolKind::TYPE_PARAMETER);
        assert_eq!(children[1].name, "double");
        assert_eq!(children[1].kind, SymbolKind::FUNCTION);
    }

    #[test]
    fn record_fields_nest_under_record_type() {
        let src = "commons demo.x {\n\
                   type Pt = { x: Int, y: Int }\n\
                   }";
        let syms = outline_of(src);
        let module = &syms[0];
        let children = module.children.as_ref().unwrap();
        let pt = &children[0];
        assert_eq!(pt.kind, SymbolKind::STRUCT);
        let fields = pt.children.as_ref().expect("record fields");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "x");
        assert_eq!(fields[0].kind, SymbolKind::FIELD);
        assert_eq!(fields[1].name, "y");
    }

    #[test]
    fn sum_variants_nest_under_enum() {
        let src = "commons demo.x {\n\
                   type Tag = enum { Foo, Bar, Baz }\n\
                   }";
        let syms = outline_of(src);
        let module = &syms[0];
        let tag = &module.children.as_ref().unwrap()[0];
        assert_eq!(tag.kind, SymbolKind::ENUM);
        let variants = tag.children.as_ref().expect("variants");
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].kind, SymbolKind::ENUM_MEMBER);
        assert_eq!(variants[2].name, "Baz");
    }

    #[test]
    fn opaque_type_uses_class_kind() {
        let src = "commons demo.x {\n\
                   type Id = opaque Int where NonNegative\n\
                   }";
        let syms = outline_of(src);
        let id = &syms[0].children.as_ref().unwrap()[0];
        assert_eq!(id.kind, SymbolKind::CLASS);
    }

    #[test]
    fn context_with_service_and_agent_produces_hierarchical_tree() {
        let src = "context demo.app {\n\
                   capability Clock { fn now() -> Int }\n\
                   service Api {\n\
                   on call(amount: Int) -> Int given Clock { amount }\n\
                   }\n\
                   agent Counter {\n\
                   key id: Int\n\
                   store value: Cell[Int]\n\
                   on call bump(amount: Int) -> Int { 0 }\n\
                   }\n\
                   }";
        let syms = outline_of(src);
        let module = &syms[0];
        assert_eq!(module.name, "demo.app");
        let children = module.children.as_ref().unwrap();
        // capability + service + agent
        let kinds: Vec<SymbolKind> = children.iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&SymbolKind::INTERFACE));
        assert!(kinds.contains(&SymbolKind::CLASS));
        let service = children
            .iter()
            .find(|c| c.name == "Api")
            .expect("Api service");
        let service_children = service.children.as_ref().unwrap();
        assert_eq!(service_children.len(), 1);
        assert_eq!(service_children[0].kind, SymbolKind::METHOD);
        let agent = children
            .iter()
            .find(|c| c.name == "Counter")
            .expect("Counter agent");
        let agent_children = agent.children.as_ref().unwrap();
        // key + store field + handler = 3 children
        assert_eq!(agent_children.len(), 3);
        assert!(
            agent_children
                .iter()
                .any(|c| c.kind == SymbolKind::PROPERTY && c.name == "value")
        );
        assert!(
            agent_children
                .iter()
                .any(|c| c.kind == SymbolKind::METHOD && c.name == "call bump")
        );
    }

    #[test]
    fn adapter_unit_outlines_its_items() {
        let src = "adapter tokens {\n\
                   binding \"./tokens.binding.ts\"\n\
                   exports capability { Jwt }\n\
                   capability Jwt {\n\
                   fn sign(secret: String) -> Effect[String]\n\
                   }\n\
                   provides Jwt = JoseJwt\n\
                   }";
        let syms = outline_of(src);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "tokens");
        let children = syms[0].children.as_ref().unwrap();
        // The capability and the external provider both appear in the outline.
        assert!(children.iter().any(|c| c.name == "Jwt"));
        assert!(children.iter().any(|c| c.name == "Jwt = JoseJwt"));
    }

    /// Every symbol's `selection_range` must be contained in its `range`, or
    /// VS Code rejects the whole `documentSymbol` response
    /// ("selectionRange must be contained in fullRange"). Verify recursively.
    fn assert_selection_contained(sym: &DocumentSymbol, path: &str) {
        let here = format!("{path}/{}", sym.name);
        let outer = sym.range;
        let inner = sym.selection_range;
        let pos_le = |a: Position, b: Position| (a.line, a.character) <= (b.line, b.character);
        assert!(
            pos_le(outer.start, inner.start) && pos_le(inner.end, outer.end),
            "selection_range not contained in range for {here}: range={outer:?} sel={inner:?}",
        );
        for c in sym.children.iter().flatten() {
            assert_selection_contained(c, &here);
        }
    }

    /// Regression: an `invariant` after a handler is a hard parse error, so
    /// recovery drops the agent and leaves the fragment-form context with no
    /// items. The context span must still cover its header (`context demo.a`)
    /// so the name-span selection range stays contained. (negative fixture 238)
    #[test]
    fn fragment_context_with_all_items_dropped_keeps_valid_ranges() {
        let src = "context demo.a\n\
                   \n\
                   agent Counter {\n\
                   key id: String\n\
                   store count: Cell[Int]\n\
                   on call bump() -> Effect[()] {\n\
                   let cur = count\n\
                   count := cur + 1\n\
                   ()\n\
                   }\n\
                   invariant bad:\n\
                   count >= 0\n\
                   }\n";
        for sym in &outline(src) {
            assert_selection_contained(sym, "");
        }
    }

    use tower_lsp::lsp_types::Position;

    #[test]
    fn doc_block_first_line_appears_as_detail() {
        let src = "commons demo.x {\n\
                   ---\n\
                   Short one-liner.\n\
                   Second line.\n\
                   ---\n\
                   type T = Int where Positive\n\
                   }";
        let syms = outline_of(src);
        let t = &syms[0].children.as_ref().unwrap()[0];
        assert_eq!(t.detail.as_deref(), Some("Short one-liner."));
    }
}
