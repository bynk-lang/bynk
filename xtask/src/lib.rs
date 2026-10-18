//! The pending-increment format validator (increment-allocation track, Slice 0).
//!
//! A feature PR adds one `design/pending/<slug>.md` declaring its bump level, a
//! one-line changelog blurb, and — when it records a decision — one or more ADR
//! prose blocks. It writes *no* version and *no* ADR number: those are the two
//! serial counters that the merge-time stamp assigns on `main`, so that parallel
//! increments stop conflicting on them. See `design/pending/README.md` and
//! ADR 0206 (`design/decisions/0206-allocation-on-main.md`).
//!
//! This module is the *format contract* between that human-authored file and the
//! future stamp. It is process tooling, not compiler behaviour, which is why it
//! lives in the unpublished `xtask` crate rather than in `bynkc`'s test suite.
//! [`check_all`] is exercised two ways: an integration test (`tests/pending_files.rs`)
//! runs it over the real `design/pending/**` as a drift guard, and the
//! `check-pending` binary subcommand exposes it for local runs.

use std::fs;
use std::path::{Path, PathBuf};

pub mod greenfield_status;
pub mod manifest_schema;
pub mod stamp;

/// The bump level an increment declares. The stamp turns this into the next
/// `X.Y.Z` in merge order; the format never carries a concrete number.
#[derive(Debug, PartialEq, Eq)]
pub enum Level {
    Minor,
    Patch,
}

/// One ADR block. The stamp writes `design/decisions/NNNN-<slug>.md` — a
/// `# NNNN — <title>` heading, a status line, then `body` verbatim — and a
/// `decisions/README.md` index row (`**<title>** … <summary>`), assigning
/// `NNNN` at merge. `title` is required (the file heading and the index bold
/// need it); `summary` defaults to `title`, `status` to `Accepted`.
#[derive(Debug, PartialEq, Eq)]
pub struct Adr {
    pub slug: String,
    pub title: String,
    pub summary: Option<String>,
    pub status: Option<String>,
    pub body: String,
}

impl Adr {
    /// The one-line distillation for the index row — the author's `summary`, or
    /// the title when none was given.
    pub fn summary(&self) -> &str {
        self.summary.as_deref().unwrap_or(&self.title)
    }

    /// The ADR status — the author's `status`, or `Accepted`.
    pub fn status(&self) -> &str {
        self.status.as_deref().unwrap_or("Accepted")
    }
}

/// A parsed, validated pending-increment file.
#[derive(Debug, PartialEq, Eq)]
pub struct Pending {
    pub level: Level,
    pub changelog: String,
    pub adrs: Vec<Adr>,
    /// Greenfield reference rule ids (`R2.3`) this increment closes (#1001).
    /// Optional and usually empty — most increments don't close a tracked rule.
    /// Syntax-checked here (each entry matches `R<major>.<minor>`); whether the
    /// id actually exists in `design/bynk-greenfield-compiler.md` is checked
    /// separately by [`known_rule_ids`], which needs the repo root this pure
    /// parse doesn't have.
    pub closes_rule: Vec<String>,
    /// Gated greenfield-status probes this increment moves (see
    /// `design/pending/README.md`). Acknowledging a probe here is what lets the
    /// increment change its reading *without* editing the shared
    /// `design/greenfield-status.md` table: the gate accepts an acknowledged
    /// disagreement, and the stamp rewrites the table's gated rows on `main` in
    /// merge order. Optional and usually empty. Each entry is checked against
    /// [`greenfield_status::GATED_PROBES`].
    pub probes: Vec<String>,
}

/// The repo root, resolved from this crate's manifest dir so it's independent
/// of the working directory (the same trick `decisions_index` uses).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// `design/pending/` under [`repo_root`].
pub fn pending_dir() -> PathBuf {
    repo_root().join("design/pending")
}

/// Validate every `*.md` under [`pending_dir`] except `README.md` (the format
/// doc, excluded like the decisions index excludes its own README). Returns the
/// number of pending files validated, or every error found across all files
/// (each prefixed with its filename) so one run reports the whole picture.
pub fn check_all() -> Result<usize, Vec<String>> {
    validated_pending_in(&repo_root()).map(|ps| ps.len())
}

/// Read and validate the pending files under `root/design/pending` (skipping
/// `README.md`), sorted by filename. Root-parameterised so the stamp — and its
/// fixture tests — can target any tree; [`check_all`] is this over the real
/// [`repo_root`].
///
/// Also cross-checks each file's `closes_rule` entries against
/// [`known_rule_ids`] — a syntactically valid id (`is_rule_id`, checked in
/// [`validate`]) that names no rule the reference actually has is still an
/// error, just one that needs `root` to catch, which the pure per-file parse
/// doesn't have.
pub fn validated_pending_in(root: &Path) -> Result<Vec<(String, Pending)>, Vec<String>> {
    let dir = root.join("design/pending");
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(err) => return Err(vec![format!("cannot read {}: {err}", dir.display())]),
    };

    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md") && n != "README.md")
        .collect();
    names.sort();

    let mut parsed = Vec::new();
    let mut errors = Vec::new();
    for name in names {
        let content = match fs::read_to_string(dir.join(&name)) {
            Ok(c) => c,
            Err(err) => {
                errors.push(format!("{name}: cannot read: {err}"));
                continue;
            }
        };
        match validate(&name, &content) {
            Ok(p) => parsed.push((name, p)),
            Err(errs) => errors.extend(errs.into_iter().map(|e| format!("{name}: {e}"))),
        }
    }

    if !parsed.iter().any(|(_, p)| !p.closes_rule.is_empty()) {
        // No file cites a rule — skip reading the (large) reference doc at all.
    } else {
        match known_rule_ids(root) {
            Ok(known) => {
                for (name, pending) in &parsed {
                    for rule in &pending.closes_rule {
                        if !known.contains(rule) {
                            errors.push(format!(
                                "{name}: closes_rule cites {rule:?}, which is not a rule id in \
                                 design/bynk-greenfield-compiler.md"
                            ));
                        }
                    }
                }
            }
            Err(e) => errors.push(format!(
                "cannot validate closes_rule entries against the reference: {e}"
            )),
        }
    }

    if errors.is_empty() {
        Ok(parsed)
    } else {
        Err(errors)
    }
}

