//! #2476: the names the emitter gives Bynk locals in the TypeScript it writes.
//!
//! A debugger sees the emitted names, not Bynk's, so a feature that points a
//! debug session at a Bynk binding (inline values) needs the emitter's mapping.
//! It lives here, below both the emitter and the language server, so neither
//! re-derives it: the emitter calls [`ts_ident`] when it declares a binding,
//! and [`emitted_local_name`] mirrors `Emitter::bind_local_name`'s choice for a
//! `let`.

use crate::locals::{LocalBinding, LocalKind};

/// The TypeScript spelling of a user identifier in a *binding or reference*
/// position (params, locals, function names, import names). Bynk identifiers
/// that are illegal as TS binding names — the JS reserved words plus the
/// strict-mode/module sets (emitted modules are always strict ESM) — and
/// names the emitter itself introduces alongside user bindings (`deps`) are
/// renamed into the generated-name namespace (`__id_<name>`), which the
/// parser keeps free of user identifiers. Property/field names never pass
/// through here: reserved words are legal there, and record field names are
/// wire format.
pub fn ts_ident(name: &str) -> String {
    if let Some(qualified) = use_qualified_name(name) {
        return qualified;
    }
    const RESERVED: &[&str] = &[
        // ES reserved words.
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "new",
        "null",
        "return",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        // Strict-mode reserved (emitted modules are always strict).
        "implements",
        "interface",
        "let",
        "package",
        "private",
        "protected",
        "public",
        "static",
        "yield",
        // Module-code reserved.
        "await",
        // Illegal binding targets in strict mode.
        "arguments",
        "eval",
        // #1653: the emitted code reaches host globals through `globalThis`
        // (`globalThis.JSON`), so no user binding may take that name.
        "globalThis",
        // Generated identifiers a user binding may sit next to: handler
        // signatures append a `deps` parameter, so a user param named `deps`
        // would otherwise duplicate it.
        "deps",
    ];
    if RESERVED.contains(&name) {
        format!("__id_{name}")
    } else {
        name.to_string()
    }
}

/// #4397: the identifier a unit binds a commons' full name (`lib.y`) to, as one
/// namespace import (`import * as __use_lib_y from "./lib/y.js"`), so a name read
/// through that qualifier (`lib.y.Status`) is `__use_lib_y.Status` in TypeScript.
/// A TypeScript `namespace` that hung the dotted name on a `lib` object would read
/// as written, but is not erasable, which `--inspect` and the in-browser eval need.
/// The `__` prefix is the generated-name space the parser keeps free of user names.
pub fn use_qualifier_binding(qualifier: &str) -> String {
    ["__use_", &qualifier.replace('.', "_")].concat()
}

/// #4397: `name` as TypeScript when it is read through a commons' full name
/// (`lib.y.Status` → `__use_lib_y.Status`), and `None` for any other name. An alias
/// qualifier (`Y.Status`) has one dot and is already a namespace import's name.
pub fn use_qualified_name(name: &str) -> Option<String> {
    let (qualifier, last) = name.rsplit_once('.')?;
    qualifier
        .contains('.')
        .then(|| [&use_qualifier_binding(qualifier), ".", last].concat())
}

/// The JS identifier the emitter declares for `binding`, or `None` when it
/// cannot be known from the source alone.
///
/// A parameter or pattern binder is always its [`ts_ident`] name. A `let` is
/// too, unless the name is already bound in an enclosing scope (a re-`let`):
/// the emitter then declares a throwaway `__r<N>`, numbered by a counter that
/// runs across the whole emitted body, so there is no name to look up and a
/// guess would show another variable's value.
pub fn emitted_local_name(binding: &LocalBinding, locals: &[LocalBinding]) -> Option<String> {
    if binding.kind == LocalKind::Let && binding.name == "_" {
        return None; // always a fresh throwaway, and never read
    }
    if binding.kind == LocalKind::Let
        && locals.iter().any(|b| {
            b.name == binding.name
                && b.def_span.start < binding.def_span.start
                && b.scope.start <= binding.def_span.start
                && binding.def_span.start <= b.scope.end
        })
    {
        return None;
    }
    Some(ts_ident(&binding.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::span::Span;

    fn binding(name: &str, def: usize, kind: LocalKind, scope: (usize, usize)) -> LocalBinding {
        LocalBinding {
            name: name.into(),
            def_span: Span::new(def, def + name.len()),
            kind,
            ty: "Int".into(),
            scope: Span::new(scope.0, scope.1),
        }
    }

    #[test]
    fn reserved_words_are_renamed_and_others_kept() {
        assert_eq!(ts_ident("class"), "__id_class");
        assert_eq!(ts_ident("deps"), "__id_deps");
        assert_eq!(ts_ident("total"), "total");
    }

    /// #4397: a commons' full name is bound as one namespace import, so the name read
    /// through it is a property of that binding; an alias (`Y.Status`) is already one.
    #[test]
    fn a_full_name_qualified_name_reads_through_its_namespace_import() {
        assert_eq!(ts_ident("lib.y.Status"), "__use_lib_y.Status");
        assert_eq!(ts_ident("a.b.c.Status"), "__use_a_b_c.Status");
        assert_eq!(ts_ident("Y.Status"), "Y.Status");
        assert_eq!(use_qualifier_binding("lib.y"), "__use_lib_y");
    }

    #[test]
    fn a_plain_let_and_a_param_keep_their_names() {
        let locals = vec![
            binding("n", 5, LocalKind::Param, (20, 60)),
            binding("class", 26, LocalKind::Let, (34, 60)),
        ];
        assert_eq!(
            emitted_local_name(&locals[0], &locals).as_deref(),
            Some("n")
        );
        assert_eq!(
            emitted_local_name(&locals[1], &locals).as_deref(),
            Some("__id_class")
        );
    }

    #[test]
    fn a_re_let_has_no_knowable_name_but_the_first_binding_does() {
        let locals = vec![
            binding("x", 26, LocalKind::Let, (34, 90)),
            binding("x", 40, LocalKind::Let, (48, 90)),
        ];
        assert_eq!(
            emitted_local_name(&locals[0], &locals).as_deref(),
            Some("x")
        );
        assert_eq!(emitted_local_name(&locals[1], &locals), None);
    }

    #[test]
    fn a_let_shadowing_a_param_is_renamed_by_the_emitter() {
        let locals = vec![
            binding("n", 5, LocalKind::Param, (20, 60)),
            binding("n", 26, LocalKind::Let, (34, 60)),
        ];
        assert_eq!(
            emitted_local_name(&locals[0], &locals).as_deref(),
            Some("n")
        );
        assert_eq!(emitted_local_name(&locals[1], &locals), None);
    }

    #[test]
    fn sibling_scopes_do_not_shadow_each_other() {
        let locals = vec![
            binding("x", 26, LocalKind::Let, (34, 50)),
            binding("x", 60, LocalKind::Let, (68, 90)),
        ];
        assert_eq!(
            emitted_local_name(&locals[1], &locals).as_deref(),
            Some("x")
        );
    }

    #[test]
    fn a_discarded_let_has_no_name() {
        let locals = vec![binding("_", 26, LocalKind::Let, (34, 50))];
        assert_eq!(emitted_local_name(&locals[0], &locals), None);
    }
}
