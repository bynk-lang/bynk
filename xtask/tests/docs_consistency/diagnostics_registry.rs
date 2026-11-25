//! Keeps the diagnostic registry, the compiler source, and the generated
//! reference page in lock-step.
//!
//! 1. Every `bynk.*` code used as a string literal in the compiler source must
//!    appear in `bynk_syntax::diagnostics::REGISTRY`, and vice versa. "Compiler
//!    source" is the `src/` of every workspace member (#1662), since emit sites
//!    are spread across crates.
//! 2. `site/src/generated/diagnostics/<code>.json`, one file per code, must match what
//!    the registry renders, with no file for a code the registry lacks. The index page is
//!    assembled from them at site build time (#4286), so it is not committed.
//!
//! Regenerate the per-code files after changing the registry with:
//!     BYNK_BLESS=1 cargo test -p xtask --test docs_consistency diagnostics_registry::

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_syntax::diagnostics::{EXPLANATIONS, REGISTRY};

fn grammar_json() -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tree-sitter-bynk/src/grammar.json");
    fs::read_to_string(path).expect("read grammar.json")
}

/// String literals that happen to share the two-dot `"bynk.x.y"` shape this
/// regex uses as its heuristic for a diagnostic code, but are not one — a
/// firstparty commons name (`firstparty.rs`'s `LOCALE_TYPES_UNIT`) or a
/// filename (`schema_registry.rs`'s `parse`, #1085 review — named in a
/// corruption message, not a `LOCK_FILE` constant as before #1078, but still
/// the literal filename). Excluded by value rather than by skipping their
/// whole file, since that file also contains genuine diagnostic emissions
/// that must still be counted.
///
/// #1662 widened the scan to every workspace member, which adds a lockfile
/// name (`bynk`'s `bynk.deploy.lock`) and three placeholder codes that tests
/// use to exercise code-agnostic machinery (rendering, `explain` and LSP
/// lookups of an unknown code, the literal scanner itself).
const NON_DIAGNOSTIC_LOOKALIKES: &[&str] = &[
    "bynk.locale.types",
    "bynk.schema.lock",
    "bynk.deploy.lock",
    // #2554: the contracts lock beside `bynk.schema.lock`.
    "bynk.contracts.lock",
    // I4 (#1970): the foreign-caller pin file beside `bynk.toml`.
    "bynk.foreign.toml",
    "bynk.test.example",
    "bynk.not.a_real_code",
    "bynk.check.something",
    // Standard-library item paths (S3, #1963): `[unstable] allow` entries and
    // stability lookups in the `bynk-std`/`bynk-check` tests, not codes.
    "bynk.list.reverse",
    "bynk.string.join",
    "bynk.text.shout",
];

/// Collect every `"bynk.x.y"` string literal across the compiler source,
/// excluding the registry module itself. Scans the `src/` of **every**
/// workspace member, read from the root `Cargo.toml`, rather than a
/// hand-kept list: #1662 found five codes emitted by crates the old list
/// missed (`bynk-fmt`, `bynk-wasm`, `bynk`), which `bynk explain` then
/// denied existed.
fn codes_used_in_source() -> BTreeSet<String> {
    let re = regex::Regex::new(r#""(bynk\.[a-z][a-z0-9_]*\.[a-z_]+)""#).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut codes = BTreeSet::new();
    for member in workspace_members(&root) {
        collect(&root.join(member).join("src"), &re, &mut codes);
    }
    codes
}

/// The `[workspace] members` of the root `Cargo.toml`.
fn workspace_members(root: &Path) -> Vec<String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let start = manifest
        .find("members = [")
        .expect("a workspace members list");
    let list = &manifest[start..start + manifest[start..].find(']').unwrap()];
    let members: Vec<String> = list
        .lines()
        .map(|l| l.split("--").next().unwrap_or("").trim())
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('"').nth(1))
        .map(str::to_string)
        .collect();
    assert!(
        members.len() > 10,
        "parsed too few workspace members: {members:?}"
    );
    members
}

