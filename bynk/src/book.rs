//! `bynk book` — the Bynk Book as this toolchain documents it (#2463).
//!
//! The editor's `bynk_search_book` language-model tool (and `bynk mcp`) need Book
//! text that matches the `bynk` the user runs, not the live site (which may
//! describe features this build lacks) and not the extension's own version. The
//! driver embeds the whole Book at build time (`build.rs`: the `llms-full.txt`
//! bundle, every page in sidebar order) and serves it two ways:
//!
//! - `bynk book` prints the whole Book under a first line naming the toolchain
//!   version;
//! - `bynk book --search <query>` returns the best-matching sections, as
//!   Markdown or (`--format json`) as data. [`search`] is the ranking, shared
//!   by every caller inside the driver.
//!
//! A build made without the bundle (a plain `cargo build`) carries none: both
//! forms then say "no Book bundled in this build" and exit non-zero, so a source
//! build never silently returns nothing. The editor falls back to the hosted
//! Book, ranked by a small TypeScript copy of [`search`].

use std::io::Write;
use std::process::ExitCode;

use bynk_syntax::diagnostics::BOOK_BASE_URL;

/// The Book bundle embedded at build time; empty when the build had none.
const BUNDLE: &str = include_str!(concat!(env!("OUT_DIR"), "/book.txt"));

/// How many sections a search returns.
pub const RESULTS: usize = 5;

/// The longest excerpt of one section, in characters.
const EXCERPT_CHARS: usize = 1_800;

/// One heading's worth of a Book page.
#[derive(Debug, PartialEq)]
pub struct Section {
    /// The page's hosted URL.
    pub url: String,
    /// The page title, then the headings down to this section.
    pub trail: Vec<String>,
    /// The section's text, headings excluded.
    pub text: String,
}

/// A Book text, split for searching.
#[derive(Debug, PartialEq)]
pub struct Book {
    pub sections: Vec<Section>,
}

/// The hosted URL of a Book source page: `reference/types.md` →
/// `/book/reference/types/`.
fn page_url(source: &str) -> String {
    let slug = source
        .strip_suffix(".mdx")
        .or_else(|| source.strip_suffix(".md"))
        .unwrap_or(source);
    let slug = if slug == "index" {
        ""
    } else {
        slug.strip_suffix("/index").unwrap_or(slug)
    };
    if slug.is_empty() {
        format!("{BOOK_BASE_URL}/book/")
    } else {
        format!("{BOOK_BASE_URL}/book/{slug}/")
    }
}

/// An ATX heading (`#` to `####`) line's depth and text, with a trailing
/// `{#anchor}` dropped.
fn heading(line: &str) -> Option<(usize, String)> {
    let depth = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=4).contains(&depth) || !line[depth..].starts_with(' ') {
        return None;
    }
    let mut text = line[depth..].trim();
    if let Some(at) = text.rfind(" {#").filter(|_| text.ends_with('}')) {
        text = text[..at].trim_end();
    }
    (!text.is_empty()).then(|| (depth, text.to_string()))
}

/// Split a Book text (the layout `site/scripts/build-llms-full.mjs` writes: each
/// page between `# ──` rules, headed `# Source: book/<page>`) into sections.
pub fn parse(text: &str) -> Book {
    let lines: Vec<&str> = text.lines().collect();
    let mut sections = Vec::new();
    let mut url: Option<String> = None;
    let mut trail: Vec<String> = Vec::new();
    let mut body: Vec<&str> = Vec::new();
    let mut in_fence = false;
    let flush =
        |url: &Option<String>, trail: &[String], body: &mut Vec<&str>, out: &mut Vec<Section>| {
            let joined = body.join("\n");
            let joined = joined.trim();
            if let (Some(url), false) = (url, joined.is_empty()) {
                out.push(Section {
                    url: url.clone(),
                    trail: trail.to_vec(),
                    text: joined.to_string(),
                });
            }
            body.clear();
        };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let is_marker = line.starts_with("# ─")
            && lines
                .get(i + 1)
                .is_some_and(|l| l.starts_with("# Source: book/"))
            && lines.get(i + 2).is_some_and(|l| l.starts_with("# ─"));
        if is_marker {
            flush(&url, &trail, &mut body, &mut sections);
            url = Some(page_url(&lines[i + 1]["# Source: book/".len()..]));
            trail.clear();
            in_fence = false;
            i += 3;
            continue;
        }
        i += 1;
        if url.is_none() {
            continue;
        }
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        match if in_fence { None } else { heading(line) } {
            None => body.push(line),
            Some((depth, title)) => {
                flush(&url, &trail, &mut body, &mut sections);
                // The page title is the first heading; deeper ones nest under it.
                trail.truncate(depth - 1);
                while trail.len() < depth - 1 {
                    trail.push(String::new());
                }
                trail.push(title);
                trail.retain(|t| !t.is_empty());
            }
        }
    }
    flush(&url, &trail, &mut body, &mut sections);
    Book { sections }
}

