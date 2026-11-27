//! The typed-HTML tokeniser's differential test (typed-HTML §11.2, #3927).
//!
//! A `proptest` property generates HTML-like templates with placeholders: elements of every
//! kind the context model treats differently, quoted and unquoted attributes, comments, raw text,
//! foreign content and `<select>`, with placeholders anywhere. Tag names come in any case, and
//! whitespace, raw-text end tags and text include CR, CRLF, tab and form feed, which a browser
//! normalises or treats as whitespace before the tokeniser decides anything. For every template the tokeniser
//! accepts, the oracle (`support/html_oracle.rs`) parses it with `html5ever`, scripting on, and
//! checks that each placeholder is where its context says, and that adversarial values each stay
//! one whole text or attribute value and add no element or attribute.

#[path = "support/html_oracle.rs"]
mod html_oracle;

use proptest::prelude::*;

/// A piece of a generated template.
#[derive(Debug, Clone)]
enum Piece {
    Text(String),
    Hole,
    /// An element, the case of its name, and its attributes, each after its
    /// whitespace, with a value quoted by its quote (a space for unquoted).
    Open(
        String,
        Case,
        Vec<(&'static str, String, Vec<Option<String>>, char)>,
    ),
    Close(String, Case),
    Comment(Vec<Option<String>>),
    /// A raw-text element, its contents, and how its end tag is written.
    Raw(String, Vec<Option<String>>, &'static str),
}

/// How a tag name is written: HTML tag names are case-insensitive.
#[derive(Debug, Clone, Copy)]
enum Case {
    Lower,
    Upper,
    Title,
}

fn cased(name: &str, case: Case) -> String {
    match case {
        Case::Lower => name.to_string(),
        Case::Upper => name.to_ascii_uppercase(),
        Case::Title => {
            let mut c = name.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        }
    }
}

fn case() -> impl Strategy<Value = Case> {
    prop_oneof![3 => Just(Case::Lower), 1 => Just(Case::Upper), 1 => Just(Case::Title)]
}

/// Whitespace as a template might hold it, before input-stream preprocessing.
const WS: &[&str] = &[" ", " ", "\t", "\n", "\r", "\r\n", "\x0C"];

/// The ways a raw-text element's end tag is written (`{}` is its name): all
/// end it but the last two, which a browser reads as text.
const RAW_END: &[&str] = &[
    "</{}>",
    "</{}>",
    "</{U}>",
    "</{T} >",
    "</{}\t>",
    "</{}\r\n>",
    "</{}\r>",
    "</{} x>",
    "</{}/>",
    "</{}x>",
    "</ {}>",
];

const ELEMENTS: &[&str] = &[
    "p", "div", "span", "b", "a", "img", "ul", "li", "table", "tr", "td", "form", "input",
    "button", "label", "section", "template", "select", "option", "svg", "math", "path", "br",
    "h1", "pre", "textarea", "title",
];
const RAW: &[&str] = &[
    "script", "style", "title", "textarea", "xmp", "iframe", "noembed", "noframes", "noscript",
];
const ATTRS: &[&str] = &[
    "class",
    "id",
    "alt",
    "title",
    "name",
    "value",
    "href",
    "src",
    "style",
    "onclick",
    "data-x",
    "type",
    "rel",
    "content",
    "http-equiv",
    "srcdoc",
    "lang",
    "shadowrootmode",
];

fn text() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-zA-Z <>&;\"'/=!\\-\\\\(#\n\r\t\x0C]{0,6}").unwrap()
}

fn value_parts() -> impl Strategy<Value = Vec<Option<String>>> {
    prop::collection::vec(
        prop_oneof![3 => text().prop_map(Some), 1 => Just(None)],
        0..3,
    )
}