fn collect(dir: &Path, re: &regex::Regex, out: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, re, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            // The registry module deliberately lists every code; skip it so the
            // comparison reflects actual emit sites. Matched by path, not file
            // name: other crates have their own `diagnostics.rs`, and those must
            // still be scanned (#1662 review).
            if path.ends_with("bynk-syntax/src/diagnostics.rs") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            for caps in re.captures_iter(&text) {
                let code = &caps[1];
                if !NON_DIAGNOSTIC_LOOKALIKES.contains(&code) {
                    out.insert(code.to_string());
                }
            }
        }
    }
}

fn registry_codes() -> BTreeSet<String> {
    REGISTRY.iter().map(|d| d.code.to_string()).collect()
}

#[test]
fn registry_has_no_duplicates_and_is_sorted() {
    let codes: Vec<&str> = REGISTRY.iter().map(|d| d.code).collect();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    assert_eq!(codes, sorted, "REGISTRY must be sorted by code");

    let unique: BTreeSet<&str> = codes.iter().copied().collect();
    assert_eq!(
        unique.len(),
        codes.len(),
        "REGISTRY contains duplicate codes"
    );
}

#[test]
fn registry_matches_codes_used_in_source() {
    let used = codes_used_in_source();
    let registered = registry_codes();

    let missing: Vec<&String> = used.difference(&registered).collect();
    let extra: Vec<&String> = registered.difference(&used).collect();

    assert!(
        missing.is_empty(),
        "codes emitted in source but missing from bynk_syntax::diagnostics::REGISTRY: {missing:#?}\n\
         Add an entry for each in bynk-syntax/src/diagnostics.rs."
    );
    assert!(
        extra.is_empty(),
        "codes in REGISTRY that are no longer used in source: {extra:#?}\n\
         Remove them from bynk-syntax/src/diagnostics.rs."
    );
}

#[test]
fn grammar_symbols_are_embeddable_rules() {
    let grammar = grammar_json();
    // An *embeddable* rule has a `{{#grammar}}` entry (and `#rule-<raw>` anchor)
    // in grammar.md, so a code page's "grammar rules it governs" link resolves. This is
    // stricter than "a real rule": a collapsed trivial wrapper has no entry.
    let embeddable: BTreeSet<String> = xtask::grammar::embeddable_rules(&grammar)
        .into_iter()
        .collect();
    for info in REGISTRY {
        for sym in info.grammar_symbol {
            assert!(
                embeddable.contains(*sym),
                "diagnostic `{}` maps to `{sym}`, which is not an embeddable grammar rule \
                 (it needs a `{{#grammar {sym}}}` entry/anchor in grammar.md; a collapsed \
                 trivial wrapper has none). Fix the grammar_symbol in bynk-syntax/src/diagnostics.rs.",
                info.code
            );
        }
    }
}

// --- #853: `bynk explain` / `codeDescription` explanations ------------------

/// A GitHub-style heading slug: lowercase, non-alphanumerics (bar spaces and
/// hyphens) dropped, spaces collapsed to single hyphens. Sufficient for the
/// simple headings the curated explanations anchor at (the guard fails loudly
/// if a chosen anchor ever needs richer slugging).
fn slug(heading: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in heading.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if (ch == ' ' || ch == '-' || ch == '_') && !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
        // Everything else (backticks, parens, punctuation) is dropped.
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// The `##`-level (and deeper) heading slugs present in a Markdown page.
/// Content inside fenced code blocks is skipped, so a `#`-led line in an example
/// (a shell comment, say) is never mistaken for a heading and can't mask a
/// stale anchor by slugging to the same value.
fn heading_slugs(markdown: &str) -> BTreeSet<String> {
    let mut slugs = BTreeSet::new();
    let mut in_fence = false;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed.starts_with('#') {
            let text = trimmed.trim_start_matches('#').trim();
            // A trailing `{#id}` is a custom heading id, which the site uses in
            // place of the slug of the text.
            if let Some(id) = text
                .strip_suffix('}')
                .and_then(|t| t.rsplit_once("{#"))
                .map(|(_, id)| id)
            {
                slugs.insert(id.to_string());
            }
            let s = slug(text);
            if !s.is_empty() {
                slugs.insert(s);
            }
        }
    }
    slugs
}

