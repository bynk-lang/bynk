//! The walkable subset of a `Matches` pattern, for `for all` to draw from (#4121).
//!
//! The pattern is a literal, so it is known at compile time. This parses the part of Unicode-mode
//! `RegExp` syntax a generator can honour into a small tree: literals and character classes
//! (as code-point ranges), sequences, alternations and bounded or unbounded repeats. A construct
//! the walk cannot honour (a lookaround, a backreference, an assertion inside the pattern, a
//! property escape) is refused with its name, and the binding keeps `bynk.val.needs_pin`.
//!
//! The tree is written as JSON for the test runtime, which walks it. The emitted `Matches` check
//! re-checks every drawn value, so a disagreement between this subset and the engine fails
//! loudly instead of drawing a wrong value.

use std::fmt::Write as _;

/// The last Unicode scalar value.
const MAX_CP: u32 = 0x10_FFFF;

/// A parsed, walkable pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Walk {
    /// Matches the empty string.
    Empty,
    /// One code point from these sorted, disjoint inclusive ranges.
    Class(Vec<(u32, u32)>),
    Seq(Vec<Walk>),
    Alt(Vec<Walk>),
    /// `min` to `max` copies (`None`: unbounded).
    Repeat(Box<Walk>, u32, Option<u32>),
}

/// Parse `literal` (a `Matches` argument, leading flag group included) into a walk, or name the
/// construct that cannot be walked.
pub fn parse_walkable(literal: &str) -> Result<Walk, &'static str> {
    let (pattern, flags) = crate::checker::split_regex_flags(literal);
    let chars: Vec<char> = pattern.chars().collect();
    let mut p = Parser {
        chars: &chars,
        i: 0,
        dot_all: flags.contains('s'),
        fold: flags.contains('i'),
    };
    let walk = p.alternation(true)?;
    if p.i < chars.len() {
        return Err("an unbalanced group");
    }
    Ok(walk)
}

/// Why a `Matches` argument cannot be walked, if it cannot.
pub fn unwalkable_reason(literal: &str) -> Option<&'static str> {
    parse_walkable(literal).err()
}

/// The walk as the JSON the test runtime reads: `["e"]`, `["c",[lo,hi,…]]`, `["s",[…]]`,
/// `["a",[…]]` or `["r",node,min,max]` (`max` is `-1` for unbounded).
pub fn walk_json(walk: &Walk) -> String {
    let mut out = String::new();
    write_json(walk, &mut out);
    out
}

fn write_json(walk: &Walk, out: &mut String) {
    match walk {
        Walk::Empty => out.push_str("[\"e\"]"),
        Walk::Class(ranges) => {
            out.push_str("[\"c\",[");
            for (k, (lo, hi)) in ranges.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{lo},{hi}");
            }
            out.push_str("]]");
        }
        Walk::Seq(items) | Walk::Alt(items) => {
            out.push_str(if matches!(walk, Walk::Seq(_)) {
                "[\"s\",["
            } else {
                "[\"a\",["
            });
            for (k, item) in items.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write_json(item, out);
            }
            out.push_str("]]");
        }
        Walk::Repeat(node, min, max) => {
            out.push_str("[\"r\",");
            write_json(node, out);
            let _ = write!(out, ",{min},{}]", max.map_or(-1, i64::from));
        }
    }
}