/// Validate a single pending file's `content`. `filename` is used to check the
/// stem is a kebab-case slug. Returns every problem found (not just the first),
/// so a malformed file reports completely.
pub fn validate(filename: &str, content: &str) -> Result<Pending, Vec<String>> {
    let mut errors = Vec::new();

    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !is_kebab(&stem) {
        errors.push(format!(
            "filename stem {stem:?} is not a kebab-case slug (a-z, 0-9, single hyphens)"
        ));
    }

    let (level, changelog, closes_rule, probes) = match parse_frontmatter(content, &mut errors) {
        Some(fm) => fm,
        None => return Err(errors),
    };
    let adrs = parse_adrs(content, &mut errors);

    if errors.is_empty() {
        Ok(Pending {
            level: level.expect("no errors implies a level"),
            changelog: changelog.expect("no errors implies a changelog"),
            adrs,
            closes_rule,
            probes,
        })
    } else {
        Err(errors)
    }
}

/// The header fields [`parse_frontmatter`] recovers: `level`, `changelog`,
/// `closes_rule`, `probes`.
type Frontmatter = (Option<Level>, Option<String>, Vec<String>, Vec<String>);

/// Parse and validate the `---`-delimited header. Pushes errors; returns the
/// fields when present and well-formed (`closes_rule` and `probes` default to
/// empty rather than `Option` — they're genuinely optional, unlike `level`/`changelog`). Returns
/// `None` only when the frontmatter block itself is missing/unterminated
/// (nothing to recover).
fn parse_frontmatter(content: &str, errors: &mut Vec<String>) -> Option<Frontmatter> {
    let mut lines = content.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        errors.push("must open with a `---` frontmatter fence on line 1".into());
        return None;
    }

    let mut header = Vec::new();
    let mut closed = false;
    for line in lines {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        header.push(line);
    }
    if !closed {
        errors.push("frontmatter is not closed with a `---` fence".into());
        return None;
    }

    let mut level = None;
    let mut changelog = None;
    let mut closes_rule = Vec::new();
    let mut probes = Vec::new();
    // Track key presence separately from a valid value: a key that is present
    // but malformed reports its own error and must not also be "missing".
    let mut saw_level = false;
    let mut saw_changelog = false;
    let mut saw_closes_rule = false;
    let mut saw_probes = false;
    for raw in header {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            errors.push(format!("frontmatter line is not `key: value`: {raw:?}"));
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "level" => {
                if saw_level {
                    errors.push("duplicate frontmatter key `level`".into());
                }
                saw_level = true;
                level = match value {
                    "minor" => Some(Level::Minor),
                    "patch" => Some(Level::Patch),
                    other => {
                        errors.push(format!("level must be `minor` or `patch`, got {other:?}"));
                        None
                    }
                };
            }
            "changelog" => {
                if saw_changelog {
                    errors.push("duplicate frontmatter key `changelog`".into());
                }
                saw_changelog = true;
                if value.is_empty() {
                    errors.push("changelog must not be empty".into());
                } else if looks_like_version_prefix(value) {
                    errors.push(format!(
                        "changelog must not start with a version number (the stamp adds it): {value:?}"
                    ));
                } else if let Some(dest) = relative_markdown_link_in(value) {
                    errors.push(format!(
                        "changelog reads as a Markdown link to the relative destination {dest:?} \
                         — the blurb is inserted verbatim into the Book's changelog table, so a \
                         bare `x[T](y)` in prose becomes a link the docs site's link-checker \
                         rejects (and it only sees the row after the stamp writes it on `main`). \
                         Wrap the code in backticks, or use an absolute URL."
                    ));
                } else {
                    changelog = Some(value.to_string());
                }
            }
            "closes_rule" => {
                if saw_closes_rule {
                    errors.push("duplicate frontmatter key `closes_rule`".into());
                }
                saw_closes_rule = true;
                if value.is_empty() {
                    errors.push(
                        "closes_rule must not be empty (omit the key entirely if there's \
                         nothing to cite)"
                            .into(),
                    );
                } else {
                    for entry in value.split(',') {
                        let entry = entry.trim();
                        if is_rule_id(entry) {
                            closes_rule.push(entry.to_string());
                        } else {
                            errors.push(format!(
                                "closes_rule entry {entry:?} is not a rule id \
                                 (expected `R<major>.<minor>`, e.g. `R2.3`)"
                            ));
                        }
                    }
                }
            }
            "probes" => {
                if saw_probes {
                    errors.push("duplicate frontmatter key `probes`".into());
                }
                saw_probes = true;
                if value.is_empty() {
                    errors.push(
                        "probes must not be empty (omit the key entirely if no gated probe \
                         moves)"
                            .into(),
                    );
                } else {
                    for entry in value.split(',') {
                        let entry = entry.trim().trim_matches('`');
                        if greenfield_status::GATED_PROBES.contains(&entry) {
                            if !probes.iter().any(|p| p == entry) {
                                probes.push(entry.to_string());
                            }
                        } else {
                            errors.push(format!(
                                "probes entry {entry:?} is not a gated greenfield-status probe \
                                 (one of: {})",
                                greenfield_status::GATED_PROBES.join(", ")
                            ));
                        }
                    }
                }
            }
            other => errors.push(format!("unknown frontmatter key {other:?}")),
        }
    }

    if !saw_level {
        errors.push("frontmatter is missing `level`".into());
    }
    if !saw_changelog {
        errors.push("frontmatter is missing `changelog`".into());
    }

    Some((level, changelog, closes_rule, probes))
}

