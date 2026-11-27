//! The typed-HTML tokeniser's oracle (typed-HTML §11.2, #3927): a browser-grade parser,
//! `html5ever` with scripting on, run over what the compiler accepts.
//!
//! Shared by the `proptest` differential (`../html_template_differential.rs`) and the fuzz target
//! (`fuzz/fuzz_targets/template.rs`), so both hold the same property: for a template the tokeniser
//! accepts, each placeholder lands where its context says (element text of an ordinary element, the
//! text of the `<title>` or `<textarea>` named, the attribute named), and filled with adversarial
//! values, every value is exactly one text or attribute value and adds no element or attribute.

use bynk_check::html_template::{Context, analyse_template};
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

/// `escapeText`, as `bynk.html` writes it.
pub fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `escapeAttr`, as `bynk.html` writes it.
pub fn escape_attr(s: &str) -> String {
    escape_text(s)
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('`', "&#96;")
}

/// The values the property fills each placeholder with: each tries to leave its context.
pub const ADVERSARIAL: &[&str] = &[
    "</title></textarea><script>alert(1)</script>",
    "\" onmouseover=\"x\" a=\"",
    "' onmouseover='x' a='",
    "<!-- x --> <img src=x onerror=alert(1)>",
    "&amp; &lt; &#60; &x",
    "` onfocus=`x`",
    "</p></div></select><svg><script>x</script>",
];

fn parse(html: &str) -> RcDom {
    let mut opts = html5ever::ParseOpts::default();
    opts.tree_builder.scripting_enabled = true;
    html5ever::parse_document(RcDom::default(), opts).one(html)
}

/// One place a marker was found: the element around it, and the attribute, or `None` for text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    element: String,
    html_ns: bool,
    attr: Option<String>,
}

fn find(handle: &Handle, needle: &str, parent: Option<(&str, bool)>, out: &mut Vec<Found>) {
    match &handle.data {
        NodeData::Text { contents } => {
            if contents.borrow().contains(needle) {
                let (element, html_ns) = parent.unwrap_or(("#document", true));
                out.push(Found {
                    element: element.to_string(),
                    html_ns,
                    attr: None,
                });
            }
        }
        NodeData::Comment { contents } if contents.contains(needle) => out.push(Found {
            element: "#comment".to_string(),
            html_ns: true,
            attr: None,
        }),
        NodeData::Element {
            name,
            attrs,
            template_contents,
            ..
        } => {
            let local = name.local.to_string();
            let html_ns = name.ns == html5ever::ns!(html);
            for a in attrs.borrow().iter() {
                if a.value.contains(needle) {
                    out.push(Found {
                        element: local.clone(),
                        html_ns,
                        attr: Some(a.name.local.to_string()),
                    });
                }
            }
            for c in handle.children.borrow().iter() {
                find(c, needle, Some((&local, html_ns)), out);
            }
            if let Some(t) = template_contents.borrow().as_ref() {
                find(t, needle, Some((&local, html_ns)), out);
            }
            return;
        }
        _ => {}
    }
    for c in handle.children.borrow().iter() {
        find(c, needle, parent, out);
    }
}

/// Every element and attribute in the tree, as a count, to compare two fills' structure.
fn shape(handle: &Handle, out: &mut (usize, usize)) {
    if let NodeData::Element {
        attrs,
        template_contents,
        ..
    } = &handle.data
    {
        out.0 += 1;
        out.1 += attrs.borrow().len();
        if let Some(t) = template_contents.borrow().as_ref() {
            shape(t, out);
        }
    }
    for c in handle.children.borrow().iter() {
        shape(c, out);
    }
}

const RAW: &[&str] = &[
    "script",
    "style",
    "title",
    "textarea",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "noscript",
    "plaintext",
];

fn expected_place(found: &Found, context: &Context) -> bool {
    match context {
        Context::Text | Context::SelectText => {
            found.attr.is_none()
                && found.html_ns
                && !RAW.contains(&found.element.as_str())
                && found.element != "#comment"
        }
        Context::Rcdata { element } => {
            found.attr.is_none() && found.html_ns && found.element == *element
        }
        Context::Attr { element, attr } => {
            found.html_ns && found.element == *element && found.attr.as_deref() == Some(attr)
        }
    }
}

