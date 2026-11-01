//! The semantic-token legend's names: the single source for the language
//! server's legend and for the editor contract (`cargo xtask editor-contract`).
//!
//! **Array order is the wire encoding** (clients index into these arrays):
//! entries are append-only, never reordered, and `bynk-lsp`'s
//! `legend_is_frozen` test pins them. Token types are standard where faithful
//! (`type`, `function`) and custom for the Bynk-distinctive kinds
//! (`capability`, `service`, `agent`, `provider`). They live here, a leaf
//! crate with no LSP dependency, so tooling that only needs the names does not
//! link the language server.

/// The token types, in wire order.
pub const TOKEN_TYPES: &[&str] = &[
    "type",
    "function",
    "capability",
    "service",
    "agent",
    "provider",
    // v0.31 (ADR 0064): local bindings + params. Standard LSP type — VS Code
    // themes it by default, no extension declaration needed.
    "variable",
    // v0.36 (ADR 0069): instance methods. Appended (never reordered) so
    // existing legend indices are unchanged. Standard LSP type.
    "method",
    // v0.36 (ADR 0069, slice 2): record fields. Appended. Standard LSP
    // type. (Capability ops reuse `method` — they're operation calls.)
    "property",
    // v0.45: actor declarations. Appended at index 9 (never reordered).
    // Custom type — the VS Code extension declares it in package.json.
    "actor",
    // v0.140 (ADR 0163): handler-position annotations (`@cache` and its
    // argument labels). Appended at index 10. Standard LSP type — VS Code
    // themes `decorator` by default.
    "decorator",
    // message-bundles slice 1: `messages <tag> { ... }` bundles.
    // Appended at index 11 (never reordered). Custom type — the VS
    // Code extension declares it in package.json.
    "messages",
    // #1888: a `--|` doc line. Appended at index 12 (never
    // reordered); standard LSP type, with the `documentation`
    // modifier. #2203: no longer emitted (one-line docs, like `---`
    // blocks, are left to the editor grammars), but the legend is
    // append-only, so the entry stays.
    "comment",
    // #1883: `asset` names (static files). Appended at index 13
    // (never reordered). Custom type — the VS Code extension declares
    // it in package.json, as a `variable` subtype.
    "asset",
    // #2387: `embed` names (embedded text). Appended at index 14
    // (never reordered); a custom type the VS Code extension declares.
    "embed",
    // Constrained generics slice 0: constraint names in a bound (`T: Eq`,
    // `where A: Eq`). Appended at index 15 (never reordered); standard LSP
    // type.
    "interface",
];

/// The token modifiers, in wire order (bit 0 first).
pub const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "refined",
    "opaque",
    "platformNative",
    // #1888: on a `--|` doc line's `comment` token. Appended at bit 4.
    // #2203: no longer emitted; kept, as the legend is append-only.
    "documentation",
];

/// The LSP-standard token types an editor extension does NOT redeclare: VS Code
/// provides and themes them. Every other entry of [`TOKEN_TYPES`] is
/// Bynk-custom and must be declared (and scope-mapped) by the extension.
pub const STANDARD_TYPES: &[&str] = &[
    "type",
    "function",
    "variable",
    "method",
    "property",
    "decorator",
    "interface",
    // #1888: `--|` doc lines (no longer emitted since #2203; the legend is
    // append-only).
    "comment",
];

/// The LSP-standard token modifiers, likewise.
pub const STANDARD_MODIFIERS: &[&str] = &["declaration", "documentation"];
