//! #3959 (capability-parameters track §13, slice 4): hover on a call that binds
//! a commons function's capability parameters names the instance each one is
//! bound to (`remember(key, given Store = Sessions)` reads `Store` → `Sessions`),
//! and hover on a binding's parameter name says what it binds.
//!
//! Read from the source text around the cursor, as signature help reads its
//! call context: a call's bindings are the part of its argument list after a
//! top-level `given`.

/// One `Param = Value` binding of a call, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub param: String,
    pub value: String,
    /// The byte range of `param` in the text.
    pub param_span: (usize, usize),
}

/// The bindings of the call whose callee name covers `offset`
/// (`rem|ember(key, given Store = Sessions)`), or `None` when that name is not
/// a call or the call binds nothing.
pub(crate) fn call_bindings_at(text: &str, offset: usize) -> Option<Vec<Binding>> {
    let (start, end) = ident_bounds(text, offset)?;
    if start == end {
        return None;
    }
    let after = &text[end..];
    let open = end + (after.len() - after.trim_start().len());
    if text[open..].chars().next() != Some('(') {
        return None;
    }
    let close = matching_paren(text, open)?;
    let bindings = bindings_in(text, open + 1, close);
    (!bindings.is_empty()).then_some(bindings)
}

/// The callee name and binding whose parameter name covers `offset`
/// (`remember(key, given St|ore = Sessions)`).
pub(crate) fn binding_param_at(text: &str, offset: usize) -> Option<(String, Binding)> {
    let open = enclosing_open_paren(text, offset)?;
    let close = matching_paren(text, open)?;
    let binding = bindings_in(text, open + 1, close)
        .into_iter()
        .find(|b| b.param_span.0 <= offset && offset <= b.param_span.1)?;
    let before = text[..open].trim_end();
    let callee_start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map(|(i, _)| i)?;
    Some((before[callee_start..].to_string(), binding))
}

/// The hover section a call's bindings add to its function's description.
pub(crate) fn bindings_section(bindings: &[Binding]) -> String {
    let lines: Vec<String> = bindings
        .iter()
        .map(|b| format!("- `{}` is bound to `{}`", b.param, b.value))
        .collect();
    format!("**Bound at this call**\n\n{}", lines.join("\n"))
}

/// The hover for a binding's parameter name.
pub(crate) fn binding_param_hover(callee: &str, binding: &Binding) -> String {
    format!(
        "`{}`: a capability parameter of `{callee}`, bound to `{}` at this call.\n\nEvery call binds each capability parameter explicitly; nothing is inferred.",
        binding.param, binding.value
    )
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The identifier around `offset`, as a byte range.
fn ident_bounds(text: &str, offset: usize) -> Option<(usize, usize)> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = offset
        + text[offset..]
            .char_indices()
            .find(|(_, c)| !is_ident(*c))
            .map_or(text.len() - offset, |(i, _)| i);
    Some((start, end))
}

/// The `)` matching the `(` at `open`, skipping string literals.
fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut prev = '\0';
    for (i, c) in text[open..].char_indices() {
        if in_str {
            if c == '"' && prev != '\\' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return (c == ')').then_some(open + i);
                    }
                }
                _ => {}
            }
        }
        prev = c;
    }
    None
}

/// The `(` of the innermost argument list still open at `offset`.
fn enclosing_open_paren(text: &str, offset: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in text[..offset.min(text.len())].char_indices().rev() {
        match c {
            ')' | ']' | '}' => depth += 1,
            '(' | '[' | '{' => {
                if depth == 0 {
                    return (c == '(').then_some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// The bindings written between `from` and `to` (an argument list's inside):
/// the top-level items after a top-level `given`.
fn bindings_in(text: &str, from: usize, to: usize) -> Vec<Binding> {
    let items = top_level_items(text, from, to);
    let Some(first) = items.iter().position(|(s, _)| {
        let item = text[*s..].trim_start();
        item.starts_with("given") && item[5..].starts_with(char::is_whitespace)
    }) else {
        return Vec::new();
    };
    items[first..]
        .iter()
        .enumerate()
        .filter_map(|(k, (s, e))| {
            let mut item_start = *s;
            if k == 0 {
                let lead = text[*s..*e].len() - text[*s..*e].trim_start().len();
                item_start = *s + lead + "given".len();
            }
            let item = &text[item_start..*e];
            let (param, value) = item.split_once('=')?;
            let lead = param.len() - param.trim_start().len();
            let p_start = item_start + lead;
            let param = param.trim();
            (!param.is_empty() && param.chars().all(is_ident)).then(|| Binding {
                param: param.to_string(),
                value: value.trim().to_string(),
                param_span: (p_start, p_start + param.len()),
            })
        })
        .collect()
}

/// The top-level comma-separated items between `from` and `to`, as byte ranges.
fn top_level_items(text: &str, from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut prev = '\0';
    let mut start = from;
    for (i, c) in text[from..to].char_indices() {
        let at = from + i;
        if in_str {
            if c == '"' && prev != '\\' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    out.push((start, at));
                    start = at + 1;
                }
                _ => {}
            }
        }
        prev = c;
    }
    out.push((start, to));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "remember(key, \"a, given X = Y\", given Store = Sessions, Cold = Kv)";

    /// #3959: a call's bindings are the items after its top-level `given`; a
    /// `given` inside a string is not one.
    #[test]
    fn a_call_lists_its_bindings() {
        let b = call_bindings_at(SRC, 3).expect("a call with bindings");
        let pairs: Vec<(&str, &str)> = b
            .iter()
            .map(|b| (b.param.as_str(), b.value.as_str()))
            .collect();
        assert_eq!(pairs, [("Store", "Sessions"), ("Cold", "Kv")]);
        assert!(bindings_section(&b).contains("`Store` is bound to `Sessions`"));
    }

    /// #3959: a call without bindings, and a name that is not a call, have none.
    #[test]
    fn no_bindings_no_section() {
        assert!(call_bindings_at("remember(key)", 3).is_none());
        assert!(call_bindings_at("let remember = 1", 6).is_none());
    }

    /// #3959: hover on a binding's parameter name names the callee and the value.
    #[test]
    fn a_binding_parameter_names_what_it_binds() {
        let at = SRC.find("Store").unwrap() + 2;
        let (callee, b) = binding_param_at(SRC, at).expect("a binding parameter");
        assert_eq!(callee, "remember");
        assert_eq!(b.value, "Sessions");
        assert!(binding_param_hover(&callee, &b).contains("bound to `Sessions`"));
        let not = SRC.find("key").unwrap();
        assert!(binding_param_at(SRC, not).is_none());
    }
}
