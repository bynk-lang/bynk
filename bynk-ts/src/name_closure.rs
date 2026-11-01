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
//! **Scope is module-wide, not lexical.** A parameter, local or generic
//! parameter declared anywhere in the module counts as bound everywhere in
//! it, and a destructuring pattern binds every identifier in it. Each
//! approximation can only hide a defect, never invent one.
//!
//! **Opaque text is mostly a blind spot.** `Raw` and `Verbatim` statements
//! and `VerbatimExpr` expressions are pre-rendered TypeScript. A name they
//! *declare* or *import* is found line by line (a line starting `import`,
//! `type`, `interface`, `class`, `enum`, `function`, `const`, `let` or `var`).
//! Of the names they *use*, only a generic type application (`Name<…>`) is
//! checked (#1977: a kernel helper's parameter types are opaque text); any
//! other use is not. Measured over the positive corpus at #1831, opaque text was 0% of
//! `index.ts`/`compose.ts`, about 14% of unit modules and about 26% of
//! `handlers.ts` and test modules: chiefly service handler bodies, which
//! `bynk-emit` still renders to text. That part of a module stays `tsc`'s
//! job (#1823 lived there).

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
    let mut w = Walk::default();
    for s in &program.stmts {
        w.stmt(s);
    }
    let types = w
        .used_types
        .iter()
        .filter(|n| !w.bound.contains(*n) && !GLOBAL_TYPES.contains(&n.as_str()));
    let values = w
        .used_values
        .iter()
        .filter(|n| !w.bound.contains(*n) && !GLOBAL_VALUES.contains(&n.as_str()));
    let all: BTreeSet<String> = types.chain(values).cloned().collect();
    all.into_iter().collect()
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
            TsStmtKind::Raw(text) | TsStmtKind::Verbatim { text, .. } => {
                opaque_exports(text, &mut out.names)
            }
            _ => {}
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
    /// #2036: each namespace import's alias and specifier.
    namespaces: BTreeMap<String, String>,
    /// #2036: every `head.member` named in type or value position.
    qualified: BTreeSet<(String, String)>,
    /// #2045: names bound only by a type-only import (`import { type X }`,
    /// `import type { X }`, `import type * as ns`), and the names bound any
    /// other way. A value use of the first and not the second is `TS1361`.
    type_only: BTreeSet<String>,
    value_bound: BTreeSet<String>,
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
            TsBindingName::Ident(n) => self.bind(n),
            TsBindingName::ObjectPattern(names) => self.bind_pattern(names),
        }
    }

    fn use_value(&mut self, text: &str) {
        if let Some(ident) = leading_ident(text) {
            self.used_values.insert(ident.to_string());
        }
    }

    fn bind_all(&mut self, names: &[String]) {
        for n in names {
            self.bind(n);
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
                    if !after_dot && b.is_ascii_uppercase() && bytes.get(i) == Some(&b'<') {
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
                {
                    self.used_types.insert(head.to_string());
                }
                for a in type_args {
                    self.ty(a);
                }
            }
            TsType::Array { element, .. } => self.ty(element),
            TsType::Object(members) => self.members(members),
            TsType::Fn { params, ret } => {
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
                    self.bind_all(generics);
                    self.params(params);
                    self.ty(ret);
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
            self.bind(&p.name);
            if let Some(t) = &p.ty {
                self.ty(t);
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
            TsStmtKind::Verbatim { text, .. } | TsStmtKind::Raw(text) => self.opaque(text),
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
            } => {
                self.bind(binding);
                self.expr(iter);
                self.stmt(body);
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
            } => {
                if let Some(p) = catch_param {
                    self.bind(p);
                }
                self.stmt(try_block);
                self.stmt(catch_block);
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
                    if let Some(t) = &case.test {
                        self.expr(t);
                    }
                    self.stmts(&case.body);
                }
            }
            TsStmtKind::Continue
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
            TsDecl::ImportDefault { alias, .. } => self.bind(alias),
            TsDecl::ReExport { .. } | TsDecl::ReExportAll { .. } => {}
            // `export { a, b };` exports names this module binds (#1852),
            // so each is a use: one never imported or declared is unbound.
            // A `type X` entry (#2036) uses `X`, and `X as Y` uses `X`.
            TsDecl::ExportNames { names } => {
                for n in names {
                    let n = n.trim();
                    match n.strip_prefix("type ") {
                        Some(t) => {
                            if let Some(ident) = leading_ident(t) {
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
                self.bind_all(type_params);
                self.members(members);
            }
            TsDecl::TypeAlias {
                name,
                type_params,
                ty,
            } => {
                self.bind(name);
                self.bind_all(type_params);
                self.ty(ty);
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
                fields,
                constructor,
                methods,
            } => {
                self.bind(name);
                for f in fields {
                    self.ty(&f.ty);
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
                self.bind_all(generics);
                self.params(params);
                self.opt_ty(return_type);
                self.stmts(body);
            }
            TsDecl::ExportDefault(e) => self.expr(e),
        }
    }

    fn expr(&mut self, expr: &TsExpr) {
        match expr {
            TsExpr::Ident(name) => self.use_value(name),
            TsExpr::Lit(_) => {}
            TsExpr::VerbatimExpr(text, _) => self.opaque(text),
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
                self.bind_all(generics);
                self.params(params);
                self.opt_ty(return_type);
                match body.as_ref() {
                    TsArrowBody::Expr(e) => self.expr(e),
                    TsArrowBody::Block(stmts) => self.stmts(stmts),
                }
            }
            TsExpr::Call { callee, args } | TsExpr::New { callee, args } => {
                self.expr(callee);
                for a in args {
                    self.expr(a);
                }
            }
            TsExpr::Object { entries, .. } => {
                for e in entries {
                    match e {
                        TsObjectEntry::Prop(_, v) | TsObjectEntry::Spread(v) => self.expr(v),
                        TsObjectEntry::Shorthand(name) => self.use_value(name),
                        TsObjectEntry::Method { params, body, .. } => {
                            self.params(params);
                            self.stmts(body);
                        }
                    }
                }
            }
            TsExpr::Array { items, .. } => {
                for i in items {
                    self.expr(i);
                }
            }
            TsExpr::TemplateLit { exprs, .. } => {
                for e in exprs {
                    self.expr(e);
                }
            }
            TsExpr::Await(e) | TsExpr::Paren(e) | TsExpr::Unary { expr: e, .. } => self.expr(e),
            TsExpr::As { expr, ty } => {
                self.expr(expr);
                self.ty(ty);
            }
            TsExpr::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            TsExpr::Conditional {
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
    use crate::program::{TsArrowBody, VerbatimOrigin};

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
                params: vec![TsParam {
                    name: "value".to_string(),
                    ty: Some(TsType::named(param_ty)),
                    optional: false,
                }],
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
                params: vec![TsParam {
                    name: "path: string = \"$\"".to_string(),
                    ty: Some(TsType::named("T")),
                    optional: false,
                }],
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
}
