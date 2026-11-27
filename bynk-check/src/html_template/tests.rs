//! The tokeniser's contexts and refusals, one case each (typed-HTML §5.2, §5.3,
//! §6). The checker-level fixtures, named after each context and code, live in
//! `bynk-tests`; these pin the tokeniser alone.

use super::tokeniser::{self, Context, Initial, Kind, Token};
use super::*;

/// The outcome of each placeholder in `text`: `Ok(context)` or `Err(kind)`,
/// and the kind of the first markup refusal, if any.
fn judge(text: &str) -> (Vec<Result<Context, Kind>>, Option<Kind>) {
    let a = analyse_template(text);
    let holes = (0..a.split.placeholders.len())
        .map(|n| match a.analysis.holes.get(n).cloned().flatten() {
            Some(Ok(c)) => Ok(c),
            Some(Err(r)) => Err(r.code),
            None => panic!("placeholder {n} of {text:?} was not judged"),
        })
        .collect();
    (holes, a.analysis.markup.first().map(|r| r.code))
}

fn only(text: &str) -> Result<Context, Kind> {
    let (holes, markup) = judge(text);
    assert_eq!(markup, None, "markup refused in {text:?}");
    assert_eq!(holes.len(), 1, "one placeholder in {text:?}");
    holes.into_iter().next().unwrap()
}

fn markup(text: &str) -> Kind {
    judge(text)
        .1
        .unwrap_or_else(|| panic!("expected {text:?} to be refused"))
}

fn attr(element: &str, attr: &str) -> Context {
    Context::Attr {
        element: element.into(),
        attr: attr.into(),
    }
}

#[test]
fn c1_element_text() {
    assert_eq!(only("<p>\\(t)</p>"), Ok(Context::Text));
    assert_eq!(only("\\(t)"), Ok(Context::Text));
    assert_eq!(only("<div><b>x</b> \\(t) &amp; y</div>"), Ok(Context::Text));
    // A `<` that opens no tag is text, and the state does not change.
    assert_eq!(only("a < b \\(t)"), Ok(Context::Text));
    // Static inline SVG with no placeholder keeps working.
    assert_eq!(only("<svg><path d=\"M0\"/></svg>\\(t)"), Ok(Context::Text));
}

#[test]
fn c2_rcdata() {
    let title = Context::Rcdata {
        element: "title".into(),
    };
    assert_eq!(only("<title>\\(t)</title>"), Ok(title.clone()));
    assert_eq!(only("<TITLE>a \\(t) b</Title >"), Ok(title));
    assert_eq!(
        only("<textarea name=\"n\">\\(t)</textarea>"),
        Ok(Context::Rcdata {
            element: "textarea".into()
        })
    );
    // After `</` a value could complete the end tag, so it is refused.
    assert_eq!(
        only("<title></\\(t)</title>"),
        Err(tokeniser::Kind::Unsupported)
    );
    assert_eq!(
        only("<title></ti\\(t)</title>"),
        Err(tokeniser::Kind::Unsupported)
    );
    assert_eq!(
        only("<title><\\(t)</title>"),
        Err(tokeniser::Kind::Unsupported)
    );
}

#[test]
fn c3_quoted_attribute() {
    assert_eq!(only("<img alt=\"\\(a)\">"), Ok(attr("img", "alt")));
    assert_eq!(only("<img alt='\\(a)'>"), Ok(attr("img", "alt")));
    assert_eq!(only("<p class=\"x \\(a) y\">"), Ok(attr("p", "class")));
    assert_eq!(
        only("<meta name=\"d\" content=\"\\(a)\">"),
        Ok(attr("meta", "content"))
    );
}

#[test]
fn c4_unquoted_attribute() {
    assert_eq!(only("<img alt=\\(a)>"), Err(tokeniser::Kind::Unquoted));
    assert_eq!(only("<img alt=x\\(a)>"), Err(tokeniser::Kind::Unquoted));
}

#[test]
fn c8_script() {
    assert_eq!(
        only("<script>var x = \\(a);</script>"),
        Err(tokeniser::Kind::InScript)
    );
    assert_eq!(
        only("<script type=\"module\">\\(a)</script>"),
        Err(tokeniser::Kind::InScript)
    );
    // A placeholder-shaped regular expression in a script is refused too.
    assert_eq!(
        only("<script>/\\(x)/.test(s)</script>"),
        Err(tokeniser::Kind::InScript)
    );
}

#[test]
fn c10_style() {
    assert_eq!(
        only("<style>p { color: \\(c) }</style>"),
        Err(tokeniser::Kind::InStyle)
    );
    assert_eq!(
        only("<p style=\"color: \\(c)\">"),
        Err(tokeniser::Kind::InStyle)
    );
}

