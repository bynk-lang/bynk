//! v0.27 (ADR 0056): the inlay-hint sink.
//!
//! A curated set of inferred-type hints harvested from the checker as it
//! computes each binding's type — `let` / `let <-` bindings and lambda
//! parameters whose annotation is absent. The sink mirrors [`RefSink`]
//! (`index.rs`): it is a `&mut` parameter threaded through the checker
//! entry points, NOT part of the `Ok(TypedCommons)` payload `check_record`
//! drops on error — so hints persist through a transient type error at
//! every site the checker still reaches.
//!
//! Labels are pre-rendered (`": " + Ty::display()`) so no `Ty` leaks
//! through the public surface; spans are bare byte offsets into the file
//! the sink was attributed to via [`HintSink::enter_file`].
//!
//! [`RefSink`]: crate::index::RefSink

use bynk_syntax::span::Span;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// v0.39 (ADR 0072): the inlay-hint kind, which drives the LSP rendering —
/// anchor side, `InlayHintKind`, and padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintKind {
    /// An inferred type, rendered after a name (`x`**`: Int`**, or a generic
    /// call's `identity`**`[Int]`**). Anchored at the span's end.
    Type,
    /// A parameter name at a call argument (**`count:`** `5`). Anchored at the
    /// argument span's start.
    Parameter,
}

/// One inlay hint: a pre-rendered label at a span, plus its [`HintKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub span: Span,
    pub label: String,
    pub kind: HintKind,
}

/// Project-relative source path → that file's hints, span-ordered.
pub type FileHints = HashMap<PathBuf, Vec<Hint>>;

/// Records inferred-type hints per file. A fresh sink records nothing
/// until [`enter_file`](Self::enter_file) attributes it.
#[derive(Debug, Default)]
pub struct HintSink {
    files: FileHints,
    file: Option<PathBuf>,
    /// Set for synthetic (toolchain-injected) and test/integration files:
    /// hints are discarded — neither surfaces in an editor.
    muted: bool,
}

impl HintSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enter a per-file recording context.
    pub fn enter_file(&mut self, file: &Path, muted: bool) {
        self.file = Some(file.to_path_buf());
        self.muted = muted;
    }

    /// Record an inferred-**type** hint at `span` (binding types, generic-call
    /// type arguments). Dropped when muted or before any `enter_file`.
    pub fn record(&mut self, span: Span, label: String) {
        // #2727: the bottom type is not source syntax, so a hint carrying it
        // could be accepted as an annotation that does not parse.
        if label.contains(crate::checker::NEVER_DISPLAY) {
            return;
        }
        self.push(span, label, HintKind::Type);
    }

    /// v0.39 (ADR 0072): record a **parameter-name** hint at a call argument's
    /// span (`count:` before `5`).
    pub fn record_param(&mut self, span: Span, label: String) {
        self.push(span, label, HintKind::Parameter);
    }

    fn push(&mut self, span: Span, label: String, kind: HintKind) {
        if self.muted {
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        self.files
            .entry(file.clone())
            .or_default()
            .push(Hint { span, label, kind });
    }

    /// Fold in what `other` recorded (#3243 S2.3), per file, in order.
    pub fn absorb(&mut self, other: HintSink) {
        for (file, entries) in other.files {
            self.files.entry(file).or_default().extend(entries);
        }
    }

    /// Drain the recorded hints, each file's entries ordered by span start.
    pub fn take_files(&mut self) -> FileHints {
        let mut files = std::mem::take(&mut self.files);
        for hints in files.values_mut() {
            hints.sort_by_key(|h| (h.span.start, h.span.end));
        }
        files
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2727: the bottom type is not source syntax, so no type hint may carry it.
    #[test]
    fn type_hints_never_offer_the_bottom_type() {
        let mut sink = HintSink::new();
        sink.enter_file(Path::new("a.bynk"), false);
        sink.record(
            Span::new(4, 5),
            format!(": {}", crate::checker::NEVER_DISPLAY),
        );
        sink.record(
            Span::new(8, 9),
            format!(": List[{}]", crate::checker::NEVER_DISPLAY),
        );
        sink.record(Span::new(12, 13), ": Int".to_string());
        let files = sink.take_files();
        let hints = &files[Path::new("a.bynk")];
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].label, ": Int");
    }
}
