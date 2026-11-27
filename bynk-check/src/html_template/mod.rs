//! Typed HTML (typed-HTML track, spine #3927): the compile-time analysis of a
//! template's markup.
//!
//! [`placeholders`] finds a template file's `\(name)` placeholders (§6), and
//! [`tokeniser`] tokenises the markup around them as a browser does and gives
//! each placeholder its context (§5.2) or refuses it, as a [`tokeniser::Kind`]. The
//! checker then judges each placeholder's field against its
//! context, and the emitter builds the template's function from the pieces.
//! Nothing here runs at run time: a template compiles to string concatenation
//! and escape calls (§8).

pub mod placeholders;
pub mod tokeniser;

#[cfg(test)]
mod tests;

pub use placeholders::{Placeholder, Split};
pub use tokeniser::{Analysis, At, Context, Refusal, Unit};

/// A template file's analysis: its text cut at its placeholders, and the
/// tokeniser's judgement of the markup and of each placeholder (by index into
/// `split.placeholders`).
#[derive(Debug, Clone)]
pub struct TemplateAnalysis {
    pub split: Split,
    pub analysis: Analysis,
}

/// The units of a split template: each piece's characters at their offsets,
/// and a hole for each placeholder.
pub fn units(split: &Split) -> Vec<Unit> {
    let mut out = Vec::new();
    for (n, (start, piece)) in split.pieces.iter().enumerate() {
        out.extend(piece.char_indices().map(|(o, c)| Unit::Char(c, start + o)));
        if let Some(p) = split.placeholders.get(n) {
            out.push(Unit::Hole(n, p.start));
        }
    }
    out
}

/// Analyse a template file's text: find its placeholders and tokenise it.
pub fn analyse_template(text: &str) -> TemplateAnalysis {
    let split = placeholders::split(text);
    let analysis = tokeniser::analyse(&units(&split));
    TemplateAnalysis { split, analysis }
}
