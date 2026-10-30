//! Which `Matches` patterns can backtrack catastrophically. #1651
//! (runtime-semantics track #1648, slice S2; decided in ADR 0423, the track's
//! summary in `design/archive/retired-tracks.md`).
//!
//! A refined `String`'s boundary check runs its pattern under the JS `RegExp`
//! engine, a backtracker, on request input. A backtracker's running time is
//! governed by how many ways the pattern can match the same text: when one
//! input has exponentially many accepting-or-failing paths, a near-miss input
//! of a few dozen characters stalls the Worker. The checker's cheap first pass
//! (`has_nested_unbounded_quantifier`) catches nested quantifiers (`(a+)+`).
//! This module catches the rest of that class, and the polynomial class, by
//! deciding ambiguity on the pattern's automaton. It follows Weideman, van der
//! Merwe, Berglund & Watson, *Analyzing matching time behavior of backtracking
//! regex matchers* (CIAA 2016), the approach behind rxxr2 and recheck:
//!
//! 1. **Parse** the pattern with ECMAScript no-flags semantics: over UTF-16 code
//!    units, with Annex B's legacy forms (a literal `{`, octal escapes).
//! 2. **Expand** bounded repeats as *nested* optionals (`x{2,4}` is
//!    `xx(x(x)?)?`). The flat form `xxx?x?` would invent ambiguity the
//!    backtracker does not have. A count above [`REPEAT_CAP`] is
//!    over-approximated as unbounded.
//! 3. Build the **Glushkov automaton**: one state per character position, no
//!    epsilon moves, and every transition into a position is labelled with that
//!    position's character set.
//! 4. **Exponential ambiguity (EDA)** exists iff a strongly connected component
//!    of the product automaton A×A, restricted to the pairs reachable from the
//!    start pair, holds both a diagonal pair `(p, p)` and an off-diagonal pair
//!    `(p, q)`. Then a loop can be traversed two different ways on the same
//!    text, so `n` traversals have `2ⁿ` paths: `(a|a)+`, `(\d|\d\d)+`,
//!    `(a{1,2})+`.
//! 5. **Polynomial ambiguity (IDA)** exists iff, for distinct states `p` and
//!    `q`, some word labels a path `p → p`, a path `p → q` and a path `q → q`.
//!    That is a path `(p, p, q) ⇝ (p, q, q)` in A³. It means two loops can split
//!    the same run of text between them in a number of ways that grows with its
//!    length: `\d*\d*` is quadratic. The **degree** is the longest chain of
//!    loops linked this way: `\d*\d*\d*` is cubic.
//!
//! **Soundness.** Every approximation errs toward *more* ambiguity, so a pattern
//! this accepts is safe, and a few safe patterns are rejected:
//! - zero-width assertions (`^`, `$`, `\b`, `\B`) are read as matching
//!   anything;
//! - a lookaround is read as matching anything where it stands, and its body is
//!   analysed separately as a pattern of its own;
//! - a backreference is read as an optional copy of the group it names, since
//!   it matches the text that group captured, or nothing when the group has not
//!   matched. Under an unbounded quantifier a backreference is rejected
//!   outright: each iteration may match a different string, which no finite
//!   copy describes;
//! - an unbounded quantifier over a body that can match the empty string is
//!   rejected outright, because each iteration may consume nothing;
//! - a pattern with more than [`MAX_POSITIONS`] positions after expansion, or
//!   whose analysis exceeds [`WORK_BUDGET`], is rejected as too complex to
//!   analyse.
//!
//! The analysis is over the anchored, flag-free pattern the emitter builds
//! (`^(?:pat)$`), so it assumes the pattern must match the whole input.

use std::collections::{HashMap, VecDeque};

/// The most positions the expanded automaton may have. Every pattern in the
/// repo, including the first-party `LocaleTag`, is far below it.
const MAX_POSITIONS: usize = 2_000;

/// A bounded repeat count above this is over-approximated: `{n,m}` with
/// `m > 64` is read as `{n,}`, and a lower bound above 64 is read as 64. Both
/// only add strings to the language.
const REPEAT_CAP: u32 = 64;

/// How many units of work the analysis may do (successor combinations in A×A
/// and A³, and edges walked by reachability) before giving up and rejecting
/// the pattern as too complex. It bounds the analysis's time on any pattern,
/// independent of [`MAX_POSITIONS`].
const WORK_BUDGET: usize = 2_000_000;

/// The analysis verdict for one pattern.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Linear: no input has more than a bounded number of matching paths.
    Linear,
    /// Exponential ambiguity. The two strings are the source text of two atoms
    /// that can match the same character on different paths through a loop.
    Exponential(String, String),
    /// Polynomial ambiguity: loops can divide the same text between them. The
    /// number is the degree, the longest chain of loops linked this way, so
    /// matching takes on the order of `nᵈᵉᵍʳᵉᵉ` steps. The strings are the
    /// source text of an atom in two linked loops.
    Polynomial(u32, String, String),
    /// The pattern uses a construct the analysis cannot bound. The string says
    /// which, for the diagnostic.
    Unanalysable(&'static str),
}

/// Decide whether `pattern` (already accepted by `regress`) can backtrack
/// super-linearly. Lookaround bodies are analysed too, and the worst verdict
/// wins.
#[cfg(test)]
pub(crate) fn analyse(pattern: &str) -> Verdict {
    analyse_with_flags(pattern, "")
}

