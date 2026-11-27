//! The HTML tokeniser behind typed templates (typed-HTML track §5.2, §5.3, §6;
//! the context-model ADR).
//!
//! A hand-written subset of the WHATWG tokeniser: data, RCDATA, RAWTEXT, script
//! data, PLAINTEXT, tag open, the tag-name and attribute states, comment,
//! doctype and CDATA in foreign content. It records the state at each
//! placeholder ("hole") and the element and attribute around it, which is the
//! hole's context. It does not build a tree. A tree builder's error recovery is
//! what the model refuses, not what it reproduces, so every input a browser
//! accepts only by recovering in a way that changes a context is refused, as is
//! every state the model does not implement.
//!
//! The tree builder's effect on the tokeniser is modelled where it matters: the
//! elements that switch it to RCDATA, RAWTEXT, script data or PLAINTEXT
//! (scripting on, so `<noscript>` is RAWTEXT), foreign content (`<svg>`,
//! `<math>`), whose subtree must close in the same piece and may hold no
//! placeholder, and `<select>`, whose insertion mode ignores tags that would
//! switch the tokeniser, so its content is limited to `<option>`, `<optgroup>`
//! and `<hr>` and it too must close in the same piece. `<frameset>` and
//! `<frame>` are refused.
//!
//! The input is a sequence of [`Unit`]s: characters with their byte offsets,
//! and holes. A hole stands for a value the compiler escapes for its context,
//! which therefore adds only inert characters to the state it is in. A hole in
//! a state where its characters would matter (a tag name, the middle of a
//! character reference, after `</` in RCDATA) is refused.

/// One unit of input: a character at a byte offset, or a hole (its index and
/// the byte offset it sits at).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Char(char, usize),
    Hole(usize, usize),
}

/// Why the tokeniser refuses a hole or the markup around it. The checker turns each into its
/// diagnostic code (`crate::templates`); the tokeniser itself names no code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Markup the tokeniser does not model, or error recovery that changes a context.
    Unsupported,
    /// C4: a hole in an unquoted attribute value.
    Unquoted,
    /// C8: a hole inside `<script>`.
    InScript,
    /// C10: a hole inside `<style>` or a `style` attribute.
    InStyle,
    /// C11: a hole in an attribute whose value is code or decides how markup is read.
    InCodeAttribute,
    /// C13: a hole in the markup's structure or inside foreign content.
    Position,
    /// C14: a hole in raw text, or a `<noscript>` left open.
    InRawText,
    /// An `<svg>` or `<math>` left open, closed without an opener, or holding an
    /// integration point or an HTML element.
    ForeignUnbalanced,
}

/// Where a refusal points: a byte offset in the input, or a hole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    Offset(usize),
    Hole(usize),
}

/// Why a piece of markup, or a hole in it, is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: Kind,
    pub at: At,
    pub message: String,
    pub note: Option<String>,
}

impl Refusal {
    fn new(code: Kind, at: At, message: impl Into<String>) -> Self {
        Self {
            code,
            at,
            message: message.into(),
            note: None,
        }
    }

    fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// The context of an accepted hole (§5.2). Every other context is a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Context {
    /// C1: element text in the data state. A scalar is escaped as text, and an
    /// `Html` value is inserted as is.
    Text,
    /// C1 inside a `<select>`, where the tree builder ignores the tags that
    /// would switch the tokeniser, so an `Html` value could leave it in another
    /// state: a scalar only, escaped as text.
    SelectText,
    /// C2: the text of `<title>` or `<textarea>`. A scalar only, escaped as text.
    Rcdata { element: String },
    /// C3: a quoted value of an ordinary attribute. A scalar only, escaped for an
    /// attribute.
    Attr { element: String, attr: String },
}

/// A token, for the conformance test against the `html5lib-tests` corpus.
/// Character references are not decoded: the tokeniser never needs their
/// value, only that they do not change the state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Doctype(String),
    StartTag {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    EndTag(String),
    Comment(String),
    Chars(String),
}

/// What the tokeniser found: each hole's context or refusal (by hole index;
/// `None` for a hole after the markup was refused, which is not judged), the
/// refusals of the markup itself, and the tokens.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub holes: Vec<Option<Result<Context, Refusal>>>,
    pub markup: Vec<Refusal>,
    pub tokens: Vec<Token>,
}

/// The tokeniser states a run can start in. A template or fragment starts in
/// [`Initial::Data`]; the others exist for the conformance corpus, which tests
/// each state on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Initial {
    Data,
    Rcdata(String),
    Rawtext(String),
    /// Script data, ended by the end tag of the element named (`script`, in a page).
    ScriptData(String),
    Plaintext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Rcdata,
    Rawtext,
    ScriptData,
    Plaintext,
    TagName,
    EndTagName,
    EndTagAfterName,
    BeforeAttrName,
    AttrName,
    AfterAttrName,
    BeforeAttrValue,
    AttrValueDq,
    AttrValueSq,
    AttrValueUnquoted,
    AfterAttrValueQuoted,
    SelfClosingStartTag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ns {
    Svg,
    Math,
}

/// A hole inside an attribute value, judged when its tag ends, because what
/// the attribute means can depend on an attribute after it
/// (`<meta content="…" http-equiv="refresh">`).
#[derive(Debug, Clone)]
struct AttrHole {
    hole: usize,
    attr: usize,
}

