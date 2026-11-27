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
pub use tokeniser::{Analysis, At, Context, Judgement, Refusal, Unit};

/// A template file's analysis: its text cut at its placeholders, and the
/// tokeniser's judgement of the markup and of each placeholder (by index into
/// `split.placeholders`).
#[derive(Debug, Clone)]
pub struct TemplateAnalysis {
    pub split: Split,
    pub analysis: Analysis,
}

/// HTML input-stream preprocessing (§10, "Normalisation"): CRLF and a lone CR
/// become LF, as a browser turns them before it tokenises. A template's text is
/// normalised when it is read, before it is tokenised, emitted or hashed, so a
/// template checked out with CRLF reads, emits and hashes as with LF.
pub fn normalise(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The characters of `piece`, which starts at byte `start` of the file,
/// normalised as [`normalise`] does, each at its offset in the file: the CR of a
/// CRLF is dropped, and a lone CR is read as LF.
fn normalised_chars(start: usize, piece: &str) -> impl Iterator<Item = Unit> + '_ {
    let mut chars = piece.char_indices().peekable();
    std::iter::from_fn(move || {
        loop {
            let (o, c) = chars.next()?;
            if c != '\r' {
                return Some(Unit::Char(c, start + o));
            }
            if chars.peek().map(|&(_, n)| n) != Some('\n') {
                return Some(Unit::Char('\n', start + o));
            }
        }
    })
}

/// The units of a split template: each piece's characters, normalised, at their
/// offsets in the file, and a hole for each placeholder. Every analysis goes
/// through here, so the tokeniser never sees a CR (and refuses one if it does).
pub fn units(split: &Split) -> Vec<Unit> {
    let mut out = Vec::new();
    for (n, (start, piece)) in split.pieces.iter().enumerate() {
        out.extend(normalised_chars(*start, piece));
        if let Some(p) = split.placeholders.get(n) {
            out.push(Unit::Hole(n, p.start));
        }
    }
    out
}

/// Analyse a template file's text: find its placeholders, normalise its line
/// breaks and tokenise it. Every offset in the result is a byte offset into
/// `text` as given; each piece in `split` is normalised, ready to be emitted.
pub fn analyse_template(text: &str) -> TemplateAnalysis {
    let mut split = placeholders::split(text);
    let analysis = tokeniser::analyse(&units(&split));
    for (_, piece) in &mut split.pieces {
        *piece = normalise(piece);
    }
    TemplateAnalysis { split, analysis }
}