struct Parser<'a> {
    chars: &'a [char],
    i: usize,
    dot_all: bool,
    fold: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    /// `top` is true for the whole pattern, where a leading `^` and a trailing `$` are the
    /// anchors `Matches` already supplies.
    fn alternation(&mut self, top: bool) -> Result<Walk, &'static str> {
        let mut alts = vec![self.concatenation(top)?];
        while self.eat('|') {
            alts.push(self.concatenation(top)?);
        }
        Ok(if alts.len() == 1 {
            alts.remove(0)
        } else {
            Walk::Alt(alts)
        })
    }

    fn concatenation(&mut self, top: bool) -> Result<Walk, &'static str> {
        let mut items = Vec::new();
        if top && self.eat('^') {
            // The anchor `Matches` supplies.
        }
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            if c == '$' {
                self.i += 1;
                if top && matches!(self.peek(), None | Some('|')) {
                    continue;
                }
                return Err("an assertion inside the pattern");
            }
            let atom = self.atom()?;
            items.push(self.quantified(atom)?);
        }
        Ok(match items.len() {
            0 => Walk::Empty,
            1 => items.remove(0),
            _ => Walk::Seq(items),
        })
    }

    fn quantified(&mut self, atom: Walk) -> Result<Walk, &'static str> {
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => {
                self.i += 1;
                self.braces()?
            }
            _ => return Ok(atom),
        };
        self.i += 1;
        // A lazy suffix changes which match is found, not which strings match.
        self.eat('?');
        Ok(Walk::Repeat(Box::new(atom), min, max))
    }

    /// `n}`, `n,}` or `n,m}` after the `{`, leaving the cursor on the `}`.
    fn braces(&mut self) -> Result<(u32, Option<u32>), &'static str> {
        let min = self.number()?;
        let max = if self.eat(',') {
            if self.peek() == Some('}') {
                None
            } else {
                Some(self.number()?)
            }
        } else {
            Some(min)
        };
        if self.peek() != Some('}') {
            return Err("a malformed repeat");
        }
        if max.is_some_and(|m| m < min) {
            return Err("a malformed repeat");
        }
        Ok((min, max))
    }

    fn number(&mut self) -> Result<u32, &'static str> {
        let start = self.i;
        let mut n: u32 = 0;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            n = n.saturating_mul(10).saturating_add(d);
            self.i += 1;
        }
        if self.i == start {
            return Err("a malformed repeat");
        }
        Ok(n)
    }

    fn atom(&mut self) -> Result<Walk, &'static str> {
        let Some(c) = self.peek() else {
            return Err("an unexpected end of the pattern");
        };
        self.i += 1;
        match c {
            '(' => self.group(),
            '[' => self.class().map(Walk::Class),
            '.' => {
                let mut set = Set::default();
                set.add(0, MAX_CP);
                if !self.dot_all {
                    for t in [0x0A, 0x0D, 0x2028, 0x2029] {
                        set.remove(t);
                    }
                }
                Ok(Walk::Class(set.finish()?))
            }
            '\\' => self.escape(false).map(|e| match e {
                Esc::Char(c) => Walk::Class(vec![(c, c)]),
                Esc::Set(r) => Walk::Class(r),
            }),
            '^' => Err("an assertion inside the pattern"),
            '*' | '+' | '?' => Err("a repeat with nothing to repeat"),
            c => Ok(Walk::Class(vec![(c as u32, c as u32)])),
        }
    }

    fn group(&mut self) -> Result<Walk, &'static str> {
        if self.eat('?') {
            if self.eat(':') {
                // Non-capturing.
            } else if self.eat('=') || self.eat('!') {
                return Err("a lookahead");
            } else if self.eat('<') {
                if matches!(self.peek(), Some('=' | '!')) {
                    return Err("a lookbehind");
                }
                while let Some(c) = self.peek() {
                    self.i += 1;
                    if c == '>' {
                        break;
                    }
                }
            } else {
                return Err("an inline group flag");
            }
        }
        let inner = self.alternation(false)?;
        if !self.eat(')') {
            return Err("an unbalanced group");
        }
        Ok(inner)
    }

    fn class(&mut self) -> Result<Vec<(u32, u32)>, &'static str> {
        let negated = self.eat('^');
        if negated && self.fold {
            return Err("a negated class under `(?i)`");
        }
        let mut set = Set::default();
        let mut first = true;
        loop {
            let Some(c) = self.peek() else {
                return Err("an unterminated class");
            };
            if c == ']' && !first {
                self.i += 1;
                break;
            }
            first = false;
            self.i += 1;
            let lo = if c == '\\' {
                match self.escape(true)? {
                    Esc::Set(r) => {
                        for (a, b) in r {
                            set.add(a, b);
                        }
                        continue;
                    }
                    Esc::Char(c) => c,
                }
            } else {
                c as u32
            };
            // A range `a-z`; a trailing `-` is a literal.
            if self.peek() == Some('-') && self.chars.get(self.i + 1).is_some_and(|n| *n != ']') {
                self.i += 1;
                let h = self.peek().ok_or("an unterminated class")?;
                self.i += 1;
                let hi = if h == '\\' {
                    match self.escape(true)? {
                        Esc::Char(c) => c,
                        Esc::Set(_) => return Err("a class shorthand as a range end"),
                    }
                } else {
                    h as u32
                };
                if hi < lo {
                    return Err("an out-of-order class range");
                }
                set.add(lo, hi);
            } else {
                set.add(lo, lo);
            }
        }
        if negated {
            set = set.negated();
        }
        set.finish()
    }

    /// After a `\`.
    fn escape(&mut self, in_class: bool) -> Result<Esc, &'static str> {
        let Some(c) = self.peek() else {
            return Err("a trailing backslash");
        };
        self.i += 1;
        let ch = |c: char| Ok(Esc::Char(c as u32));
        match c {
            'd' => Ok(Esc::Set(Set::of(&[(0x30, 0x39)]).finish()?)),
            'w' => Ok(Esc::Set(word().finish()?)),
            's' => Ok(Esc::Set(space().finish()?)),
            'D' | 'W' | 'S' => {
                if self.fold {
                    return Err("a negated class under `(?i)`");
                }
                let base = match c {
                    'D' => Set::of(&[(0x30, 0x39)]),
                    'W' => word(),
                    _ => space(),
                };
                Ok(Esc::Set(base.negated().finish()?))
            }
            'n' => ch('\n'),
            'r' => ch('\r'),
            't' => ch('\t'),
            'f' => ch('\u{0C}'),
            'v' => ch('\u{0B}'),
            '0' if !self.peek().is_some_and(|d| d.is_ascii_digit()) => ch('\0'),
            'b' if in_class => ch('\u{08}'),
            'b' | 'B' => Err("a word-boundary assertion"),
            '1'..='9' | '0' => Err("a backreference"),
            'k' => Err("a backreference"),
            'p' | 'P' => Err("a Unicode property escape"),
            'c' => Err("a control-letter escape"),
            'x' => self.hex(2).map(Esc::Char),
            'u' => {
                if self.eat('{') {
                    let mut v: u32 = 0;
                    let mut any = false;
                    while let Some(d) = self.peek().and_then(|c| c.to_digit(16)) {
                        v = v.saturating_mul(16).saturating_add(d);
                        any = true;
                        self.i += 1;
                    }
                    if !any || !self.eat('}') || v > MAX_CP {
                        return Err("a malformed `\\u{…}` escape");
                    }
                    Ok(Esc::Char(v))
                } else {
                    let v = self.hex(4)?;
                    // A surrogate pair `😀` names one astral code point.
                    if (0xD800..0xDC00).contains(&v)
                        && self.peek() == Some('\\')
                        && self.chars.get(self.i + 1) == Some(&'u')
                    {
                        let save = self.i;
                        self.i += 2;
                        let lo = self.hex(4)?;
                        if (0xDC00..0xE000).contains(&lo) {
                            return Ok(Esc::Char(0x10000 + ((v - 0xD800) << 10) + (lo - 0xDC00)));
                        }
                        self.i = save;
                    }
                    if (0xD800..0xE000).contains(&v) {
                        return Err("a lone surrogate escape");
                    }
                    Ok(Esc::Char(v))
                }
            }
            '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'
            | '/' => ch(c),
            '-' if in_class => ch('-'),
            _ => Err("an unsupported escape"),
        }
    }

    fn hex(&mut self, digits: usize) -> Result<u32, &'static str> {
        let mut v = 0u32;
        for _ in 0..digits {
            let d = self
                .peek()
                .and_then(|c| c.to_digit(16))
                .ok_or("a malformed hex escape")?;
            v = v * 16 + d;
            self.i += 1;
        }
        Ok(v)
    }
}