#[test]
fn explanations_reference_real_codes_and_are_sorted() {
    let registered: BTreeSet<&str> = REGISTRY.iter().map(|d| d.code).collect();
    for e in EXPLANATIONS {
        assert!(
            registered.contains(e.code),
            "EXPLANATIONS entry `{}` is not a real diagnostic code in REGISTRY \
             (bynk-syntax/src/diagnostics.rs).",
            e.code
        );
    }
    let codes: Vec<&str> = EXPLANATIONS.iter().map(|e| e.code).collect();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    assert_eq!(codes, sorted, "EXPLANATIONS must be sorted by code");

    let unique: BTreeSet<&str> = codes.iter().copied().collect();
    assert_eq!(
        unique.len(),
        codes.len(),
        "EXPLANATIONS has a duplicate code"
    );
}

/// The diagnostic codes a blurb names: each backtick span that is exactly
/// `bynk.<category>.<name>`, lower-case with underscores. A longer or
/// capitalised path (`bynk.cloudflare.RateLimit`, a unit and a capability) is
/// not a code, and nor is a code in running text without backticks.
fn codes_named_in(blurb: &str) -> Vec<&str> {
    let is_part = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    blurb
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| {
            let parts: Vec<&str> = span.split('.').collect();
            parts.len() == 3 && parts[0] == "bynk" && parts[1..].iter().all(|p| is_part(p))
        })
        .collect()
}

/// A blurb cross-references sibling codes in backticks; each must be a real
/// registry code, so a rename or a typo cannot leave a dangling reference.
#[test]
fn explanation_blurbs_name_only_real_codes() {
    let registered: BTreeSet<&str> = REGISTRY.iter().map(|d| d.code).collect();
    let dangling: Vec<String> = EXPLANATIONS
        .iter()
        .flat_map(|e| {
            let registered = &registered;
            codes_named_in(e.blurb)
                .into_iter()
                .filter(move |c| !registered.contains(c))
                .map(move |c| format!("`{}` names `{c}`", e.code))
        })
        .collect();
    assert!(
        dangling.is_empty(),
        "an EXPLANATIONS blurb names a code that is not in REGISTRY \
         (bynk-syntax/src/diagnostics.rs):\n  {}",
        dangling.join("\n  ")
    );
}

#[test]
fn blurb_codes_are_read_from_backtick_spans_only() {
    let blurb = "like `bynk.generics.missing_eq_bound`, not bynk.a.b or \
                 `bynk.cloudflare.RateLimit` or `bynk.crypto` or `bynk.a.b.c`, \
                 but `bynk.x1.y_2` and `Json.decode`";
    assert_eq!(
        codes_named_in(blurb),
        vec!["bynk.generics.missing_eq_bound", "bynk.x1.y_2"]
    );
}

/// The heading slugs of the diagnostic index: one per category, as the per-code files record.
fn index_anchors() -> BTreeSet<String> {
    bynk_syntax::diagnostics::render_json_per_code()
        .iter()
        .map(|(_, json)| {
            let doc: serde_json::Value = serde_json::from_str(json).expect("per-code json");
            doc["categoryAnchor"]
                .as_str()
                .expect("categoryAnchor")
                .to_string()
        })
        .collect()
}