#[derive(Debug, Clone, Default)]
struct Tag {
    name: String,
    end: bool,
    self_closing: bool,
    start: usize,
    attrs: Vec<(String, String)>,
    holes: Vec<AttrHole>,
}

fn is_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | ' ')
}

/// HTML elements that make the tree builder leave foreign content (WHATWG
/// "in foreign content", the breakout list).
const FOREIGN_BREAKOUT: &[&str] = &[
    "b",
    "big",
    "blockquote",
    "body",
    "br",
    "center",
    "code",
    "dd",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "font",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "hr",
    "i",
    "img",
    "li",
    "listing",
    "menu",
    "meta",
    "nobr",
    "ol",
    "p",
    "pre",
    "ruby",
    "s",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "sup",
    "table",
    "tt",
    "u",
    "ul",
    "var",
];

/// The attributes whose literal value decides how another part of the element
/// is read, so a placeholder in one could change another's context.
fn decides_another(element: &str, attr: &str) -> bool {
    matches!(
        (element, attr),
        ("script", "type")
            | ("script", "language")
            | ("meta", "http-equiv")
            | ("meta", "charset")
            | ("link", "rel")
            | ("iframe", "sandbox")
    )
}

/// URL attributes (§5.2): Go's `html/template` list, with its rule that a name
/// containing `src`, `uri` or `url` (after any `data-` prefix) is a URL.
pub fn is_url_attribute(attr: &str) -> bool {
    const URL_ATTRS: &[&str] = &[
        "action",
        "archive",
        "background",
        "cite",
        "classid",
        "codebase",
        "data",
        "formaction",
        "href",
        "icon",
        "longdesc",
        "manifest",
        "ping",
        "poster",
        "profile",
        "src",
        "usemap",
        "xlink:href",
        "xmlns",
    ];
    if URL_ATTRS.contains(&attr) {
        return true;
    }
    let bare = attr.strip_prefix("data-").unwrap_or(attr);
    bare.contains("src") || bare.contains("uri") || bare.contains("url")
}

/// `srcset` and `imagesrcset` (C12): lists of URLs with their own grammar.
pub fn is_srcset_attribute(attr: &str) -> bool {
    matches!(attr, "srcset" | "imagesrcset")
}

struct Tok<'a> {
    units: &'a [Unit],
    i: usize,
    state: State,
    tree: bool,
    tag: Tag,
    /// The element whose end tag ends the current RCDATA, RAWTEXT or script data.
    raw: String,
    raw_at: usize,
    foreign: Vec<(String, Ns)>,
    foreign_at: usize,
    select_at: Option<usize>,
    /// A literal `&` began a character reference that has not ended yet.
    charref: bool,
    text: String,
    out: Analysis,
    failed: bool,
}

impl<'a> Tok<'a> {
    fn offset(&self, k: usize) -> usize {
        match self.units.get(k) {
            Some(Unit::Char(_, o)) | Some(Unit::Hole(_, o)) => *o,
            None => self.units.last().map_or(0, |u| match u {
                Unit::Char(c, o) => o + c.len_utf8(),
                Unit::Hole(_, o) => *o,
            }),
        }
    }

    fn char_at(&self, k: usize) -> Option<char> {
        match self.units.get(k) {
            Some(Unit::Char(c, _)) => Some(*c),
            _ => None,
        }
    }

    /// Whether the characters from `k` spell `word`, ignoring ASCII case.
    fn spells(&self, k: usize, word: &str) -> bool {
        word.chars().enumerate().all(|(n, w)| {
            self.char_at(k + n)
                .is_some_and(|c| c.eq_ignore_ascii_case(&w))
        })
    }

    fn refuse(&mut self, r: Refusal) {
        self.out.markup.push(r);
        self.failed = true;
    }

    fn refuse_at(&mut self, k: usize, code: Kind, message: impl Into<String>) {
        let at = At::Offset(self.offset(k));
        self.refuse(Refusal::new(code, at, message));
    }

    fn judge(&mut self, hole: usize, outcome: Result<Context, Refusal>) {
        if self.out.holes.len() <= hole {
            self.out.holes.resize(hole + 1, None);
        }
        // A hole refused once stays refused: a later look at the same hole
        // (after `<` in RCDATA, say) must not accept it.
        if !matches!(self.out.holes[hole], Some(Err(_))) {
            self.out.holes[hole] = Some(outcome);
        }
    }

    fn refuse_hole(&mut self, hole: usize, code: Kind, message: impl Into<String>) {
        self.judge(hole, Err(Refusal::new(code, At::Hole(hole), message)));
    }