/// K6c (#2600): [`analyse`] under `RegExp` flags. `s` makes `.` match every
/// code unit, line terminators included; `i` case-folds every character set,
/// so `(a|A)+` is seen to repeat one set two ways. Folding only adds code
/// units, so a fold too generous errs towards refusing, never towards
/// accepting. Other flags do not change what an atom matches here.
pub(crate) fn analyse_with_flags(pattern: &str, flags: &str) -> Verdict {
    let units: Vec<u16> = pattern.encode_utf16().collect();
    let groups = scan_groups(&units);
    let mut parser = Parser {
        units: &units,
        i: 0,
        groups: &groups,
        next_group: 0,
        dot_all: flags.contains('s'),
        ignore_case: flags.contains('i'),
    };
    let root = parser.alternation();
    if parser.i < units.len() {
        // Unreachable for a pattern `regress` accepted (a stray `)`).
        return Verdict::Unanalysable("it could not be parsed for analysis");
    }
    let mut group_nodes = HashMap::new();
    collect_groups(&root, &mut group_nodes);

    let mut bodies = vec![root.clone()];
    let mut worst = Verdict::Linear;
    while let Some(body) = bodies.pop() {
        let mut ex = Expander {
            positions: Vec::new(),
            groups: &group_nodes,
            open_groups: Vec::new(),
            following: Vec::new(),
            lookarounds: Vec::new(),
            seen_nodes: Vec::new(),
            bounded_bodies: Vec::new(),
        };
        let mut verdict = match ex.expand(&body, false) {
            Err(why) => Verdict::Unanalysable(why),
            Ok(rx) => decide(&ex.positions, &rx, &units, false),
        };
        // A bounded repeat `B{n,m}` with `m ≥ 2` has no cycle in the exact
        // expansion, yet multiplies its paths per repetition just as a loop
        // does, up to its count: `(?:a|a){0,24}` has 2²³ paths on 23 `a`s.
        // It is exponential in the count iff `B*` is exponentially ambiguous,
        // so each such body gets that test on its own.
        for b in ex.bounded_bodies {
            if verdict != Verdict::Linear && !matches!(verdict, Verdict::Polynomial(..)) {
                break;
            }
            let mut local = Expander {
                positions: Vec::new(),
                groups: &group_nodes,
                open_groups: Vec::new(),
                following: Vec::new(),
                lookarounds: Vec::new(),
                seen_nodes: Vec::new(),
                bounded_bodies: Vec::new(),
            };
            if let Ok(rx) = local.expand(&b, false) {
                let looped = Rx::Star(Box::new(rx));
                let v = decide(&local.positions, &looped, &units, true);
                if rank(&v) > rank(&verdict) {
                    verdict = v;
                }
            }
        }
        bodies.extend(ex.lookarounds);
        if rank(&verdict) > rank(&worst) {
            worst = verdict;
        }
    }
    worst
}

fn rank(v: &Verdict) -> u8 {
    match v {
        Verdict::Linear => 0,
        Verdict::Polynomial(..) => 1,
        Verdict::Unanalysable(_) => 2,
        Verdict::Exponential(..) => 3,
    }
}

// ---------------------------------------------------------------------------
// Character sets over UTF-16 code units.
// ---------------------------------------------------------------------------

/// A set of UTF-16 code units, as sorted, disjoint, non-adjacent inclusive
/// ranges. With no `u` flag, JS matches code units, not code points.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CharSet(Vec<(u16, u16)>);

impl CharSet {
    fn unit(c: u16) -> Self {
        CharSet(vec![(c, c)])
    }

    fn from_ranges(ranges: &[(u16, u16)]) -> Self {
        let mut s = CharSet::default();
        for &(a, b) in ranges {
            s.add(a, b);
        }
        s
    }

