//! The typed-HTML tokeniser against the `html5lib-tests` tokeniser corpus (typed-HTML §11.2,
//! #3927; the corpus is vendored under `html5lib-tokenizer/`, MIT).
//!
//! The tokeniser implements the WHATWG states the context model needs and refuses everything
//! else, so the conformance property is one-sided: every corpus input it **accepts**, from every
//! initial state it implements, tokenises exactly as the corpus says. An input it refuses is
//! fine (that is the model refusing what it does not model), and the test counts both so a change
//! that refused everything would fail too. Inputs with a `&` are skipped: the tokeniser never
//! decodes a character reference, because a reference never changes the state, so the corpus's
//! decoded text would not match its raw text.

use std::collections::BTreeMap;
use std::path::Path;

use bynk_check::html_template::tokeniser::{self, Initial, Token};
use serde_json::Value;

/// The initial states of the corpus the tokeniser implements, or `None` for one it does not.
fn initial(state: &str, last_start_tag: Option<&str>) -> Option<Initial> {
    let last = last_start_tag.unwrap_or("").to_string();
    match state {
        "Data state" => Some(Initial::Data),
        "RCDATA state" => Some(Initial::Rcdata(last)),
        "RAWTEXT state" => Some(Initial::Rawtext(last)),
        "Script data state" => Some(Initial::ScriptData(last)),
        "PLAINTEXT state" => Some(Initial::Plaintext),
        _ => None,
    }
}

/// `\uXXXX` escapes in a `doubleEscaped` test's strings.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    let mut units: Vec<u16> = Vec::new();
    let flush = |units: &mut Vec<u16>, out: &mut String| {
        out.push_str(&String::from_utf16_lossy(units));
        units.clear();
    };
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'u') {
            chars.next();
            let hex: String = chars.by_ref().take(4).collect();
            units.push(u16::from_str_radix(&hex, 16).unwrap());
        } else {
            flush(&mut units, &mut out);
            out.push(c);
        }
    }
    flush(&mut units, &mut out);
    out
}

/// The corpus's expected tokens as ours, adjacent characters merged; `None` for a token kind the
/// tokeniser never produces (a doctype with identifiers, which it refuses).
fn expected(output: &[Value], double: bool) -> Option<Vec<Token>> {
    let s = |v: &Value| {
        let t = v.as_str().unwrap_or_default();
        if double { unescape(t) } else { t.to_string() }
    };
    let mut out: Vec<Token> = Vec::new();
    for tok in output {
        let tok = tok.as_array()?;
        let t = match tok[0].as_str()? {
            "Character" => Token::Chars(s(&tok[1])),
            "Comment" => Token::Comment(s(&tok[1])),
            "EndTag" => Token::EndTag(s(&tok[1])),
            "StartTag" => {
                let attrs: BTreeMap<String, String> = tok[2]
                    .as_object()?
                    .iter()
                    .map(|(k, v)| (k.clone(), s(v)))
                    .collect();
                Token::StartTag {
                    name: s(&tok[1]),
                    attrs: attrs.into_iter().collect(),
                    self_closing: tok.get(3).and_then(Value::as_bool).unwrap_or(false),
                }
            }
            "DOCTYPE" => {
                if !tok[2].is_null() || !tok[3].is_null() || tok[4] != Value::Bool(true) {
                    return None;
                }
                Token::Doctype(s(&tok[1]))
            }
            _ => return None,
        };
        merge(&mut out, t);
    }
    Some(out)
}

fn merge(out: &mut Vec<Token>, t: Token) {
    if let (Some(Token::Chars(prev)), Token::Chars(more)) = (out.last_mut(), &t) {
        prev.push_str(more);
        return;
    }
    out.push(t);
}