    fn refuse_hole_with(&mut self, hole: usize, r: Refusal) {
        self.judge(hole, Err(r));
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.out.tokens.push(Token::Chars(text));
        }
    }

    /// A hole in the middle of a tag, wherever it is not an attribute value.
    fn hole_in_tag(&mut self, hole: usize, place: &str) {
        self.refuse_hole_with(
            hole,
            Refusal::new(Kind::Position, At::Hole(hole), format!("a placeholder cannot be {place}"))
                .note("a placeholder fills element text or a quoted attribute value, never the markup's structure"),
        );
    }

    /// The context of a hole in element text, or its refusal.
    fn text_hole(&mut self, hole: usize) {
        if self.charref {
            self.refuse_hole(
                hole,
                Kind::Unsupported,
                "a placeholder cannot continue a character reference",
            );
        } else if !self.foreign.is_empty() {
            self.refuse_foreign_hole(hole);
        } else if self.select_at.is_some() {
            self.judge(hole, Ok(Context::SelectText));
        } else {
            self.judge(hole, Ok(Context::Text));
        }
    }

    fn refuse_foreign_hole(&mut self, hole: usize) {
        let root = self
            .foreign
            .first()
            .map_or("svg", |(n, _)| n.as_str())
            .to_string();
        self.refuse_hole_with(
            hole,
            Refusal::new(Kind::Position, At::Hole(hole), format!("a placeholder cannot be inside `<{root}>`"))
                .note("SVG and MathML are parsed by different rules, so a placeholder is refused anywhere inside them"),
        );
    }

    fn run(&mut self) {
        while self.i < self.units.len() && !self.failed {
            match self.state {
                State::Data => self.data(),
                State::Rcdata | State::Rawtext | State::ScriptData => self.raw_text(),
                State::Plaintext => self.plaintext(),
                _ => self.in_tag(),
            }
        }
        if !self.failed {
            self.finish();
        }
    }

    fn data(&mut self) {
        let unit = self.units[self.i];
        match unit {
            Unit::Hole(h, _) => {
                self.text_hole(h);
                self.i += 1;
            }
            Unit::Char('<', _) => self.tag_open(),
            Unit::Char(c, _) => {
                self.track_charref(c);
                self.text.push(c);
                self.i += 1;
            }
        }
    }

    fn track_charref(&mut self, c: char) {
        if c == '&' {
            self.charref = true;
        } else if self.charref && !(c.is_ascii_alphanumeric() || c == '#') {
            self.charref = false;
        }
    }

    /// At a `<` in the data state.
    fn tag_open(&mut self) {
        let start = self.i;
        match self.units.get(self.i + 1).copied() {
            Some(Unit::Char('!', _)) => self.markup_declaration(),
            Some(Unit::Char('/', _)) => match self.units.get(self.i + 2).copied() {
                Some(Unit::Char(c, _)) if c.is_ascii_alphabetic() => {
                    self.flush_text();
                    self.charref = false;
                    self.tag = Tag {
                        end: true,
                        start: self.offset(start),
                        ..Tag::default()
                    };
                    self.i += 2;
                    self.state = State::EndTagName;
                }
                Some(Unit::Hole(h, _)) => {
                    self.hole_in_tag(h, "in an end tag's name");
                    self.text.push_str("</");
                    self.i += 3;
                }
                None => {
                    self.text.push_str("</");
                    self.i += 2;
                }
                Some(Unit::Char('>', _)) => self.refuse_at(
                    start,
                    Kind::Unsupported,
                    "`</>` is not an end tag, and a browser drops it",
                ),
                Some(Unit::Char(..)) => self.refuse_at(
                    start,
                    Kind::Unsupported,
                    "`</` not followed by a tag name is read as a comment by a browser",
                ),
            },
            Some(Unit::Char(c, _)) if c.is_ascii_alphabetic() => {
                self.flush_text();
                self.charref = false;
                self.tag = Tag {
                    start: self.offset(start),
                    ..Tag::default()
                };
                self.i += 1;
                self.state = State::TagName;
            }
            Some(Unit::Char('?', _)) => self.refuse_at(
                start,
                Kind::Unsupported,
                "`<?` is read as a comment by a browser; HTML has no processing instructions",
            ),
            Some(Unit::Hole(h, _)) => {
                self.hole_in_tag(h, "a tag name");
                self.text.push('<');
                self.i += 2;
            }
            _ => {
                self.charref = false;
                self.text.push('<');
                self.i += 1;
            }
        }
    }

    /// At `<!`: a comment, a doctype, CDATA in foreign content, or refused.
    fn markup_declaration(&mut self) {
        let start = self.i;
        let k = self.i + 2;
        if self.spells(k, "--") {
            self.comment(start, k + 2);
        } else if self.spells(k, "doctype") {
            self.doctype(start, k + 7);
        } else if self.spells(k, "[CDATA[") {
            if self.foreign.is_empty() {
                self.refuse_at(
                    start,
                    Kind::Unsupported,
                    "`<![CDATA[` outside SVG or MathML is read as a comment by a browser",
                );
            } else {
                self.cdata(k + 7);
            }
        } else if let Some(h) = self.hole_within(k, 9) {
            self.hole_in_tag(h, "in a comment or doctype");
            self.refuse_at(
                start,
                Kind::Unsupported,
                "`<!` must start a comment or a doctype",
            );
        } else {
            self.refuse_at(
                start,
                Kind::Unsupported,
                "`<!` that starts neither a comment nor a doctype is read as a comment by a browser",
            );
        }
    }

    /// The first hole among the `n` units from `k`.
    fn hole_within(&self, k: usize, n: usize) -> Option<usize> {
        (k..k + n).find_map(|j| match self.units.get(j) {
            Some(Unit::Hole(h, _)) => Some(*h),
            _ => None,
        })
    }

    /// Scan from `k` to the first `end`, refusing the holes on the way. The
    /// index just past `end`, or `None` when the input ends first.
    fn scan_to(&mut self, k: usize, end: &str, place: &str, data: &mut String) -> Option<usize> {
        let mut j = k;
        while j < self.units.len() {
            if self.spells(j, end) {
                return Some(j + end.chars().count());
            }
            match self.units[j] {
                Unit::Hole(h, _) => self.hole_in_tag(h, place),
                Unit::Char(c, _) => data.push(c),
            }
            j += 1;
        }
        None
    }

    fn comment(&mut self, start: usize, k: usize) {
        if self.spells(k, ">") || self.spells(k, "->") {
            self.refuse_at(
                start,
                Kind::Unsupported,
                "`<!-->` and `<!--->` end a comment early in a browser",
            );
            return;
        }
        let mut data = String::new();
        match self.scan_to(k, "-->", "in a comment", &mut data) {
            Some(next) => {
                if data.contains("--!>") {
                    self.refuse_at(
                        start,
                        Kind::Unsupported,
                        "`--!>` ends a comment early in a browser",
                    );
                    return;
                }
                self.flush_text();
                self.out.tokens.push(Token::Comment(data));
                self.i = next;
            }
            None => {
                if data.contains("--!>") {
                    self.refuse_at(
                        start,
                        Kind::Unsupported,
                        "`--!>` ends a comment early in a browser",
                    );
                } else {
                    self.refuse_at(
                        start,
                        Kind::Unsupported,
                        "this comment is never closed with `-->`",
                    );
                }
            }
        }
    }

    fn doctype(&mut self, start: usize, k: usize) {
        let mut data = String::new();
        let Some(next) = self.scan_to(k, ">", "in a doctype", &mut data) else {
            self.refuse_at(
                start,
                Kind::Unsupported,
                "this doctype is never closed with `>`",
            );
            return;
        };
        let trimmed = data.trim_matches(is_ws);
        let starts_ws = data.starts_with(is_ws);
        if !starts_ws || trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_alphanumeric()) {
            self.refuse_at(
                start,
                Kind::Unsupported,
                "only a doctype with a plain name, such as `<!doctype html>`, is supported",
            );
            return;
        }
        self.flush_text();
        self.out
            .tokens
            .push(Token::Doctype(trimmed.to_ascii_lowercase()));
        self.i = next;
    }

    fn cdata(&mut self, k: usize) {
        let mut data = String::new();
        match self.scan_to(k, "]]>", "in CDATA", &mut data) {
            Some(next) => {
                self.text.push_str(&data);
                self.i = next;
            }
            None => self.refuse_at(
                k,
                Kind::Unsupported,
                "this CDATA section is never closed with `]]>`",
            ),
        }
    }

    /// RCDATA, RAWTEXT or script data: everything is text up to the end tag of
    /// the element that opened it.
    fn raw_text(&mut self) {
        let unit = self.units[self.i];
        match unit {
            Unit::Hole(h, _) => {
                self.raw_hole(h);
                self.i += 1;
            }
            Unit::Char('<', _) => self.raw_lt(),
            Unit::Char(c, _) => {
                if self.state == State::Rcdata {
                    self.track_charref(c);
                }
                self.text.push(c);
                self.i += 1;
            }
        }
    }

    fn raw_hole(&mut self, h: usize) {
        let element = self.raw.clone();
        match self.state {
            State::Rcdata if self.charref => self.refuse_hole(
                h,
                Kind::Unsupported,
                "a placeholder cannot continue a character reference",
            ),
            State::Rcdata => self.judge(h, Ok(Context::Rcdata { element })),
            State::ScriptData => self.refuse_hole_with(
                h,
                Refusal::new(Kind::InScript, At::Hole(h), "a placeholder cannot be inside `<script>`")
                    .note("a script's text is code, and no escape makes a value safe there; pass data through a data attribute instead"),
            ),
            _ if element == "style" => self.refuse_hole_with(
                h,
                Refusal::new(Kind::InStyle, At::Hole(h), "a placeholder cannot be inside `<style>`")
                    .note("CSS has its own URL and escaping rules and can leak data without any URL, so style takes no placeholder"),
            ),
            _ => self.refuse_hole_with(
                h,
                Refusal::new(Kind::InRawText, At::Hole(h), format!("a placeholder cannot be inside `<{element}>`"))
                    .note(format!("the text of `<{element}>` is not parsed as markup, so it takes no placeholder")),
            ),
        }
    }

    /// At a `<` in RCDATA, RAWTEXT or script data.
    fn raw_lt(&mut self) {
        let start = self.i;
        let script = self.state == State::ScriptData;
        if script && self.spells(start + 1, "!--") {
            self.refuse_at(
                start,
                Kind::Unsupported,
                "`<!--` inside `<script>` changes how a browser finds the script's end",
            );
            return;
        }
        if !self.spells(start + 1, "/") {
            if let Some(Unit::Hole(h, _)) = self.units.get(start + 1).copied() {
                self.refuse_after_lt(h);
            }
            self.text.push('<');
            self.i += 1;
            return;
        }
        let name = self.raw.clone();
        let mut k = start + 2;
        let mut matched = 0;
        for (n, w) in name.chars().enumerate() {
            match self.char_at(k + n) {
                Some(c) if c.eq_ignore_ascii_case(&w) => matched += 1,
                _ => break,
            }
        }
        if matched < name.chars().count() {
            // A hole that could complete the end tag's name.
            if let Some(Unit::Hole(h, _)) = self.units.get(k + matched).copied() {
                self.refuse_after_lt(h);
            }
            self.text.push_str("</");
            self.i += 2;
            return;
        }
        k += matched;
        match self.units.get(k).copied() {
            Some(Unit::Char('>', _)) => self.end_raw(k + 1),
            Some(Unit::Char(c, _)) if is_ws(c) => {
                let mut j = k;
                while self.char_at(j).is_some_and(is_ws) {
                    j += 1;
                }
                if self.char_at(j) == Some('>') {
                    self.end_raw(j + 1);
                } else {
                    self.refuse_at(
                        start,
                        Kind::Unsupported,
                        format!("an end tag `</{name}` with attributes is not supported"),
                    );
                }
            }
            Some(Unit::Char('/', _)) => self.refuse_at(
                start,
                Kind::Unsupported,
                format!("`</{name}/` is not supported; write `</{name}>`"),
            ),
            None => self.refuse_at(start, Kind::Unsupported, format!("the end tag `</{name}` is never closed")),
            Some(Unit::Char(c, _)) if c.is_ascii_alphanumeric() => self.refuse_at(
                start,
                Kind::Unsupported,
                format!("`</{name}` followed by more of a name is not the end tag, and reads too much like it"),
            ),
            Some(Unit::Hole(h, _)) => {
                self.refuse_after_lt(h);
                self.text.push_str("</");
                self.i += 2;
            }
            Some(Unit::Char(..)) => {
                self.text.push_str("</");
                self.i += 2;
            }
        }
    }

    fn refuse_after_lt(&mut self, h: usize) {
        if self.state == State::Rcdata {
            self.refuse_hole(
                h,
                Kind::Unsupported,
                "a placeholder cannot follow `<` or `</` here, where it could complete an end tag",
            );
        } else {
            self.raw_hole(h);
        }
    }

    fn end_raw(&mut self, next: usize) {
        let name = std::mem::take(&mut self.raw);
        self.flush_text();
        self.out.tokens.push(Token::EndTag(name));
        self.state = State::Data;
        self.charref = false;
        self.i = next;
    }

    fn plaintext(&mut self) {
        match self.units[self.i] {
            Unit::Hole(h, _) => self.refuse_hole_with(
                h,
                Refusal::new(
                    Kind::InRawText,
                    At::Hole(h),
                    "a placeholder cannot follow `<plaintext>`",
                )
                .note("everything after `<plaintext>` is text, to the end of the page"),
            ),
            Unit::Char(c, _) => self.text.push(c),
        }
        self.i += 1;
    }

    /// One unit in a tag-name or attribute state.
    fn in_tag(&mut self) {
        let unit = self.units[self.i];
        let k = self.i;
        let state = self.state;
        let c = match unit {
            Unit::Hole(h, _) => {
                self.tag_hole(h, state);
                self.i += 1;
                return;
            }
            Unit::Char(c, _) => c,
        };
        self.i += 1;
        match state {
            State::TagName | State::EndTagName => match c {
                c if is_ws(c) => {
                    self.state = if state == State::TagName {
                        State::BeforeAttrName
                    } else {
                        State::EndTagAfterName
                    }
                }
                '/' if state == State::TagName => self.state = State::SelfClosingStartTag,
                '/' => self.refuse_at(k, Kind::Unsupported, "an end tag cannot be self-closing"),
                '>' => self.emit_tag(),
                '<' | '\0' => self.refuse_at(
                    k,
                    Kind::Unsupported,
                    "this character cannot be in a tag name",
                ),
                c => self.tag.name.push(c.to_ascii_lowercase()),
            },
            State::EndTagAfterName => match c {
                c if is_ws(c) => {}
                '>' => self.emit_tag(),
                _ => self.refuse_at(k, Kind::Unsupported, "an end tag cannot have attributes"),
            },
            State::BeforeAttrName => match c {
                c if is_ws(c) => {}
                '/' => self.state = State::SelfClosingStartTag,
                '>' => self.emit_tag(),
                '=' => self.refuse_at(
                    k,
                    Kind::Unsupported,
                    "an attribute name cannot start with `=`",
                ),
                _ => {
                    self.tag.attrs.push((String::new(), String::new()));
                    self.i -= 1;
                    self.state = State::AttrName;
                }
            },
            State::AttrName => match c {
                c if is_ws(c) => {
                    self.end_attr_name(k);
                    self.state = State::AfterAttrName;
                }
                '/' => {
                    self.end_attr_name(k);
                    self.state = State::SelfClosingStartTag;
                }
                '>' => {
                    if self.end_attr_name(k) {
                        self.emit_tag();
                    }
                }
                '=' => {
                    self.end_attr_name(k);
                    self.state = State::BeforeAttrValue;
                }
                '"' | '\'' | '<' | '\0' => self.refuse_at(
                    k,
                    Kind::Unsupported,
                    "this character cannot be in an attribute name",
                ),
                c => {
                    if let Some(a) = self.tag.attrs.last_mut() {
                        a.0.push(c.to_ascii_lowercase());
                    }
                }
            },
            State::AfterAttrName => match c {
                c if is_ws(c) => {}
                '/' => self.state = State::SelfClosingStartTag,
                '=' => self.state = State::BeforeAttrValue,
                '>' => self.emit_tag(),
                _ => {
                    self.tag.attrs.push((String::new(), String::new()));
                    self.i -= 1;
                    self.state = State::AttrName;
                }
            },
            State::BeforeAttrValue => match c {
                c if is_ws(c) => {}
                '"' => self.state = State::AttrValueDq,
                '\'' => self.state = State::AttrValueSq,
                '>' => self.refuse_at(
                    k,
                    Kind::Unsupported,
                    "an attribute's `=` must be followed by a value",
                ),
                _ => {
                    self.i -= 1;
                    self.state = State::AttrValueUnquoted;
                }
            },
            State::AttrValueDq | State::AttrValueSq => {
                let quote = if state == State::AttrValueDq {
                    '"'
                } else {
                    '\''
                };
                if c == quote {
                    self.charref = false;
                    self.state = State::AfterAttrValueQuoted;
                } else {
                    self.push_value(c);
                }
            }
            State::AttrValueUnquoted => match c {
                c if is_ws(c) => {
                    self.charref = false;
                    self.state = State::BeforeAttrName;
                }
                '>' => {
                    self.charref = false;
                    self.emit_tag();
                }
                '"' | '\'' | '<' | '=' | '`' | '\0' => self.refuse_at(
                    k,
                    Kind::Unsupported,
                    "this character cannot be in an unquoted attribute value; quote the value",
                ),
                c => self.push_value(c),
            },
            State::AfterAttrValueQuoted => match c {
                c if is_ws(c) => self.state = State::BeforeAttrName,
                '/' => self.state = State::SelfClosingStartTag,
                '>' => self.emit_tag(),
                _ => {
                    self.i -= 1;
                    self.state = State::BeforeAttrName;
                }
            },
            State::SelfClosingStartTag => match c {
                '>' => {
                    self.tag.self_closing = true;
                    self.emit_tag();
                }
                _ => {
                    self.i -= 1;
                    self.state = State::BeforeAttrName;
                }
            },
            _ => unreachable!("in_tag is only called in a tag state"),
        }
    }

    fn push_value(&mut self, c: char) {
        self.track_charref(c);
        if let Some(a) = self.tag.attrs.last_mut() {
            a.1.push(c);
        }
    }

    /// The attribute name just ended: a repeat of an earlier one is refused,
    /// because a browser keeps the first and drops the rest. Whether the tag
    /// may go on.
    fn end_attr_name(&mut self, k: usize) -> bool {
        let Some((name, _)) = self.tag.attrs.last() else {
            return true;
        };
        let name = name.clone();
        let n = self.tag.attrs.len();
        if self.tag.attrs[..n - 1].iter().any(|(a, _)| *a == name) {
            self.refuse_at(
                k,
                Kind::Unsupported,
                format!("the attribute `{name}` is repeated, and a browser keeps only the first"),
            );
            return false;
        }
        true
    }

    fn tag_hole(&mut self, h: usize, state: State) {
        match state {
            State::TagName => self.hole_in_tag(h, "in a tag name"),
            State::EndTagName | State::EndTagAfterName => self.hole_in_tag(h, "in an end tag"),
            State::BeforeAttrName | State::AttrName | State::AfterAttrName => {
                self.hole_in_tag(h, "an attribute name")
            }
            State::AfterAttrValueQuoted | State::SelfClosingStartTag => {
                self.hole_in_tag(h, "between attributes")
            }
            State::BeforeAttrValue | State::AttrValueUnquoted => {
                let attr = self.tag.attrs.last().map_or(String::new(), |a| a.0.clone());
                self.refuse_hole_with(
                    h,
                    Refusal::new(Kind::Unquoted, At::Hole(h), format!("the placeholder in `{attr}=` is not quoted"))
                        .note(format!("quote the value, `{attr}=\"…\"`: an unquoted value ends at the first space, so a value could add attributes")),
                );
                if state == State::BeforeAttrValue {
                    self.state = State::AttrValueUnquoted;
                }
            }
            State::AttrValueDq | State::AttrValueSq => {
                if self.charref {
                    self.refuse_hole(
                        h,
                        Kind::Unsupported,
                        "a placeholder cannot continue a character reference",
                    );
                } else {
                    let attr = self.tag.attrs.len().saturating_sub(1);
                    self.tag.holes.push(AttrHole { hole: h, attr });
                }
            }
            _ => unreachable!("tag_hole is only called in a tag state"),
        }
    }

    fn emit_tag(&mut self) {
        let tag = std::mem::take(&mut self.tag);
        self.state = State::Data;
        self.charref = false;
        if tag.end {
            self.out.tokens.push(Token::EndTag(tag.name.clone()));
        } else {
            self.out.tokens.push(Token::StartTag {
                name: tag.name.clone(),
                attrs: tag.attrs.clone(),
                self_closing: tag.self_closing,
            });
        }
        if !tag.end {
            self.judge_attr_holes(&tag);
        }
        if !self.tree {
            return;
        }
        if !self.foreign.is_empty() {
            self.foreign_tag(&tag);
        } else if tag.end {
            self.html_end(&tag);
        } else {
            self.html_start(&tag);
        }
    }

    fn judge_attr_holes(&mut self, tag: &Tag) {
        for ah in &tag.holes {
            let (attr, _) = &tag.attrs[ah.attr];
            let outcome = self.attr_context(tag, attr, ah.hole);
            self.judge(ah.hole, outcome);
        }
    }

    fn attr_context(&self, tag: &Tag, attr: &str, h: usize) -> Result<Context, Refusal> {
        let element = tag.name.as_str();
        let at = At::Hole(h);
        if self.tree && (!self.foreign.is_empty() || matches!(element, "svg" | "math")) {
            let root = self.foreign.first().map_or(element, |(n, _)| n.as_str());
            return Err(Refusal::new(Kind::Position, at, format!("a placeholder cannot be inside `<{root}>`"))
                .note("SVG and MathML are parsed by different rules, so a placeholder is refused anywhere inside them"));
        }
        if attr.starts_with("on") {
            return Err(Refusal::new(
                Kind::InCodeAttribute,
                at,
                format!("a placeholder cannot be in the event handler `{attr}`"),
            )
            .note("an event handler's value is code, and no escape makes a value safe there"));
        }
        if attr == "style" {
            return Err(Refusal::new(Kind::InStyle, at, "a placeholder cannot be in a `style` attribute")
                .note("CSS has its own URL and escaping rules and can leak data without any URL, so style takes no placeholder"));
        }
        if attr == "srcdoc" {
            return Err(Refusal::new(
                Kind::InCodeAttribute,
                at,
                "a placeholder cannot be in `srcdoc`",
            )
            .note("`srcdoc` is a whole nested document"));
        }
        let has = |name: &str| tag.attrs.iter().any(|(a, _)| a == name);
        if element == "meta" && attr == "content" && has("http-equiv") {
            return Err(Refusal::new(Kind::InCodeAttribute, at, "a placeholder cannot be in `<meta content>` beside `http-equiv`")
                .note("with `http-equiv`, `content` is read as a header, such as a refresh to another URL"));
        }
        if decides_another(element, attr) {
            return Err(Refusal::new(Kind::InCodeAttribute, at, format!("a placeholder cannot be in `<{element} {attr}>`"))
                .note(format!("`{attr}` decides how the rest of `<{element}>` is read, so it must be written literally")));
        }
        if is_srcset_attribute(attr) || is_url_attribute(attr) || element == "base" {
            return Err(Refusal::new(Kind::Unsupported, at, format!("a placeholder in the URL attribute `{attr}` is not supported yet"))
                .note("a URL attribute takes a checked URL, not text; until templates support one, write the URL literally"));
        }
        Ok(Context::Attr {
            element: element.to_string(),
            attr: attr.to_string(),
        })
    }

    fn html_start(&mut self, tag: &Tag) {
        let name = tag.name.as_str();
        let at = tag.start;
        if self.select_at.is_some() && !matches!(name, "option" | "optgroup" | "hr") {
            self.refuse(Refusal::new(Kind::Unsupported, At::Offset(at), format!("`<{name}>` cannot be inside `<select>` here"))
                .note("inside `<select>` a browser ignores most tags, so a template's `<select>` holds only `<option>`, `<optgroup>` and `<hr>`"));
            return;
        }
        match name {
            "svg" | "math" if !tag.self_closing => {
                let ns = if name == "svg" { Ns::Svg } else { Ns::Math };
                self.foreign.push((name.to_string(), ns));
                self.foreign_at = at;
            }
            "title" | "textarea" => self.open_raw(name, at, State::Rcdata),
            "style" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript" => {
                self.open_raw(name, at, State::Rawtext)
            }
            "script" => self.open_raw(name, at, State::ScriptData),
            "plaintext" => {
                self.flush_text();
                self.state = State::Plaintext;
            }
            "select" => self.select_at = Some(at),
            "frameset" | "frame" => self.refuse(Refusal::new(
                Kind::Unsupported,
                At::Offset(at),
                format!("`<{name}>` is not supported in a template"),
            )),
            _ => {}
        }
    }

    fn open_raw(&mut self, name: &str, at: usize, state: State) {
        self.raw = name.to_string();
        self.raw_at = at;
        self.state = state;
    }

    fn html_end(&mut self, tag: &Tag) {
        let name = tag.name.as_str();
        if matches!(name, "svg" | "math") {
            self.refuse(
                Refusal::new(
                    Kind::ForeignUnbalanced,
                    At::Offset(tag.start),
                    format!("`</{name}>` closes a `<{name}>` this template never opened"),
                )
                .note(
                    "an `<svg>` or `<math>` must open and close in the same template or fragment",
                ),
            );
            return;
        }
        if self.select_at.is_some() {
            match name {
                "select" => self.select_at = None,
                "option" | "optgroup" => {}
                _ => self.refuse(Refusal::new(
                    Kind::Unsupported,
                    At::Offset(tag.start),
                    format!("`</{name}>` cannot be inside `<select>` here"),
                )),
            }
        }
    }

    fn foreign_tag(&mut self, tag: &Tag) {
        let name = tag.name.as_str();
        let at = At::Offset(tag.start);
        let ns = self.foreign.last().map_or(Ns::Svg, |(_, ns)| *ns);
        let root = self
            .foreign
            .first()
            .map_or("svg", |(n, _)| n.as_str())
            .to_string();
        if tag.end {
            if matches!(name, "br" | "p") {
                self.refuse(Refusal::new(
                    Kind::ForeignUnbalanced,
                    at,
                    format!("`</{name}>` inside `<{root}>` ends it early in a browser"),
                ));
                return;
            }
            if self.foreign.last().is_some_and(|(n, _)| n == name) {
                self.foreign.pop();
            } else {
                self.refuse(Refusal::new(Kind::ForeignUnbalanced, at, format!("`</{name}>` does not close the element open inside `<{root}>`"))
                    .note("inside SVG and MathML every element must be closed in order, so the subtree's end is where a browser sees it"));
            }
            return;
        }
        if FOREIGN_BREAKOUT.contains(&name) {
            self.refuse(
                Refusal::new(
                    Kind::ForeignUnbalanced,
                    at,
                    format!("`<{name}>` inside `<{root}>` ends it early in a browser"),
                )
                .note("an HTML element such as this one makes a browser leave SVG or MathML"),
            );
            return;
        }
        let integration = match ns {
            Ns::Svg => matches!(name, "foreignobject" | "desc" | "title"),
            Ns::Math => matches!(name, "annotation-xml" | "mi" | "mo" | "mn" | "ms" | "mtext"),
        };
        if integration {
            self.refuse(
                Refusal::new(
                    Kind::ForeignUnbalanced,
                    at,
                    format!("`<{name}>` inside `<{root}>` is not supported"),
                )
                .note("inside it a browser reads HTML again, so it is refused in this version"),
            );
            return;
        }
        if !tag.self_closing {
            self.foreign.push((name.to_string(), ns));
        }
    }

    fn finish(&mut self) {
        let end = self.offset(self.units.len());
        match self.state {
            State::Data | State::Plaintext => {}
            State::Rcdata | State::Rawtext | State::ScriptData => {
                if !self.tree {
                    self.flush_text();
                    return;
                }
                let name = self.raw.clone();
                if name == "noscript" {
                    self.refuse(Refusal::new(Kind::InRawText, At::Offset(self.raw_at), "this `<noscript>` is never closed")
                        .note("`<noscript>` is read differently with scripting on and off, so it must close in the same template or fragment"));
                } else {
                    self.refuse(Refusal::new(
                        Kind::Unsupported,
                        At::Offset(self.raw_at),
                        format!("this `<{name}>` is never closed"),
                    ));
                }
                return;
            }
            _ => {
                self.refuse(Refusal::new(
                    Kind::Unsupported,
                    At::Offset(end),
                    "the markup ends inside a tag",
                ));
                return;
            }
        }
        self.flush_text();
        if !self.tree {
            return;
        }
        if let Some((root, _)) = self.foreign.first() {
            let root = root.clone();
            self.refuse(
                Refusal::new(
                    Kind::ForeignUnbalanced,
                    At::Offset(self.foreign_at),
                    format!("this `<{root}>` is never closed"),
                )
                .note(
                    "an `<svg>` or `<math>` must open and close in the same template or fragment",
                ),
            );
        } else if let Some(at) = self.select_at {
            self.refuse(Refusal::new(Kind::Unsupported, At::Offset(at), "this `<select>` is never closed")
                .note("a browser ignores most tags inside `<select>`, so it must close in the same template or fragment"));
        }
    }
}

