//! Name closure: every name a module's tree uses is bound in that module
//! (#1831).
//!
//! [`unbound_names`] walks a [`TsProgram`] and returns each name it uses that
//! the module neither imports, declares, nor gets from TypeScript's, the
//! Workers runtime's or Node's globals. Two positions are checked:
//!
//! - **type position**: a [`TsType::Named`], or the first segment of a
//!   qualified `ns.T`;
//! - **value position**: an identifier expression (`TsExpr::Ident`, first
//!   segment of `a.b`), an object shorthand `{ x }`.
//!
//! A non-empty result is a `TS2304`/`TS2552` (`Cannot find name`) that `tsc`
//! would report, found without `tsc`. That is the defect family of #1736,
//! #1778, #1815, #1818, #1823 and #1829: the emitter names a type, or calls a
//! codec, that the module never imports.
//!
//! **Scope is module-wide for values, node-local for type parameters.** A
//! parameter or local declared anywhere in the module counts as bound
//! everywhere in it, and a destructuring pattern binds every identifier in it.
//! A type parameter (of a function, arrow, method, interface or alias) and a
//! mapped type's key are bound only inside their own node (1A.36a): the same
//! name used outside it is a use of something else. Each approximation can
//! only hide a defect, never invent one.
//!
//! **Opaque text is mostly a blind spot.** `Raw` and `Verbatim` statements
//! and `VerbatimExpr` expressions are pre-rendered TypeScript. A name they
//! *declare* or *import* is found line by line (a line starting `import`,
//! `type`, `interface`, `class`, `enum`, `function`, `const`, `let` or `var`).
//! Of the names they *use*, only a generic type application (`Name<…>`) is
//! checked (#1977: a kernel helper's parameter types are opaque text); any
//! other use is not; a `TsStmt::raw_using` statement also declares the names its
//! fixed text uses (1A.36a), and those are counted as uses. Measured over the positive corpus at #1831, opaque text was 0% of
//! `index.ts`/`compose.ts`, about 14% of unit modules and about 26% of
//! `handlers.ts` and test modules: chiefly service handler bodies, which
//! `bynk-emit` still renders to text. That part of a module stays `tsc`'s
//! job (#1823 lived there).
//!
//! **A type written as text is read.** A [`TsType::Named`] whose name carries
//! its arguments or members as text (`Option<A>`, `Result<T, __BoundaryError>`,
//! a union or function type) uses the names in that text, and a type
//! parameter's constraint or default (`T extends Shape`) uses the names it
//! holds (1A.36b-1). What is read is a type the emitter wrote itself, so the
//! reading is narrow: capitalised words, not members, keys or string literals.
//!
//! **A global never hides a name the caller provides.** See
//! [`unbound_names_providing`]: a module that imports a name sharing a
//! global's (`Request`, `Response`) uses its import.

use std::collections::{BTreeMap, BTreeSet};

use crate::program::{
    TsArrowBody, TsBindingName, TsDecl, TsExpr, TsObjectEntry, TsParam, TsProgram, TsStmt,
    TsStmtKind, TsType, TsTypeMember,
};

/// Type names TypeScript, the Workers runtime or Node provide globally, plus
/// the primitive keywords. A name here is never reported.
const GLOBAL_TYPES: &[&str] = &[
    // Keywords.
    "string",
    "number",
    "boolean",
    "bigint",
    "symbol",
    "object",
    "unknown",
    "never",
    "void",
    "undefined",
    "null",
    "any",
    "this",
    "true",
    "false",
    // `x as const`: the const assertion names no type (I4's generated client,
    // #1970, asserts its `CONTRACTS` literal this way).
    "const",
    // ECMAScript / lib.dom globals the emitter names.
    "Array",
    "ReadonlyArray",
    "Record",
    "Partial",
    "Readonly",
    "Required",
    "Pick",
    "Omit",
    "Exclude",
    "Extract",
    "NonNullable",
    "ReturnType",
    "Parameters",
    "Awaited",
    "BufferSource",
    "Promise",
    "PromiseLike",
    "Map",
    "ReadonlyMap",
    "ReadonlySet",
    "Set",
    "ReadonlySet",
    "WeakMap",
    "Date",
    "Error",
    "RegExp",
    "Uint8Array",
    "ArrayBuffer",
    "Iterable",
    "IterableIterator",
    "AsyncIterable",
    "AsyncIterableIterator",
    "AsyncGenerator",
    "Generator",
    "JSON",
    "Response",
    "Request",
    "Headers",
    "URL",
    "URLSearchParams",
    "ReadableStream",
    "WritableStream",
    "TextEncoder",
    "TextDecoder",
    "AbortSignal",
    "WebSocket",
    "MessageEvent",
    "Blob",
    "FormData",
    "Function",
    "PropertyKey",
    // `globalThis.Response` and the like: the qualifier itself.
    "globalThis",
];

/// Value names the ECMAScript, Workers and Node globals provide.
const GLOBAL_VALUES: &[&str] = &[
    "globalThis",
    "undefined",
    "NaN",
    "Infinity",
    "this",
    "arguments",
    "true",
    "false",
    "null",
    "JSON",
    "Object",
    "Number",
    "String",
    "Boolean",
    "Array",
    "BufferSource",
    "Promise",
    "Math",
    "Date",
    "Error",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "Symbol",
    "BigInt",
    "Map",
    "Set",
    "WeakMap",
    "Reflect",
    "Intl",
    "Uint8Array",
    "ArrayBuffer",
    "TextEncoder",
    "TextDecoder",
    "URL",
    "URLSearchParams",
    "Response",
    "Request",
    "Headers",
    "crypto",
    "console",
    "structuredClone",
    "setTimeout",
    "clearTimeout",
    "queueMicrotask",
    "atob",
    "btoa",
    "fetch",
    "encodeURIComponent",
    "decodeURIComponent",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "AbortController",
];

/// Every name `program` uses, in type or value position, that it does not
/// bind, sorted and deduplicated.
pub fn unbound_names(program: &TsProgram) -> Vec<String> {
    unbound_names_providing(program, |_| false)
}

/// [`unbound_names`] for a module whose imports `provided` knows: a name that
/// `provided` accepts is never hidden by the global tables.
///
/// The global tables exist so that `Promise` or `URL`, which TypeScript and the
/// Workers runtime provide, are not reported. They also hide a name the module
/// imports that happens to share a global's name (a user type `Request` or
/// `Response` of `bynk`, or a runtime export spelled like a global): the module
/// uses its own import, not the global, and an import list derived from the
/// tree has to see it. Emitted code spells every real global qualified
/// (`globalThis.Promise`), so a bare use of such a name is a use of the import.
/// `provided` answers for the names a caller can import: the runtime's exports
/// when deriving a runtime header, the module's own imports when comparing.
pub fn unbound_names_providing(
    program: &TsProgram,
    provided: impl Fn(&str) -> bool,
) -> Vec<String> {
    unbound_names_in(&program.stmts, provided)
}

/// [`unbound_names_providing`] over a module's statements as a slice, for a
/// caller that holds them before it has a [`TsProgram`] (an emitter deriving a
/// module's own header from the body it just built).
pub fn unbound_names_in(stmts: &[TsStmt], provided: impl Fn(&str) -> bool) -> Vec<String> {
    let mut w = Walk::default();
    for s in stmts {
        w.stmt(s);
    }
    let types = w
        .used_types
        .iter()
        .filter(|n| !w.bound.contains(*n) && (provided(n) || !GLOBAL_TYPES.contains(&n.as_str())));
    let values = w
        .used_values
        .iter()
        .filter(|n| !w.bound.contains(*n) && (provided(n) || !GLOBAL_VALUES.contains(&n.as_str())));
    // 1A.36a (Decision D): a name a `Raw` statement declares it uses may be a
    // type or a value, so it is judged against both global tables.
    let declared = w.used_declared.iter().filter(|n| {
        !w.bound.contains(*n)
            && (provided(n)
                || (!GLOBAL_TYPES.contains(&n.as_str()) && !GLOBAL_VALUES.contains(&n.as_str())))
    });
    let all: BTreeSet<String> = types.chain(values).chain(declared).cloned().collect();
    all.into_iter().collect()
}

/// 1A.39b (Decision D): every `TsExpr::Ident` of `program` that is not an identifier path
/// (`[A-Za-z_$][\w$]*(\.[A-Za-z_$][\w$]*)*`) and every `TsBindingName::Ident` that is not an
/// identifier, sorted and deduplicated. Such a text is expression text or a name built from it
/// held in a variable-like node: it hides what it uses from [`unbound_names`] and from the
/// printer's precedence. Member chains (`globalThis.Number`, `this.state`) pass: they print as
/// they read and their head is what the closure sees.
pub fn text_idents(program: &TsProgram) -> Vec<String> {
    let mut w = Walk::default();
    for s in &program.stmts {
        w.stmt(s);
    }
    w.text_idents.into_iter().collect()
}

/// 1B.5e: the text of every opaque statement (`TsStmt::raw`, `raw_using`, a `Verbatim`) in
/// `stmts`, **at any depth** — inside a function body, a `try`, a switch case or an object
/// method, not only at the top level. A producer that reads a tree for the names it uses asks
/// for this to check what the tree cannot see: a shallow `raw_text()` over the top-level
/// statements alone misses a nested one.
pub fn raw_statement_texts(stmts: &[TsStmt]) -> Vec<String> {
    let mut w = Walk::default();
    w.stmts(stmts);
    w.raw_texts
}