/// The query's words, lower-cased and de-duplicated.
fn terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lower = query.to_lowercase();
    for word in lower.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if word.chars().count() > 1 && !out.iter().any(|t| t == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// The best `limit` sections for `query`: a word in a heading counts more than
/// one in the body, a section holding every word beats one holding some, and a
/// verbatim phrase beats both. Ties keep the Book's reading order.
pub fn search<'a>(book: &'a Book, query: &str, limit: usize) -> Vec<&'a Section> {
    let terms = terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let phrase = query.trim().to_lowercase();
    let mut scored: Vec<(f64, usize, &Section)> = Vec::new();
    for (order, section) in book.sections.iter().enumerate() {
        let head = section.trail.join(" ").to_lowercase();
        let body = section.text.to_lowercase();
        let mut matched = 0usize;
        let mut score = 0f64;
        for t in &terms {
            let in_head = head.matches(t.as_str()).count();
            let in_body = body.matches(t.as_str()).count();
            if in_head + in_body == 0 {
                continue;
            }
            matched += 1;
            score += (in_head.min(2) * 6 + in_body.min(5)) as f64;
        }
        if matched == 0 {
            continue;
        }
        let coverage = matched as f64 / terms.len() as f64;
        score *= coverage * coverage;
        if terms.len() > 1 && (head.contains(&phrase) || body.contains(&phrase)) {
            score += 10.0;
        }
        scored.push((score, order, section));
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|s| s.2).collect()
}

/// At most `max` characters of `text`, from the first line holding one of
/// `terms` when the section is longer, with any code fence it cut left closed.
fn excerpt(text: &str, terms: &[String], max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    let from = lines
        .iter()
        .position(|l| {
            let l = l.to_lowercase();
            terms.iter().any(|t| l.contains(t.as_str()))
        })
        .unwrap_or(0);
    let mut taken: Vec<String> = Vec::new();
    let mut used = 0;
    for line in &lines[from..] {
        let len = line.chars().count() + 1;
        if used + len > max && !taken.is_empty() {
            break;
        }
        // One enormous line still gets shown, cut.
        taken.push(line.chars().take(max).collect());
        used += len;
    }
    let cut_after = from + taken.len() < lines.len();
    if taken
        .iter()
        .filter(|l| l.trim_start().starts_with("```"))
        .count()
        % 2
        == 1
    {
        taken.push("```".to_string());
    }
    let lead = if from > 0 { "…\n" } else { "" };
    let tail = if cut_after { "\n…" } else { "" };
    format!("{lead}{}{tail}", taken.join("\n"))
}

/// A section's page title and own heading; the headings between are noise.
fn short_trail(trail: &[String]) -> String {
    match trail {
        [] => "(untitled)".to_string(),
        [only] => only.clone(),
        [first, .., last] => format!("{first} › {last}"),
    }
}

/// The line that labels a result with the Book's version.
fn source_note() -> String {
    format!(
        "Source: the Bynk Book v{}, from the installed toolchain.",
        env!("CARGO_PKG_VERSION")
    )
}

/// `bynk book --search` as Markdown.
fn render_markdown(book: &Book, query: &str) -> String {
    let note = source_note();
    let hits = search(book, query, RESULTS);
    if hits.is_empty() {
        return format!(
            "# Bynk Book: no match for \"{query}\"\n\n{note}\n\n\
             Try other words, or browse {BOOK_BASE_URL}/book/.\n"
        );
    }
    let terms = terms(query);
    let parts: Vec<String> = hits
        .iter()
        .enumerate()
        .map(|(i, h)| {
            format!(
                "**{}. {}** — {}\n\n{}\n",
                i + 1,
                short_trail(&h.trail),
                h.url,
                excerpt(&h.text, &terms, EXCERPT_CHARS)
            )
        })
        .collect();
    format!(
        "# Bynk Book: {} result(s) for \"{query}\"\n\n{note}\n\n{}",
        hits.len(),
        parts.join("\n---\n\n")
    )
}

/// `bynk book --search --format json`.
fn render_json(book: &Book, query: &str) -> String {
    let terms = terms(query);
    let results: Vec<serde_json::Value> = search(book, query, RESULTS)
        .into_iter()
        .map(|h| {
            serde_json::json!({
                "trail": h.trail,
                "url": h.url,
                "text": excerpt(&h.text, &terms, EXCERPT_CHARS),
            })
        })
        .collect();
    let doc = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "query": query,
        "results": results,
    });
    format!("{doc}\n")
}

/// How `bynk book --search` prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

/// The embedded Book, or `None` for a build made without the bundle.
pub fn embedded() -> Option<Book> {
    (!BUNDLE.trim().is_empty()).then(|| parse(BUNDLE))
}

