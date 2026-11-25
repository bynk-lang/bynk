//! The ledger of stamped pending files (#4319): what keeps the stamp idempotent.
//!
//! `cargo xtask stamp --apply` consumes each `design/pending/<slug>.md` by assigning it a version
//! and deleting it. A branch that carries a copy of a file the stamp has since consumed can bring
//! it back when it merges, and the stamp would then give that one increment a second version and a
//! second changelog row (#4234, #4269). Nothing on `main` remembered that the file had been
//! stamped.
//!
//! `design/stamped.txt` is that memory: one line for each file the stamp has consumed,
//!
//! ```text
//! <slug> <content hash> <version> [adr:NNNN,NNNN]
//! ```
//!
//! appended by the stamp alone, so no pull request touches it and it cannot conflict. A pending
//! file is *already stamped* when its slug **and** its content hash are on the ledger. The hash is
//! what lets a slug be reused: a branch name restarted for a new increment has different bytes,
//! while a stale copy of a consumed file is byte-identical to what the stamp read.
//!
//! The stamp deletes an already-stamped file and assigns nothing. `cargo xtask check-pending` fails
//! a pull request that carries one, so the author sees it on the PR instead of at the merge queue.

use std::fs;
use std::io;
use std::path::Path;

/// The ledger, relative to the repo root.
pub const LEDGER_PATH: &str = "design/stamped.txt";

/// One consumed pending file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub slug: String,
    pub hash: String,
    pub version: String,
    /// The ADR numbers the increment was given, as written (`0940`); empty for the files
    /// recorded when the ledger was back-filled from history.
    pub adrs: Vec<String>,
}

/// A stable 64-bit FNV-1a hash of `bytes`, as 16 hex digits, over the bytes with every `\r\n`
/// read as `\n`: a Windows checkout with CRLF line endings must match the file the stamp read on
/// Linux. Not cryptographic: the ledger asks whether this exact file was stamped, and a pending
/// file is not an adversary. It needs no dependency, and the same value is computable from any
/// language (the back-fill used Python).
pub fn content_hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i += 1;
        }
        h ^= u64::from(bytes[i]);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    format!("{h:016x}")
}

/// The ledger text, one entry per line. Blank lines and `#` comments are ignored.
pub fn parse(text: &str) -> Vec<Entry> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (slug, hash, version) = (it.next()?, it.next()?, it.next()?);
            let adrs = it
                .next()
                .and_then(|a| a.strip_prefix("adr:"))
                .map(|a| a.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            Some(Entry {
                slug: slug.to_string(),
                hash: hash.to_string(),
                version: version.to_string(),
                adrs,
            })
        })
        .collect()
}