/// The gated probes acknowledged by every pending file under `root` — the union
/// of their `probes` keys. The pending files present in a tree are exactly the
/// increments not yet stamped (the stamp deletes each one as it rewrites the
/// table), so on a PR's merge ref this covers both the PR's own acknowledgement
/// and any just-merged increment whose stamp hasn't landed yet. An unreadable or
/// invalid pending directory acknowledges nothing; `pending_files` reports why.
pub fn acknowledged_probes(root: &Path) -> std::collections::BTreeSet<String> {
    validated_pending_in(root)
        .map(|ps| ps.into_iter().flat_map(|(_, p)| p.probes).collect())
        .unwrap_or_default()
}

/// Is `s` shaped like a greenfield-reference rule id — `R` followed by
/// `<digits>.<digits>` (e.g. `R2.3`, `R0.1`)? Syntax only; whether the id
/// actually exists in the reference is [`known_rule_ids`]'s job.
pub fn is_rule_id(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('R') else {
        return false;
    };
    let Some((major, minor)) = rest.split_once('.') else {
        return false;
    };
    !major.is_empty()
        && major.chars().all(|c| c.is_ascii_digit())
        && !minor.is_empty()
        && minor.chars().all(|c| c.is_ascii_digit())
}

/// Every rule id (`R2.3`, …) enumerated in the greenfield reference
/// (`design/bynk-greenfield-compiler.md`) — the existence check for a pending
/// file's `closes_rule` entries, separate from [`is_rule_id`]'s pure syntax
/// check because it needs the repo root. Root-parameterised like
/// `stamp::next_adr_number`, so a fixture tree can supply its own reference doc.
///
/// Rules are written inline as `**R2.3 — <title>.**`; this scans every `**R`
/// occurrence for the dotted id immediately following, rather than requiring a
/// line-start anchor — the doc's own precedent (`grep -oE '\*\*R[0-9]+\.[0-9]+
/// —'`) confirmed this finds exactly the 130 rules the reference claims.
pub fn known_rule_ids(root: &Path) -> Result<std::collections::HashSet<String>, String> {
    let path = root.join("design/bynk-greenfield-compiler.md");
    let text =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut ids = std::collections::HashSet::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(rel) = text[i..].find("**R") {
        let start = i + rel + 2; // skip `**`, keep the leading `R`
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'.') {
            end += 1;
        }
        let candidate = &text[start..end];
        if is_rule_id(candidate) {
            ids.insert(candidate.to_string());
        }
        i = end.max(start + 1);
    }
    Ok(ids)
}

/// The PR number a commit `subject` (the first line of its message) names, if
/// it ends in the `(#NNNN)` GitHub's squash-merge appends to the PR title —
/// pure and root-independent, so `xtask/src/main.rs`'s `pr_number_from_head`
/// (the only caller, which runs `git log -1 --format=%s` to get `subject`)
/// stays a thin wrapper over this, and this half — the part with the
/// interesting edge cases — is unit-testable without a git tree (#1001,
/// caught by review as the one piece of new logic with no test coverage).
///
/// **Known limitation, stated rather than guarded against:** this matches on
/// shape alone. Any subject ending `(#NNNN)` is read as the merging PR,
/// including a hand-written commit that happens to end in an *issue*
/// reference (`"fix: handle empty spans (#1001)"`, where 1001 names an issue,
/// not the PR that closes it) — correct on the squash-merge path this exists
/// for, a silent false positive off it (a hand-run `stamp --apply` on an
/// unmerged local commit, say). Not guarded against because the fix — only
/// trust the parse when the run is known to be CI/merge-triggered — would
/// also suppress the *legitimate* case of manually re-running `stamp --apply`
/// against an already-merged commit to recover from a failed push (`stamp.yml`
/// names this as the documented recovery path), which is a worse trade.
pub fn pr_number_from_subject(subject: &str) -> Option<u32> {
    let inner = subject.strip_suffix(')')?.rsplit_once("(#")?.1;
    inner.parse().ok()
}