/// A code's links (#3876): `site/src/data/diagnostic-links.json` maps codes to
/// the Book pages that explain them, which the code's page lists. Every key
/// must be a registry code, and every link a page on disk with, where it names
/// one, a heading of that slug. (`astro build`'s link checker is a second,
/// independent guard.)
#[test]
fn diagnostic_links_name_registry_codes_and_real_pages() {
    let site = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site");
    let text = fs::read_to_string(site.join("src/data/diagnostic-links.json"))
        .expect("read site/src/data/diagnostic-links.json");
    let links: std::collections::BTreeMap<String, Vec<String>> =
        serde_json::from_str(&text).expect("diagnostic-links.json is a map of code to links");
    let docs_root = site.join("src/content/docs");
    for (code, targets) in &links {
        assert!(
            REGISTRY.iter().any(|d| d.code == code),
            "diagnostic-links.json names `{code}`, which is not in REGISTRY \
             (bynk-syntax/src/diagnostics.rs). Remove or rename the entry."
        );
        assert!(
            !targets.is_empty(),
            "diagnostic-links.json `{code}` has no links"
        );
        for target in targets {
            let (path, anchor) = target.split_once('#').unwrap_or((target, ""));
            assert!(
                path.starts_with('/') && path.ends_with('/'),
                "diagnostic-links.json `{code}`: `{target}` must be a root-relative `/…/` path"
            );
            let stem = path.trim_start_matches('/').trim_end_matches('/');
            // The index is assembled at build time (#4286), so there is no file to read: its
            // headings are the categories the registry's codes fall into.
            if stem == "reference/diagnostics" {
                assert!(
                    anchor.is_empty() || index_anchors().contains(anchor),
                    "diagnostic-links.json `{code}` links `{target}`, but the diagnostic index \
                     has no category with that slug. Present slugs: {:?}",
                    index_anchors()
                );
                continue;
            }
            let file = ["md", "mdx"]
                .iter()
                .map(|x| docs_root.join(format!("{stem}.{x}")))
                .chain(
                    ["md", "mdx"]
                        .iter()
                        .map(|x| docs_root.join(format!("{stem}/index.{x}"))),
                )
                .find(|f| f.exists())
                .unwrap_or_else(|| {
                    panic!("diagnostic-links.json `{code}` links `{target}`, which has no page")
                });
            if !anchor.is_empty() {
                let slugs = heading_slugs(&fs::read_to_string(&file).unwrap());
                assert!(
                    slugs.contains(anchor),
                    "diagnostic-links.json `{code}` links `{target}`, but {} has no heading \
                     with that slug. Present slugs: {slugs:?}",
                    file.display()
                );
            }
        }
    }
}

/// A link to a code's page (`/reference/diagnostics/<code>/`) must name a
/// registry code. The site's links validator cannot see these custom pages, so
/// it skips them; this is the check in its place, over every page the site
/// builds from (#3876).
#[test]
fn links_to_code_pages_name_registry_codes() {
    const PREFIX: &str = "/reference/diagnostics/";
    let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site/src/content");
    let mut files = vec![content];
    let mut bad = Vec::new();
    while let Some(path) = files.pop() {
        if path.is_dir() {
            files.extend(fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            continue;
        }
        if !path.extension().is_some_and(|x| x == "md" || x == "mdx") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for (i, _) in text.match_indices(PREFIX) {
            let rest = &text[i + PREFIX.len()..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_'))
                .unwrap_or(rest.len());
            let code = &rest[..end];
            // The index itself (`…/diagnostics/` or `…/diagnostics/#anchor`).
            if code.is_empty() {
                continue;
            }
            if !REGISTRY.iter().any(|d| d.code == code) {
                bad.push(format!("{}: {PREFIX}{code}/", path.display()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "links to a diagnostic code's page that is not in REGISTRY \
         (bynk-syntax/src/diagnostics.rs):\n  {}",
        bad.join("\n  ")
    );
}

/// `site/src/generated/diagnostics/<code>.json`, the data behind each code's page (#3876) and the
/// index (#4286), must match what the registry renders, one file per code. A file for a code the
/// registry no longer has fails too, so a removed code takes its file with it.
#[test]
fn generated_diagnostics_json_is_up_to_date() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site/src/generated/diagnostics");
    let rendered = bynk_syntax::diagnostics::render_json_per_code();
    if std::env::var_os("BYNK_BLESS").is_some() {
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (code, json) in &rendered {
            fs::write(dir.join(format!("{code}.json")), json).unwrap();
        }
        return;
    }
    let mut problems = Vec::new();
    for (code, json) in &rendered {
        let current = fs::read_to_string(dir.join(format!("{code}.json"))).unwrap_or_default();
        if current != *json {
            problems.push(format!("{code}.json is missing or out of date"));
        }
    }
    let known: BTreeSet<String> = rendered.iter().map(|(c, _)| format!("{c}.json")).collect();
    for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !known.contains(&name) {
            problems.push(format!("{name} is not a registry code"));
        }
    }
    assert!(
        problems.is_empty(),
        "site/src/generated/diagnostics/ is out of date with the registry:\n  {}\n\
         Regenerate with: BYNK_BLESS=1 cargo test -p xtask --test docs_consistency diagnostics_registry::",
        problems.join("\n  ")
    );
}