#[test]
fn c11_code_attributes() {
    assert_eq!(
        only("<a onclick=\"\\(a)\">"),
        Err(tokeniser::Kind::InCodeAttribute)
    );
    assert_eq!(
        only("<iframe srcdoc=\"\\(a)\"></iframe>"),
        Err(tokeniser::Kind::InCodeAttribute)
    );
    assert_eq!(
        only("<meta http-equiv=\"refresh\" content=\"\\(a)\">"),
        Err(tokeniser::Kind::InCodeAttribute)
    );
    // `http-equiv` after `content` still counts.
    assert_eq!(
        only("<meta content=\"\\(a)\" http-equiv=\"refresh\">"),
        Err(tokeniser::Kind::InCodeAttribute)
    );
    assert_eq!(
        only("<script type=\"\\(a)\"></script>"),
        Err(tokeniser::Kind::InCodeAttribute)
    );
    assert_eq!(
        only("<link rel=\"\\(a)\">"),
        Err(tokeniser::Kind::InCodeAttribute)
    );
}

#[test]
fn url_attributes_are_not_ordinary_attributes() {
    for t in [
        "<a href=\"\\(h)\">",
        "<img src=\"\\(h)\">",
        "<form action=\"/x/\\(h)\">",
        "<img data-src=\"\\(h)\">",
        "<img srcset=\"\\(h)\">",
        "<base target=\"\\(h)\">",
    ] {
        assert_eq!(only(t), Err(tokeniser::Kind::Unsupported), "{t}");
    }
}

#[test]
fn c13_positions() {
    for t in [
        "<\\(n)>",
        "<p\\(n)>",
        "<p \\(n)=\"x\">",
        "<p a\\(n)=\"x\">",
        "</p\\(n)>",
        "<p a=\"x\"\\(n)>",
        "<!-- \\(n) -->",
        "<!doctype \\(n)>",
        "<svg><text>\\(n)</text></svg>",
        "<svg width=\"\\(n)\"></svg>",
        "<math><mrow>\\(n)</mrow></math>",
        "<svg><![CDATA[\\(n)]]></svg>",
    ] {
        // The placeholder is refused; the markup around it may be too.
        assert_eq!(judge(t).0, vec![Err(tokeniser::Kind::Position)], "{t}");
    }
}

#[test]
fn c14_raw_text() {
    for t in [
        "<xmp>\\(n)</xmp>",
        "<iframe>\\(n)</iframe>",
        "<noembed>\\(n)</noembed>",
        "<noframes>\\(n)</noframes>",
        "<plaintext>\\(n)",
        "<noscript>\\(n)</noscript>",
    ] {
        assert_eq!(only(t), Err(tokeniser::Kind::InRawText), "{t}");
    }
    // A `<noscript>` left open.
    assert_eq!(markup("<noscript><p>x</p>"), tokeniser::Kind::InRawText);
}

#[test]
fn foreign_content_balances_within_one_piece() {
    // The three-fragment case: each piece alone.
    assert_eq!(markup("<svg>"), tokeniser::Kind::ForeignUnbalanced);
    assert_eq!(markup("</svg>"), tokeniser::Kind::ForeignUnbalanced);
    assert_eq!(
        markup("<math><mi>x</mi></math>"),
        tokeniser::Kind::ForeignUnbalanced
    );
    assert_eq!(
        markup("<svg><foreignObject></foreignObject></svg>"),
        tokeniser::Kind::ForeignUnbalanced
    );
    assert_eq!(markup("<svg><p></svg>"), tokeniser::Kind::ForeignUnbalanced);
    assert_eq!(markup("<svg><g></svg>"), tokeniser::Kind::ForeignUnbalanced);
    // Inside foreign content `<script>` does not switch the tokeniser.
    assert_eq!(
        judge("<svg><script>a</script></svg><p>\\(t)</p>").0,
        vec![Ok(Context::Text)]
    );
}

#[test]
fn select_holds_options_only() {
    assert_eq!(
        only("<select><option>\\(t)</option></select>"),
        Ok(Context::SelectText)
    );
    assert_eq!(
        markup("<select><xmp></xmp></select>"),
        tokeniser::Kind::Unsupported
    );
    assert_eq!(
        markup("<select><option>a</option>"),
        tokeniser::Kind::Unsupported
    );
}

#[test]
fn error_recovery_that_changes_a_context_is_refused() {
    for t in [
        "<p a=\"1\" a=\"2\">",
        "<p a<b=\"1\">",
        "<!-- never closed",
        "<!-->",
        "<!-- a --!> b -->",
        "<script>a</scriptx></script>",
        "<script><!-- a</script>",
        "<?php ?>",
        "</>",
        "</ x>",
        "<!x>",
        "<![CDATA[x]]>",
        "<p",
        "<p a=\"x",
        "<title>x",
        "<frameset></frameset>",
        "</p class=\"x\">",
        "<!doctype html public \"x\">",
    ] {
        assert_eq!(markup(t), tokeniser::Kind::Unsupported, "{t}");
    }
}

#[test]
fn a_placeholder_cannot_continue_a_character_reference() {
    assert_eq!(only("<p>&\\(t)</p>"), Err(tokeniser::Kind::Unsupported));
    assert_eq!(only("<p>&am\\(t)</p>"), Err(tokeniser::Kind::Unsupported));
    assert_eq!(only("<p a=\"&#\\(t)\">"), Err(tokeniser::Kind::Unsupported));
    // A finished reference is plain text.
    assert_eq!(only("<p>&amp; \\(t)</p>"), Ok(Context::Text));
}

