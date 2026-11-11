use std::path::PathBuf;

use bynk_syntax::error::CompileError;

/// v0.24 (ADR 0052): a compile error attributed — where possible — to the
/// project-relative source file it belongs to, tagged at the collection
/// point (the phase that produced it knows which file it was processing).
/// `None` is the project-level bucket: validations spanning files
/// (group/cycle/directory consistency) with no single owning file.
///
/// P4.0 (#1113): moved here from `bynk-emit`'s `project::diagnostics` — the
/// one item of that module with no `Mode`/`ErrorSink`-shaped driving logic
/// behind it, just a plain attributed-diagnostic value type. `Mode`,
/// `ErrorSink`, `ProjectAnalysis`, `ProjectFailure`, `ContextSequenceInfo`,
/// and `ContextBoundaryInfo` all stay in `bynk-emit` (each is a fact about
/// how the pipeline is driven or consumed, not about the project model).
#[derive(Clone)]
pub struct AttributedError {
    pub source_path: Option<PathBuf>,
    pub error: CompileError,
    /// The file each of `error.labels` points into, index for index. A
    /// secondary label may lie in a sibling file — a duplicate's "previously
    /// declared here" in another file of the same multi-file unit — and its
    /// span is an offset into *that* file's text, not `source_path`'s. Filled
    /// in when the sink is drained, from the `FileId` each label's span
    /// carries. Shorter than `error.labels` (empty, for an error built outside
    /// the sink) means [`LabelFile::Primary`] for the rest; read it through
    /// [`Self::label_file`].
    pub label_files: Vec<LabelFile>,
}

/// Which file an [`AttributedError`]'s secondary label lies in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelFile {
    /// The same file as the error itself — `source_path`'s text. Also what a
    /// label whose span carries no file identity (`FileId::UNKNOWN`) means.
    Primary,
    /// Another project file, by identity path — `snapshots`' key for its text.
    Other(PathBuf),
    /// A toolchain-injected first-party unit (#1956): its synthetic identity
    /// path (`bynk.bynk`) and its source text, which is not a project file and
    /// so is never in `snapshots`. A renderer that can show it draws the label
    /// against `source`; one that can't (the LSP, with no document to point
    /// at) makes it a note. Never drawn against the primary file's text.
    FirstParty {
        identity_path: &'static str,
        source: &'static str,
    },
    /// A file with no text to show it against. Rendered as a note, never
    /// against the primary file's text.
    Unavailable,
}

impl AttributedError {
    /// `error`, attributed to `source_path`, every label in that same file.
    pub fn new(source_path: Option<PathBuf>, error: CompileError) -> Self {
        Self {
            source_path,
            error,
            label_files: Vec::new(),
        }
    }

    /// The file `error.labels[index]` lies in.
    pub fn label_file(&self, index: usize) -> &LabelFile {
        self.label_files.get(index).unwrap_or(&LabelFile::Primary)
    }
}