enum Esc {
    Char(u32),
    Set(Vec<(u32, u32)>),
}

fn word() -> Set {
    Set::of(&[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)])
}

fn space() -> Set {
    Set::of(&[
        (0x09, 0x0D),
        (0x20, 0x20),
        (0xA0, 0xA0),
        (0x1680, 0x1680),
        (0x2000, 0x200A),
        (0x2028, 0x2029),
        (0x202F, 0x202F),
        (0x205F, 0x205F),
        (0x3000, 0x3000),
        (0xFEFF, 0xFEFF),
    ])
}

/// A set of code points as ranges, merged on demand.
#[derive(Default)]
struct Set(Vec<(u32, u32)>);

impl Set {
    fn of(ranges: &[(u32, u32)]) -> Self {
        Set(ranges.to_vec())
    }

    fn add(&mut self, lo: u32, hi: u32) {
        self.0.push((lo, hi));
    }

    fn remove(&mut self, cp: u32) {
        let merged = self.merged();
        let mut out = Vec::new();
        for (lo, hi) in merged {
            if cp < lo || cp > hi {
                out.push((lo, hi));
                continue;
            }
            if lo < cp {
                out.push((lo, cp - 1));
            }
            if cp < hi {
                out.push((cp + 1, hi));
            }
        }
        self.0 = out;
    }