/// Run `bynk book`: print the whole Book, or with a `query` the matching
/// sections. Exits `0` on success (including a search with no match) and `1`
/// when this build carries no Book.
pub fn run(query: Option<&str>, format: Format) -> ExitCode {
    if BUNDLE.trim().is_empty() {
        eprintln!(
            "error: no Book bundled in this build of `bynk`. Read it at \
             {BOOK_BASE_URL}/book/ (or {BOOK_BASE_URL}/llms-full.txt)."
        );
        return ExitCode::FAILURE;
    }
    let text = match query {
        None => format!("Bynk Book v{}\n\n{BUNDLE}", env!("CARGO_PKG_VERSION")),
        Some(q) => {
            let book = parse(BUNDLE);
            match format {
                Format::Text => render_markdown(&book, q),
                Format::Json => render_json(&book, q),
            }
        }
    };
    // A closed pipe (`bynk book | head`) is the reader's choice.
    let _ = std::io::stdout().write_all(text.as_bytes());
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = "# ────────────────────────────────────────────────────────────";

    fn page(source: &str, body: &str) -> String {
        format!("{RULE}\n# Source: book/{source}\n{RULE}\n\n{body}\n")
    }

    fn bundle() -> String {
        let types = "# Types\n\nEvery value has a type.\n\n## Refined types\n\nA refined type adds a `where` predicate.\n\n\
                     ```bynk\n# not a heading\ntype Qty = Int where Positive\n```\n\n## Aliases\n\nA name for a type.";
        let agents = "# Agents\n\nAn agent holds state per key.\n\n## Handlers {#handlers}\n\nHandlers mutate state.";
        format!(
            "# Bynk — Full Documentation\n\nintro\n\n{}\n{}\n{}",
            page("reference/types.md", types),
            page("guides/agents/index.md", agents),
            page("index.md", "# The Bynk Book\n\nWelcome."),
        )
    }

    fn trails(sections: &[&Section]) -> Vec<String> {
        sections.iter().map(|s| s.trail.join(" > ")).collect()
    }

    #[test]
    fn maps_a_source_page_to_its_hosted_url() {
        assert_eq!(
            page_url("reference/types.md"),
            "https://bynk-lang.org/book/reference/types/"
        );
        assert_eq!(
            page_url("guides/agents/index.mdx"),
            "https://bynk-lang.org/book/guides/agents/"
        );
        assert_eq!(page_url("index.md"), "https://bynk-lang.org/book/");
    }

    #[test]
    fn splits_pages_by_heading_leaving_code_fences_whole() {
        let book = parse(&bundle());
        let all: Vec<&Section> = book.sections.iter().collect();
        assert_eq!(
            trails(&all),
            [
                "Types",
                "Types > Refined types",
                "Types > Aliases",
                "Agents",
                "Agents > Handlers",
                "The Bynk Book"
            ]
        );
        assert!(book.sections[1].text.contains("# not a heading"));
        assert_eq!(
            book.sections[1].url,
            "https://bynk-lang.org/book/reference/types/"
        );
    }

    #[test]
    fn ranks_a_heading_match_above_a_body_mention() {
        let book = parse(&bundle());
        assert_eq!(
            trails(&search(&book, "refined types", 5))[0],
            "Types > Refined types"
        );
    }

    #[test]
    fn prefers_a_section_holding_every_word() {
        let book = parse(&bundle());
        assert_eq!(
            trails(&search(&book, "agent handlers", 5))[0],
            "Agents > Handlers"
        );
    }

    #[test]
    fn finds_nothing_for_words_the_book_lacks_or_an_empty_query() {
        let book = parse(&bundle());
        assert!(search(&book, "zebra", 5).is_empty());
        assert!(search(&book, "  ", 5).is_empty());
    }

    #[test]
    fn labels_results_with_the_toolchain_version() {
        let book = parse(&bundle());
        let out = render_markdown(&book, "refined types");
        assert!(out.starts_with("# Bynk Book: "), "{out}");
        assert!(out.contains(&format!(
            "Source: the Bynk Book v{}, from the installed toolchain.",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(out.contains(
            "**1. Types › Refined types** — https://bynk-lang.org/book/reference/types/"
        ));
        let none = render_markdown(&book, "zebra");
        assert!(none.starts_with("# Bynk Book: no match for \"zebra\""));
        assert!(none.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn json_carries_the_version_and_results() {
        let book = parse(&bundle());
        let doc: serde_json::Value =
            serde_json::from_str(&render_json(&book, "refined types")).unwrap();
        assert_eq!(doc["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(doc["results"][0]["trail"][1], "Refined types");
    }

    #[test]
    fn cuts_a_long_section_near_the_match_and_closes_a_cut_fence() {
        let body = format!(
            "# A\n\n## Big\n\n{}needle here\n\n```bynk\n{}```\n",
            "filler line\n".repeat(400),
            "x\n".repeat(900)
        );
        let book = parse(&page("a.md", &body));
        let out = render_markdown_with(&book, "needle", 300);
        assert!(out.contains("needle here"), "{out}");
        assert!(out.chars().count() < 1_200, "{}", out.chars().count());
        assert_eq!(
            out.lines().filter(|l| l.starts_with("```")).count() % 2,
            0,
            "{out}"
        );
    }

    fn render_markdown_with(book: &Book, query: &str, max: usize) -> String {
        let terms = terms(query);
        search(book, query, 1)
            .iter()
            .map(|h| excerpt(&h.text, &terms, max))
            .collect()
    }
}