/// Ours, normalised as the corpus is: adjacent characters merged, attributes by name.
fn normalised(tokens: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::new();
    for t in tokens {
        let t = match t {
            Token::StartTag {
                name,
                attrs,
                self_closing,
            } => {
                let mut attrs = attrs;
                attrs.sort();
                Token::StartTag {
                    name,
                    attrs,
                    self_closing,
                }
            }
            Token::Chars(c) if c.is_empty() => continue,
            other => other,
        };
        merge(&mut out, t);
    }
    out
}

#[test]
fn every_accepted_corpus_input_tokenises_as_the_corpus_says() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/html5lib-tokenizer");
    let (mut accepted, mut refused, mut skipped) = (0, 0, 0);
    let mut unimplemented: std::collections::BTreeSet<String> = Default::default();
    let mut failures = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "test"))
        .collect();
    files.sort();
    for file in files {
        let json: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let Some(tests) = json.get("tests").and_then(Value::as_array) else {
            continue;
        };
        for t in tests {
            let double = t
                .get("doubleEscaped")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let raw = t["input"].as_str().unwrap();
            let input = if double {
                unescape(raw)
            } else {
                raw.to_string()
            };
            // Input-stream preprocessing, as the compiler normalises a template's text.
            let input = input.replace("\r\n", "\n").replace('\r', "\n");
            let output = t["output"].as_array().unwrap();
            if input.contains('&') || input.contains('\0') {
                skipped += 1;
                continue;
            }
            let Some(want) = expected(output, double) else {
                skipped += 1;
                continue;
            };
            let states: Vec<String> = t
                .get("initialStates")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
                .unwrap_or_else(|| vec!["Data state".to_string()]);
            let last = t.get("lastStartTag").and_then(Value::as_str);
            for state in states {
                let Some(init) = initial(&state, last) else {
                    unimplemented.insert(state);
                    continue;
                };
                let a = tokeniser::tokenise(&tokeniser::chars(&input), init);
                if !a.markup.is_empty() {
                    refused += 1;
                    continue;
                }
                accepted += 1;
                let got = normalised(a.tokens);
                if got != want {
                    failures.push(format!(
                        "{} / {:?} / {state}:\n  input:    {input:?}\n  expected: {want:?}\n  got:      {got:?}",
                        file.file_name().unwrap().to_string_lossy(),
                        t["description"].as_str().unwrap_or_default()
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} accepted corpus inputs tokenise differently:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // The one corpus state outside the model is CDATA, which the tokeniser reaches only from
    // `<![CDATA[` inside foreign content, never as a starting state.
    assert_eq!(
        unimplemented.into_iter().collect::<Vec<_>>(),
        vec!["CDATA section state".to_string()],
        "the corpus starts in a state the model neither implements nor names here"
    );
    eprintln!(
        "html5lib-tests tokeniser corpus: {accepted} accepted, {refused} refused, {skipped} skipped"
    );
    assert!(
        accepted >= 300 && refused >= 100,
        "the corpus should exercise both sides: {accepted} accepted, {refused} refused, {skipped} skipped"
    );
}

/// Every state the model does not implement is refused, never guessed: a sample of inputs that
/// reach each one.
#[test]
fn every_state_outside_the_model_is_refused() {
    for input in [
        "<![CDATA[x]]>",           // CDATA outside foreign content: a bogus comment
        "<?xml version=\"1.0\"?>", // processing instruction: a bogus comment
        "<!x>",                    // markup declaration open, bogus
        "</ x>",                   // end tag open, bogus comment
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\">", // doctype identifiers
        "<p a=\"1\"",              // EOF in a tag
    ] {
        let a = tokeniser::tokenise(&tokeniser::chars(input), Initial::Data);
        assert!(!a.markup.is_empty(), "{input:?} should be refused");
    }
    // The script data escaped states, and a `</script` that is not the end tag.
    for input in ["<!--<script>x</script>-->", "a</scriptx>"] {
        let a = tokeniser::tokenise(
            &tokeniser::chars(input),
            Initial::ScriptData("script".into()),
        );
        assert!(
            !a.markup.is_empty(),
            "{input:?} in script data should be refused"
        );
    }
}