    fn merged(&self) -> Vec<(u32, u32)> {
        let mut r = self.0.clone();
        r.sort_unstable();
        let mut out: Vec<(u32, u32)> = Vec::new();
        for (lo, hi) in r {
            match out.last_mut() {
                Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
                _ => out.push((lo, hi)),
            }
        }
        out
    }

    /// The complement over the Unicode scalar values (surrogates excluded).
    fn negated(&self) -> Set {
        let mut all = vec![(0, 0xD7FF), (0xE000, MAX_CP)];
        for (lo, hi) in self.merged() {
            let mut next = Vec::new();
            for (a, b) in all {
                if hi < a || lo > b {
                    next.push((a, b));
                    continue;
                }
                if a < lo {
                    next.push((a, lo - 1));
                }
                if hi < b {
                    next.push((hi + 1, b));
                }
            }
            all = next;
        }
        Set(all)
    }

    /// The ranges with surrogates removed, or an error for an empty class.
    fn finish(mut self) -> Result<Vec<(u32, u32)>, &'static str> {
        let mut out = Vec::new();
        for (lo, hi) in self.merged() {
            if hi < 0xD800 || lo > 0xDFFF {
                out.push((lo, hi));
                continue;
            }
            if lo < 0xD800 {
                out.push((lo, 0xD7FF));
            }
            if hi > 0xDFFF {
                out.push((0xE000, hi));
            }
        }
        self.0 = out.clone();
        if out.is_empty() {
            return Err("an empty character class");
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(p: &str) -> String {
        walk_json(&parse_walkable(p).unwrap())
    }

    #[test]
    fn literals_classes_and_repeats_walk() {
        assert_eq!(json("ab"), "[\"s\",[[\"c\",[97,97]],[\"c\",[98,98]]]]");
        assert_eq!(json("[a-c]{2,3}"), "[\"r\",[\"c\",[97,99]],2,3]");
        assert_eq!(json("a+"), "[\"r\",[\"c\",[97,97]],1,-1]");
        assert_eq!(json("a|b"), "[\"a\",[[\"c\",[97,97]],[\"c\",[98,98]]]]");
    }

    #[test]
    fn anchors_at_the_ends_are_the_ones_matches_supplies() {
        assert_eq!(json("^a$"), json("a"));
    }

    #[test]
    fn a_leading_flag_group_is_read() {
        assert!(parse_walkable("(?i)abc").is_ok());
        assert_eq!(
            unwalkable_reason("(?i)[^a]"),
            Some("a negated class under `(?i)`")
        );
    }

    #[test]
    fn what_the_walk_cannot_honour_is_named() {
        assert_eq!(unwalkable_reason("a(?=b)"), Some("a lookahead"));
        assert_eq!(unwalkable_reason("(?<=a)b"), Some("a lookbehind"));
        assert_eq!(unwalkable_reason("(a)\\1"), Some("a backreference"));
        assert_eq!(
            unwalkable_reason("\\bword"),
            Some("a word-boundary assertion")
        );
        assert_eq!(
            unwalkable_reason("\\p{L}"),
            Some("a Unicode property escape")
        );
        assert_eq!(
            unwalkable_reason("a^b"),
            Some("an assertion inside the pattern")
        );
    }

    #[test]
    fn a_negated_class_leaves_out_surrogates() {
        let Walk::Class(r) = parse_walkable("[^a]").unwrap() else {
            panic!("a class");
        };
        assert!(r.iter().all(|(lo, hi)| *hi < 0xD800 || *lo > 0xDFFF));
        assert!(!r.iter().any(|(lo, hi)| (*lo..=*hi).contains(&0x61)));
    }
}