/// Parse `## ADR: <slug>` blocks from the body (everything after the closing
/// frontmatter fence). Zero blocks is valid — an increment may record no
/// decision. Pushes errors for a non-kebab or duplicate slug, or an empty body.
fn parse_adrs(content: &str, errors: &mut Vec<String>) -> Vec<Adr> {
    // Body starts after the second `---` fence.
    let mut fences = 0;
    let mut body_lines = Vec::new();
    for line in content.lines() {
        if fences < 2 {
            if line.trim_end() == "---" {
                fences += 1;
            }
            continue;
        }
        body_lines.push(line);
    }

    // A `## ADR:` line inside a ``` code fence is prose (e.g. a pending file
    // documenting the format inline), not a block header. Mark each line's
    // header-ness up front, toggling on backtick-fence delimiters, so both the
    // outer scan and the body-collecting loop below agree on where blocks start.
    let mut in_fence = false;
    let is_header: Vec<bool> = body_lines
        .iter()
        .map(|line| {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                false
            } else {
                !in_fence && adr_header_slug(line).is_some()
            }
        })
        .collect();

    let mut adrs: Vec<Adr> = Vec::new();
    let mut i = 0;
    while i < body_lines.len() {
        if is_header[i] {
            let slug = adr_header_slug(body_lines[i])
                .expect("is_header implies an ADR header")
                .trim()
                .to_string();
            i += 1;
            let mut block = Vec::new();
            while i < body_lines.len() && !is_header[i] {
                block.push(body_lines[i]);
                i += 1;
            }

            // The block opens with `title:`/`summary:`/`status:` key lines (any
            // order, `title` required), then a blank line, then the verbatim
            // body. Consume leading blanks and known keys; the first other line
            // starts the body.
            let mut title = None;
            let mut summary = None;
            let mut status = None;
            let mut body_start = block.len();
            for (idx, raw) in block.iter().enumerate() {
                let line = raw.trim();
                if line.is_empty() && title.is_none() && summary.is_none() && status.is_none() {
                    continue;
                }
                if let Some(v) = line.strip_prefix("title:") {
                    title = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("summary:") {
                    summary = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("status:") {
                    status = Some(v.trim().to_string());
                } else {
                    body_start = idx;
                    break;
                }
            }
            let body = block[body_start..].join("\n").trim().to_string();

            if !is_kebab(&slug) {
                errors.push(format!(
                    "ADR slug {slug:?} is not a kebab-case slug (a-z, 0-9, single hyphens)"
                ));
            } else if adrs.iter().any(|a| a.slug == slug) {
                errors.push(format!("duplicate ADR slug {slug:?}"));
            }
            match &title {
                Some(t) if t.is_empty() => {
                    errors.push(format!("ADR {slug:?} has an empty `title:`"))
                }
                None => errors.push(format!("ADR {slug:?} is missing a `title:` line")),
                _ => {}
            }
            if body.is_empty() {
                errors.push(format!("ADR {slug:?} has an empty body"));
            }
            adrs.push(Adr {
                slug,
                title: title.unwrap_or_default(),
                summary: summary.filter(|s| !s.is_empty()),
                status: status.filter(|s| !s.is_empty()),
                body,
            });
        } else {
            i += 1;
        }
    }
    adrs
}

/// The slug text of a `## ADR: <slug>` header line, if this line is one.
fn adr_header_slug(line: &str) -> Option<&str> {
    line.trim().strip_prefix("## ADR:")
}

/// A kebab-case slug: non-empty, `a-z0-9` and single interior hyphens only.
fn is_kebab(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether the changelog's first token reads as a repo version the author has
/// accidentally prefixed — the stamp prepends the number, so the blurb must not
/// carry one. Matched to the repo's actual spellings: a `v` prefix (`v0.186`,
/// the banner form) or three-plus numeric groups (`0.186.0`, the Cargo form).
/// A bare two-group token like `3.0` is *not* a version here, so a blurb such as
/// "3.0 rendering pipeline added" is allowed.
fn looks_like_version_prefix(changelog: &str) -> bool {
    let raw = changelog.split_whitespace().next().unwrap_or("");
    let had_v = raw.starts_with('v') || raw.starts_with('V');
    let groups: Vec<&str> = raw.trim_start_matches(['v', 'V']).split('.').collect();
    let all_numeric = groups.len() >= 2
        && groups
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    all_numeric && (had_v || groups.len() >= 3)
}

/// The destination of the first Markdown inline link in `blurb` that resolves
/// *relatively*, if there is one.
///
/// The blurb is inserted verbatim into a table row in the Book's
/// `reference/changelog.md`, so `[…](…)` in prose is a real link once it lands —
/// and an unbackticked generic call like `Events.emit[E](event)` is exactly that
/// shape, a link to a relative `event`. The docs site's own link-checker
/// (starlight-links-validator, the `Docs site (astro build)` gate) rejects those,
/// but it only ever sees the row *after* the stamp writes it on `main`, so that
/// gate fires post-merge with nothing left to review. This is the pre-merge
/// stand-in, run against the pending file the PR actually carries.
///
/// Deliberate links stay allowed — absolute (`https://…`, the `[#548](…)` issue
/// citations the corpus is full of), site-root (`/book/…`), and anchors. Code
/// spans are skipped, which is also the fix the error message asks for: write
/// ``` `Events.emit[E](event)` ```.
fn relative_markdown_link_in(blurb: &str) -> Option<String> {
    let bytes = blurb.as_bytes();
    let mut i = 0;
    let mut saw_open_bracket = false;
    while i < bytes.len() {
        match bytes[i] {
            // A backslash escape covers the next byte, whatever it is.
            b'\\' => i += 1,
            // Skip a code span: a run of N backticks closes on the next run of
            // exactly N. An *unmatched* run is literal text in CommonMark and
            // the rest of the blurb still parses as Markdown, so resume just
            // after it rather than swallowing the remainder — otherwise a
            // half-applied fix (`` `Events.emit[E](event) ``, opening backtick
            // only) would sail through this check and break `main` anyway.
            b'`' => {
                let fence = bytes[i..].iter().take_while(|&&b| b == b'`').count();
                let mut j = i + fence;
                let mut closed = false;
                while j < bytes.len() {
                    if bytes[j] == b'`' {
                        let run = bytes[j..].iter().take_while(|&&b| b == b'`').count();
                        j += run;
                        if run == fence {
                            closed = true;
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                i = if closed { j } else { i + fence };
                continue;
            }
            b'[' => saw_open_bracket = true,
            // `](` is only a link if some `[` opened it.
            b']' if saw_open_bracket && bytes.get(i + 1) == Some(&b'(') => {
                saw_open_bracket = false;
                let start = i + 2;
                let Some(len) = bytes[start..].iter().position(|&b| b == b')') else {
                    break;
                };
                if let Some(dest) = link_destination(&blurb[start..start + len])
                    && !is_absolute_link_destination(dest)
                {
                    return Some(dest.to_string());
                }
                i = start + len;
            }
            b']' => saw_open_bracket = false,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The destination inside a `](…)` link tail, or `None` when the tail is not one.
///
/// CommonMark accepts `(dest)` and `(dest "title")` (also `'…'` and `(…)` titles),
/// and a bare destination may not contain spaces unless it is `<…>`-wrapped. So a
/// multi-parameter generic call in prose — `map[K, V](key, value)` — is genuinely
/// not a link, and reporting one would assert something the site's own checker
/// would never do. Backticking it is still the house convention; this check just
/// does not claim it is a broken link.
fn link_destination(tail: &str) -> Option<&str> {
    let tail = tail.trim();
    if let Some(rest) = tail.strip_prefix('<') {
        return rest.split_once('>').map(|(dest, _)| dest);
    }
    let (dest, rest) = match tail.split_once(char::is_whitespace) {
        Some((dest, rest)) => (dest, rest.trim_start()),
        None => (tail, ""),
    };
    // Whatever follows the destination has to be a title, or this is just prose.
    if rest.is_empty() || rest.starts_with(['"', '\'', '(']) {
        Some(dest)
    } else {
        None
    }
}

/// Whether a Markdown link destination resolves without depending on the page it
/// is written on — the only kind a changelog blurb may carry.
///
/// Note this is about *shape*, not existence: the site's link-checker also
/// resolves site-root paths and anchors, which nothing here can do without the
/// built site. A hand-written `/book/typo/` still passes this and fails the docs
/// build. `design/pending/README.md` says so rather than implying a guarantee.
fn is_absolute_link_destination(dest: &str) -> bool {
    dest.starts_with("https://")
        || dest.starts_with("http://")
        || dest.starts_with("mailto:")
        || dest.starts_with('/')
        || dest.starts_with('#')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(name: &str, content: &str) -> Pending {
        validate(name, content).unwrap_or_else(|e| panic!("expected valid, got {e:?}"))
    }
    fn err(name: &str, content: &str) -> Vec<String> {
        validate(name, content).expect_err("expected invalid")
    }

    #[test]
    fn minimal_no_adr_is_valid() {
        let p = ok(
            "add-a-thing.md",
            "---\nlevel: minor\nchangelog: Add a thing to the language\n---\n",
        );
        assert_eq!(p.level, Level::Minor);
        assert_eq!(p.changelog, "Add a thing to the language");
        assert!(p.adrs.is_empty());
    }

    #[test]
    fn patch_level_is_valid() {
        assert_eq!(
            ok(
                "fix-a-thing.md",
                "---\nlevel: patch\nchangelog: Fix a non-language thing\n---\n"
            )
            .level,
            Level::Patch
        );
    }

    #[test]
    fn one_adr_parses_slug_title_and_body() {
        let p = ok(
            "unit-tier.md",
            "---\nlevel: minor\nchangelog: Drive a handler at the unit tier\n---\n\n## ADR: unit-tier-service-address\ntitle: A case addresses a handler by surface\n\n**Decision.** A case addresses by surface.\n",
        );
        assert_eq!(p.adrs.len(), 1);
        let adr = &p.adrs[0];
        assert_eq!(adr.slug, "unit-tier-service-address");
        assert_eq!(adr.title, "A case addresses a handler by surface");
        // summary/status default to title/"Accepted" when absent.
        assert_eq!(adr.summary(), adr.title);
        assert_eq!(adr.status(), "Accepted");
        assert!(adr.body.contains("addresses by surface"));
        assert!(!adr.body.contains("title:"), "the title line is not body");
    }

    #[test]
    fn adr_summary_and_status_are_parsed() {
        let p = ok(
            "x.md",
            "---\nlevel: minor\nchangelog: x\n---\n\n## ADR: a-slug\ntitle: The title\nsummary: The one-line index distillation\nstatus: Proposed\n\nBody.\n",
        );
        let adr = &p.adrs[0];
        assert_eq!(adr.summary(), "The one-line index distillation");
        assert_eq!(adr.status(), "Proposed");
    }

    #[test]
    fn adr_missing_title_rejected() {
        assert!(
            err(
                "x.md",
                "---\nlevel: minor\nchangelog: x\n---\n\n## ADR: a-slug\n\nBody with no title line.\n"
            )
            .iter()
            .any(|e| e.contains("missing a `title:`"))
        );
    }

    #[test]
    fn two_adrs_parse() {
        let p = ok(
            "two.md",
            "---\nlevel: minor\nchangelog: Two decisions\n---\n\n## ADR: first-one\ntitle: First\n\nBody one.\n\n## ADR: second-one\ntitle: Second\n\nBody two.\n",
        );
        assert_eq!(p.adrs.len(), 2);
        assert_eq!(p.adrs[0].slug, "first-one");
        assert_eq!(p.adrs[1].slug, "second-one");
    }

    #[test]
    fn bad_level_rejected() {
        assert!(
            err("x.md", "---\nlevel: major\nchangelog: x\n---\n")
                .iter()
                .any(|e| e.contains("level must be"))
        );
    }

    #[test]
    fn missing_level_rejected() {
        assert!(
            err("x.md", "---\nchangelog: x\n---\n")
                .iter()
                .any(|e| e.contains("missing `level`"))
        );
    }

    #[test]
    fn missing_changelog_rejected() {
        assert!(
            err("x.md", "---\nlevel: minor\n---\n")
                .iter()
                .any(|e| e.contains("missing `changelog`"))
        );
    }

    #[test]
    fn empty_changelog_rejected() {
        assert!(
            err("x.md", "---\nlevel: minor\nchangelog:   \n---\n")
                .iter()
                .any(|e| e.contains("changelog"))
        );
    }

    #[test]
    fn version_prefixed_changelog_rejected() {
        for cl in ["v0.186 Add a thing", "0.186.0 Add a thing"] {
            let content = format!("---\nlevel: minor\nchangelog: {cl}\n---\n");
            assert!(
                err("x.md", &content)
                    .iter()
                    .any(|e| e.contains("version number")),
                "expected rejection for {cl:?}"
            );
        }
    }

    #[test]
    fn plain_changelog_with_a_dot_is_allowed() {
        // A blurb ending in a version-like word must not false-positive; only the
        // *first* token is checked.
        ok(
            "x.md",
            "---\nlevel: minor\nchangelog: Support semver ranges like 1.2.3\n---\n",
        );
    }

    #[test]
    fn bare_two_group_leading_number_is_allowed() {
        // `3.0` is not a repo version (no `v`, only two groups) — a blurb may
        // legitimately open with it.
        ok(
            "x.md",
            "---\nlevel: minor\nchangelog: 3.0 rendering pipeline added\n---\n",
        );
    }

    #[test]
    fn accidental_relative_markdown_link_in_changelog_rejected() {
        // The real regression (v0.249.1): an unbackticked generic call in prose
        // parses as `[E](event)`, a link to a relative `event`, and failed the
        // docs site's link-checker on `main` — post-merge, after the stamp had
        // already written the row.
        let content = "---\nlevel: patch\nchangelog: Read Callee to detect an \
                       Events.emit[E](event) call\n---\n";
        assert!(
            err("x.md", content)
                .iter()
                .any(|e| e.contains("relative destination \"event\"")),
            "expected rejection, got {:?}",
            err("x.md", content)
        );
    }

    #[test]
    fn backticked_generic_call_in_changelog_is_allowed() {
        // The documented fix, and the corpus's own convention for code in prose.
        ok(
            "x.md",
            "---\nlevel: patch\nchangelog: Detect an `Events.emit[E](event)` call\n---\n",
        );
    }

    #[test]
    fn absolute_links_in_changelog_are_allowed() {
        // Issue citations are the single most common blurb decoration; site-root
        // links and anchors resolve independently of the page too.
        for cl in [
            "Close [#548](https://github.com/accuser/bynk/issues/548)",
            "See [the roadmap](/book/about/versioning-and-roadmap/)",
            "See [below](#notes)",
        ] {
            let content = format!("---\nlevel: minor\nchangelog: {cl}\n---\n");
            ok("x.md", &content);
        }
    }

    #[test]
    fn bracket_without_a_link_in_changelog_is_allowed() {
        // `]` with no `(` after it, and `(` with no `[` before it, are ordinary
        // punctuation — neither forms a link.
        ok(
            "x.md",
            "---\nlevel: minor\nchangelog: Widen ts_any [see the probe] (P7.0) for writes\n---\n",
        );
    }

    #[test]
    fn a_half_backticked_changelog_is_still_rejected() {
        // The likeliest mistake after reading the error message: the author adds
        // the opening backtick and forgets the closing one. CommonMark treats the
        // unmatched run as literal text and still links `[E](event)` (confirmed
        // against the site's own parser), so the guard must not stop scanning.
        let content = "---\nlevel: patch\nchangelog: Detect an `Events.emit[E](event) call\n---\n";
        assert!(
            err("x.md", content)
                .iter()
                .any(|e| e.contains("relative destination \"event\"")),
            "expected rejection, got {:?}",
            err("x.md", content)
        );
    }

    #[test]
    fn a_stray_backtick_does_not_blind_the_rest_of_the_blurb() {
        // Same failure mode from an unrelated typo earlier in the sentence.
        let content = "---\nlevel: patch\nchangelog: A stray `Callee typo, then \
                       Events.emit[E](event) later\n---\n";
        assert!(
            err("x.md", content)
                .iter()
                .any(|e| e.contains("relative destination \"event\"")),
            "expected rejection, got {:?}",
            err("x.md", content)
        );
    }

    #[test]
    fn a_multi_parameter_generic_call_is_not_reported_as_a_link() {
        // `(key, value)` holds a space, so CommonMark does not link it — claiming
        // otherwise would assert a failure the site checker would never produce.
        ok(
            "x.md",
            "---\nlevel: minor\nchangelog: Lift a fn map[K, V](key, value) call\n---\n",
        );
    }

    #[test]
    fn a_titled_link_is_still_read_as_a_link() {
        // `(dest "title")` is a real link, so the destination is still checked
        // even though a space follows it.
        let content = "---\nlevel: minor\nchangelog: See [it](there \"a title\")\n---\n";
        assert!(
            err("x.md", content)
                .iter()
                .any(|e| e.contains("relative destination \"there\"")),
            "expected rejection, got {:?}",
            err("x.md", content)
        );
    }

    #[test]
    fn duplicate_frontmatter_key_rejected() {
        assert!(
            err(
                "x.md",
                "---\nlevel: minor\nlevel: patch\nchangelog: x\n---\n"
            )
            .iter()
            .any(|e| e.contains("duplicate frontmatter key `level`"))
        );
    }

    #[test]
    fn adr_header_inside_a_code_fence_is_not_a_block() {
        // A pending file documenting the format inline must not have its fenced
        // `## ADR:` example split off into a spurious block.
        let p = ok(
            "x.md",
            "---\nlevel: minor\nchangelog: Document the format\n---\n\n\
             Example:\n\n```markdown\n## ADR: not-a-real-block\nfenced prose\n```\n\n\
             ## ADR: the-real-one\ntitle: The real one\n\nReal body.\n",
        );
        assert_eq!(p.adrs.len(), 1);
        assert_eq!(p.adrs[0].slug, "the-real-one");
    }

    #[test]
    fn no_frontmatter_rejected() {
        assert!(
            err("x.md", "just some text\n")
                .iter()
                .any(|e| e.contains("open with a `---`"))
        );
    }

    #[test]
    fn unclosed_frontmatter_rejected() {
        assert!(
            err("x.md", "---\nlevel: minor\nchangelog: x\n")
                .iter()
                .any(|e| e.contains("not closed"))
        );
    }

    #[test]
    fn unknown_key_rejected() {
        assert!(
            err("x.md", "---\nlevel: minor\nchangelog: x\nversion: 9\n---\n")
                .iter()
                .any(|e| e.contains("unknown frontmatter key"))
        );
    }

    // --- probes --------------------------------------------------------------

    #[test]
    fn probes_is_optional_and_defaults_empty() {
        let p = ok("x.md", "---\nlevel: patch\nchangelog: x\n---\n");
        assert!(p.probes.is_empty());
    }

    #[test]
    fn probes_parses_a_list_trims_backticks_and_dedupes() {
        let p = ok(
            "x.md",
            "---\nlevel: patch\nchangelog: x\nprobes: `ts_writes`, diagnostic_coverage ,ts_writes\n---\n",
        );
        assert_eq!(
            p.probes,
            vec!["ts_writes".to_string(), "diagnostic_coverage".to_string()]
        );
    }

    #[test]
    fn probes_rejects_a_trend_or_unknown_probe() {
        let errs = err(
            "x.md",
            "---\nlevel: patch\nchangelog: x\nprobes: wildcard_arms, nope\n---\n",
        );
        assert_eq!(
            errs.iter()
                .filter(|e| e.contains("is not a gated greenfield-status probe"))
                .count(),
            2
        );
    }

    #[test]
    fn probes_rejects_empty_and_duplicate_keys() {
        assert!(
            err("x.md", "---\nlevel: patch\nchangelog: x\nprobes:\n---\n")
                .iter()
                .any(|e| e.contains("probes must not be empty"))
        );
        assert!(
            err(
                "x.md",
                "---\nlevel: patch\nchangelog: x\nprobes: ts_any\nprobes: ts_any\n---\n"
            )
            .iter()
            .any(|e| e.contains("duplicate frontmatter key `probes`"))
        );
    }

    // --- closes_rule (#1001) --------------------------------------------------

    #[test]
    fn closes_rule_is_optional_and_defaults_empty() {
        let p = ok("x.md", "---\nlevel: patch\nchangelog: x\n---\n");
        assert!(p.closes_rule.is_empty());
    }

    #[test]
    fn closes_rule_parses_a_single_id() {
        let p = ok(
            "x.md",
            "---\nlevel: patch\nchangelog: x\ncloses_rule: R2.3\n---\n",
        );
        assert_eq!(p.closes_rule, vec!["R2.3".to_string()]);
    }

    #[test]
    fn closes_rule_parses_a_comma_separated_list_and_trims_whitespace() {
        let p = ok(
            "x.md",
            "---\nlevel: patch\nchangelog: x\ncloses_rule: R2.3,  R2.12 ,R0.1\n---\n",
        );
        assert_eq!(
            p.closes_rule,
            vec!["R2.3".to_string(), "R2.12".to_string(), "R0.1".to_string()]
        );
    }

    #[test]
    fn closes_rule_rejects_a_malformed_entry() {
        assert!(
            err(
                "x.md",
                "---\nlevel: patch\nchangelog: x\ncloses_rule: not-a-rule\n---\n"
            )
            .iter()
            .any(|e| e.contains("closes_rule entry") && e.contains("not a rule id"))
        );
    }

    #[test]
    fn closes_rule_rejects_empty_value() {
        assert!(
            err(
                "x.md",
                "---\nlevel: patch\nchangelog: x\ncloses_rule: \n---\n"
            )
            .iter()
            .any(|e| e.contains("closes_rule must not be empty"))
        );
    }

    #[test]
    fn closes_rule_rejects_duplicate_key() {
        assert!(
            err(
                "x.md",
                "---\nlevel: patch\nchangelog: x\ncloses_rule: R2.3\ncloses_rule: R2.4\n---\n"
            )
            .iter()
            .any(|e| e.contains("duplicate frontmatter key `closes_rule`"))
        );
    }

    #[test]
    fn is_rule_id_accepts_and_rejects() {
        assert!(is_rule_id("R2.3"));
        assert!(is_rule_id("R0.1"));
        assert!(is_rule_id("R12.34"));
        assert!(!is_rule_id("2.3"));
        assert!(!is_rule_id("R2"));
        assert!(!is_rule_id("R2.3.4"));
        assert!(!is_rule_id("R.3"));
        assert!(!is_rule_id("R2."));
        assert!(!is_rule_id("Rx.y"));
    }

    // --- pr_number_from_subject (#1001) --------------------------------------

    #[test]
    fn pr_number_from_subject_finds_a_trailing_squash_merge_suffix() {
        assert_eq!(
            pr_number_from_subject("feat(xtask): thing (#1234)"),
            Some(1234)
        );
    }

    #[test]
    fn pr_number_from_subject_requires_the_suffix_at_the_very_end() {
        // Trailing prose after the `)` means this isn't a squash-merge title.
        assert_eq!(pr_number_from_subject("feat: thing (#12) then more"), None);
    }

    #[test]
    fn pr_number_from_subject_is_not_confused_by_earlier_nested_parens() {
        assert_eq!(pr_number_from_subject("chore: bump (deps) (#12)"), Some(12));
    }

    #[test]
    fn pr_number_from_subject_rejects_a_non_numeric_hash() {
        assert_eq!(pr_number_from_subject("feat: thing (#abc)"), None);
    }

    #[test]
    fn pr_number_from_subject_rejects_a_number_too_large_for_u32() {
        assert_eq!(pr_number_from_subject("(#99999999999999)"), None);
    }

    #[test]
    fn pr_number_from_subject_none_without_any_suffix() {
        assert_eq!(pr_number_from_subject("Merge branch 'x'"), None);
    }

    /// The documented limitation, pinned so it can't silently change meaning:
    /// a hand-written commit ending in an *issue* reference is indistinguishable
    /// from a squash-merge PR title by shape alone.
    #[test]
    fn pr_number_from_subject_cannot_distinguish_an_issue_reference() {
        assert_eq!(
            pr_number_from_subject("fix: handle empty spans (#1001)"),
            Some(1001)
        );
    }

    /// A throwaway fixture tree, named per calling test so parallel runs don't
    /// collide — the same convention `xtask/tests/stamp_apply.rs`'s `fixture`
    /// uses. Removed and recreated on construction, not cleaned up after (the OS
    /// temp dir is not this test's to manage beyond that).
    fn rule_fixture(tag: &str, reference_body: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("xtask-closes-rule-{tag}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("design/pending")).unwrap();
        fs::write(
            root.join("design/bynk-greenfield-compiler.md"),
            reference_body,
        )
        .unwrap();
        root
    }

    #[test]
    fn acknowledged_probes_is_the_union_across_pending_files() {
        let dir = rule_fixture("ack-union", "");
        let pending = dir.join("design/pending");
        fs::write(
            pending.join("a.md"),
            "---\nlevel: patch\nchangelog: a\nprobes: ts_writes, ts_any\n---\n",
        )
        .unwrap();
        fs::write(
            pending.join("b.md"),
            "---\nlevel: patch\nchangelog: b\nprobes: ts_any, diagnostic_coverage\n---\n",
        )
        .unwrap();
        fs::write(
            pending.join("c.md"),
            "---\nlevel: patch\nchangelog: c\n---\n",
        )
        .unwrap();
        fs::write(pending.join("README.md"), "not a pending file").unwrap();
        let ack: Vec<String> = acknowledged_probes(&dir).into_iter().collect();
        assert_eq!(ack, ["diagnostic_coverage", "ts_any", "ts_writes"]);
    }

    #[test]
    fn acknowledged_probes_is_empty_when_any_pending_file_is_invalid() {
        let dir = rule_fixture("ack-invalid", "");
        let pending = dir.join("design/pending");
        fs::write(
            pending.join("a.md"),
            "---\nlevel: patch\nchangelog: a\nprobes: ts_writes\n---\n",
        )
        .unwrap();
        fs::write(
            pending.join("b.md"),
            "---\nlevel: nope\nchangelog: b\n---\n",
        )
        .unwrap();
        assert!(acknowledged_probes(&dir).is_empty());
    }

    #[test]
    fn acknowledged_probes_is_empty_without_a_pending_dir() {
        let dir = std::env::temp_dir().join("xtask-ack-no-pending-dir");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(acknowledged_probes(&dir).is_empty());
    }

    #[test]
    fn known_rule_ids_finds_bold_rule_headers() {
        let dir = rule_fixture(
            "finds-bold-headers",
            "Some prose.\n\n**R2.3 — A rule about spans.**\n\nMore prose citing **R2.3** again \
             in passing, and introducing **R10.11 — a second rule.**\n",
        );
        let ids = known_rule_ids(&dir).unwrap();
        assert_eq!(ids.len(), 2, "expected exactly 2 distinct ids: {ids:?}");
        assert!(ids.contains("R2.3"));
        assert!(ids.contains("R10.11"));
    }

    #[test]
    fn validated_pending_in_rejects_a_closes_rule_citing_an_unknown_id() {
        let dir = rule_fixture("rejects-unknown", "**R2.3 — real.**\n");
        fs::write(
            dir.join("design/pending/x.md"),
            "---\nlevel: patch\nchangelog: x\ncloses_rule: R99.99\n---\n",
        )
        .unwrap();
        let errors = validated_pending_in(&dir).expect_err("R99.99 does not exist");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("R99.99") && e.contains("not a rule id in")),
            "{errors:?}"
        );
    }

    #[test]
    fn validated_pending_in_accepts_a_closes_rule_citing_a_known_id() {
        let dir = rule_fixture("accepts-known", "**R2.3 — real.**\n");
        fs::write(
            dir.join("design/pending/x.md"),
            "---\nlevel: patch\nchangelog: x\ncloses_rule: R2.3\n---\n",
        )
        .unwrap();
        let parsed = validated_pending_in(&dir).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].1.closes_rule, vec!["R2.3".to_string()]);
    }

    #[test]
    fn non_kebab_adr_slug_rejected() {
        assert!(
            err(
                "x.md",
                "---\nlevel: minor\nchangelog: x\n---\n\n## ADR: Not_Kebab\ntitle: T\n\nBody.\n"
            )
            .iter()
            .any(|e| e.contains("not a kebab-case slug"))
        );
    }

    #[test]
    fn duplicate_adr_slug_rejected() {
        assert!(err(
            "x.md",
            "---\nlevel: minor\nchangelog: x\n---\n\n## ADR: dup\ntitle: A\n\nBody a.\n\n## ADR: dup\ntitle: B\n\nBody b.\n"
        )
        .iter()
        .any(|e| e.contains("duplicate ADR slug")));
    }

    #[test]
    fn empty_adr_body_rejected() {
        assert!(
            err(
                "x.md",
                "---\nlevel: minor\nchangelog: x\n---\n\n## ADR: empty\ntitle: T\n\n## ADR: next\ntitle: N\n\nBody.\n"
            )
            .iter()
            .any(|e| e.contains("empty body"))
        );
    }

    #[test]
    fn non_kebab_filename_rejected() {
        assert!(
            err("Not_A_Slug.md", "---\nlevel: minor\nchangelog: x\n---\n")
                .iter()
                .any(|e| e.contains("filename stem"))
        );
    }
}