/// #2045: each `(specifier, name)` a structured named import brings in as a
/// value: every entry of an `import { … }` but its `type X` ones, none of an
/// `import type { … }`. Namespace and opaque-text imports are not reported.
pub fn value_imports(program: &TsProgram) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for stmt in &program.stmts {
        if let TsStmtKind::Decl(TsDecl::Import {
            type_only: false,
            names,
            from,
        }) = &stmt.kind
        {
            out.extend(
                names
                    .iter()
                    .map(|n| n.trim())
                    .filter(|n| !n.starts_with("type "))
                    .map(|n| (from.clone(), n.to_string())),
            );
        }
    }
    out
}

/// #2045: every name `program` uses as a value that only a type-only import
/// binds (`import { type Status }` then `Status.Active`): a `TS1361` (`cannot
/// be used as a value because it was imported using 'import type'`). Opaque
/// text is not searched for uses, as in [`unbound_names`].
pub fn type_only_value_uses(program: &TsProgram) -> Vec<String> {
    let mut w = Walk::default();
    for s in &program.stmts {
        w.stmt(s);
    }
    w.used_values
        .iter()
        .filter(|n| w.type_only.contains(*n) && !w.value_bound.contains(*n))
        .cloned()
        .collect()
}

/// #2036: a member a module names through one of its namespace imports:
/// `handlers.Batch` in type position, `handlers.__deserialise_Batch(…)` in
/// value position.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NamespaceMemberUse {
    /// The namespace import's specifier (`./handlers.js`).
    pub from: String,
    /// The member named (`Batch`).
    pub member: String,
}

/// #2036: every member `program` names through a namespace import
/// (`import * as ns`, `import type * as ns`), sorted and deduplicated. A
/// member the target module does not export is a `TS2694`/`TS2339` that
/// `tsc` would report (`Namespace 'handlers' has no exported member`), which
/// [`unbound_names`] cannot see: the namespace itself is bound. Checking it
/// needs the target module too, so a caller pairs this with
/// [`module_exports`]. Opaque text is not searched for uses.
pub fn namespace_member_uses(program: &TsProgram) -> Vec<NamespaceMemberUse> {
    let mut w = Walk::default();
    for s in &program.stmts {
        w.stmt(s);
    }
    let uses: BTreeSet<NamespaceMemberUse> = w
        .qualified
        .iter()
        .filter_map(|(head, member)| {
            w.namespaces.get(head).map(|from| NamespaceMemberUse {
                from: from.clone(),
                member: member.clone(),
            })
        })
        .collect();
    uses.into_iter().collect()
}

/// #2036: what a module exports, for [`namespace_member_uses`]' members.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModuleExports {
    /// Every name exported directly or re-exported by name.
    pub names: BTreeSet<String>,
    /// The specifier of each `export * from "…"`, whose module's exports are
    /// this one's too (a split context's `handlers.ts`, #1820).
    pub star_from: Vec<String>,
}

/// #2036: the names `program` exports. Structured exports are read with
/// [`TsStmt::exported_names`]; opaque text line by line, as [`unbound_names`]
/// reads its declarations: a line starting `export` and then a declaration
/// keyword or a `{ … }` list. Over-reporting an export can only hide a
/// defect.
pub fn module_exports(program: &TsProgram) -> ModuleExports {
    let mut out = ModuleExports::default();
    for stmt in &program.stmts {
        match &stmt.kind {
            TsStmtKind::Decl(TsDecl::ReExportAll { from }) => out.star_from.push(from.clone()),
            TsStmtKind::Decl(_) => {
                out.names
                    .extend(stmt.exported_names().iter().map(|e| e.name.to_string()));
            }
            TsStmtKind::Raw(text, _) | TsStmtKind::Verbatim { text, .. } => {
                opaque_exports(text, &mut out.names)
            }
            TsStmtKind::Const { .. }
            | TsStmtKind::Let { .. }
            | TsStmtKind::ExprStmt(_)
            | TsStmtKind::Return(_)
            | TsStmtKind::Throw(_)
            | TsStmtKind::If { .. }
            | TsStmtKind::ForOf { .. }
            | TsStmtKind::For { .. }
            | TsStmtKind::TryCatch { .. }
            | TsStmtKind::Block(_)
            | TsStmtKind::Continue(_)
            | TsStmtKind::Break(_)
            | TsStmtKind::While { .. }
            | TsStmtKind::Labelled { .. }
            | TsStmtKind::LetList(_)
            | TsStmtKind::Assign { .. }
            | TsStmtKind::Comment(_)
            | TsStmtKind::DocComment(_)
            | TsStmtKind::Blank
            | TsStmtKind::Switch { .. }
            | TsStmtKind::InlineBlock(_)
            | TsStmtKind::Increment(_) => {}
        }
    }
    out
}

/// The names opaque text exports, line by line (see [`module_exports`]).
fn opaque_exports(text: &str, out: &mut BTreeSet<String>) {
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("export ") else {
            continue;
        };
        let mut rest = rest.trim_start();
        for prefix in ["declare ", "async "] {
            rest = rest.strip_prefix(prefix).unwrap_or(rest).trim_start();
        }
        // `export type { A, B }` lists types, as `export { A, B }` lists names.
        let list = rest
            .strip_prefix("type ")
            .map(str::trim_start)
            .unwrap_or(rest)
            .strip_prefix('{');
        if let Some(list) = list {
            let inner = list.split('}').next().unwrap_or("");
            for spec in inner.split(',') {
                let spec = spec.trim();
                let spec = spec.strip_prefix("type ").unwrap_or(spec).trim();
                if let Some(name) = leading_ident(spec.rsplit(" as ").next().unwrap_or(spec)) {
                    out.insert(name.to_string());
                }
            }
            continue;
        }
        for keyword in [
            "function ",
            "const ",
            "let ",
            "var ",
            "class ",
            "interface ",
            "type ",
            "enum ",
        ] {
            if let Some(after) = rest.strip_prefix(keyword)
                && let Some(name) = leading_ident(after)
            {
                out.insert(name.to_string());
            }
        }
    }
}

#[derive(Default)]
struct Walk {
    bound: BTreeSet<String>,
    used_types: BTreeSet<String>,
    used_values: BTreeSet<String>,
    /// 1A.36a (Decision D): names a `TsStmt::raw_using` statement declares its
    /// fixed text uses, in either position.
    used_declared: BTreeSet<String>,
    /// 1A.36a: type parameters and mapped-type keys currently in scope. Each is
    /// bound only inside its own node (see [`Walk::scoped`]), so a use of the
    /// name outside it is still a use of something else.
    scope: Vec<String>,
    /// #2036: each namespace import's alias and specifier.
    namespaces: BTreeMap<String, String>,
    /// #2036: every `head.member` named in type or value position.
    qualified: BTreeSet<(String, String)>,
    /// #2045: names bound only by a type-only import (`import { type X }`,
    /// `import type { X }`, `import type * as ns`), and the names bound any
    /// other way. A value use of the first and not the second is `TS1361`.
    type_only: BTreeSet<String>,
    value_bound: BTreeSet<String>,
    /// 1B.5e: the text of every `Raw` or `Verbatim` statement met, at any depth, for
    /// [`raw_statement_texts`].
    raw_texts: Vec<String>,
    /// 1A.39b (Decision D): each `TsExpr::Ident` or `TsBindingName::Ident` text that is not an
    /// identifier path (see [`text_idents`]).
    text_idents: BTreeSet<String>,
}

/// Whether `text` is an identifier (`[A-Za-z_$][\w$]*`).
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Whether `text` is an identifier path: identifiers joined by single dots (`this.state`,
/// `globalThis.Number`).
pub(crate) fn is_identifier_path(text: &str) -> bool {
    text.split('.').all(is_identifier)
}

/// The identifier a piece of text starts with: `value` of `value.name`,
/// `path` of `path: string = "$"`. `None` when the text does not start with
/// one (a literal, a parenthesised expression).
fn leading_ident(text: &str) -> Option<&str> {
    let text = text.trim_start();
    let end = text
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '$'))
        .map_or(text.len(), |(i, _)| i);
    let ident = &text[..end];
    ident
        .chars()
        .next()
        .filter(|c| c.is_alphabetic() || *c == '_' || *c == '$')
        .map(|_| ident)
}