/// Tokenise `units` from the data state with the tree builder's effects
/// modelled: a template or a fragment.
pub fn analyse(units: &[Unit]) -> Analysis {
    run(units, Initial::Data, true)
}

/// Tokenise `units` from `initial` with no tree builder, as the conformance
/// corpus tests the tokeniser on its own.
pub fn tokenise(units: &[Unit], initial: Initial) -> Analysis {
    run(units, initial, false)
}

fn run(units: &[Unit], initial: Initial, tree: bool) -> Analysis {
    let (state, raw) = match initial {
        Initial::Data => (State::Data, String::new()),
        Initial::Rcdata(n) => (State::Rcdata, n),
        Initial::Rawtext(n) => (State::Rawtext, n),
        Initial::ScriptData(n) => (State::ScriptData, n),
        Initial::Plaintext => (State::Plaintext, String::new()),
    };
    let mut tok = Tok {
        units,
        i: 0,
        state,
        tree,
        tag: Tag::default(),
        raw,
        raw_at: 0,
        foreign: Vec::new(),
        foreign_at: 0,
        select_at: None,
        charref: false,
        text: String::new(),
        out: Analysis::default(),
        failed: false,
    };
    tok.run();
    tok.out
}

/// The units of plain text with no holes, each character at its byte offset.
pub fn chars(text: &str) -> Vec<Unit> {
    text.char_indices().map(|(o, c)| Unit::Char(c, o)).collect()
}