#[test]
fn a_literal_backslash_paren_is_text() {
    let (holes, markup) = judge("<p>\\(x^2\\) &#92;(name) \\\\(name)</p>");
    assert!(holes.is_empty());
    assert_eq!(markup, None);
}

#[test]
fn tokens_are_what_a_browser_tokenises() {
    let a = tokeniser::tokenise(
        &tokeniser::chars("<!doctype html><p class=\"a\" id=x>t<!--c--></p><br/>"),
        Initial::Data,
    );
    assert!(a.markup.is_empty());
    assert_eq!(
        a.tokens,
        vec![
            Token::Doctype("html".into()),
            Token::StartTag {
                name: "p".into(),
                attrs: vec![("class".into(), "a".into()), ("id".into(), "x".into())],
                self_closing: false
            },
            Token::Chars("t".into()),
            Token::Comment("c".into()),
            Token::EndTag("p".into()),
            Token::StartTag {
                name: "br".into(),
                attrs: vec![],
                self_closing: true
            },
        ]
    );
}

#[test]
fn line_breaks_are_normalised_before_tokenising() {
    // A browser's input-stream preprocessing turns CRLF and a lone CR into LF
    // before it tokenises (§10), so each CR below is whitespace to it.
    for nl in ["\r", "\r\n"] {
        let t = |s: &str| s.replace('|', nl);
        assert_eq!(only(&t("<script|>\\(a)</script>")), Err(Kind::InScript));
        assert_eq!(
            only(&t("<a x|onclick=\"\\(a)\">")),
            Err(Kind::InCodeAttribute)
        );
        assert_eq!(
            only(&t("<a href=\"x\"|onclick=\"\\(a)\">")),
            Err(Kind::InCodeAttribute)
        );
        assert_eq!(only(&t("<a x|href=\"\\(a)\">")), Err(Kind::Unsupported));
        assert_eq!(
            only(&t("<title|>\\(a)</title>")),
            Ok(Context::Rcdata {
                element: "title".into()
            })
        );
        assert_eq!(only(&t("<style|>\\(a)</style>")), Err(Kind::InStyle));
        assert_eq!(
            only(&t("<noscript|>\\(a)</noscript>")),
            Err(Kind::InRawText)
        );
        assert_eq!(only(&t("<plaintext|>\\(a)")), Err(Kind::InRawText));
        assert_eq!(
            only(&t("<select|><option>\\(a)</option></select>")),
            Ok(Context::SelectText)
        );
        assert_eq!(
            markup(&t("<select|><xmp></xmp></select>")),
            Kind::Unsupported
        );
        assert_eq!(judge(&t("<svg|>\\(a)</svg>")).0, vec![Err(Kind::Position)]);
        // An attribute's name ends at the line break.
        assert_eq!(only(&t("<p|class=\"\\(a)\">")), Ok(attr("p", "class")));
        // A raw-text end tag may end with a line break before `>`.
        assert_eq!(
            only(&t("<textarea>x</textarea|><p>\\(a)</p>")),
            Ok(Context::Text)
        );
    }
}

#[test]
fn offsets_stay_offsets_into_the_file_as_written() {
    let a = analyse_template("<p>\r\n\r\n<\\(n)>");
    assert_eq!(a.split.placeholders[0].start, 8);
    let r = a.analysis.refused_holes().next().unwrap().1;
    assert_eq!(r.at, At::Hole(0));
    let a = analyse_template("\r\n<!-- x");
    assert_eq!(a.analysis.markup[0].at, At::Offset(2));
    // The pieces are normalised, ready to emit.
    let a = analyse_template("<p>a\r\nb\rc\\(t)\r</p>");
    assert_eq!(a.split.pieces[0].1, "<p>a\nb\nc");
    assert_eq!(a.split.pieces[1].1, "\n</p>");
}

#[test]
fn a_carriage_return_that_reaches_the_tokeniser_is_refused() {
    let a = tokeniser::analyse(&tokeniser::chars("<script\r>x</script>"));
    assert_eq!(a.markup[0].code, Kind::Unsupported);
    assert!(a.holes().is_err());
}

#[test]
fn a_hole_is_not_accepted_in_refused_markup() {
    let a = analyse_template("<p>\\(t)</p><!-- never closed");
    assert!(a.analysis.holes().is_err());
    assert_eq!(a.analysis.refused_holes().count(), 0);
}

#[test]
fn declarative_shadow_root_attributes_decide_the_template() {
    for a in [
        "shadowrootmode",
        "shadowrootdelegatesfocus",
        "shadowrootclonable",
        "shadowrootserializable",
    ] {
        assert_eq!(
            only(&format!("<template {a}=\"\\(a)\"></template>")),
            Err(Kind::InCodeAttribute),
            "{a}"
        );
    }
    // Static, they are ordinary markup.
    assert_eq!(
        only("<template shadowrootmode=\"open\"><p>\\(t)</p></template>"),
        Ok(Context::Text)
    );
}
