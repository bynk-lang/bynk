//! Shared test helper: turn a generated mdBook-format reference page into the
//! Starlight page committed under `site/src/content/docs/book/`. This is the
//! same transform `site/scripts/migrate-page.mjs` applied during the Book
//! migration — lift the leading `# H1` into `title:` frontmatter (dropping the
//! heading), and rewrite relative `.md` links to `/book/<slug>/` routes — so the
//! generated-page drift guards keep their committed site pages byte-identical to
//! the migrated originals once `docs/` is gone.

use regex::{Captures, Regex};

/// Transform `markdown` (a generated reference page, mdBook form) into its
/// committed Starlight form. `page_dir` is the page's directory within the Book
/// (e.g. `"reference"`), used to resolve relative links.
pub fn to_site_page(markdown: &str, page_dir: &str) -> String {
    let (title, body) = lift_title(markdown);
    let body = rewrite_links(&body, page_dir);
    let mut out = format!("---\ntitle: {}\n---\n", yaml_string(&title));
    out.push_str(body.trim_start_matches('\n'));
    out
}

/// Split off the first `# H1` as the title; the rest (heading removed) is the body.
fn lift_title(source: &str) -> (String, String) {
    let mut lines: Vec<&str> = source.split('\n').collect();
    for i in 0..lines.len() {
        if let Some(rest) = lines[i].strip_prefix("# ") {
            let title = rest.trim().to_string();
            lines.remove(i);
            return (title, lines.join("\n"));
        }
        if !lines[i].trim().is_empty() {
            break;
        }
    }
    panic!("generated page has no leading `# H1` to lift into the title");
}

/// Rewrite relative `*.md` links to `/book/<slug>/` routes, resolving each
/// against `page_dir`. External / in-page / already-rooted links are untouched.
fn rewrite_links(body: &str, page_dir: &str) -> String {
    let re = Regex::new(r"\]\(([^)]+)\)").unwrap();
    re.replace_all(body, |caps: &Captures| {
        let target = &caps[1];
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("mailto:")
            || target.starts_with('#')
            || target.starts_with('/')
        {
            return format!("]({target})");
        }
        let (path_part, anchor) = match target.split_once('#') {
            Some((p, a)) => (p, Some(a)),
            None => (target, None),
        };
        if !path_part.ends_with(".md") {
            return format!("]({target})");
        }
        let joined = normalize(&format!("{page_dir}/{path_part}"));
        if joined.starts_with("..") {
            return format!("]({target})"); // outside the Book tree — leave as-is
        }
        let mut slug = joined.strip_suffix(".md").unwrap().to_string();
        if let Some(prefix) = slug.strip_suffix("/index") {
            slug = prefix.to_string();
        } else if slug == "index" {
            slug.clear();
        }
        let hash = anchor.map(|a| format!("#{a}")).unwrap_or_default();
        format!("](/book/{slug}/{hash})")
    })
    .into_owned()
}

/// Collapse `.` and `..` components in a `/`-separated relative path.
fn normalize(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if matches!(stack.last(), Some(&c) if c != "..") {
                    stack.pop();
                } else {
                    stack.push("..");
                }
            }
            c => stack.push(c),
        }
    }
    stack.join("/")
}

/// Quote a frontmatter title only when YAML could misread the bare scalar —
/// matching `migrate-page.mjs`'s rule so titles render identically.
fn yaml_string(value: &str) -> String {
    let bare = Regex::new(r"^[A-Za-z][A-Za-z0-9_ ,.'’!?()&/-]*$").unwrap();
    let colon_space = Regex::new(r":\s").unwrap();
    if bare.is_match(value) && !colon_space.is_match(value) {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}