impl Walk {
    /// Bind a declared name. A destructuring pattern (`[k, v]`, `{ a, b: c }`)
    /// binds every identifier in it; over-binding can only hide a defect.
    fn bind(&mut self, name: &str) {
        let name = name.trim_start();
        if name.starts_with('[') || name.starts_with('{') {
            for word in name.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')) {
                if let Some(ident) = leading_ident(word) {
                    self.bound.insert(ident.to_string());
                    self.value_bound.insert(ident.to_string());
                }
            }
        } else if let Some(ident) = leading_ident(name) {
            self.bound.insert(ident.to_string());
            self.value_bound.insert(ident.to_string());
        }
    }

    /// #2045: bind a name a type-only import brings in.
    fn bind_type_only(&mut self, name: &str) {
        if let Some(ident) = leading_ident(name) {
            self.bound.insert(ident.to_string());
            self.type_only.insert(ident.to_string());
        }
    }

    /// A destructuring pattern's names: `a`, `a: b` (binds `b`), `a = 1`.
    fn bind_pattern(&mut self, names: &[String]) {
        for n in names {
            let local = n.split('=').next().unwrap_or(n);
            let local = local.rsplit(':').next().unwrap_or(local);
            self.bind(local);
        }
    }

    fn binding(&mut self, name: &TsBindingName) {
        match name {
            TsBindingName::Ident(n) => {
                if !is_identifier(n) {
                    self.text_idents.insert(n.clone());
                }
                self.bind(n)
            }
            TsBindingName::ObjectPattern(names) => self.bind_pattern(names),
            TsBindingName::ArrayPattern(names) => {
                for n in names.iter().filter(|n| !n.is_empty()) {
                    self.bind(n);
                }
            }
        }
    }

    fn use_value(&mut self, text: &str) {
        // A pre-rendered construction (`new globalThis.Map<K, V>()`, a state
        // initialiser) uses the constructor, not the `new` keyword.
        let text = text.trim_start();
        let text = text.strip_prefix("new ").unwrap_or(text);
        if let Some(ident) = leading_ident(text).filter(|i| !self.in_scope(i)) {
            self.used_values.insert(ident.to_string());
        }
    }

    /// Whether `name` is a type parameter or mapped key of an enclosing node.
    fn in_scope(&self, name: &str) -> bool {
        self.scope.iter().any(|n| n == name)
    }

    /// Run `f` with `names` (type parameters, a mapped key) bound for the
    /// duration of one node only. They never reach the flat `bound` and
    /// `value_bound` sets, so they bind nothing outside the node.
    ///
    /// A type parameter may be written with its constraint and default
    /// (`T extends Shape = Circle`): the text after the name uses types, which
    /// can name the parameters of the same list (1A.36b-1).
    fn scoped(&mut self, names: &[String], f: impl FnOnce(&mut Self)) {
        let depth = self.scope.len();
        for n in names {
            if let Some(ident) = leading_ident(n) {
                self.scope.push(ident.to_string());
            }
        }
        for n in names {
            if let Some(ident) = leading_ident(n) {
                self.use_type_text(&n.trim_start()[ident.len()..]);
            }
        }
        f(self);
        self.scope.truncate(depth);
    }

    /// The names a type written as text uses: every identifier that starts with
    /// an upper-case letter or `_`, outside string literals and not a member
    /// (`.name`) or a key (`name:`, `name?:`). A qualified head (`ns.T`) is the
    /// use of a namespace and records the member, as for a structured name.
    /// Lower-case words are keywords, primitives and parameters (a `typeof`
    /// operand is a use of the value it names), none of them an import of a generated module.
    fn use_type_text(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
        // A mapped key (`[S in keyof T]`) and an `infer X` binder are bound by the
        // text itself, wherever the text spells them.
        let words: Vec<&str> = text
            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .filter(|w| !w.is_empty())
            .collect();
        let local: Vec<&str> = words
            .windows(2)
            .filter(|w| w[1] == "in" || w[0] == "infer")
            .map(|w| if w[0] == "infer" { w[1] } else { w[0] })
            .collect();
        // The unmatched `{`, `(` and `[` before the cursor: a word followed by `:`
        // is an object key only when the nearest of them is `{`. Elsewhere the
        // colon belongs to a conditional type (`A extends B ? C : D`), a
        // parameter or a tuple label.
        let mut openers: Vec<u8> = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match b {
                b'"' | b'\'' | b'`' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 1;
                }
                _ if ident(b) => {
                    let start = i;
                    while i < bytes.len() && ident(bytes[i]) {
                        i += 1;
                    }
                    let word = &text[start..i];
                    // `typeof x` uses the value `x` (a namespace, for
                    // `typeof ns.__makeSurface`), never a type.
                    if word == "typeof" {
                        let operand = text[i..].trim_start();
                        // `typeof import("./x")` names a module, not a value: `import`
                        // is the keyword, so nothing is recorded.
                        if let Some(name) = leading_ident(operand).filter(|n| *n != "import") {
                            if !self.in_scope(name) {
                                self.used_values.insert(name.to_string());
                                if let Some(member) = operand[name.len()..]
                                    .strip_prefix('.')
                                    .and_then(leading_ident)
                                {
                                    self.qualified
                                        .insert((name.to_string(), member.to_string()));
                                }
                            }
                            i = text.len() - operand.len() + name.len();
                        }
                        continue;
                    }
                    let after_dot = start > 0 && bytes[start - 1] == b'.';
                    let rest = text[i..].trim_start();
                    let after_question = text[..start].trim_end().ends_with('?');
                    let is_key = openers.last() == Some(&b'{')
                        && !after_question
                        && (rest.starts_with(':') || rest.starts_with("?:"));
                    if after_dot || is_key || !(b.is_ascii_uppercase() || b == b'_') {
                        continue;
                    }
                    if self.in_scope(word) || local.contains(&word) {
                        continue;
                    }
                    self.used_types.insert(word.to_string());
                    if let Some(member) = text[i..].strip_prefix('.').and_then(leading_ident) {
                        self.qualified
                            .insert((word.to_string(), member.to_string()));
                    }
                }
                b'{' | b'(' | b'[' => {
                    openers.push(b);
                    i += 1;
                }
                b'}' | b')' | b']' => {
                    openers.pop();
                    i += 1;
                }
                _ => i += 1,
            }
        }
    }

    /// An import specifier: `X`, `type X`, `X as Y` or `type X as Y` binds
    /// the local name.
    fn bind_specifier(&mut self, spec: &str, decl_type_only: bool) {
        let spec = spec.trim();
        let (spec, type_only) = match spec.strip_prefix("type ") {
            Some(rest) => (rest.trim(), true),
            None => (spec, decl_type_only),
        };
        let local = spec.rsplit(" as ").next().unwrap_or(spec).trim();
        if local.is_empty() {
            return;
        }
        if type_only {
            self.bind_type_only(local);
        } else {
            self.bind(local);
        }
    }

    /// Lexical scan of opaque text for the names it declares or imports,
    /// line by line: only a line that *starts* a declaration (`type`,
    /// `interface`, `class`, `enum`, `function`, after any `export`/`declare`/
    /// `default`/`async`) or an `import` counts, so a cast like `x as T` in a
    /// function body is never mistaken for an import alias.
    fn bind_opaque(&mut self, text: &str) {
        for line in text.lines() {
            let mut rest = line.trim_start();
            for prefix in ["export ", "declare ", "default ", "async "] {
                rest = rest.strip_prefix(prefix).unwrap_or(rest).trim_start();
            }
            if let Some(import) = rest.strip_prefix("import ") {
                let head = import.split(" from ").next().unwrap_or("");
                let braces = head
                    .find('{')
                    .and_then(|open| head[open..].find('}').map(|close| (open, open + close)));
                if let Some((open, close)) = braces {
                    for spec in head[open + 1..close].split(',') {
                        self.bind_specifier(spec, false);
                    }
                } else if let Some(alias) = head.trim().strip_prefix("* as ") {
                    self.bind(alias.trim());
                }
                continue;
            }
            for keyword in [
                "type ",
                "interface ",
                "class ",
                "enum ",
                "function ",
                "const ",
                "let ",
                "var ",
            ] {
                if let Some(after) = rest.strip_prefix(keyword) {
                    let after = after.trim_start();
                    if let Some(pattern) = after.strip_prefix('{') {
                        let inner = pattern.split('}').next().unwrap_or("");
                        let names: Vec<String> = inner.split(',').map(str::to_string).collect();
                        self.bind_pattern(&names);
                    } else {
                        self.bind(after);
                    }
                }
            }
        }
    }

    /// Opaque text: the names it declares or imports, and the generic types
    /// it applies.
    fn opaque(&mut self, text: &str) {
        self.bind_opaque(text);
        self.use_opaque_generics(text);
    }

    /// #1977: the one *use* of a name opaque text is checked for — a generic
    /// type application, a capitalised identifier immediately followed by `<`
    /// (`MapEntry<string, number>` in a kernel helper's parameter types). That
    /// shape is a type in every position the emitter writes it, and it never
    /// collides with a comparison (`a < b` is spaced, its operands lower-case).
    /// An identifier after a `.` is a member or the tail of a qualified or
    /// inline-import spelling (`globalThis.Promise<T>`, `import("./m").T<U>`),
    /// whose head, if any, is not this identifier; text inside string,
    /// template and comment literals is skipped. Wider checks of opaque text
    /// (an annotation `x: T`, a bare value use) collide with object keys,
    /// locals and ternaries, so they stay `tsc`'s job.
    fn use_opaque_generics(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match b {
                b'"' | b'\'' | b'`' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 1;
                }
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    i += 2;
                    while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                        i += 1;
                    }
                    i += 2;
                }
                _ if ident(b) => {
                    let start = i;
                    while i < bytes.len() && ident(bytes[i]) {
                        i += 1;
                    }
                    let after_dot = start > 0 && bytes[start - 1] == b'.';
                    if !after_dot
                        && b.is_ascii_uppercase()
                        && bytes.get(i) == Some(&b'<')
                        && !self.in_scope(&text[start..i])
                    {
                        self.used_types.insert(text[start..i].to_string());
                    }
                }
                _ => i += 1,
            }
        }
    }

    fn ty(&mut self, t: &TsType) {
        match t {
            TsType::Named { name, type_args } => {
                let mut segments = name.splitn(2, '.');
                let head = segments.next().unwrap_or(name).trim();
                // A name may carry its arguments as text (`ns.T<number>`).
                if let Some(member) = segments.next().and_then(leading_ident) {
                    self.qualified
                        .insert((head.to_string(), member.to_string()));
                }
                if head
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
                    && head
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                    && !self.in_scope(head)
                {
                    self.used_types.insert(head.to_string());
                }
                for a in type_args {
                    self.ty(a);
                }
                // A name that is more than an identifier carries a type as
                // text: `Option<A>`, `Result<T, __BoundaryError>`, a union
                // `A | B`, a function type. Its arguments and members use names
                // too (1A.36b-1).
                if name
                    .chars()
                    .any(|c| !(c.is_alphanumeric() || matches!(c, '_' | '$' | '.')))
                {
                    self.use_type_text(name);
                }
            }
            TsType::Array { element, .. } => self.ty(element),
            TsType::Object(members) => self.members(members),
            TsType::Fn { params, ret, .. } => {
                for p in params {
                    self.ty(p);
                }
                self.ty(ret);
            }
            TsType::Union { members, .. } => {
                for m in members {
                    self.ty(m);
                }
            }
            TsType::Intersection(members) => {
                for m in members {
                    self.ty(m);
                }
            }
            // The key is the type's own binding, for its value.
            TsType::Mapped {
                key,
                keys_of,
                value,
                ..
            } => {
                self.ty(keys_of);
                self.scoped(std::slice::from_ref(key), |w| w.ty(value));
            }
            // `typeof x` uses the value `x`.
            TsType::TypeOf(operand) => self.expr(operand),
            TsType::Indexed { object, index } => {
                self.ty(object);
                self.ty(index);
            }
            TsType::StringLiteral(_) | TsType::NumberLiteral(_) => {}
            TsType::Paren(inner) => self.ty(inner),
        }
    }

    fn members(&mut self, members: &[TsTypeMember]) {
        for m in members {
            match m {
                TsTypeMember::Prop { ty, .. } => self.ty(ty),
                TsTypeMember::Method {
                    generics,
                    params,
                    ret,
                    ..
                } => {
                    self.scoped(generics, |w| {
                        w.params(params);
                        w.ty(ret);
                    });
                }
                TsTypeMember::Index {
                    key_ty, value_ty, ..
                } => {
                    self.ty(key_ty);
                    self.ty(value_ty);
                }
            }
        }
    }

    fn params(&mut self, params: &[TsParam]) {
        for p in params {
            match &p.array_pattern {
                Some(names) => {
                    for n in names.iter().filter(|n| !n.is_empty()) {
                        self.bind(n);
                    }
                }
                None => self.bind(&p.name),
            }
            if let Some(t) = &p.ty {
                self.ty(t);
            }
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }
    }

    fn opt_ty(&mut self, t: &Option<TsType>) {
        if let Some(t) = t {
            self.ty(t);
        }
    }

    fn stmts(&mut self, stmts: &[TsStmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, stmt: &TsStmt) {
        match &stmt.kind {
            TsStmtKind::Verbatim { text, .. } => {
                self.raw_texts.push(text.clone());
                self.opaque(text)
            }
            TsStmtKind::Raw(text, uses) => {
                self.raw_texts.push(text.clone());
                self.opaque(text);
                for n in uses {
                    if !self.in_scope(n) {
                        self.used_declared.insert(n.clone());
                    }
                }
            }
            TsStmtKind::Decl(decl) => self.decl(decl),
            TsStmtKind::Const { name, ty, init } => {
                self.binding(name);
                self.opt_ty(ty);
                self.expr(init);
            }
            TsStmtKind::Let { name, ty, init, .. } => {
                self.binding(name);
                self.opt_ty(ty);
                if let Some(e) = init {
                    self.expr(e);
                }
            }
            TsStmtKind::ExprStmt(e) | TsStmtKind::Throw(e) | TsStmtKind::Increment(e) => {
                self.expr(e)
            }
            TsStmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            TsStmtKind::If {
                cond,
                then_branch,
                else_branch,
                same_line_else: _,
            } => {
                self.expr(cond);
                self.stmt(then_branch);
                if let Some(e) = else_branch {
                    self.stmt(e);
                }
            }
            TsStmtKind::ForOf {
                binding,
                iter,
                body,
                is_await: _,
            } => {
                self.bind(binding);
                self.expr(iter);
                self.stmt(body);
            }
            TsStmtKind::While { cond, body } => {
                self.expr(cond);
                self.stmt(body);
            }
            TsStmtKind::Labelled { label: _, body } => self.stmt(body),
            TsStmtKind::LetList(decls) => {
                for d in decls {
                    self.bind(&d.name);
                    self.opt_ty(&d.ty);
                    if let Some(e) = &d.init {
                        self.expr(e);
                    }
                }
            }
            TsStmtKind::For {
                name,
                init,
                test,
                body,
            } => {
                self.bind(name);
                self.expr(init);
                self.expr(test);
                self.stmt(body);
            }
            TsStmtKind::TryCatch {
                try_block,
                catch_param,
                catch_block,
                finally_block,
            } => {
                if let Some(p) = catch_param {
                    self.bind(p);
                }
                self.stmt(try_block);
                if let Some(c) = catch_block {
                    self.stmt(c);
                }
                if let Some(f) = finally_block {
                    self.stmt(f);
                }
            }
            TsStmtKind::Block(stmts) | TsStmtKind::InlineBlock(stmts) => self.stmts(stmts),
            TsStmtKind::Assign { target, value } => {
                self.expr(target);
                self.expr(value);
            }
            TsStmtKind::Switch {
                discriminant,
                cases,
            } => {
                self.expr(discriminant);
                for case in cases {
                    for t in &case.labels {
                        self.expr(t);
                    }
                    self.stmts(&case.body);
                }
            }
            TsStmtKind::Continue(_)
            | TsStmtKind::Break(_)
            | TsStmtKind::Comment(_)
            | TsStmtKind::DocComment(_)
            | TsStmtKind::Blank => {}
        }
    }

    fn decl(&mut self, decl: &TsDecl) {
        match decl {
            TsDecl::Import {
                names, type_only, ..
            } => {
                for n in names {
                    self.bind_specifier(n, *type_only);
                }
            }
            TsDecl::ImportNamespace {
                alias,
                from,
                type_only,
            } => {
                self.namespaces.insert(alias.clone(), from.clone());
                if *type_only {
                    self.bind_type_only(alias)
                } else {
                    self.bind(alias)
                }
            }
            TsDecl::ImportDefault { alias, named, .. } => {
                self.bind(alias);
                for n in named {
                    self.bind_specifier(n, false);
                }
            }
            TsDecl::ReExport { .. } | TsDecl::ReExportAll { .. } => {}
            // `export { a, b };` exports names this module binds (#1852),
            // so each is a use: one never imported or declared is unbound.
            // A `type X` entry (#2036) uses `X`, and `X as Y` uses `X`.
            TsDecl::ExportNames { names } => {
                for n in names {
                    let n = n.trim();
                    match n.strip_prefix("type ") {
                        Some(t) => {
                            if let Some(ident) = leading_ident(t).filter(|i| !self.in_scope(i)) {
                                self.used_types.insert(ident.to_string());
                            }
                        }
                        None => self.use_value(n),
                    }
                }
            }
            TsDecl::Export(inner) => self.decl(inner),
            TsDecl::Interface {
                name,
                type_params,
                members,
            } => {
                self.bind(name);
                self.scoped(type_params, |w| w.members(members));
            }
            TsDecl::TypeAlias {
                name,
                type_params,
                ty,
            } => {
                self.bind(name);
                self.scoped(type_params, |w| w.ty(ty));
            }
            TsDecl::ConstDecl { name, ty, init } => {
                self.bind(name);
                self.opt_ty(ty);
                self.expr(init);
            }
            TsDecl::DeclareConst { name, ty } => {
                self.bind(name);
                self.ty(ty);
            }
            TsDecl::Class {
                name,
                implements,
                fields,
                constructor,
                methods,
                ..
            } => {
                self.bind(name);
                for ty in implements {
                    self.ty(ty);
                }
                for f in fields {
                    self.opt_ty(&f.ty);
                    if let Some(init) = &f.init {
                        self.expr(init);
                    }
                }
                if let Some(ctor) = constructor {
                    self.params(&ctor.params);
                    self.stmts(&ctor.body);
                }
                for m in methods {
                    self.params(&m.params);
                    self.opt_ty(&m.return_type);
                    self.stmts(&m.body);
                }
            }
            TsDecl::Function {
                name,
                generics,
                params,
                return_type,
                body,
                ..
            } => {
                self.bind(name);
                self.scoped(generics, |w| {
                    w.params(params);
                    w.opt_ty(return_type);
                    w.stmts(body);
                });
            }
            TsDecl::ExportDefault(e) => self.expr(e),
        }
    }

    fn expr(&mut self, expr: &TsExpr) {
        match expr {
            TsExpr::Ident(name) => {
                if !is_identifier_path(name) {
                    self.text_idents.insert(name.clone());
                }
                self.use_value(name)
            }
            TsExpr::Lit(_) => {}
            TsExpr::VerbatimExpr(text, _) => self.opaque(text),
            // Text the emitter printed from nodes, with the names it uses declared
            // beside it (1A.36b-1): counted as `raw_using`'s are.
            TsExpr::RawUsing(_, names) => {
                self.used_declared.extend(names.iter().cloned());
            }
            TsExpr::Member { object, property } | TsExpr::OptionalMember { object, property } => {
                if let TsExpr::Ident(head) = object.as_ref() {
                    self.qualified.insert((head.clone(), property.clone()));
                }
                self.expr(object)
            }
            TsExpr::Index { object, index } | TsExpr::OptionalIndex { object, index } => {
                self.expr(object);
                self.expr(index);
            }
            TsExpr::Arrow {
                params,
                generics,
                return_type,
                body,
                ..
            } => {
                self.scoped(generics, |w| {
                    w.params(params);
                    w.opt_ty(return_type);
                    match body.as_ref() {
                        TsArrowBody::Expr(e) => w.expr(e),
                        TsArrowBody::Block(stmts)
                        | TsArrowBody::Multiline(stmts)
                        | TsArrowBody::Flat(stmts) => w.stmts(stmts),
                    }
                });
            }
            // 1A.21: the default is read in the enclosing scope; the parameter is bound for the body.
            TsExpr::DefaultIife {
                name,
                default,
                return_type,
                body,
            } => {
                self.expr(default);
                self.bind(name);
                self.ty(return_type);
                self.stmts(body);
            }
            TsExpr::Call { callee, args } | TsExpr::New { callee, args } => {
                self.expr(callee);
                for a in args {
                    self.expr(a);
                }
            }
            // 1A.20: the names the type arguments mention are uses, as for `As`.
            TsExpr::TypeApp { callee, type_args } => {
                self.expr(callee);
                for t in type_args {
                    self.ty(t);
                }
            }
            TsExpr::Object { entries, .. } => {
                for e in entries {
                    match e {
                        TsObjectEntry::Prop { value: v, .. } | TsObjectEntry::Spread(v) => {
                            self.expr(v)
                        }
                        TsObjectEntry::Shorthand(name) => self.use_value(name),
                        TsObjectEntry::Method {
                            generics,
                            params,
                            return_type,
                            body,
                            ..
                        } => {
                            self.scoped(generics, |w| {
                                w.params(params);
                                w.opt_ty(return_type);
                                w.stmts(body);
                            });
                        }
                    }
                }
            }
            TsExpr::Array { items, .. } | TsExpr::LinesArray(items) => {
                for i in items {
                    self.expr(i);
                }
            }
            TsExpr::TemplateLit { exprs, .. } => {
                for e in exprs {
                    self.expr(e);
                }
            }
            TsExpr::Await(e)
            | TsExpr::Paren(e)
            | TsExpr::Spread(e)
            | TsExpr::NonNull(e)
            | TsExpr::Yield(e)
            | TsExpr::Unary { expr: e, .. } => self.expr(e),
            TsExpr::Sequence(items) => {
                for i in items {
                    self.expr(i);
                }
            }
            TsExpr::OptionalCall { callee, args } => {
                self.expr(callee);
                for a in args {
                    self.expr(a);
                }
            }
            TsExpr::Function {
                params,
                return_type,
                body,
                ..
            } => {
                self.params(params);
                self.opt_ty(return_type);
                self.stmts(body);
            }
            TsExpr::As { expr, ty } => {
                self.expr(expr);
                self.ty(ty);
            }
            TsExpr::AsConst(expr) => self.expr(expr),
            TsExpr::Assign { target, value }
            | TsExpr::AddAssign { target, value }
            | TsExpr::NullishAssign { target, value } => {
                self.expr(target);
                self.expr(value);
            }
            TsExpr::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            TsExpr::Conditional {
                test,
                consequent,
                alternate,
            }
            | TsExpr::MultilineConditional {
                test,
                consequent,
                alternate,
            } => {
                self.expr(test);
                self.expr(consequent);
                self.expr(alternate);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{TsArrowBody, TsDeclarator, VerbatimOrigin};

    fn program(stmts: Vec<TsStmt>) -> TsProgram {
        TsProgram { stmts }
    }

    fn import(names: &[&str]) -> TsStmt {
        TsStmt::decl(
            TsDecl::Import {
                type_only: false,
                names: names.iter().map(|n| n.to_string()).collect(),
                from: "./m.js".to_string(),
            },
            None,
        )
    }

    /// `export function <name>(value: <param_ty>): __JsonValue { return <call>(value); }`
    fn codec(name: &str, param_ty: &str, call: &str) -> TsStmt {
        TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::Function {
                name: name.to_string(),
                generics: vec![],
                params: vec![TsParam::new(
                    "value".to_string(),
                    Some(TsType::named(param_ty)),
                )],
                return_type: Some(TsType::named("__JsonValue")),
                body: vec![TsStmt::return_stmt(
                    Some(TsExpr::Call {
                        callee: Box::new(TsExpr::Ident(call.to_string())),
                        args: vec![TsExpr::Ident("value".to_string())],
                    }),
                    None,
                )],
                is_async: false,
                inline: false,
            })),
            None,
        )
    }

    #[test]
    fn a_type_named_but_never_imported_is_unbound() {
        // #1829's shape: a codec's parameter type the module never imports.
        let p = program(vec![
            import(&["type __JsonValue", "__serialise_Name"]),
            codec("__serialise_Note", "Note", "__serialise_Name"),
        ]);
        assert_eq!(unbound_names(&p), ["Note"]);
    }

    /// A function over `s` whose parameter type is `ty` and whose body does nothing.
    fn function_over(ty: TsType) -> TsStmt {
        TsStmt::decl(
            TsDecl::Function {
                name: "f".to_string(),
                generics: vec![],
                params: vec![TsParam::new("s".to_string(), Some(ty))],
                return_type: None,
                body: vec![],
                is_async: false,
                inline: false,
            },
            None,
        )
    }

    #[test]
    fn typeof_in_a_type_uses_the_value_it_names() {
        // `typeof s` names the parameter, so it is bound there and unbound outside.
        let p = program(vec![function_over(TsType::type_of(TsExpr::Ident(
            "s".to_string(),
        )))]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
        let p = program(vec![TsStmt::decl(
            TsDecl::TypeAlias {
                name: "T".to_string(),
                type_params: vec![],
                ty: TsType::type_of(TsExpr::Ident("missing".to_string())),
            },
            None,
        )]);
        assert_eq!(unbound_names(&p), ["missing"]);
        // The same operand inside type text: a namespace spelled only there.
        let over = |name: &str| program(vec![function_over(TsType::named(name))]);
        assert_eq!(
            unbound_names(&over(
                "globalThis.ReturnType<typeof commerce_payment.__makeSurface>"
            )),
            ["commerce_payment"]
        );
        let bound = program(vec![
            import(&["commerce_payment"]),
            function_over(TsType::named("Array<typeof commerce_payment.f>")),
        ]);
        assert!(unbound_names(&bound).is_empty());
    }

    #[test]
    fn typeof_import_records_no_used_name() {
        // `import` after `typeof` is the keyword of an import type, not a value.
        let p = program(vec![function_over(TsType::named("typeof import(\"./x\")"))]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    #[test]
    fn a_capitalised_tuple_label_or_parameter_name_reads_as_a_use() {
        // Pinned, conservative behaviour: `[First: A]` and `(Req: X) => Y` read `First` and
        // `Req` as uses, because only `{`-keys are recognised as labels. Over-reporting an
        // unbound name is the safe direction; a lower-case label is already ignored.
        let over = |name: &str| program(vec![function_over(TsType::named(name))]);
        assert_eq!(unbound_names(&over("[First: A]")), ["A", "First"]);
        assert_eq!(unbound_names(&over("(Req: X) => Y")), ["Req", "X", "Y"]);
    }

    #[test]
    fn a_mapped_type_binds_its_key_and_uses_the_names_it_mentions() {
        let mapped = |source: &str| {
            TsType::mapped(
                "K",
                TsType::named(source),
                TsType::indexed(TsType::named(source), TsType::named("K")),
            )
        };
        // `K` is the mapped type's own, so only the source counts as a use.
        let p = program(vec![function_over(mapped("Shape"))]);
        assert_eq!(unbound_names(&p), ["Shape"]);
        let p = program(vec![
            import(&["type Shape"]),
            function_over(mapped("Shape")),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    /// `type <name><params> = <ty>;`, exported nowhere: a declaration that binds only its own name.
    fn alias(name: &str, type_params: &[&str], ty: TsType) -> TsStmt {
        TsStmt::decl(
            TsDecl::TypeAlias {
                name: name.to_string(),
                type_params: type_params.iter().map(|p| p.to_string()).collect(),
                ty,
            },
            None,
        )
    }

    /// 1A.36a: a mapped key is bound inside its own type and nowhere else. It used to enter
    /// the flat bound sets, so one mapped `K` made every `K` in the module, type or value,
    /// look bound.
    #[test]
    fn a_mapped_key_binds_only_inside_its_own_type() {
        let mapped = TsType::mapped(
            "K",
            TsType::named("Shape"),
            TsType::indexed(TsType::named("Shape"), TsType::named("K")),
        );
        let program_with = |outside: TsStmt| {
            program(vec![
                import(&["type Shape"]),
                alias("Mapped", &[], mapped.clone()),
                outside,
            ])
        };
        // Alone, the mapped type is closed.
        let p = program(vec![
            import(&["type Shape"]),
            alias("Mapped", &[], mapped.clone()),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
        // A type named `K` outside it is not the key.
        let p = program_with(alias("Other", &[], TsType::named("K")));
        assert_eq!(unbound_names(&p), ["K"]);
        // Nor is a value named `K`.
        let p = program_with(TsStmt::expr_stmt(TsExpr::Ident("K".to_string()), None));
        assert_eq!(unbound_names(&p), ["K"]);
        // The key is not in scope for the `keyof` operand: `[K in keyof K]` uses a `K` from outside.
        let p = program(vec![alias(
            "Self",
            &[],
            TsType::mapped("K", TsType::named("K"), TsType::named("string")),
        )]);
        assert_eq!(unbound_names(&p), ["K"]);
    }

    /// 1A.36a: a type parameter is bound inside its own declaration, function, arrow or method
    /// and nowhere else.
    #[test]
    fn a_type_parameter_binds_only_inside_its_own_node() {
        let generic_fn = |generics: &[&str]| {
            TsStmt::decl(
                TsDecl::Function {
                    name: "id".to_string(),
                    generics: generics.iter().map(|g| g.to_string()).collect(),
                    params: vec![TsParam::new("x".to_string(), Some(TsType::named("T")))],
                    return_type: Some(TsType::named("T")),
                    body: vec![],
                    is_async: false,
                    inline: false,
                },
                None,
            )
        };
        let p = program(vec![generic_fn(&["T"])]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
        // Without the parameter, `T` is a use of an unbound name.
        let p = program(vec![generic_fn(&[])]);
        assert_eq!(unbound_names(&p), ["T"]);
        // The parameter of one function does not bind `T` in the next declaration.
        let p = program(vec![
            generic_fn(&["T"]),
            alias("Later", &[], TsType::named("T")),
        ]);
        assert_eq!(unbound_names(&p), ["T"]);
        // Nor does an alias's own parameter, or an arrow's, bind it outside.
        let p = program(vec![
            alias("Box", &["T"], TsType::named("T")),
            alias("Later", &[], TsType::named("T")),
        ]);
        assert_eq!(unbound_names(&p), ["T"]);
        let arrow = TsExpr::Arrow {
            params: vec![TsParam::new("x".to_string(), Some(TsType::named("T")))],
            return_type: None,
            generics: vec!["T".to_string()],
            body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Ident("x".to_string())))),
            is_async: false,
        };
        let p = program(vec![
            TsStmt::expr_stmt(arrow, None),
            alias("Later", &[], TsType::named("T")),
        ]);
        assert_eq!(unbound_names(&p), ["T"]);
        // A constraint (`T extends Shape`) binds `T`, not the whole text.
        let p = program(vec![
            import(&["type Shape"]),
            alias("Box", &["T extends Shape"], TsType::named("T")),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    /// 1A.36b-1: a constraint or a default (`T extends Shape = Circle`) uses types, and can name
    /// the other parameters of its own list. A type named only there is unbound unless imported.
    #[test]
    fn a_type_parameter_constraint_and_default_use_the_names_they_hold() {
        let boxed = |param: &str| alias("Box", &[param], TsType::named("T"));
        // `Shape` appears nowhere else: only the constraint uses it.
        let p = program(vec![boxed("T extends Shape")]);
        assert_eq!(unbound_names(&p), ["Shape"]);
        let p = program(vec![import(&["type Shape"]), boxed("T extends Shape")]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
        // A default is a use too; an `extends` clause may use a generic type.
        let p = program(vec![boxed("T = Circle")]);
        assert_eq!(unbound_names(&p), ["Circle"]);
        let p = program(vec![boxed("T extends Readonly<Option<Shape>>")]);
        assert_eq!(unbound_names(&p), ["Option", "Shape"]);
        // A parameter of the same list is bound inside the constraint, and a
        // global is not reported.
        let p = alias("Pair", &["A", "B extends Array<A>"], TsType::named("B"));
        assert!(unbound_names(&program(vec![p])).is_empty());
        // A function's own list is read the same way.
        let f = TsStmt::decl(
            TsDecl::Function {
                name: "f".to_string(),
                generics: vec!["T extends Shape".to_string()],
                params: vec![],
                return_type: None,
                body: vec![],
                is_async: false,
                inline: false,
            },
            None,
        );
        assert_eq!(unbound_names(&program(vec![f])), ["Shape"]);
    }

    /// 1A.36b-1: a `Named` type whose name carries its arguments as text uses the head and every
    /// name in the arguments, however it was spelled.
    #[test]
    fn a_named_type_that_carries_its_arguments_as_text_uses_them() {
        let over = |name: &str| program(vec![function_over(TsType::named(name))]);
        assert_eq!(unbound_names(&over("Option<A>")), ["A", "Option"]);
        assert_eq!(
            unbound_names(&over("Result<unknown, __BoundaryError>")),
            ["Result", "__BoundaryError"]
        );
        // A union, a function type and a global-qualified generic.
        assert_eq!(unbound_names(&over("Foo | Bar | null")), ["Bar", "Foo"]);
        assert_eq!(
            unbound_names(&over("(a: Req) => globalThis.Promise<Res>")),
            ["Req", "Res"]
        );
        // A mapped key and an `infer` binder are bound by the text.
        assert_eq!(
            unbound_names(&over("{ readonly [S in keyof Services]: Services[S] }")),
            ["Services"]
        );
        assert_eq!(unbound_names(&over("Box<infer U> | U")), ["Box"]);
        // Members, string literals, numbers, keys and lower-case words are not names.
        assert!(unbound_names(&over("\"Some\" | 200 | string")).is_empty());
        assert!(unbound_names(&over("{ Key: string }")).is_empty());
        assert_eq!(unbound_names(&over("ns.T<number>")), ["ns"]);
        // Imported, they are bound; a type parameter in scope is not a use.
        let p = program(vec![
            import(&["type Option", "type A"]),
            function_over(TsType::named("Option<A>")),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    /// 1A.36b-2: a capitalised word before `:` is a key only inside `{ }`. In a conditional type
    /// the colon separates the branches, so the true branch is a use.
    #[test]
    fn a_conditional_type_branch_is_a_use_not_a_key() {
        let over = |name: &str| program(vec![function_over(TsType::named(name))]);
        assert_eq!(
            unbound_names(&over("A extends B ? C : D")),
            ["A", "B", "C", "D"]
        );
        // Inside an object type the same shape stays a key and a conditional value is a use.
        assert_eq!(
            unbound_names(&over("{ Key: A extends B ? C : D }")),
            ["A", "B", "C", "D"]
        );
        assert!(unbound_names(&over("{ Key?: string; Other: number }")).is_empty());
        // A key after a nested group closes is still a key.
        assert_eq!(
            unbound_names(&over("{ F: (x: X) => Y; K: Z }")),
            ["X", "Y", "Z"]
        );
    }

    /// 1A.36b-1: the global tables hide a name the module imports that shares a global's name,
    /// unless the caller says it provides that name.
    #[test]
    fn a_provided_name_is_not_hidden_by_a_global() {
        let p = program(vec![function_over(TsType::named("Response"))]);
        assert!(unbound_names(&p).is_empty());
        let provided = |n: &str| n == "Response";
        assert_eq!(unbound_names_providing(&p, provided), ["Response"]);
        // Value position, and a name the caller does not provide, stay hidden.
        let call = TsStmt::expr_stmt(
            TsExpr::Call {
                callee: Box::new(TsExpr::Ident("Request".to_string())),
                args: vec![],
            },
            None,
        );
        let p = program(vec![call]);
        assert!(unbound_names_providing(&p, provided).is_empty());
        assert_eq!(unbound_names_providing(&p, |n| n == "Request"), ["Request"]);
        // Imported, the provided name is bound whatever it is called.
        let p = program(vec![
            import(&["Response"]),
            function_over(TsType::named("Response")),
        ]);
        assert!(unbound_names_providing(&p, provided).is_empty());
    }

    /// 1A.36b-1: an expression's `raw_using` names are uses, as the statement's are, and the
    /// nodes the text was printed from are read the same way to find them.
    #[test]
    fn an_expression_that_declares_its_uses_counts_them() {
        let text = TsExpr::raw_using(
            "(__j: __JsonValue) => { return Ok(__j); }",
            vec!["Ok".to_string(), "__JsonValue".to_string()],
        );
        let p = program(vec![TsStmt::expr_stmt(text.clone(), None)]);
        assert_eq!(unbound_names(&p), ["Ok", "__JsonValue"]);
        let p = program(vec![
            import(&["Ok", "type __JsonValue"]),
            TsStmt::expr_stmt(text, None),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
        // The nodes it was printed from: parameters and locals bind, the rest is used.
        let arrow = TsExpr::Arrow {
            params: vec![TsParam::new(
                "__j".to_string(),
                Some(TsType::named("__JsonValue")),
            )],
            is_async: false,
            generics: vec![],
            return_type: None,
            body: Box::new(TsArrowBody::Block(vec![TsStmt::return_stmt(
                Some(TsExpr::Call {
                    callee: Box::new(TsExpr::Ident("Ok".to_string())),
                    args: vec![TsExpr::Ident("__j".to_string())],
                }),
                None,
            )])),
        };
        assert_eq!(
            unbound_names_in(&[TsStmt::expr_stmt(arrow, None)], |_| false),
            ["Ok", "__JsonValue"]
        );
    }

    /// 1A.39a: a `Flat` body is read as the statements it holds, so what it declares binds and what
    /// it calls is a use (the witness decoder's body is exactly this).
    #[test]
    fn a_flat_arrow_body_is_read_like_a_block() {
        let arrow = TsExpr::Arrow {
            params: vec![TsParam::new("__j".to_string(), None)],
            is_async: false,
            generics: vec![],
            return_type: None,
            body: Box::new(TsArrowBody::Flat(vec![
                TsStmt::const_stmt(
                    TsBindingName::Ident("__v".to_string()),
                    None,
                    TsExpr::Ident("__j".to_string()),
                    None,
                ),
                TsStmt::return_stmt(
                    Some(TsExpr::Call {
                        callee: Box::new(TsExpr::Ident("Ok".to_string())),
                        args: vec![TsExpr::Ident("__v".to_string())],
                    }),
                    None,
                ),
            ])),
        };
        assert_eq!(
            unbound_names_in(&[TsStmt::expr_stmt(arrow, None)], |_| false),
            ["Ok"]
        );
    }

    /// 1A.36b-1: `names_imported_from` reads what a module takes from one place.
    #[test]
    fn names_are_read_by_the_specifier_they_come_from() {
        let from = |spec: &str, names: &[&str]| {
            TsStmt::decl(
                TsDecl::Import {
                    type_only: false,
                    names: names.iter().map(|n| n.to_string()).collect(),
                    from: spec.to_string(),
                },
                None,
            )
        };
        let p = program(vec![
            from("./__bynk_runtime/result.js", &["Ok", "type Option"]),
            from("./other.js", &["Thing as Renamed"]),
        ]);
        let runtime = p.names_imported_from(|f| f.contains("__bynk_runtime/"));
        assert_eq!(runtime.into_iter().collect::<Vec<_>>(), ["Ok", "Option"]);
        let other = p.names_imported_from(|f| f == "./other.js");
        assert_eq!(other.into_iter().collect::<Vec<_>>(), ["Renamed"]);
    }

    /// 1A.36a (Decision D): a `raw_using` statement's names are uses, in either position, and
    /// what the text declares is still bound. A plain `raw` carries no names.
    #[test]
    fn raw_using_counts_the_names_it_declares_as_uses() {
        let raw = |names: &[&str]| TsStmt::raw_using("export function f(): void {}\n", names);
        // A type and a value the module never binds are both unbound.
        let p = program(vec![raw(&["Option", "__bynkBytesToBase64"])]);
        assert_eq!(unbound_names(&p), ["Option", "__bynkBytesToBase64"]);
        // Imported, they are bound.
        let p = program(vec![
            import(&["type Option", "__bynkBytesToBase64"]),
            raw(&["Option", "__bynkBytesToBase64"]),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
        // A global of either table is not reported; a name the text declares is bound.
        let p = program(vec![TsStmt::raw_using(
            "interface Local {}\n",
            &["Local", "Record", "JSON"],
        )]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
        // No names: the text is a blind spot, as before.
        let p = program(vec![TsStmt::raw("const x = Missing;\n", None), raw(&[])]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
        // The names change nothing about printing.
        let with = raw(&["Option"]);
        let without = TsStmt::raw("export function f(): void {}\n", None);
        assert_eq!(crate::print_stmt(&with, 0), crate::print_stmt(&without, 0));
    }

    #[test]
    fn a_parenthesised_type_uses_the_names_it_holds() {
        let thunk = |name: &str| {
            TsType::paren(TsType::Fn {
                rest: false,
                names: vec![],
                params: vec![],
                ret: Box::new(TsType::named(name)),
            })
        };
        let p = program(vec![function_over(thunk("Row"))]);
        assert_eq!(unbound_names(&p), ["Row"]);
        let p = program(vec![import(&["type Row"]), function_over(thunk("Row"))]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    #[test]
    fn a_function_called_but_never_imported_is_unbound() {
        // #1815's shape: a codec helper called without its import.
        let p = program(vec![
            import(&["type __JsonValue", "type Amount"]),
            codec("__serialise_Quote", "Amount", "__serialise_Amount"),
        ]);
        assert_eq!(unbound_names(&p), ["__serialise_Amount"]);
    }

    #[test]
    fn a_bare_export_list_uses_each_name_it_exports() {
        // #1852: a Workers context's `export { … };` of the commons codecs it
        // imports. A name exported without being imported is unbound.
        let p = program(vec![
            import(&["__serialise_Money"]),
            TsStmt::decl(
                TsDecl::ExportNames {
                    names: vec![
                        "__serialise_Money".to_string(),
                        "__deserialise_Money".to_string(),
                    ],
                },
                None,
            ),
        ]);
        assert_eq!(unbound_names(&p), ["__deserialise_Money"]);
    }

    #[test]
    fn imports_bind_their_local_names_including_aliases_and_type_only() {
        let p = program(vec![
            import(&[
                "type __JsonValue",
                "Note as __CommonsNote",
                "__serialise_Base",
            ]),
            codec("__serialise_Note", "__CommonsNote", "__serialise_Base"),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    #[test]
    fn a_namespace_import_binds_the_qualifier_of_a_qualified_name() {
        let p = program(vec![
            TsStmt::decl(
                TsDecl::ImportNamespace {
                    type_only: true,
                    alias: "handlers".to_string(),
                    from: "./handlers.js".to_string(),
                },
                None,
            ),
            TsStmt::let_stmt(
                TsBindingName::Ident("o".to_string()),
                Some(TsType::named("handlers.Order")),
                None,
                None,
            ),
            TsStmt::let_stmt(
                TsBindingName::Ident("p".to_string()),
                Some(TsType::named("other.Order")),
                None,
                None,
            ),
        ]);
        assert_eq!(unbound_names(&p), ["other"]);
    }

    #[test]
    fn declarations_in_the_module_bind_their_names() {
        let p = program(vec![
            import(&["type __JsonValue"]),
            TsStmt::decl(
                TsDecl::TypeAlias {
                    name: "Note".to_string(),
                    type_params: vec![],
                    ty: TsType::named("string"),
                },
                None,
            ),
            codec("__serialise_Note", "Note", "__serialise_Note"),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    #[test]
    fn parameters_locals_and_destructured_names_are_bound() {
        let p = program(vec![TsStmt::decl(
            TsDecl::Function {
                name: "f".to_string(),
                generics: vec!["T".to_string()],
                params: vec![TsParam::new(
                    "path: string = \"$\"".to_string(),
                    Some(TsType::named("T")),
                )],
                return_type: None,
                body: vec![
                    TsStmt::const_stmt(
                        TsBindingName::ObjectPattern(vec!["a".to_string(), "b: c".to_string()]),
                        None,
                        TsExpr::Ident("path".to_string()),
                        None,
                    ),
                    TsStmt::for_of(
                        "[k, v]",
                        TsExpr::Ident("c".to_string()),
                        TsStmt::block(
                            vec![
                                TsStmt::expr_stmt(TsExpr::Ident("k.length".to_string()), None),
                                TsStmt::expr_stmt(TsExpr::Ident("v".to_string()), None),
                                TsStmt::expr_stmt(TsExpr::Ident("a".to_string()), None),
                            ],
                            None,
                        ),
                        None,
                    ),
                ],
                is_async: false,
                inline: false,
            },
            None,
        )]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    #[test]
    fn globals_and_literals_are_bound() {
        let p = program(vec![TsStmt::expr_stmt(
            TsExpr::Arrow {
                params: vec![],
                return_type: Some(TsType::named_with_args(
                    "Promise",
                    vec![TsType::named("globalThis.Response")],
                )),
                generics: vec![],
                body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Call {
                    callee: Box::new(TsExpr::Ident("JSON.stringify".to_string())),
                    args: vec![TsExpr::Ident("false".to_string())],
                }))),
                is_async: true,
            },
            None,
        )]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    #[test]
    fn opaque_text_binds_what_it_declares_or_imports_line_by_line() {
        let p = program(vec![
            TsStmt::raw(
                "import { type __JsonValue, Amount as __A } from \"./m.js\";\n\
                 export const __serialise_Amount = (v: __A): __JsonValue => v;",
                None,
            ),
            TsStmt::verbatim(
                VerbatimOrigin::NotYetConverted,
                "export type Note = string;\nexport function __serialise_Note(n: Note) { return n; }",
                None,
            ),
            codec("__serialise_Quote", "Note", "__serialise_Amount"),
            codec("__serialise_Other", "__A", "__serialise_Note"),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    #[test]
    fn a_cast_in_opaque_text_is_not_mistaken_for_an_import_alias() {
        // The first version scanned for `import` anywhere and read `x as Note`
        // in a codec body as an alias, hiding #1829.
        let p = program(vec![
            import(&["type __JsonValue", "__serialise_Base"]),
            TsStmt::raw("// imports are below\nconst y = { z: x as Note };", None),
            codec("__serialise_Note", "Note", "__serialise_Base"),
        ]);
        assert_eq!(unbound_names(&p), ["Note"]);
    }

    /// 1A.21: a `DefaultIife`'s default is a name use, so a name only the default uses is unbound
    /// when nothing declares it.
    #[test]
    fn a_default_iife_default_is_a_name_use() {
        let iife = |name: &str| TsExpr::DefaultIife {
            name: "p".to_string(),
            default: Box::new(TsExpr::Ident(name.to_string())),
            return_type: TsType::named("string"),
            body: vec![TsStmt::return_stmt(
                Some(TsExpr::Ident("p".to_string())),
                None,
            )],
        };
        let p = program(vec![TsStmt::expr_stmt(iife("missing"), None)]);
        assert_eq!(unbound_names(&p), ["missing"]);
        let p = program(vec![TsStmt::expr_stmt(iife("Err"), None), import(&["Err"])]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    #[test]
    fn a_generic_type_applied_in_opaque_text_must_be_bound() {
        // #1977's shape: a kernel helper typed with a generic record the
        // module never imports.
        let p = program(vec![TsStmt::raw(
            "return ((__xs: readonly MapEntry<string, number>[], __acc: number) => __acc)(all, 0);",
            None,
        )]);
        assert_eq!(unbound_names(&p), ["MapEntry"]);
    }

    #[test]
    fn opaque_generics_skip_members_comparisons_globals_and_literals() {
        let p = program(vec![
            import(&["type Page"]),
            TsStmt::raw(
                "const a: globalThis.Promise<void> = f();\n\
                 const b: import(\"./m.js\").Note<string> = g();\n\
                 const c = __i < __xs.length && N < 3;\n\
                 const d: Record<string, Page<number>> = {};\n\
                 const e = \"Missing<T>\" + `Other<${x}>`; // Gone<T>\n\
                 /* Absent<T> */",
                None,
            ),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    #[test]
    fn a_name_used_only_inside_opaque_text_is_not_checked() {
        let p = program(vec![TsStmt::raw("return __serialise_Missing(x);", None)]);
        assert!(unbound_names(&p).is_empty());
    }

    #[test]
    fn a_type_only_export_entry_uses_its_name() {
        // #2036: `export { type Batch };` of an imported consumed type.
        let export = |name: &str| {
            TsStmt::decl(
                TsDecl::ExportNames {
                    names: vec![format!("type {name}")],
                },
                None,
            )
        };
        let p = program(vec![import(&["Batch"]), export("Batch"), export("Missing")]);
        assert_eq!(unbound_names(&p), ["Missing"]);
    }

    #[test]
    fn namespace_members_are_reported_with_their_specifier() {
        // #2036: `handlers.Batch` in type position and
        // `handlers.__deserialise_Batch(…)` in value position; a qualified
        // name whose head is no namespace import is not a member use.
        let p = program(vec![
            TsStmt::decl(
                TsDecl::ImportNamespace {
                    type_only: false,
                    alias: "handlers".to_string(),
                    from: "./handlers.js".to_string(),
                },
                None,
            ),
            TsStmt::let_stmt(
                TsBindingName::Ident("b".to_string()),
                Some(TsType::named("handlers.Batch<number>")),
                Some(TsExpr::Call {
                    callee: Box::new(TsExpr::Member {
                        object: Box::new(TsExpr::Ident("handlers".to_string())),
                        property: "__deserialise_Batch".to_string(),
                    }),
                    args: vec![],
                }),
                None,
            ),
            TsStmt::let_stmt(
                TsBindingName::Ident("c".to_string()),
                Some(TsType::named("globalThis.Response")),
                None,
                None,
            ),
        ]);
        let uses: Vec<(String, String)> = namespace_member_uses(&p)
            .into_iter()
            .map(|u| (u.from, u.member))
            .collect();
        assert_eq!(
            uses,
            [
                ("./handlers.js".to_string(), "Batch".to_string()),
                (
                    "./handlers.js".to_string(),
                    "__deserialise_Batch".to_string()
                ),
            ]
        );
    }

    #[test]
    fn module_exports_reads_structured_and_opaque_exports_and_star_re_exports() {
        let p = program(vec![
            TsStmt::decl(
                TsDecl::ExportNames {
                    names: vec!["type Batch".to_string(), "a as b".to_string()],
                },
                None,
            ),
            codec("__serialise_Note", "Note", "__serialise_Name"),
            TsStmt::raw(
                "export async function run() {}\nexport { x, type Y };\nexport type { A, B as C };\nexport type D = number;\nconst hidden = 1;",
                None,
            ),
            TsStmt::decl(
                TsDecl::ReExportAll {
                    from: "./handlers/a.js".to_string(),
                },
                None,
            ),
        ]);
        let exports = module_exports(&p);
        let names: Vec<&str> = exports.names.iter().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "A",
                "Batch",
                "C",
                "D",
                "Y",
                "__serialise_Note",
                "b",
                "run",
                "x"
            ]
        );
        assert_eq!(exports.star_from, ["./handlers/a.js"]);
    }

    #[test]
    fn a_type_only_import_used_as_a_value_is_reported() {
        // #2045: `import { type Status }` then `Status.Active` is TS1361; a
        // type position, or a name also bound as a value, is not.
        let member = |object: &str| TsExpr::Member {
            object: Box::new(TsExpr::Ident(object.to_string())),
            property: "Active".to_string(),
        };
        let p = program(vec![
            import(&["type Status", "type Batch", "Kept"]),
            TsStmt::let_stmt(
                TsBindingName::Ident("a".to_string()),
                Some(TsType::named("Batch")),
                Some(member("Status")),
                None,
            ),
            TsStmt::let_stmt(
                TsBindingName::Ident("b".to_string()),
                None,
                Some(member("Kept")),
                None,
            ),
        ]);
        assert_eq!(type_only_value_uses(&p), ["Status"]);
        assert!(unbound_names(&p).is_empty());
    }

    /// 1A.20: a name that only a `TypeApp`'s type argument mentions is a use, so
    /// an import it needs is not reported as missing or dropped.
    #[test]
    fn a_type_application_argument_is_a_name_use() {
        let p = program(vec![
            import(&["type Batch"]),
            TsStmt::let_stmt(
                TsBindingName::Ident("a".to_string()),
                None,
                Some(TsExpr::Call {
                    callee: Box::new(TsExpr::TypeApp {
                        callee: Box::new(TsExpr::Ident("f".to_string())),
                        type_args: vec![TsType::named("Batch")],
                    }),
                    args: vec![],
                }),
                None,
            ),
        ]);
        assert_eq!(unbound_names(&p), ["f"]);
        assert!(type_only_value_uses(&p).is_empty());
    }

    #[test]
    fn value_imports_skip_type_only_entries_and_declarations() {
        let p = program(vec![
            import(&["type Status", "Batch"]),
            TsStmt::decl(
                TsDecl::Import {
                    type_only: true,
                    names: vec!["Receipt".to_string()],
                    from: "./r.js".to_string(),
                },
                None,
            ),
        ]);
        assert_eq!(
            value_imports(&p),
            [("./m.js".to_string(), "Batch".to_string())]
        );
    }

    /// 1A.35a part B: the new statements and expressions bind what they declare and use what
    /// they mention, so a program built from them reports only the genuinely unbound names.
    #[test]
    fn the_part_b_nodes_bind_and_use_their_names() {
        let id = |n: &str| TsExpr::Ident(n.to_string());
        let generator = TsExpr::Function {
            is_async: true,
            is_generator: true,
            params: vec![TsParam::array_pattern(
                vec!["k".to_string(), "v".to_string()],
                None,
            )],
            return_type: None,
            body: vec![
                TsStmt::let_list(
                    vec![
                        TsDeclarator::new("a"),
                        TsDeclarator::new("b").typed(TsType::named("Missing")),
                    ],
                    None,
                ),
                TsStmt::labelled(
                    "LOOP",
                    TsStmt::while_stmt(
                        id("a"),
                        TsStmt::block(
                            vec![
                                TsStmt::for_await_of(
                                    "e",
                                    id("src"),
                                    TsStmt::expr_stmt(
                                        TsExpr::Sequence(vec![
                                            TsExpr::Yield(Box::new(id("e"))),
                                            TsExpr::OptionalCall {
                                                callee: Box::new(id("k")),
                                                args: vec![id("v"), id("b")],
                                            },
                                        ]),
                                        None,
                                    ),
                                    None,
                                ),
                                TsStmt::continue_label_stmt("LOOP", None),
                            ],
                            None,
                        ),
                        None,
                    ),
                    None,
                ),
            ],
        };
        let p = program(vec![TsStmt::expr_stmt(generator, None)]);
        // `a`, `b`, `k`, `v` and `e` are bound inside; `src` and the type `Missing` are not.
        assert_eq!(unbound_names(&p), ["Missing", "src"]);
    }

    /// 1A.38a: a default import with named imports binds the default alias and each
    /// named local, so a use of either is bound; a name it does not bring is not.
    #[test]
    fn a_default_import_with_named_imports_binds_all_of_them() {
        let imports = TsStmt::decl(
            TsDecl::ImportDefault {
                alias: "__entry0".to_string(),
                named: vec!["isMiss as __miss0".to_string()],
                from: "./a.js".to_string(),
            },
            None,
        );
        let uses = |names: &[&str]| {
            TsStmt::expr_stmt(
                TsExpr::array(names.iter().map(|n| TsExpr::Ident(n.to_string())).collect()),
                None,
            )
        };
        let p = program(vec![imports.clone(), uses(&["__entry0", "__miss0"])]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
        // The imported name is `isMiss`; only its local alias is bound here.
        let p = program(vec![imports, uses(&["isMiss"])]);
        assert_eq!(unbound_names(&p), ["isMiss"]);
    }

    /// 1A.38a: `MultilineConditional` is walked like `Conditional`: a name in its
    /// test, consequent or alternate is a use.
    #[test]
    fn a_multiline_conditional_uses_the_names_in_all_three_arms() {
        let id = |n: &str| TsExpr::Ident(n.to_string());
        let p = program(vec![TsStmt::expr_stmt(
            TsExpr::multiline_conditional(id("t"), id("c"), id("a")),
            None,
        )]);
        assert_eq!(unbound_names(&p), ["a", "c", "t"]);
        let p = program(vec![
            import(&["t", "c", "a"]),
            TsStmt::expr_stmt(
                TsExpr::multiline_conditional(id("t"), id("c"), id("a")),
                None,
            ),
        ]);
        assert!(unbound_names(&p).is_empty(), "{:?}", unbound_names(&p));
    }

    /// 1A.38b: a parameter's default is a use of the names it mentions.
    #[test]
    fn a_name_in_a_parameter_default_is_unbound_unless_imported() {
        let f = || {
            TsStmt::decl(
                TsDecl::Function {
                    name: "f".to_string(),
                    generics: vec![],
                    params: vec![
                        TsParam::new("a", None).with_default(TsExpr::Ident("fallback".to_string())),
                    ],
                    return_type: None,
                    body: vec![],
                    is_async: false,
                    inline: false,
                },
                None,
            )
        };
        assert_eq!(unbound_names(&program(vec![f()])), ["fallback"]);
        assert!(unbound_names(&program(vec![import(&["fallback"]), f()])).is_empty());
    }

    /// 1A.39b (Decision D): an identifier path passes; expression text held in an `Ident` is
    /// reported, as is a binding name that is not an identifier.
    #[test]
    fn text_idents_reports_expression_text_and_passes_identifier_paths() {
        let id = |n: &str| TsExpr::Ident(n.to_string());
        let p = program(vec![
            TsStmt::expr_stmt(id("globalThis.Number"), None),
            TsStmt::expr_stmt(id("this.state"), None),
            TsStmt::expr_stmt(id("f(a) as K"), None),
            TsStmt::expr_stmt(id("a..b"), None),
            TsStmt::const_stmt(
                TsBindingName::Ident("x y".to_string()),
                None,
                id("ok"),
                None,
            ),
        ]);
        assert_eq!(text_idents(&p), vec!["a..b", "f(a) as K", "x y"]);
    }
}