fn piece() -> impl Strategy<Value = Piece> {
    let attr = (
        prop::sample::select(WS),
        prop::sample::select(ATTRS),
        value_parts(),
        prop::sample::select(&['"', '\'', ' '][..]),
    )
        .prop_map(|(w, n, v, q)| (w, n.to_string(), v, q));
    prop_oneof![
        4 => text().prop_map(Piece::Text),
        3 => Just(Piece::Hole),
        4 => (prop::sample::select(ELEMENTS), case(), prop::collection::vec(attr, 0..3))
            .prop_map(|(e, c, a)| Piece::Open(e.to_string(), c, a)),
        3 => (prop::sample::select(ELEMENTS), case())
            .prop_map(|(e, c)| Piece::Close(e.to_string(), c)),
        1 => value_parts().prop_map(Piece::Comment),
        2 => (prop::sample::select(RAW), value_parts(), prop::sample::select(RAW_END))
            .prop_map(|(e, v, end)| Piece::Raw(e.to_string(), v, end)),
    ]
}

fn render(pieces: &[Piece]) -> String {
    let mut n = 0;
    let mut hole = || {
        n += 1;
        format!("\\(v{n})")
    };
    let mut out = String::new();
    let parts = |out: &mut String, parts: &[Option<String>], hole: &mut dyn FnMut() -> String| {
        for p in parts {
            match p {
                Some(t) => out.push_str(t),
                None => out.push_str(&hole()),
            }
        }
    };
    for p in pieces {
        match p {
            Piece::Text(t) => out.push_str(t),
            Piece::Hole => out.push_str(&hole()),
            Piece::Open(e, c, attrs) => {
                out.push('<');
                out.push_str(&cased(e, *c));
                for (ws, name, v, q) in attrs {
                    out.push_str(ws);
                    out.push_str(name);
                    out.push('=');
                    if *q != ' ' {
                        out.push(*q);
                    }
                    parts(&mut out, v, &mut hole);
                    if *q != ' ' {
                        out.push(*q);
                    }
                }
                out.push('>');
            }
            Piece::Close(e, c) => {
                out.push_str("</");
                out.push_str(&cased(e, *c));
                out.push('>');
            }
            Piece::Comment(v) => {
                out.push_str("<!--");
                parts(&mut out, v, &mut hole);
                out.push_str("-->");
            }
            Piece::Raw(e, v, end) => {
                out.push('<');
                out.push_str(e);
                out.push('>');
                parts(&mut out, v, &mut hole);
                out.push_str(
                    &end.replace("{U}", &cased(e, Case::Upper))
                        .replace("{T}", &cased(e, Case::Title))
                        .replace("{}", e),
                );
            }
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn the_tokeniser_agrees_with_a_browser(pieces in prop::collection::vec(piece(), 0..12)) {
        let text = render(&pieces);
        if let Err(why) = html_oracle::check(&text) {
            prop_assert!(false, "{}", why);
        }
    }
}

/// The generator reaches accepted templates with placeholders in every accepted context, so the
/// property is not vacuous.
#[test]
fn the_property_checks_every_accepted_context() {
    for text in [
        "<p>\\(a)</p>",
        "<title>\\(a)</title>",
        "<textarea>\\(a)</textarea>",
        "<p class=\"\\(a)\" title='x \\(b)'>t</p>",
        "<select><option>\\(a)</option></select>",
        "<table><tr><td>\\(a)</td></tr></table>",
        "<template><p>\\(a)</p></template>",
        "<svg><path/></svg><p>\\(a)</p>",
    ] {
        assert_eq!(html_oracle::check(text), Ok(true), "{text}");
    }
}

/// The generator is not vacuous: a fair share of what it makes is accepted with placeholders in
/// it, so the property above is checked on real templates, not only on refusals.
#[test]
fn the_generator_reaches_accepted_templates_with_placeholders() {
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = prop::collection::vec(piece(), 0..12);
    let (mut accepted, total) = (0, 2000);
    for _ in 0..total {
        let text = render(&strategy.new_tree(&mut runner).unwrap().current());
        if text.contains("\\(") && html_oracle::check(&text) == Ok(true) {
            accepted += 1;
        }
    }
    assert!(
        accepted * 10 >= total,
        "only {accepted} of {total} generated templates were accepted with a placeholder"
    );
}
