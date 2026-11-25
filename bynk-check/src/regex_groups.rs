//! #3979: the named capture groups of a regex literal.
//!
//! A regex in Bynk is always a literal checked at compile time, so its group
//! names are known to the compiler. `findRegexNamed` keys its result by them;
//! the checker refuses a pattern with none, and the emitter writes one entry per
//! name. Both read the names through this one function, so they cannot disagree.

/// The names of `pattern`'s named capture groups (`(?<major>\d+)`), in the order
/// they appear. A lookbehind (`(?<=…)`, `(?<!…)`), an escaped parenthesis and a
/// parenthesis inside a character class are not groups.
fn scan(pattern: &str) -> Vec<String> {
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
                if chars.get(end) == Some(&'>') && !name.is_empty() {
                    names.push(name);
                }
            }
            _ => {}
        }
        i += 1;
    }
    names
}

/// The names of `pattern`'s named capture groups, each once, in the order they
/// first appear.
pub fn group_names(pattern: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in scan(pattern) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The first group name `pattern` uses more than once, if any. The target
/// runtimes refuse one (`new RegExp` throws "Duplicate capture group name"
/// before ES2025's duplicate-names-in-alternatives), so it is a compile error.
pub fn duplicate_group_name(pattern: &str) -> Option<String> {
    let names = scan(pattern);
    names
        .iter()
        .enumerate()
        .find(|(i, n)| names[..*i].contains(n))
        .map(|(_, n)| n.clone())
}

#[cfg(test)]
mod tests {
    use super::{duplicate_group_name, group_names};

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
    fn a_repeated_name_is_one_key_and_is_reported() {
        assert_eq!(group_names(r"(?<n>a)|(?<n>b)"), ["n"]);
        assert_eq!(
            duplicate_group_name(r"(?<n>a)|(?<n>b)").as_deref(),
            Some("n")
        );
        assert_eq!(duplicate_group_name(r"(?<n>a)(?<m>b)"), None);
    }
}