/// The entries in `root`'s ledger; none when the file does not exist yet.
pub fn read(root: &Path) -> io::Result<Vec<Entry>> {
    match fs::read_to_string(root.join(LEDGER_PATH)) {
        Ok(text) => Ok(parse(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// The entry that stamped exactly this slug and content, if any.
pub fn find<'a>(entries: &'a [Entry], slug: &str, hash: &str) -> Option<&'a Entry> {
    entries.iter().find(|e| e.slug == slug && e.hash == hash)
}

/// The entry for a pending file that git history shows was already consumed, for stamps the
/// ledger has no line for: those made by the old stamp before the ledger landed, whether before
/// or after the back-filled snapshot. A stamp deletes the pending file it consumes, so a file at this path
/// that a commit on this history deleted, with the same content, was stamped. The version is the
/// workspace version at that commit, which for a commit that stamped several files is the last
/// one's. `None` when `root` is no git checkout or git cannot be run, which a fixture tree is
/// not and the stamp's own checkout always is (it fetches full history).
pub fn find_in_git_history(root: &Path, file: &str, hash: &str) -> Option<Entry> {
    let git = |args: &[&str]| -> Option<Vec<u8>> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()?;
        out.status.success().then_some(out.stdout)
    };
    let path = format!("design/pending/{file}");
    let deleting = git(&["log", "--diff-filter=D", "--format=%H", "--", &path])?;
    for commit in String::from_utf8_lossy(&deleting).lines() {
        // Only commits from before the ledger existed. Every stamp made once `design/stamped.txt`
        // is in the tree writes its line, so from then on the ledger is the only authority: a
        // stamp that was reverted to be redone leaves a deleting commit behind, and must stamp
        // again, as must a file deleted by anything other than the stamp.
        if git(&["cat-file", "-e", &format!("{commit}:{LEDGER_PATH}")]).is_some() {
            continue;
        }
        let Some(before) = git(&["show", &format!("{commit}^:{path}")]) else {
            continue;
        };
        if content_hash(&before) != hash {
            continue;
        }
        let version = git(&["show", &format!("{commit}:Cargo.toml")])
            .and_then(|c| crate::stamp::parse_workspace_version(&String::from_utf8_lossy(&c)))
            .map_or_else(|| format!("(commit {})", &commit[..8]), |v| v.to_string());
        let slug = file.strip_suffix(".md").unwrap_or(file);
        return Some(Entry {
            slug: slug.to_string(),
            hash: hash.to_string(),
            version,
            adrs: Vec::new(),
        });
    }
    None
}

/// The line for a file stamped as `version` with `adrs` (ADR numbers, in order).
pub fn line(slug: &str, hash: &str, version: &str, adrs: &[u32]) -> String {
    let mut out = format!("{slug} {hash} {version}");
    if !adrs.is_empty() {
        let numbers: Vec<String> = adrs.iter().map(|n| format!("{n:04}")).collect();
        out.push_str(&format!(" adr:{}", numbers.join(",")));
    }
    out
}

/// `existing` with `lines` appended, each on its own line.
pub fn appended(existing: &str, lines: &[String]) -> String {
    let mut out = String::from(existing);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// The warning `check-pending` prints for a pending file whose slug is on the ledger with
/// different content (#4319): a reused slug is legitimate, but an edited near-copy of a consumed
/// file looks the same, so it is made visible.
pub fn reused_slug_warning(file: &str, entry: &Entry) -> String {
    let slug = file.strip_suffix(".md").unwrap_or(file);
    format!(
        "warning: {file}: slug `{slug}` was stamped before, as v{}, with different content; \
         fine for a new increment under a reused name, but if this is an edited copy of that \
         increment's file, drop it",
        entry.version
    )
}

/// What `check-pending` says about a pending file the stamp has already consumed.
pub fn already_stamped_message(file: &str, entry: &Entry) -> String {
    let slug = file.strip_suffix(".md").unwrap_or(file);
    format!(
        "{file}: `{slug}` was already stamped as v{} on main; drop design/pending/{file}, \
         main has consumed it. (A branch that merged main while this file was pending, then \
         merged after the stamp, carries a stale copy.)",
        entry.version
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_fnv_1a_64_in_sixteen_hex_digits() {
        // The published FNV-1a 64-bit test vectors, which the back-fill's Python agrees with.
        assert_eq!(content_hash(b""), "cbf29ce484222325");
        assert_eq!(content_hash(b"a"), "af63dc4c8601ec8c");
        assert_eq!(content_hash(b"foobar"), "85944171f73967e8");
    }

    #[test]
    fn the_hash_ignores_crlf_line_endings() {
        assert_eq!(content_hash(b"a\r\nb\r\n"), content_hash(b"a\nb\n"));
        // A lone CR is content, not a line ending.
        assert_ne!(content_hash(b"a\rb"), content_hash(b"a\nb"));
        assert_ne!(content_hash(b"a\n"), content_hash(b"a\n\n"));
    }

    #[test]
    fn a_line_round_trips_with_and_without_adrs() {
        let with = line("feat-x", "00ff00ff00ff00ff", "0.9.0", &[7, 8]);
        assert_eq!(with, "feat-x 00ff00ff00ff00ff 0.9.0 adr:0007,0008");
        let without = line("feat-y", "1111111111111111", "0.9.1", &[]);
        assert_eq!(without, "feat-y 1111111111111111 0.9.1");
        let entries = parse(&format!("# comment\n\n{with}\n{without}\n"));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].adrs, vec!["0007", "0008"]);
        assert!(entries[1].adrs.is_empty());
        assert!(find(&entries, "feat-x", "00ff00ff00ff00ff").is_some());
        assert!(find(&entries, "feat-x", "1111111111111111").is_none());
    }

    #[test]
    fn appending_keeps_the_existing_lines_and_ends_in_a_newline() {
        assert_eq!(appended("", &["a b c".into()]), "a b c\n");
        assert_eq!(appended("x y z", &["a b c".into()]), "x y z\na b c\n");
        assert_eq!(appended("x y z\n", &[]), "x y z\n");
    }
}
