//! v0.31 (ADR 0064): the local-binding sink.
//!
//! Records each local binding — `let`/`let <-`, lambda/fn/handler parameters,
//! and match-arm pattern bindings — with its **lexical scope range**, so the
//! LSP can offer/navigate locals (the recurring deferral: the v0.25 index, the
//! v0.27 hints, the v0.28 tokens, and v0.30.2 completion all stop at top-level
//! symbols for want of a scope-at-offset query).
//!
//! Mirrors [`HintSink`](crate::hints::HintSink): a `&mut` sink threaded through
//! the checker, recording at the binding sites as types are computed — so it
//! survives a transient error at the sites the checker still reaches, and (like
//! hints) it is not part of the `Ok(TypedCommons)` payload. Scope ranges are
//! taken from the enclosing block/body/arm span the checker already has at each
//! binding site, not re-derived — so nesting and shadowing are the checker's
//! own (tested) scoping, resolved in [`locals_at`]. Only synthetic files are
//! muted (locals serve completion/navigation in test files too).

use bynk_syntax::span::Span;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What kind of local binding this is — the distinction hover renders as its
/// `let`/`param` prefix (v0.122, editor-currency slice 1). Every
/// [`LocalsSink::record`] site knows its kind statically; this is a threaded
/// argument, not inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    /// A `let =` / `let <-` binding.
    Let,
    /// A parameter — fn / handler / op / lambda, or an actor `by`-binder.
    Param,
}

/// One local binding: its name, the binding-name span (the def site), its
/// [`kind`](LocalKind), its rendered type (Bynk surface syntax, as hints
/// render — no `Ty` on the surface), and the source range over which it is in
/// scope.
#[derive(Debug, Clone)]
pub struct LocalBinding {
    pub name: String,
    pub def_span: Span,
    pub kind: LocalKind,
    pub ty: String,
    pub scope: Span,
}

/// Project-relative source path → that file's local bindings, in source order.
pub type FileLocals = HashMap<PathBuf, Vec<LocalBinding>>;

/// Records local bindings per file. A fresh sink records nothing until
/// [`enter_file`](Self::enter_file) attributes it.
#[derive(Debug, Default)]
pub struct LocalsSink {
    files: FileLocals,
    file: Option<PathBuf>,
    /// Set for synthetic (toolchain-injected) files only.
    muted: bool,
}

impl LocalsSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enter a per-file recording context.
    pub fn enter_file(&mut self, file: &Path, muted: bool) {
        self.file = Some(file.to_path_buf());
        self.muted = muted;
    }

    /// Record a binding `name` defined at `def_span`, of the given `kind` and
    /// rendered type `ty`, in scope over `scope`. Dropped when muted or before
    /// any `enter_file`.
    pub fn record(
        &mut self,
        name: String,
        def_span: Span,
        kind: LocalKind,
        ty: String,
        scope: Span,
    ) {
        if self.muted {
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        let entries = self.files.entry(file.clone()).or_default();
        // #2701: an `is` binding in a left-nested `&&` chain is in scope in
        // every later operand, so the checker records it once per operand: one
        // binding, whose scope grows to cover them all.
        if let Some(prev) = entries
            .iter_mut()
            .rev()
            .take(8)
            .find(|b| b.def_span == def_span && b.name == name)
        {
            prev.scope = prev.scope.merge(scope);
            return;
        }
        entries.push(LocalBinding {
            name,
            def_span,
            kind,
            ty,
            scope,
        });
    }

    /// Drain the recorded bindings, each file's entries ordered by def span.
    pub fn take_files(&mut self) -> FileLocals {
        let mut files = std::mem::take(&mut self.files);
        for locals in files.values_mut() {
            locals.sort_by_key(|b| (b.def_span.start, b.def_span.end));
        }
        files
    }
}

/// The local bindings in scope at `offset`, deduplicated by name with the
/// **innermost/latest** definition winning (shadowing) — the completion and
/// navigation query.
pub fn locals_at(entries: &[LocalBinding], offset: usize) -> Vec<&LocalBinding> {
    let mut by_name: HashMap<&str, &LocalBinding> = HashMap::new();
    for b in entries {
        if b.scope.start <= offset && offset <= b.scope.end {
            // Later def (larger start) shadows an earlier same-name binding.
            by_name
                .entry(&b.name)
                .and_modify(|cur| {
                    if b.def_span.start >= cur.def_span.start {
                        *cur = b;
                    }
                })
                .or_insert(b);
        }
    }
    let mut out: Vec<&LocalBinding> = by_name.into_values().collect();
    out.sort_by_key(|b| (b.def_span.start, b.def_span.end));
    out
}

/// The single binding whose **def-name** span covers `offset` — the
/// definition-site query (for references/rename on a local's declaration).
pub fn binding_at_def(entries: &[LocalBinding], offset: usize) -> Option<&LocalBinding> {
    entries
        .iter()
        .find(|b| b.def_span.start <= offset && offset <= b.def_span.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(name: &str, def: usize, scope: (usize, usize)) -> LocalBinding {
        LocalBinding {
            name: name.to_string(),
            def_span: Span::new(def, def + name.len()),
            kind: LocalKind::Let,
            ty: "Int".to_string(),
            scope: Span::new(scope.0, scope.1),
        }
    }

    #[test]
    fn locals_at_filters_by_scope_and_resolves_shadowing() {
        let entries = vec![
            b("x", 0, (3, 50)),   // outer x
            b("y", 10, (13, 30)), // y, narrower scope
            b("x", 20, (23, 50)), // inner x shadows the outer
        ];
        // Before y's scope: just the outer x.
        assert_eq!(names(&locals_at(&entries, 5)), vec!["x"]);
        // Inside y's scope, before the inner x: x (outer) + y.
        assert_eq!(names(&locals_at(&entries, 15)), vec!["x", "y"]);
        // After the inner x and past y's scope: one x (the inner shadows).
        let at = locals_at(&entries, 40);
        assert_eq!(names(&at), vec!["x"]);
        assert_eq!(at[0].def_span.start, 20, "latest x wins");
    }

    #[test]
    fn binding_at_def_finds_the_declaration_under_the_cursor() {
        let entries = vec![b("total", 4, (12, 40))];
        assert!(binding_at_def(&entries, 6).is_some()); // on the name
        assert!(binding_at_def(&entries, 20).is_none()); // a use site, not the def
    }

    fn names(bs: &[&LocalBinding]) -> Vec<String> {
        bs.iter().map(|b| b.name.clone()).collect()
    }
}
