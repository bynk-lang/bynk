//! #3979: the named capture groups of a regex literal.
//!
//! A regex in Bynk is always a literal checked at compile time, so its group
//! names are known to the compiler. `findRegexNamed` keys its result by them;
//! the checker refuses a pattern with none, and the emitter writes one entry per
//! name. Both read the names through this one function, so they cannot disagree.

/// The names of `pattern`'s named capture groups (`(?<major>\d+)`), in the order
/// they first appear, each once. A lookbehind (`(?<=…)`, `(?<!…)`), an escaped
/// parenthesis and a parenthesis inside a character class are not groups.
pub fn group_names(pattern: &str) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut names: Vec<String> = Vec::new();
    let mut i = 0;
    let mut in_class = false;
    while i < chars.len() {
        match chars[i] {
            // An escape consumes the next character, whatever it is.
            '\\' => i += 1,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class
                && chars.get(i + 1) == Some(&'?')
                && chars.get(i + 2) == Some(&'<')
                && !matches!(chars.get(i + 3), Some('=') | Some('!')) =>
            {
                let name: String = chars[i + 3..].iter().take_while(|c| **c != '>').collect();
                let end = i + 3 + name.chars().count();
                if chars.get(end) == Some(&'>') && !name.is_empty() && !names.contains(&name) {
                    names.push(name);
                }
            }
            _ => {}
        }
        i += 1;
    }
    names
}

#[cfg(test)]
mod tests {
    use super::group_names;

    #[test]
    fn named_groups_come_out_in_order() {
        assert_eq!(
            group_names(r"v(?<major>\d+)\.(?<minor>\d+)"),
            ["major", "minor"]
        );
    }

    #[test]
    fn unnamed_groups_and_lookarounds_are_not_named_groups() {
        assert!(group_names(r"(\d+)(?:a)(?=b)(?<=c)(?<!d)(?!e)").is_empty());
    }

    #[test]
    fn an_escaped_paren_or_a_class_does_not_open_a_group() {
        assert!(group_names(r"\(?<x>a\)").is_empty());
        assert!(group_names(r"[(?<x>a)]").is_empty());
        assert_eq!(group_names(r"[\]](?<y>b)"), ["y"]);
    }

    #[test]
    fn a_name_repeated_in_alternatives_is_one_key() {
        assert_eq!(group_names(r"(?<n>a)|(?<n>b)"), ["n"]);
    }
}