    fn add(&mut self, lo: u16, hi: u16) {
        self.0.push((lo, hi));
        self.0.sort_unstable();
        let mut merged: Vec<(u16, u16)> = Vec::with_capacity(self.0.len());
        for &(a, b) in &self.0 {
            match merged.last_mut() {
                Some(last) if u32::from(a) <= u32::from(last.1) + 1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        self.0 = merged;
    }

    fn union(&mut self, other: &CharSet) {
        for &(a, b) in &other.0 {
            self.add(a, b);
        }
    }

    /// K6c (#2600): the set with every code unit's other cases added, as the
    /// `i` flag matches them. Built through a bitmap, so a large set costs one
    /// pass rather than an `add` (and a sort) per code unit.
    fn case_folded(&self) -> CharSet {
        let mut member = vec![false; 0x10000];
        for &(a, b) in &self.0 {
            for c in u32::from(a)..=u32::from(b) {
                member[c as usize] = true;
                if let Some(ch) = char::from_u32(c) {
                    for other in ch.to_lowercase().chain(ch.to_uppercase()) {
                        if let Ok(unit) = u16::try_from(u32::from(other)) {
                            member[usize::from(unit)] = true;
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        let mut c = 0usize;
        while c < member.len() {
            if member[c] {
                let start = c;
                while c + 1 < member.len() && member[c + 1] {
                    c += 1;
                }
                out.push((start as u16, c as u16));
            }
            c += 1;
        }
        CharSet(out)
    }

    fn negate(&self) -> CharSet {
        let mut out = Vec::new();
        let mut next: u32 = 0;
        for &(a, b) in &self.0 {
            if u32::from(a) > next {
                out.push((next as u16, a - 1));
            }
            next = u32::from(b) + 1;
        }
        if next <= 0xFFFF {
            out.push((next as u16, 0xFFFF));
        }
        CharSet(out)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn intersect(&self, other: &CharSet) -> CharSet {
        let (mut i, mut j) = (0, 0);
        let mut out = Vec::new();
        while i < self.0.len() && j < other.0.len() {
            let (a1, b1) = self.0[i];
            let (a2, b2) = other.0[j];
            let lo = a1.max(a2);
            let hi = b1.min(b2);
            if lo <= hi {
                out.push((lo, hi));
            }
            if b1 < b2 {
                i += 1;
            } else {
                j += 1;
            }
        }
        CharSet(out)
    }

    fn intersects(&self, other: &CharSet) -> bool {
        let (mut i, mut j) = (0, 0);
        while i < self.0.len() && j < other.0.len() {
            let (a1, b1) = self.0[i];
            let (a2, b2) = other.0[j];
            if a1.max(a2) <= b1.min(b2) {
                return true;
            }
            if b1 < b2 {
                i += 1;
            } else {
                j += 1;
            }
        }
        false
    }
}

fn digit() -> CharSet {
    CharSet::from_ranges(&[(0x30, 0x39)])
}

fn word() -> CharSet {
    CharSet::from_ranges(&[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)])
}

/// ECMAScript `\s`: WhiteSpace and LineTerminator.
fn space() -> CharSet {
    CharSet::from_ranges(&[
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

/// `.` without the `s` flag: anything but a line terminator.
fn dot() -> CharSet {
    CharSet::from_ranges(&[(0x0A, 0x0A), (0x0D, 0x0D), (0x2028, 0x2029)]).negate()
}

// ---------------------------------------------------------------------------
// Parsing.
// ---------------------------------------------------------------------------

/// A parsed pattern. Source offsets are UTF-16 code-unit indices.
#[derive(Clone, Debug)]
enum Node {
    /// Matches the empty string: an empty alternative, or an assertion.
    Empty,
    /// One code unit from `set`.
    Atom {
        set: CharSet,
        src: (usize, usize),
    },
    Concat(Vec<Node>),
    Alternation(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
    },
    /// A group; `index` is its 1-based capture number when it captures.
    Group {
        index: Option<usize>,
        node: Box<Node>,
    },
    /// A lookahead or lookbehind (positive or negative).
    Lookaround(Box<Node>),
    /// A backreference to capture group `index`.
    Backref(usize),
}

/// The capture groups, in the order their `(` appears: the total count, and
/// the 1-based index of each named group.
struct Groups {
    count: usize,
    names: HashMap<Vec<u16>, usize>,
}

const fn u(c: char) -> u16 {
    c as u16
}

/// Count capture groups up front: whether `\2` is a backreference or an
/// octal escape depends on how many groups the *whole* pattern has.
fn scan_groups(units: &[u16]) -> Groups {
    let mut groups = Groups {
        count: 0,
        names: HashMap::new(),
    };
    let mut i = 0;
    let mut in_class = false;
    while i < units.len() {
        let c = units[i];
        if c == u('\\') {
            i += 2;
            continue;
        }
        if in_class {
            if c == u(']') {
                in_class = false;
            }
        } else if c == u('[') {
            in_class = true;
        } else if c == u('(') {
            if units.get(i + 1) != Some(&u('?')) {
                groups.count += 1;
            } else if units.get(i + 2) == Some(&u('<'))
                && !matches!(units.get(i + 3), Some(&c) if c == u('=') || c == u('!'))
            {
                groups.count += 1;
                let start = i + 3;
                let mut end = start;
                while end < units.len() && units[end] != u('>') {
                    end += 1;
                }
                groups
                    .names
                    .insert(units[start..end].to_vec(), groups.count);
            }
        }
        i += 1;
    }
    groups
}

struct Parser<'a> {
    units: &'a [u16],
    i: usize,
    groups: &'a Groups,
    next_group: usize,
    /// K6c (#2600): the `s` flag: `.` matches line terminators too.
    dot_all: bool,
    /// K6c (#2600): the `i` flag: character sets are case-folded.
    ignore_case: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u16> {
        self.units.get(self.i).copied()
    }

    fn peek_at(&self, k: usize) -> Option<u16> {
        self.units.get(self.i + k).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(u(c)) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn alternation(&mut self) -> Node {
        let mut branches = vec![self.concatenation()];
        while self.eat('|') {
            branches.push(self.concatenation());
        }
        if branches.len() == 1 {
            branches.pop().unwrap_or(Node::Empty)
        } else {
            Node::Alternation(branches)
        }
    }

    fn concatenation(&mut self) -> Node {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == u('|') || c == u(')') {
                break;
            }
            let atom = self.atom();
            items.push(self.quantified(atom));
        }
        match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap_or(Node::Empty),
            _ => Node::Concat(items),
        }
    }

    /// Apply a following quantifier, if any, to `atom`.
    fn quantified(&mut self, atom: Node) -> Node {
        let (min, max) = match self.peek() {
            Some(c) if c == u('*') => {
                self.i += 1;
                (0, None)
            }
            Some(c) if c == u('+') => {
                self.i += 1;
                (1, None)
            }
            Some(c) if c == u('?') => {
                self.i += 1;
                (0, Some(1))
            }
            Some(c) if c == u('{') => match self.brace_quantifier() {
                Some(q) => q,
                None => return atom,
            },
            _ => return atom,
        };
        // A lazy quantifier explores the same paths in a different order; the
        // worst case is the same.
        self.eat('?');
        Node::Repeat {
            node: Box::new(atom),
            min,
            max,
        }
    }

    /// `{n}`, `{n,}` or `{n,m}` at the cursor, consumed if well formed. Under
    /// Annex B anything else leaves the `{` to be read as a literal.
    fn brace_quantifier(&mut self) -> Option<(u32, Option<u32>)> {
        let save = self.i;
        self.i += 1;
        let min = self.decimal();
        let result = match min {
            None => None,
            Some(min) => {
                if self.eat(',') {
                    if self.eat('}') {
                        Some((min, None))
                    } else {
                        match self.decimal() {
                            Some(max) if self.eat('}') => Some((min, Some(max))),
                            _ => None,
                        }
                    }
                } else if self.eat('}') {
                    Some((min, Some(min)))
                } else {
                    None
                }
            }
        };
        if result.is_none() {
            self.i = save;
        }
        result
    }

    fn decimal(&mut self) -> Option<u32> {
        let start = self.i;
        let mut value: u32 = 0;
        while let Some(c) = self.peek() {
            if !(u('0')..=u('9')).contains(&c) {
                break;
            }
            value = value
                .saturating_mul(10)
                .saturating_add(u32::from(c - u('0')));
            self.i += 1;
        }
        (self.i > start).then_some(value)
    }

    fn atom(&mut self) -> Node {
        let start = self.i;
        let Some(c) = self.peek() else {
            return Node::Empty;
        };
        self.i += 1;
        let set = match c {
            c if c == u('(') => return self.group(),
            c if c == u('[') => self.class(),
            c if c == u('.') && self.dot_all => CharSet::from_ranges(&[(0, 0xFFFF)]),
            c if c == u('.') => dot(),
            c if c == u('^') || c == u('$') => return Node::Empty,
            c if c == u('\\') => match self.escape() {
                Escaped::Set(set) => set,
                Escaped::Assertion => return Node::Empty,
                Escaped::Backref(index) => return Node::Backref(index),
            },
            c => CharSet::unit(c),
        };
        let set = if self.ignore_case {
            set.case_folded()
        } else {
            set
        };
        Node::Atom {
            set,
            src: (start, self.i),
        }
    }

    fn group(&mut self) -> Node {
        let mut index = None;
        let mut lookaround = false;
        if self.eat('?') {
            if self.eat(':') {
            } else if self.eat('=') || self.eat('!') {
                lookaround = true;
            } else if self.eat('<') {
                if self.eat('=') || self.eat('!') {
                    lookaround = true;
                } else {
                    while let Some(c) = self.peek() {
                        self.i += 1;
                        if c == u('>') {
                            break;
                        }
                    }
                    self.next_group += 1;
                    index = Some(self.next_group);
                }
            }
        } else {
            self.next_group += 1;
            index = Some(self.next_group);
        }
        let body = self.alternation();
        self.eat(')');
        if lookaround {
            Node::Lookaround(Box::new(body))
        } else {
            Node::Group {
                index,
                node: Box::new(body),
            }
        }
    }

    /// A bracketed class, after the `[`.
    fn class(&mut self) -> CharSet {
        let negated = self.eat('^');
        let mut set = CharSet::default();
        while let Some(c) = self.peek() {
            if c == u(']') {
                self.i += 1;
                break;
            }
            let lo = self.class_atom();
            // A range `a-b`, unless the `-` is last or an end is a class
            // escape such as `\d` (Annex B reads the `-` literally then).
            if self.peek() == Some(u('-')) && self.peek_at(1).is_some_and(|c| c != u(']')) {
                self.i += 1;
                let hi = self.class_atom();
                match (single(&lo), single(&hi)) {
                    (Some(a), Some(b)) if a <= b => set.add(a, b),
                    _ => {
                        set.union(&lo);
                        set.union(&hi);
                        set.add(u('-'), u('-'));
                    }
                }
            } else {
                set.union(&lo);
            }
        }
        if negated { set.negate() } else { set }
    }

    fn class_atom(&mut self) -> CharSet {
        let Some(c) = self.peek() else {
            return CharSet::default();
        };
        self.i += 1;
        if c != u('\\') {
            return CharSet::unit(c);
        }
        let Some(e) = self.peek() else {
            return CharSet::unit(u('\\'));
        };
        match e {
            e if e == u('b') => {
                self.i += 1;
                CharSet::unit(0x08)
            }
            e if e == u('-') => {
                self.i += 1;
                CharSet::unit(u('-'))
            }
            // Inside a class a digit escape is never a backreference.
            e if (u('0')..=u('9')).contains(&e) => CharSet::unit(self.legacy_octal()),
            _ => match self.escape() {
                Escaped::Set(set) => set,
                // Unreachable inside a class; read conservatively as anything.
                Escaped::Assertion | Escaped::Backref(_) => CharSet::default().negate(),
            },
        }
    }

    /// An escape, after the `\`, outside a class (and the shared forms
    /// inside one).
    fn escape(&mut self) -> Escaped {
        let Some(e) = self.peek() else {
            return Escaped::Set(CharSet::unit(u('\\')));
        };
        self.i += 1;
        let ch = char::from_u32(u32::from(e)).unwrap_or('\0');
        Escaped::Set(match ch {
            'd' => digit(),
            'D' => digit().negate(),
            'w' => word(),
            'W' => word().negate(),
            's' => space(),
            'S' => space().negate(),
            'b' | 'B' => return Escaped::Assertion,
            'n' => CharSet::unit(0x0A),
            'r' => CharSet::unit(0x0D),
            't' => CharSet::unit(0x09),
            'v' => CharSet::unit(0x0B),
            'f' => CharSet::unit(0x0C),
            'c' => match self.peek() {
                Some(l) if is_ascii_letter(l) => {
                    self.i += 1;
                    CharSet::unit(l % 32)
                }
                // Annex B: `\c` not followed by a letter is a literal `\`,
                // and the `c` is read next as itself.
                _ => {
                    self.i -= 1;
                    CharSet::unit(u('\\'))
                }
            },
            'x' => match self.hex(2) {
                Some(v) => CharSet::unit(v),
                None => CharSet::unit(u('x')),
            },
            'u' => match self.hex(4) {
                Some(v) => CharSet::unit(v),
                None => CharSet::unit(u('u')),
            },
            '0' if !self.peek().is_some_and(|c| (u('0')..=u('9')).contains(&c)) => CharSet::unit(0),
            '1'..='9' => {
                self.i -= 1;
                let save = self.i;
                let n = self.decimal().unwrap_or(0) as usize;
                if n >= 1 && n <= self.groups.count {
                    return Escaped::Backref(n);
                }
                // Annex B: not a group number, so a legacy octal escape (or a
                // literal `8`/`9`).
                self.i = save;
                CharSet::unit(self.legacy_octal())
            }
            '0' => {
                self.i -= 1;
                CharSet::unit(self.legacy_octal())
            }
            'k' if !self.groups.names.is_empty() && self.eat('<') => {
                let start = self.i;
                while self.peek().is_some_and(|c| c != u('>')) {
                    self.i += 1;
                }
                let name = self.units[start..self.i].to_vec();
                self.eat('>');
                match self.groups.names.get(&name) {
                    Some(&index) => return Escaped::Backref(index),
                    None => CharSet::default().negate(),
                }
            }
            // An identity escape: the character itself.
            _ => CharSet::unit(e),
        })
    }

    /// Annex B's legacy octal escape at the cursor (`\0`–`\377`), or a
    /// literal `8`/`9`.
    fn legacy_octal(&mut self) -> u16 {
        let first = self.peek().unwrap_or(u('0'));
        self.i += 1;
        if first == u('8') || first == u('9') {
            return first;
        }
        let mut value = first - u('0');
        for _ in 0..2 {
            match self.peek() {
                Some(c) if (u('0')..=u('7')).contains(&c) && value * 8 + (c - u('0')) <= 0o377 => {
                    value = value * 8 + (c - u('0'));
                    self.i += 1;
                }
                _ => break,
            }
        }
        value
    }

    fn hex(&mut self, digits: usize) -> Option<u16> {
        let mut value: u16 = 0;
        for k in 0..digits {
            let d = char::from_u32(u32::from(self.peek_at(k)?))?.to_digit(16)?;
            value = value * 16 + d as u16;
        }
        self.i += digits;
        Some(value)
    }
}

fn is_ascii_letter(c: u16) -> bool {
    (u('a')..=u('z')).contains(&c) || (u('A')..=u('Z')).contains(&c)
}

enum Escaped {
    Set(CharSet),
    Assertion,
    Backref(usize),
}

fn single(set: &CharSet) -> Option<u16> {
    match set.0.as_slice() {
        [(a, b)] if a == b => Some(*a),
        _ => None,
    }
}

fn collect_groups(node: &Node, out: &mut HashMap<usize, Node>) {
    match node {
        Node::Empty | Node::Atom { .. } | Node::Backref(_) => {}
        Node::Concat(items) | Node::Alternation(items) => {
            items.iter().for_each(|n| collect_groups(n, out));
        }
        Node::Repeat { node, .. } | Node::Lookaround(node) => collect_groups(node, out),
        Node::Group { index, node } => {
            if let Some(i) = index {
                out.insert(*i, (**node).clone());
            }
            collect_groups(node, out);
        }
    }
}

fn nullable(node: &Node) -> bool {
    match node {
        Node::Empty | Node::Lookaround(_) | Node::Backref(_) => true,
        Node::Atom { .. } => false,
        Node::Concat(items) => items.iter().all(nullable),
        Node::Alternation(items) => items.iter().any(nullable),
        Node::Repeat { node, min, .. } => *min == 0 || nullable(node),
        Node::Group { node, .. } => nullable(node),
    }
}

// ---------------------------------------------------------------------------
// Expansion to a Glushkov expression.
// ---------------------------------------------------------------------------

/// A regular expression over numbered positions, with bounded repeats
/// expanded away.
enum Rx {
    Eps,
    Pos(usize),
    Cat(Vec<Rx>),
    Alt(Vec<Rx>),
    Star(Box<Rx>),
    Plus(Box<Rx>),
    Opt(Box<Rx>),
}

struct Position {
    set: CharSet,
    src: (usize, usize),
}

struct Expander<'a> {
    positions: Vec<Position>,
    groups: &'a HashMap<usize, Node>,
    /// Capture groups the expansion is currently inside.
    open_groups: Vec<usize>,
    /// Groups being expanded for a backreference, to stop a cycle.
    following: Vec<usize>,
    /// Lookaround bodies found, for separate analysis, and the nodes they
    /// came from: a lookaround inside a repeat is expanded once per copy but
    /// analysed once.
    lookarounds: Vec<Node>,
    /// Lookaround and bounded-repeat nodes already recorded, by identity in
    /// the parsed pattern.
    seen_nodes: Vec<*const Node>,
    /// The bodies of bounded repeats with a maximum of two or more, each
    /// checked on its own as a loop (see `analyse`).
    bounded_bodies: Vec<Node>,
}

impl Expander<'_> {
    fn expand(&mut self, node: &Node, in_loop: bool) -> Result<Rx, &'static str> {
        Ok(match node {
            Node::Empty => Rx::Eps,
            Node::Atom { set, src } => {
                if self.positions.len() >= MAX_POSITIONS {
                    return Err("it is too large to analyse for catastrophic backtracking");
                }
                self.positions.push(Position {
                    set: set.clone(),
                    src: *src,
                });
                Rx::Pos(self.positions.len() - 1)
            }
            Node::Concat(items) => Rx::Cat(
                items
                    .iter()
                    .map(|n| self.expand(n, in_loop))
                    .collect::<Result<_, _>>()?,
            ),
            Node::Alternation(items) => Rx::Alt(
                items
                    .iter()
                    .map(|n| self.expand(n, in_loop))
                    .collect::<Result<_, _>>()?,
            ),
            Node::Group { index, node } => {
                if let Some(i) = index {
                    self.open_groups.push(*i);
                }
                let rx = self.expand(node, in_loop);
                if index.is_some() {
                    self.open_groups.pop();
                }
                rx?
            }
            Node::Lookaround(body) => {
                let key: *const Node = &**body;
                if !self.seen_nodes.contains(&key) {
                    self.seen_nodes.push(key);
                    self.lookarounds.push((**body).clone());
                }
                Rx::Eps
            }
            Node::Backref(index) => {
                if in_loop {
                    return Err("it repeats a backreference without bound");
                }
                if self.open_groups.contains(index) || self.following.contains(index) {
                    return Err("a backreference refers to a group that contains it");
                }
                let Some(group) = self.groups.get(index) else {
                    return Ok(Rx::Eps);
                };
                self.following.push(*index);
                let rx = self.expand(group, in_loop);
                self.following.pop();
                Rx::Opt(Box::new(rx?))
            }
            Node::Repeat { node, min, max } => {
                let max = max.filter(|&m| m <= REPEAT_CAP);
                let min = (*min).min(REPEAT_CAP);
                match max {
                    None => {
                        if nullable(node) {
                            return Err("it repeats, without bound, a part that can match nothing");
                        }
                        let mut parts = Vec::new();
                        for _ in 1..min {
                            parts.push(self.expand(node, true)?);
                        }
                        let body = Box::new(self.expand(node, true)?);
                        parts.push(if min == 0 {
                            Rx::Star(body)
                        } else {
                            Rx::Plus(body)
                        });
                        Rx::Cat(parts)
                    }
                    Some(max) => {
                        let key: *const Node = &**node;
                        if max >= 2 && !self.seen_nodes.contains(&key) {
                            self.seen_nodes.push(key);
                            self.bounded_bodies.push((**node).clone());
                        }
                        let mut parts = Vec::new();
                        for _ in 0..min.min(max) {
                            parts.push(self.expand(node, in_loop)?);
                        }
                        // `max - min` optional copies, each nested inside
                        // the one before: `x(x(x)?)?`.
                        let mut tail = Rx::Eps;
                        for _ in min.min(max)..max {
                            let copy = self.expand(node, in_loop)?;
                            tail = Rx::Opt(Box::new(Rx::Cat(vec![copy, tail])));
                        }
                        parts.push(tail);
                        Rx::Cat(parts)
                    }
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// The Glushkov automaton and the ambiguity tests.
// ---------------------------------------------------------------------------

/// Build the Glushkov automaton for `rx` over `positions` and decide its
/// ambiguity.
fn decide(positions: &[Position], rx: &Rx, units: &[u16], exponential_only: bool) -> Verdict {
    let n = positions.len();
    let mut follow: Vec<Vec<usize>> = vec![Vec::new(); n];
    let (_, first, _) = glushkov(rx, &mut follow);

    // State 0 is the start; position `p` is state `p + 1`. A position whose
    // set is empty (`[]`) can never be entered, so it gets no in-edges.
    let live = |p: &usize| !positions[*p].set.is_empty();
    let mut succ: Vec<Vec<usize>> = Vec::with_capacity(n + 1);
    succ.push(dedup(first.iter().filter(|p| live(p)).map(|p| p + 1)));
    for f in &follow {
        succ.push(dedup(f.iter().filter(|p| live(p)).map(|p| p + 1)));
    }
    let set = |s: usize| &positions[s - 1].set;
    let src = |s: usize| {
        let (a, b) = positions[s - 1].src;
        String::from_utf16_lossy(&units[a..b])
    };

    // A transition the construction derived twice is two distinct paths
    // between the same states on the same character. On a cycle that doubles
    // the paths per traversal: `(a+)+` is one position whose `a → a` edge comes
    // from both the inner and the outer loop. (The nested-quantifier first pass
    // rejects this shape before it gets here; the check keeps this analysis
    // sound on its own.)
    let cycles = tarjan(&succ);
    for (x, f) in follow.iter().enumerate() {
        let mut targets: Vec<usize> = f.iter().copied().filter(|p| live(p)).collect();
        targets.sort_unstable();
        if let Some(w) = targets.windows(2).find(|w| w[0] == w[1])
            && cycles[x + 1] == cycles[w[0] + 1]
        {
            return Verdict::Exponential(src(x + 1), src(w[0] + 1));
        }
    }

    // Every phase below draws on one work budget, so the analysis is bounded by
    // it, not only by the position count: a wide same-character alternation
    // under a loop has quadratically many pairs, each with quadratically many
    // successor combinations.
    let mut budget = WORK_BUDGET;
    let too_complex =
        || Verdict::Unanalysable("it is too complex to analyse for catastrophic backtracking");

    // The pairs of A×A reachable from (start, start).
    let mut index: HashMap<(usize, usize), usize> = HashMap::new();
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut edges: Vec<Vec<usize>> = Vec::new();
    index.insert((0, 0), 0);
    pairs.push((0, 0));
    edges.push(Vec::new());
    let mut k = 0;
    while k < pairs.len() {
        let (p, q) = pairs[k];
        for &p2 in &succ[p] {
            for &q2 in &succ[q] {
                if !spend(&mut budget) {
                    return too_complex();
                }
                if !set(p2).intersects(set(q2)) {
                    continue;
                }
                let next = *index.entry((p2, q2)).or_insert_with(|| {
                    pairs.push((p2, q2));
                    edges.push(Vec::new());
                    pairs.len() - 1
                });
                edges[k].push(next);
            }
        }
        k += 1;
    }

    // EDA: an SCC holding a diagonal and an off-diagonal pair.
    let scc = tarjan(&edges);
    let mut has_diagonal = vec![false; pairs.len()];
    let mut off_diagonal: Vec<Option<usize>> = vec![None; pairs.len()];
    for (i, &(p, q)) in pairs.iter().enumerate() {
        if p == q {
            has_diagonal[scc[i]] = true;
        } else {
            off_diagonal[scc[i]].get_or_insert(i);
        }
    }
    for c in 0..pairs.len() {
        if let (true, Some(i)) = (has_diagonal[c], off_diagonal[c]) {
            let (p, q) = pairs[i];
            return Verdict::Exponential(src(p), src(q));
        }
    }

    if exponential_only {
        return Verdict::Linear;
    }

    // IDA: (p, p, q) ⇝ (p, q, q) in A³ for p ≠ q. A candidate (p, q) must lie
    // on a path (p, p) ⇝ (p, q) ⇝ (q, q) in A×A, which prunes the search to
    // the few pairs where it can succeed. Both reachability sets are cached per
    // diagonal pair, forward from (p, p) and backward to (q, q).
    let mut reverse: Vec<Vec<usize>> = vec![Vec::new(); pairs.len()];
    for (x, out) in edges.iter().enumerate() {
        for &y in out {
            reverse[y].push(x);
        }
    }
    let mut forward_from: HashMap<usize, Vec<bool>> = HashMap::new();
    let mut backward_to: HashMap<usize, Vec<bool>> = HashMap::new();
    // Each IDA witness links the loop holding `p` to the loop holding `q`;
    // `q`'s loop is downstream of `p`'s, so the links form a DAG. Its longest
    // chain of loops is the polynomial's degree.
    let mut links: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut witness = None;
    for (i, &(p, q)) in pairs.iter().enumerate() {
        // Without EDA, an IDA witness never has both states in one loop.
        if p == q || cycles[p] == cycles[q] {
            continue;
        }
        if links
            .get(&cycles[p])
            .is_some_and(|l| l.contains(&cycles[q]))
        {
            continue;
        }
        let (Some(&pp), Some(&qq)) = (index.get(&(p, p)), index.get(&(q, q))) else {
            continue;
        };
        if let std::collections::hash_map::Entry::Vacant(slot) = forward_from.entry(pp) {
            let Some(seen) = reach(&edges, pp, &mut budget) else {
                return too_complex();
            };
            slot.insert(seen);
        }
        if let std::collections::hash_map::Entry::Vacant(slot) = backward_to.entry(qq) {
            let Some(seen) = reach(&reverse, qq, &mut budget) else {
                return too_complex();
            };
            slot.insert(seen);
        }
        if !forward_from[&pp][i] || !backward_to[&qq][i] {
            continue;
        }
        match triple_path(&succ, &set, (p, p, q), (p, q, q), &mut budget) {
            Some(true) => {
                links.entry(cycles[p]).or_default().push(cycles[q]);
                witness.get_or_insert((p, q));
            }
            Some(false) => {}
            None => return too_complex(),
        }
    }
    let Some((p, q)) = witness else {
        return Verdict::Linear;
    };
    let mut memo = HashMap::new();
    let degree = links
        .keys()
        .map(|&c| chain_length(c, &links, &mut memo))
        .max()
        .unwrap_or(2);
    Verdict::Polynomial(degree, src(p), src(q))
}

/// Charge one unit of work; `false` once the budget is spent.
fn spend(budget: &mut usize) -> bool {
    match budget.checked_sub(1) {
        Some(b) => {
            *budget = b;
            true
        }
        None => false,
    }
}

/// The nodes reachable from `from` by a non-empty path over `edges`, or
/// `None` when the budget runs out.
fn reach(edges: &[Vec<usize>], from: usize, budget: &mut usize) -> Option<Vec<bool>> {
    let mut seen = vec![false; edges.len()];
    let mut stack = vec![from];
    while let Some(x) = stack.pop() {
        for &y in &edges[x] {
            if !spend(budget) {
                return None;
            }
            if !seen[y] {
                seen[y] = true;
                stack.push(y);
            }
        }
    }
    Some(seen)
}

/// How many loops the longest chain of IDA links starting at loop `c` holds.
fn chain_length(
    c: usize,
    links: &HashMap<usize, Vec<usize>>,
    memo: &mut HashMap<usize, u32>,
) -> u32 {
    if let Some(&n) = memo.get(&c) {
        return n;
    }
    let below = links
        .get(&c)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|d| chain_length(d, links, memo))
        .max()
        .unwrap_or(0);
    memo.insert(c, below + 1);
    below + 1
}

/// Is there a non-empty path from `from` to `to` in A³ whose three
/// components read the same word? `None` when the budget runs out.
fn triple_path<'a>(
    succ: &[Vec<usize>],
    set: &impl Fn(usize) -> &'a CharSet,
    from: (usize, usize, usize),
    to: (usize, usize, usize),
    budget: &mut usize,
) -> Option<bool> {
    let mut seen: HashMap<(usize, usize, usize), ()> = HashMap::new();
    let mut queue = VecDeque::from([from]);
    while let Some((a, b, c)) = queue.pop_front() {
        for &a2 in &succ[a] {
            for &b2 in &succ[b] {
                if !spend(budget) {
                    return None;
                }
                let ab = set(a2).intersect(set(b2));
                if ab.is_empty() {
                    continue;
                }
                for &c2 in &succ[c] {
                    if !spend(budget) {
                        return None;
                    }
                    if !ab.intersects(set(c2)) {
                        continue;
                    }
                    let next = (a2, b2, c2);
                    if next == to {
                        return Some(true);
                    }
                    if seen.insert(next, ()).is_none() {
                        queue.push_back(next);
                    }
                }
            }
        }
    }
    Some(false)
}

/// `(nullable, first, last)` of `rx`, adding its follow edges to `follow`.
fn glushkov(rx: &Rx, follow: &mut [Vec<usize>]) -> (bool, Vec<usize>, Vec<usize>) {
    match rx {
        Rx::Eps => (true, Vec::new(), Vec::new()),
        Rx::Pos(p) => (false, vec![*p], vec![*p]),
        Rx::Cat(items) => {
            let mut acc: (bool, Vec<usize>, Vec<usize>) = (true, Vec::new(), Vec::new());
            for item in items {
                let (n2, f2, l2) = glushkov(item, follow);
                for &x in &acc.2 {
                    follow[x].extend(&f2);
                }
                if acc.0 {
                    acc.1.extend(&f2);
                }
                acc.2 = if n2 {
                    let mut l = acc.2;
                    l.extend(l2);
                    l
                } else {
                    l2
                };
                acc.0 &= n2;
            }
            acc
        }
        Rx::Alt(items) => {
            let mut acc: (bool, Vec<usize>, Vec<usize>) = (false, Vec::new(), Vec::new());
            for item in items {
                let (n2, f2, l2) = glushkov(item, follow);
                acc.0 |= n2;
                acc.1.extend(f2);
                acc.2.extend(l2);
            }
            acc
        }
        Rx::Star(body) | Rx::Plus(body) => {
            let (n, f, l) = glushkov(body, follow);
            for &x in &l {
                follow[x].extend(&f);
            }
            (n || matches!(rx, Rx::Star(_)), f, l)
        }
        Rx::Opt(body) => {
            let (_, f, l) = glushkov(body, follow);
            (true, f, l)
        }
    }
}

fn dedup(it: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut v: Vec<usize> = it.collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The strongly connected component of each node (iterative Tarjan).
fn tarjan(edges: &[Vec<usize>]) -> Vec<usize> {
    let n = edges.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack = Vec::new();
    let mut comp = vec![usize::MAX; n];
    let mut next_index = 0;
    let mut next_comp = 0;
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        let mut call: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some(&mut (v, ref mut edge)) = call.last_mut() {
            if *edge == 0 && index[v] == usize::MAX {
                index[v] = next_index;
                low[v] = next_index;
                next_index += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if let Some(&w) = edges[v].get(*edge) {
                *edge += 1;
                if index[w] == usize::MAX {
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            call.pop();
            if let Some(&(parent, _)) = call.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    comp[w] = next_comp;
                    if w == v {
                        break;
                    }
                }
                next_comp += 1;
            }
        }
    }
    comp
}

#[cfg(test)]
mod tests {
    use super::{Verdict, analyse};

    fn exponential(p: &str) -> bool {
        matches!(analyse(p), Verdict::Exponential(..))
    }

    fn polynomial(p: &str) -> bool {
        matches!(analyse(p), Verdict::Polynomial(..))
    }

    fn linear(p: &str) -> bool {
        analyse(p) == Verdict::Linear
    }

    #[test]
    fn flags_the_exponential_shapes_from_the_track_table() {
        for p in [
            "(a|a)+",
            "(\\d|\\d\\d)+",
            "(a|aa)*",
            "(\\w|\\d)+",
            "(a{1,2})+",
            "(a{2,3})+",
            "(a?a)+",
            "(a|ab|b)*",
            "(?:x|[a-z])+",
            "(a+)+",
            "(?:a|b+)+",
        ] {
            assert!(
                exponential(p),
                "`{p}` should be exponential: {:?}",
                analyse(p)
            );
        }
    }

    #[test]
    fn flags_polynomial_ambiguity() {
        for p in ["\\d*\\d*", "a*a*", "[a-z]+[a-z0-9]+", "\\w+\\d+", ".*.*=.*"] {
            assert!(
                polynomial(p),
                "`{p}` should be polynomial: {:?}",
                analyse(p)
            );
        }
    }

    #[test]
    fn accepts_unambiguous_loops() {
        for p in [
            "(foo|foobar)+",
            "(a|b)+",
            "(foo|bar)+",
            "(ab)+",
            "a+b+",
            "[a-z]+-[0-9]+",
            "a{2,}b{2,}",
            "(a+)?",
            "x(a|b)*y",
            "a{1,1000}",
            "(?:ab){100}",
            "",
            "abc",
            "a|b|c",
        ] {
            assert!(linear(p), "`{p}` should be linear: {:?}", analyse(p));
        }
    }

    #[test]
    fn bounded_repeats_expand_as_nested_optionals() {
        // The flat form `aaa?a?` would read as ambiguous; the backtracker's
        // `a{2,4}` is not.
        assert!(linear("a{2,4}"));
        assert!(linear("(?:ab){2,5}c"));
    }

    #[test]
    fn analyses_lookaround_bodies_on_their_own() {
        assert!(linear("[a-z]+(?<=ing)"));
        assert!(exponential("(?=(a|a)+b)a"));
        assert!(exponential("x(?<!(a|a)+)"));
    }

    #[test]
    fn rejects_a_backreference_under_an_unbounded_quantifier() {
        // The body has a real character, so the nullable-body rule does not
        // answer first: this is the backreference rule.
        for p in ["(a)(?:b\\1)+", "(?<x>a)(?:\\k<x>b)*"] {
            assert_eq!(
                analyse(p),
                Verdict::Unanalysable("it repeats a backreference without bound"),
                "`{p}`"
            );
        }
    }

    #[test]
    fn reads_a_bounded_backreference_as_a_copy_of_its_group() {
        assert!(linear("(ab)c\\1"));
        // The copy is optional, so it can meet a following loop.
        assert!(polynomial("(a+)b\\1a*"));
    }

    #[test]
    fn rejects_a_nullable_body_under_an_unbounded_quantifier() {
        assert!(matches!(analyse("(a?b?)*"), Verdict::Unanalysable(_)));
        assert!(matches!(analyse("(?:|a)+"), Verdict::Unanalysable(_)));
    }

    #[test]
    fn decimal_escapes_are_octal_when_there_is_no_such_group() {
        // `\1` with no group is the octal escape U+0001, not a backreference.
        assert!(linear("\\1+"));
        assert!(linear("[\\1-\\7]+"));
    }

    #[test]
    fn a_brace_that_is_not_a_quantifier_is_a_literal() {
        assert!(linear("a{,3}"));
        assert!(linear("{x}+"));
    }

    #[test]
    fn classes_intersect_by_code_unit() {
        // Disjoint classes under one loop are unambiguous; overlapping ones
        // are not.
        assert!(linear("([a-f]|[g-z])+"));
        assert!(exponential("([a-f]|[f-z])+"));
        assert!(linear("(\\d|[^\\d])+"));
        assert!(exponential("(\\s|\\u00a0)+"));
        assert!(linear("(.|\\n)+"));
    }

    #[test]
    fn the_degree_is_the_longest_chain_of_linked_loops() {
        let degree = |p: &str| match analyse(p) {
            Verdict::Polynomial(d, ..) => d,
            other => panic!("`{p}` should be polynomial: {other:?}"),
        };
        assert_eq!(degree("\\d*\\d*"), 2);
        assert_eq!(degree("\\d*\\d*\\d*"), 3);
        assert_eq!(degree("\\d*\\d*\\d*\\d*"), 4);
        // Two independent quadratic pairs, separated by a literal, are still
        // quadratic.
        assert_eq!(degree("\\d*\\d*-\\d*\\d*"), 2);
    }

    #[test]
    fn the_work_budget_bounds_wide_alternations() {
        // Each of these would do ~10¹⁰ work in A×A without the budget. The
        // test is that they return at all; either verdict rejects or proves.
        let same = format!("(?:{})+", vec!["a"; 500].join("|"));
        assert_ne!(analyse(&same), Verdict::Linear);
        let words: Vec<String> = (0..200).map(|i| format!("w{i:03}x")).collect();
        let distinct = format!("(?:{})+", words.join("|"));
        assert!(matches!(
            analyse(&distinct),
            Verdict::Linear | Verdict::Unanalysable(_)
        ));
    }

    #[test]
    fn a_bounded_repeat_of_an_ambiguous_body_is_exponential() {
        // Exponential in the count, not the input: V8 takes 653 ms on
        // `(?:a|a){0,24}` with 23 `a`s, and the count can reach 64.
        assert!(exponential("(?:a|a){0,24}"));
        assert!(exponential("(?:a{1,2}){0,20}"));
        assert!(exponential("(?:a|a){2,3}"));
        // A bounded repeat of an unambiguous body stays linear.
        assert!(linear("(?:ab?){0,8}"));
        assert!(linear("(?:-[a-z0-9]{2,8}){1,8}"));
        // 64 identical optional copies are too many paths to analyse within
        // the budget, so the pattern is rejected, conservatively: V8 itself is
        // fast here only because the spec fails empty iterations.
        assert!(matches!(analyse("(?:a?){0,64}"), Verdict::Unanalysable(_)));
    }

    #[test]
    fn a_repeat_count_above_the_cap_is_read_as_unbounded() {
        // In the exact expansion, `{0,100}` is over-approximated as `*`, which
        // then trips the nullable-body rule. Conservative, and pinned.
        assert!(matches!(analyse("(?:a?){0,100}"), Verdict::Unanalysable(_)));
    }

    #[test]
    fn rejects_patterns_too_large_to_analyse() {
        let huge = "(?:[a-z]{64}){64}";
        assert!(matches!(analyse(huge), Verdict::Unanalysable(_)));
    }
}