/// The start tags the tree builder can ignore, with their attributes, where they cannot go:
/// table parts outside a table, a nested `<form>`, a `<select>` in a `<select>`, and a second
/// `<html>` or `<body>`, whose attributes merge only where the first lacks them.
const IGNORABLE: &[&str] = &[
    "tr", "td", "th", "tbody", "thead", "tfoot", "caption", "col", "colgroup", "form", "select",
    "html", "body", "head", "frame",
];

/// Whether `found` is an acceptable outcome for a placeholder in `context`: every place it is
/// found is the one the context names, and it is found at least once. Two tree-builder effects
/// are not escapes: it ignores some start tags where they cannot go ([`IGNORABLE`]), and their
/// attributes with them, so an attribute value of one of those, and only those, can vanish; and
/// it clones a formatting element it reopens (`<b class="…">` after a `<p>` closed it),
/// attributes and all, so a value can appear twice, each time in its own context.
fn acceptable(found: &[Found], context: &Context) -> bool {
    if found.is_empty() {
        return matches!(context, Context::Attr { element, .. } if IGNORABLE.contains(&element.as_str()));
    }
    found.iter().all(|f| expected_place(f, context))
}

/// The template's text with each accepted placeholder filled by `value(n)` escaped for its
/// context, or `None` when the tokeniser did not accept the template.
fn fill(text: &str, value: &dyn Fn(usize) -> String) -> Option<(String, Vec<Context>)> {
    let a = analyse_template(text);
    let holes = a.analysis.holes().ok()?;
    let mut contexts = Vec::new();
    for n in 0..a.split.placeholders.len() {
        match holes.get(n).cloned().flatten() {
            Some(Ok(c)) => contexts.push(c),
            _ => return None,
        }
    }
    let mut out = String::new();
    for (n, (_, piece)) in a.split.pieces.iter().enumerate() {
        out.push_str(piece);
        if let Some(c) = contexts.get(n) {
            let v = value(n);
            out.push_str(&match c {
                Context::Attr { .. } => escape_attr(&v),
                _ => escape_text(&v),
            });
        }
    }
    Some((out, contexts))
}

/// Check the oracle's property on `text`: `Ok(true)` when the tokeniser accepted it and the
/// browser agrees, `Ok(false)` when the tokeniser refused it (nothing to check), and `Err` with
/// what disagreed.
pub fn check(text: &str) -> Result<bool, String> {
    let marker = |n: usize| format!("zqmark{n}q");
    let Some((benign, contexts)) = fill(text, &marker) else {
        return Ok(false);
    };
    let dom = parse(&benign);
    for (n, context) in contexts.iter().enumerate() {
        let mut found = Vec::new();
        find(&dom.document, &marker(n), None, &mut found);
        if !acceptable(&found, context) {
            return Err(format!(
                "placeholder {n} is {context:?} to the tokeniser, but the browser puts it at {found:?}\n--- input ---\n{text}\n--- filled ---\n{benign}"
            ));
        }
    }
    let mut want = (0, 0);
    shape(&dom.document, &mut want);
    for bad in ADVERSARIAL {
        let tagged = |n: usize| format!("{}{bad}{}", marker(n), marker(n));
        let (filled, _) = fill(text, &tagged).expect("accepted once, accepted again");
        let dom = parse(&filled);
        let mut got = (0, 0);
        shape(&dom.document, &mut got);
        if got != want {
            return Err(format!(
                "filling with {bad:?} changed the tree from {want:?} to {got:?} (elements, attributes)\n--- input ---\n{text}\n--- filled ---\n{filled}"
            ));
        }
        for (n, context) in contexts.iter().enumerate() {
            let whole = format!("{}{bad}{}", marker(n), marker(n));
            let mut found = Vec::new();
            find(&dom.document, &whole, None, &mut found);
            if !acceptable(&found, context) {
                return Err(format!(
                    "filled with {bad:?}, placeholder {n} ({context:?}) is not one whole value: {found:?}\n--- input ---\n{text}\n--- filled ---\n{filled}"
                ));
            }
        }
    }
    Ok(true)
}
