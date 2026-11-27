//! The typed-HTML tokeniser's differential test (typed-HTML §11.2, #3927).
//!
//! A `proptest` property generates HTML-like templates with placeholders: elements of every
//! kind the context model treats differently, quoted and unquoted attributes, comments, raw text,
//! foreign content and `<select>`, with placeholders anywhere. For every template the tokeniser
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
    Open(String, Vec<(String, Vec<Option<String>>, char)>),
    Close(String),
    Comment(Vec<Option<String>>),
    Raw(String, Vec<Option<String>>),
}

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
];

fn text() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-z <>&;\"'/=!\\-\\\\(#\n]{0,6}").unwrap()
}

fn value_parts() -> impl Strategy<Value = Vec<Option<String>>> {
    prop::collection::vec(
        prop_oneof![3 => text().prop_map(Some), 1 => Just(None)],
        0..3,
    )
}

fn piece() -> impl Strategy<Value = Piece> {
    let attr = (
        prop::sample::select(ATTRS),
        value_parts(),
        prop::sample::select(&['"', '\'', ' '][..]),
    )
        .prop_map(|(n, v, q)| (n.to_string(), v, q));
    prop_oneof![
        4 => text().prop_map(Piece::Text),
        3 => Just(Piece::Hole),
        4 => (prop::sample::select(ELEMENTS), prop::collection::vec(attr, 0..3))
            .prop_map(|(e, a)| Piece::Open(e.to_string(), a)),
        3 => prop::sample::select(ELEMENTS).prop_map(|e| Piece::Close(e.to_string())),
        1 => value_parts().prop_map(Piece::Comment),
        2 => (prop::sample::select(RAW), value_parts())
            .prop_map(|(e, v)| Piece::Raw(e.to_string(), v)),
    ]
}

fn render(pieces: &[Piece]) -> String {
    let mut n = 0;
    let mut hole = || {
        n += 1;
        format!("\\(v{n})")
    };
    let mut out = String::new();
    let mut parts =
        |out: &mut String, parts: &[Option<String>], hole: &mut dyn FnMut() -> String| {
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
            Piece::Open(e, attrs) => {
                out.push('<');
                out.push_str(e);
                for (name, v, q) in attrs {
                    out.push(' ');
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
            Piece::Close(e) => {
                out.push_str("</");
                out.push_str(e);
                out.push('>');
            }
            Piece::Comment(v) => {
                out.push_str("<!--");
                parts(&mut out, v, &mut hole);
                out.push_str("-->");
            }
            Piece::Raw(e, v) => {
                out.push('<');
                out.push_str(e);
                out.push('>');
                parts(&mut out, v, &mut hole);
                out.push_str("</");
                out.push_str(e);
                out.push('>');
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
