//! Placeholder recognition in a template file (typed-HTML track §6, Q1).
//!
//! Only `\(` followed by a name and `)` is a placeholder, and only when its
//! backslash does not follow another backslash. Any other `\(` is literal text,
//! and the file has no other escapes, so MathJax's `\(x^2\)`, a regular
//! expression such as `/\(/` in an inline script, and `\\(` all pass through
//! unchanged. A literal `\(name)` is written with a character reference,
//! `&#92;(name)`, which this scan never sees as a placeholder.

/// One recognised placeholder: its name and the byte range of the whole
/// `\(name)` in the file, so a diagnostic can point at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    pub name: String,
    pub start: usize,
    pub end: usize,
}

/// A template's text cut at its placeholders: literal pieces, each with the
/// byte offset it starts at, and the placeholders between them. There is always
/// one more piece than there are placeholders (a piece may be empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub pieces: Vec<(usize, String)>,
    pub placeholders: Vec<Placeholder>,
}

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_name_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The placeholder that starts at `i` in `bytes`, as its name and the offset
/// just past its `)`, when there is one there.
fn placeholder_at(bytes: &[u8], i: usize) -> Option<(String, usize)> {
    if bytes.get(i) != Some(&b'\\') || bytes.get(i + 1) != Some(&b'(') {
        return None;
    }
    if i > 0 && bytes[i - 1] == b'\\' {
        return None;
    }
    let start = i + 2;
    if !bytes.get(start).copied().is_some_and(is_name_start) {
        return None;
    }
    let mut j = start + 1;
    while bytes.get(j).copied().is_some_and(is_name_continue) {
        j += 1;
    }
    if bytes.get(j) != Some(&b')') {
        return None;
    }
    // The name is ASCII, so the slice is on character boundaries.
    let name = String::from_utf8_lossy(&bytes[start..j]).into_owned();
    Some((name, j + 1))
}

/// Cut `text` at its placeholders.
pub fn split(text: &str) -> Split {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut placeholders = Vec::new();
    let mut piece_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if let Some((name, end)) = placeholder_at(bytes, i) {
            pieces.push((piece_start, text[piece_start..i].to_string()));
            placeholders.push(Placeholder {
                name,
                start: i,
                end,
            });
            piece_start = end;
            i = end;
        } else {
            i += 1;
        }
    }
    pieces.push((piece_start, text[piece_start..].to_string()));
    Split {
        pieces,
        placeholders,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<String> {
        split(text)
            .placeholders
            .into_iter()
            .map(|p| p.name)
            .collect()
    }

    #[test]
    fn a_placeholder_is_a_backslash_paren_name_paren() {
        assert_eq!(names("<p>\\(title)</p>"), vec!["title"]);
        assert_eq!(names("\\(a)\\(b_2)"), vec!["a", "b_2"]);
        let s = split("x\\(a)y");
        assert_eq!(s.pieces, vec![(0, "x".to_string()), (5, "y".to_string())]);
        assert_eq!(s.placeholders[0].start, 1);
        assert_eq!(s.placeholders[0].end, 5);
    }

    #[test]
    fn any_other_backslash_paren_is_literal_text() {
        // MathJax, a regular expression, an escaped backslash, a character reference.
        assert!(names("\\(x^2\\)").is_empty());
        assert!(names("/\\(/").is_empty());
        assert!(names("\\\\(name)").is_empty());
        assert!(names("&#92;(name)").is_empty());
        assert!(names("\\( name)").is_empty());
        assert!(names("\\(1x)").is_empty());
        assert!(names("\\(name").is_empty());
        assert!(names("\\name)").is_empty());
    }
}
