//! The reference static registry and `bynk add` (packaging slice 4a, #2274,
//! decisions H and I).
//!
//! A static registry is a directory any static host can serve:
//!
//! ```text
//! <dir>/config.json             protocol, id, auth, the key chain
//! <dir>/<org>/<package>         one signed JSON line per version (append-only)
//! <dir>/dl/sha256/<hex>         each version's archive, named by tree checksum
//! ```
//!
//! `bynk registry init` writes `config.json` and a signing key (never inside
//! the directory); `bynk registry add` packs a package, signs its line and
//! appends it; `bynk registry yank` flips a line's `yanked`, which the
//! signature doesn't cover, so nothing is re-signed and nothing is deleted.
//! `bynk registry withdraw` (#2330) is the takedown: it yanks a line, records
//! why in its unsigned `withdrawn`, and deletes its archive.

use std::path::{Path, PathBuf};

use super::archive;
use super::protocol::{
    Config, ConfigTrust, IndexLine, KeyEntry, SigningKey, delegation_payload, fingerprint,
    revocation_payload, rotation_payload, verify,
};

/// `bynk registry init`: a new static registry at `dir` with id `id`, its
/// signing key written to `key_out`. Returns the `key = "…"` line consumers
/// pin.
pub fn init(dir: &Path, id: &str, key_out: &Path) -> Result<String, String> {
    if bynk_project::manifest::registry_name_problem(id).is_some() {
        return Err(format!("`{id}` isn't a registry id: use `[a-z][a-z0-9-]*`"));
    }
    let config_path = dir.join("config.json");
    if config_path.exists() {
        return Err(format!("`{}` already exists", config_path.display()));
    }
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    if abs(key_out).starts_with(abs(dir)) {
        return Err(format!(
            "the signing key `{}` would be inside the registry directory, which is published — write it elsewhere with `--key-out`",
            key_out.display()
        ));
    }
    if key_out.exists() {
        return Err(format!("`{}` already exists", key_out.display()));
    }
    let (key, pkcs8) = SigningKey::generate()?;
    write_private(key_out, &pkcs8)?;
    let config = Config {
        id: id.to_string(),
        dl: "dl".to_string(),
        api: None,
        token_auth: false,
        keys: vec![KeyEntry {
            key: key.public(),
            sig: None,
        }],
        delegations: vec![],
        revoked: vec![],
    };
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("could not create `{}`: {e}", dir.display()))?;
    std::fs::write(&config_path, config.render())
        .map_err(|e| format!("could not write `{}`: {e}", config_path.display()))?;
    Ok(format!("key = \"{}\"", key.public()))
}

/// `bynk registry add`: pack the package at `package_dir`, sign its line with
/// the key in `key_file`, and append it to the registry at `dir`. Returns
/// `org/package version`.
pub fn add(dir: &Path, package_dir: &Path, key_file: &Path) -> Result<String, String> {
    let config = read_config(dir)?;
    let key = signing_key(key_file)?;
    current_key(&config, &key, key_file)?;
    let (bytes, _) =
        archive::pack(package_dir).map_err(|e| format!("could not pack the package: {e}"))?;
    let admitted = admit(dir, &config, &key, &bytes, None).map_err(|r| r.message)?;
    let line = &admitted.line;
    Ok(if admitted.fresh {
        format!("{} {}", line.name, line.version)
    } else {
        format!(
            "{} {} — already present, identical, so nothing changed",
            line.name, line.version
        )
    })
}

pub(crate) use bynk_registry_core::Refusal;

/// #2311 (D10): what admitting an upload produced — the line, and whether it
/// is new (`fresh`) or an identical tree's existing line (a retry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Admitted {
    pub line: IndexLine,
    pub fresh: bool,
}

fn refuse(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal {
        code,
        message: message.into(),
    }
}

/// #2287: admit an uploaded archive to the static registry at `dir` — the one
/// set of rules `bynk registry add` and `bynk registry serve` both apply.
///
/// The archive is admitted in memory by `bynk-registry-core` (the one
/// implementation the hosted registry's wasm module shares): its own `bynk.toml`
/// must name a `[package]` with a `version` (and, when `expected` is given,
/// the very `org/package` and version the request named), its dependencies
/// must all be registry dependencies, and the version must be new — or, an
/// existing version whose tree is the very one uploaded, which answers with
/// its existing line and changes nothing (#2311, D10: a retry). The tree is
/// re-packed canonically, its archive written before its line, and the index
/// rewritten through a rename, so a crash never leaves a line without its
/// archive. The caller serialises calls for one registry.
pub(crate) fn admit(
    dir: &Path,
    config: &Config,
    key: &SigningKey,
    upload: &[u8],
    expected: Option<(&str, &str)>,
) -> Result<Admitted, Refusal> {
    if config.dl.contains("://") {
        return Err(refuse(
            "internal",
            format!(
                "registry `{}` keeps its archives at `{}`, which isn't a directory this registry can write",
                config.id, config.dl
            ),
        ));
    }
    admit_checked(dir, config, key, upload, expected)
}

fn admit_checked(
    dir: &Path,
    config: &Config,
    key: &SigningKey,
    upload: &[u8],
    expected: Option<(&str, &str)>,
) -> Result<Admitted, Refusal> {
    let core = bynk_registry_core::admit(upload, expected)?;
    let (name, version, checksum) = (core.name, core.version, core.checksum);
    let (org, package) = name
        .split_once('/')
        .expect("an admitted name is org/package");
    let index_path = dir.join(org).join(package);
    let existing = std::fs::read_to_string(&index_path).unwrap_or_default();
    for raw in existing.lines().filter(|l| !l.trim().is_empty()) {
        let line = IndexLine::parse(raw).map_err(|e| refuse("internal", e))?;
        if line.version == version {
            // #2330 (E): a withdrawn version stays down — even a retry of the
            // very tree must not quietly restore a takedown.
            if line.withdrawn.is_some() {
                return Err(refuse(
                    "withdrawn",
                    format!(
                        "`{name}` {version} was withdrawn from the registry and can't be published again; bump `version`"
                    ),
                ));
            }
            if line.checksum == checksum {
                return Ok(Admitted { line, fresh: false });
            }
            return Err(refuse(
                "version_exists",
                format!(
                    "`{name}` {version} is already in the registry with a different tree — its index is append-only; bump `version`"
                ),
            ));
        }
    }
    let hex = bynk_project::lock::checksum_hex(&checksum).expect("a checksum pack wrote");
    let archive_path = dir.join(&config.dl).join("sha256").join(&hex);
    write_replacing(&archive_path, &core.archive)
        .map_err(|e| refuse("internal", format!("could not write the archive: {e}")))?;
    let mut line = IndexLine {
        name,
        version,
        checksum,
        deps: core.deps,
        npm: core.npm,
        toolchain: None,
        yanked: false,
        withdrawn: None,
        sig: String::new(),
    };
    line.sig = key.sign(&line.payload(&config.id));
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&line.render());
    out.push('\n');
    write_replacing(&index_path, out.as_bytes())
        .map_err(|e| refuse("internal", format!("could not write the index: {e}")))?;
    Ok(Admitted { line, fresh: true })
}

/// Write `bytes` to `path` through a temporary file and a rename, so a reader
/// (or a crash) never sees half a file.
pub(crate) fn write_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Refuse a key that isn't the registry's current (newest) key.
pub(crate) fn current_key(
    config: &Config,
    key: &SigningKey,
    key_file: &Path,
) -> Result<(), String> {
    // #2316 (E): the chain's newest key, or a valid, unrevoked delegate.
    let public = key.public();
    let signers = newest_signers(config)?;
    if signers.revoked.contains(&public) {
        return Err(format!(
            "`{}` is a key registry `{}` revoked",
            key_file.display(),
            config.id
        ));
    }
    let newest = &config.keys.last().expect("parse checks").key;
    let delegate = signers.keys.contains(&public) && !config.keys.iter().any(|k| k.key == public);
    if public != *newest && !delegate {
        return Err(format!(
            "`{}` isn't registry `{}`'s current key or a delegate of it",
            key_file.display(),
            config.id
        ));
    }
    Ok(())
}

/// #2316: the registry's own chain, every link verified from its first key —
/// the keys an operator's own tooling trusts.
fn own_chain(config: &Config) -> Result<Vec<String>, String> {
    let first = fingerprint(&config.keys[0].key).ok_or_else(|| {
        format!(
            "registry `{}`'s first key isn't an `ed25519:` key",
            config.id
        )
    })?;
    config
        .chain_from(&first)
        .ok_or_else(|| {
            format!(
                "registry `{}`'s key chain doesn't verify from its first key — a link in `config.json`'s `keys` isn't signed by the key before it; restore it from a backup, or start a new chain with `bynk registry init` and have clients re-pin",
                config.id
            )
        })
}

/// #2316: who may sign lines as a client pinned to the **newest** chain key
/// sees it — the strictest view, since a client anchored earlier reaches the
/// newest key too. The tooling judges delegates by it, so a delegate a fresh
/// pin wouldn't accept is never treated as valid.
fn newest_signers(config: &Config) -> Result<super::protocol::Signers, String> {
    own_chain(config)?;
    let newest = config.keys.last().expect("parse checks").key.clone();
    Ok(config.signers(&[newest]))
}

/// #2316: the key in `key_file`, which must be the registry's **newest**
/// chain key: only it may delegate or revoke. (A client pinned to a later link
/// trusts only the chain from there, so a delegation an older link signed
/// would be invalid for it.)
fn chain_key(config: &Config, key_file: &Path) -> Result<SigningKey, String> {
    let key = signing_key(key_file)?;
    own_chain(config)?;
    if key.public() != config.keys.last().expect("parse checks").key {
        return Err(format!(
            "`{}` isn't registry `{}`'s newest chain key — only it may delegate or revoke",
            key_file.display(),
            config.id
        ));
    }
    Ok(key)
}

/// A new key written to `out`, refused inside the registry directory.
fn new_key(dir: &Path, out: &Path) -> Result<SigningKey, String> {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    if abs(out).starts_with(abs(dir)) {
        return Err(format!(
            "the key `{}` would be inside the registry directory, which is published — write it elsewhere",
            out.display()
        ));
    }
    if out.exists() {
        return Err(format!("`{}` already exists", out.display()));
    }
    let (key, pkcs8) = SigningKey::generate()?;
    write_private(out, &pkcs8)?;
    Ok(key)
}

fn write_config(dir: &Path, config: &Config) -> Result<(), String> {
    // Check what is about to be published reads back as written.
    let text = config.render();
    if Config::parse(text.as_bytes()).as_ref() != Ok(config) {
        return Err("the new `config.json` doesn't read back as written".to_string());
    }
    write_replacing(&dir.join("config.json"), text.as_bytes())
        .map_err(|e| format!("could not write `config.json`: {e}"))
}

/// `bynk registry rotate` (#2316): append a chain link — a new key, signed by
/// the current (newest) chain key in `key_file`. Returns the new key.
pub fn rotate(dir: &Path, key_file: &Path, key_out: &Path) -> Result<String, String> {
    let mut config = read_config(dir)?;
    let old = signing_key(key_file)?;
    if old.public() != config.keys.last().expect("parse checks").key {
        return Err(format!(
            "`{}` isn't registry `{}`'s newest chain key, which is the only key that may extend the chain",
            key_file.display(),
            config.id
        ));
    }
    let valid = newest_signers(&config)?;
    let new = new_key(dir, key_out)?;
    config.keys.push(KeyEntry {
        key: new.public(),
        sig: Some(old.sign(&rotation_payload(&config.id, &new.public()))),
    });
    // A client that pins the new key trusts only it, so carry every valid
    // delegation and revocation over by re-signing it with the new key; one
    // that wasn't valid stays as it was, ignored.
    let id = config.id.clone();
    for d in &mut config.delegations {
        if valid.keys.contains(&d.key) {
            d.sig = Some(new.sign(&delegation_payload(&id, &d.key)));
        }
    }
    for r in &mut config.revoked {
        if valid.revoked.contains(&r.key) {
            r.sig = Some(new.sign(&revocation_payload(&id, &r.key)));
        }
    }
    let after = newest_signers(&config)?;
    if after.keys[1..] != valid.keys[1..] || after.revoked != valid.revoked {
        return Err("rotating would change which delegates are valid".to_string());
    }
    write_config(dir, &config)?;
    Ok(new.public())
}

/// `bynk registry delegate` (#2316): a new key that may sign index lines,
/// delegated by the chain key in `key_file`. Returns the new key.
pub fn delegate(dir: &Path, key_file: &Path, key_out: &Path) -> Result<String, String> {
    let mut config = read_config(dir)?;
    let chain = chain_key(&config, key_file)?;
    let new = new_key(dir, key_out)?;
    config.delegations.push(KeyEntry {
        key: new.public(),
        sig: Some(chain.sign(&delegation_payload(&config.id, &new.public()))),
    });
    if !config
        .signers(&[config.keys.last().expect("parse checks").key.clone()])
        .keys
        .contains(&new.public())
    {
        return Err("the delegation doesn't verify".to_string());
    }
    write_config(dir, &config)?;
    Ok(new.public())
}

/// `bynk registry revoke` (#2316): revoke the delegate `target`, signed by the
/// chain key in `key_file`. Refuses, unless `force`, when a line would be
/// left with no valid signer. Returns how many lines it signed.
pub fn revoke(dir: &Path, key_file: &Path, target: &str, force: bool) -> Result<usize, String> {
    let mut config = read_config(dir)?;
    let chain = chain_key(&config, key_file)?;
    if !config.delegations.iter().any(|d| d.key == target) {
        return Err(format!(
            "`{target}` isn't a delegate of registry `{}` — only delegates can be revoked",
            config.id
        ));
    }
    if config.revoked.iter().any(|r| r.key == target) {
        return Err(format!("`{target}` is already revoked"));
    }
    config.revoked.push(KeyEntry {
        key: target.to_string(),
        sig: Some(chain.sign(&revocation_payload(&config.id, target))),
    });
    let signers = newest_signers(&config)?;
    if !signers.revoked.iter().any(|k| k == target) {
        return Err("the revocation doesn't verify".to_string());
    }
    let mut stranded = 0;
    let mut by_target = 0;
    for path in index_files(dir, &config)? {
        for line in read_lines(&path)? {
            let payload = line.payload(&config.id);
            if verify(target, &payload, &line.sig) {
                by_target += 1;
            }
            if !signers.keys.iter().any(|k| verify(k, &payload, &line.sig)) {
                stranded += 1;
            }
        }
    }
    if stranded > 0 && !force {
        return Err(format!(
            "{stranded} index line(s) would have no valid signer — re-sign them with another key first (`bynk registry resign`), or pass `--force`"
        ));
    }
    write_config(dir, &config)?;
    Ok(by_target)
}

/// `bynk registry resign` (#2316): re-sign every index line with the key in
/// `key_file` (the newest chain key, or a valid delegate). Returns the number
/// of lines re-signed. Only `sig` changes.
pub fn resign(dir: &Path, key_file: &Path) -> Result<usize, String> {
    let config = read_config(dir)?;
    let key = signing_key(key_file)?;
    current_key(&config, &key, key_file)?;
    // Re-sign only what a key the registry knows signed — the chain, a valid
    // delegate, or a revoked one (replacing those is the point) — so a line
    // planted in the directory is never laundered into a valid one.
    //
    // Each file is read once, and what was checked is exactly what is signed
    // and written. Don't run it while a server appends to the same directory:
    // a line appended between the read and the write would be lost.
    let signers = newest_signers(&config)?;
    let known: Vec<&String> = signers.keys.iter().chain(&signers.revoked).collect();
    let mut files = Vec::new();
    for path in index_files(dir, &config)? {
        let lines = read_lines(&path)?;
        files.push((path, lines));
    }
    let mut unknown = Vec::new();
    for (_, lines) in &files {
        for line in lines {
            let payload = line.payload(&config.id);
            if !known.iter().any(|k| verify(k, &payload, &line.sig)) {
                unknown.push(format!("{} {}", line.name, line.version));
            }
        }
    }
    if !unknown.is_empty() {
        return Err(format!(
            "no key this registry knows signed {} — refusing to re-sign it: {}",
            if unknown.len() == 1 {
                "this line"
            } else {
                "these lines"
            },
            unknown.join(", ")
        ));
    }
    let mut count = 0;
    for (path, lines) in files {
        let mut out = String::new();
        for mut line in lines {
            line.sig = key.sign(&line.payload(&config.id));
            out.push_str(&line.render());
            out.push('\n');
            count += 1;
        }
        write_replacing(&path, out.as_bytes())
            .map_err(|e| format!("could not write `{}`: {e}", path.display()))?;
    }
    Ok(count)
}

/// Every index file under `dir`: `<org>/<package>`, outside the archive base.
fn index_files(dir: &Path, config: &Config) -> Result<Vec<PathBuf>, String> {
    let dl = std::path::absolute(dir.join(&config.dl)).unwrap_or_else(|_| dir.join(&config.dl));
    let mut out = Vec::new();
    let read = |p: &Path| {
        std::fs::read_dir(p).map_err(|e| format!("could not read `{}`: {e}", p.display()))
    };
    for org in read(dir)?.flatten() {
        let org = org.path();
        let abs = std::path::absolute(&org).unwrap_or_else(|_| org.clone());
        let hidden = org
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if !org.is_dir() || hidden || abs.starts_with(&dl) {
            continue;
        }
        for file in read(&org)?.flatten() {
            let file = file.path();
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if file.is_file() && !name.starts_with('.') && !name.contains(".tmp.") {
                out.push(file);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn read_lines(path: &Path) -> Result<Vec<IndexLine>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read `{}`: {e}", path.display()))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| IndexLine::parse(l).map_err(|e| format!("`{}`: {e}", path.display())))
        .collect()
}

/// `bynk registry yank`: set (or, with `undo`, clear) `yanked` on one line.
pub fn yank(dir: &Path, package: &str, version: &str, undo: bool) -> Result<(), String> {
    yank_line(dir, package, version, undo).map_err(|r| r.message)
}

/// #2287: flip `yanked` on one line — `bynk registry yank` and the publish
/// API's yank. Nothing is deleted or re-signed; `yanked` isn't signed.
pub(crate) fn yank_line(
    dir: &Path,
    package: &str,
    version: &str,
    undo: bool,
) -> Result<(), Refusal> {
    let index_path = dir.join(package);
    let text = std::fs::read_to_string(&index_path).map_err(|_| {
        refuse(
            "not_found",
            format!("the registry has no package `{package}`"),
        )
    })?;
    let mut found = false;
    let mut out = String::new();
    for raw in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut line = IndexLine::parse(raw).map_err(|e| refuse("internal", e))?;
        if line.version == version {
            // #2330 (E): un-yanking a withdrawn version would advertise a
            // version whose archive is gone.
            if undo && line.withdrawn.is_some() {
                return Err(refuse(
                    "withdrawn",
                    format!(
                        "`{package}` {version} was withdrawn, so it can't be un-yanked; a withdrawal is final"
                    ),
                ));
            }
            line.yanked = !undo;
            found = true;
        }
        out.push_str(&line.render());
        out.push('\n');
    }
    if !found {
        return Err(refuse(
            "not_found",
            format!("the registry has no `{package}` {version}"),
        ));
    }
    write_replacing(&index_path, out.as_bytes())
        .map_err(|e| refuse("internal", format!("could not write the index: {e}")))
}

/// #2330 (D): what `bynk registry withdraw` did to the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveOutcome {
    /// The archive was deleted.
    Deleted,
    /// Another line names the same tree, so its archive was kept; carries
    /// that line's `org/package version`.
    Shared(String),
    /// There was no archive to delete.
    Missing,
    /// The version was already withdrawn but its archive was still there (an
    /// interrupted withdrawal); it was deleted now.
    Finished,
}

/// #2330 (D): `bynk registry withdraw` — the takedown of `package` `version`
/// on the registry at `dir`. Sets `yanked` and `withdrawn` (`reason`) on its
/// line, re-signing nothing, then deletes its archive unless another line
/// names the same tree. Final: a withdrawn version is refused here (unless an
/// interrupted withdrawal left its archive behind, which is then deleted), by
/// `add`, by the publish API and by an un-yank.
pub fn withdraw(
    dir: &Path,
    package: &str,
    version: &str,
    reason: &str,
) -> Result<ArchiveOutcome, String> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("give the reason a client will print with `--reason`".to_string());
    }
    if reason.chars().count() > super::protocol::WITHDRAWN_MAX {
        return Err(format!(
            "the reason is limited to {} characters",
            super::protocol::WITHDRAWN_MAX
        ));
    }
    if reason.chars().any(char::is_control) {
        return Err("the reason must be one line of text, with no control characters".to_string());
    }
    let config = read_config(dir)?;
    if config.dl.contains("://") {
        return Err(format!(
            "registry `{}` keeps its archives at `{}`, which isn't a directory this command can delete from",
            config.id, config.dl
        ));
    }
    let index_path = dir.join(package);
    if !index_path.is_file() {
        return Err(format!("the registry has no package `{package}`"));
    }
    let mut lines = read_lines(&index_path)?;
    let Some(line) = lines.iter_mut().find(|l| l.version == version) else {
        return Err(format!("the registry has no `{package}` {version}"));
    };
    let (name, checksum) = (line.name.clone(), line.checksum.clone());
    // Everything that can fail is read before anything is written, so a
    // refusal leaves the registry as it was. Identical trees under two
    // versions share one archive: keep it while another line, not itself
    // withdrawn, still names it.
    let hex = bynk_project::lock::checksum_hex(&checksum)
        .ok_or_else(|| format!("`{package}` {version}'s checksum is malformed"))?;
    let archive = dir.join(&config.dl).join("sha256").join(hex);
    let mut shared = None;
    for path in index_files(dir, &config)? {
        for other in read_lines(&path)? {
            let itself = other.name == name && other.version == version;
            if !itself && other.checksum == checksum && other.withdrawn.is_none() {
                shared.get_or_insert(format!("{} {}", other.name, other.version));
            }
        }
    }
    if line.withdrawn.is_some() {
        // A withdrawal interrupted after its index write left the archive
        // behind: finish it rather than leave the takedown half done.
        if shared.is_none() && archive.is_file() {
            return delete_archive(&archive).map(|_| ArchiveOutcome::Finished);
        }
        return Err(format!("`{package}` {version} is already withdrawn"));
    }
    line.yanked = true;
    line.withdrawn = Some(reason.to_string());
    let out: String = lines.iter().map(|l| l.render() + "\n").collect();
    write_replacing(&index_path, out.as_bytes())
        .map_err(|e| format!("could not write `{}`: {e}", index_path.display()))?;
    match shared {
        Some(other) => Ok(ArchiveOutcome::Shared(other)),
        None => delete_archive(&archive),
    }
}

fn delete_archive(archive: &Path) -> Result<ArchiveOutcome, String> {
    match std::fs::remove_file(archive) {
        Ok(()) => Ok(ArchiveOutcome::Deleted),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ArchiveOutcome::Missing),
        Err(e) => Err(format!(
            "could not delete `{}`: {e} — run `bynk registry withdraw` again to finish",
            archive.display()
        )),
    }
}

/// `bynk add`: add `package` (`org/package`, optionally `@range`) to the
/// manifest at `project`, choosing the highest version its registry offers.
/// Returns the line written.
pub fn add_dependency(
    project: &Path,
    spec: &str,
    alias: Option<&str>,
) -> Result<AddedDependency, String> {
    let (package, range) = match spec.split_once('@') {
        Some((p, r)) => (p.to_string(), Some(r.to_string())),
        None => (spec.to_string(), None),
    };
    if let Some(problem) = bynk_project::manifest::package_name_problem(&package) {
        return Err(format!("`{package}` {problem}"));
    }
    let (org, pkg) = package.split_once('/').expect("checked");
    let alias = alias.unwrap_or(pkg).to_string();
    if let Some(problem) = bynk_project::manifest::identifier_problem(&alias) {
        return Err(format!(
            "the alias `{alias}` {problem} — choose another with `--as`"
        ));
    }
    let root = bynk_project::lock::lock_root(project);
    let configs = registries_of(&root);
    let mut client = super::Client::new(configs, None);
    let registry = client.route(org);
    let lines = client.lines(&registry, &package).map_err(|e| e.message)?;
    let toolchain =
        semver::Version::parse(crate::DRIVER_VERSION).unwrap_or(semver::Version::new(0, 0, 0));
    let ranges: Vec<String> = range.iter().cloned().collect();
    let chosen =
        super::select::choose(&lines, &ranges, None, &toolchain).map_err(|_| match &range {
            Some(r) => format!("registry `{registry}` has no `{package}` matching `{r}`"),
            None => format!("registry `{registry}` has no usable version of `{package}`"),
        })?;
    let written = range.unwrap_or_else(|| format!("^{}", chosen.version));
    let manifest = project.join("bynk.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("could not read `bynk.toml`: {e}"))?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| "`bynk.toml` isn't valid TOML".to_string())?;
    if doc.get("dependencies").is_none() {
        doc["dependencies"] = toml_edit::table();
    }
    let deps = doc["dependencies"]
        .as_table_mut()
        .ok_or("`[dependencies]` isn't a table")?;
    if deps.contains_key(&alias) {
        return Err(format!(
            "`[dependencies]` already has `{alias}` — choose another alias with `--as`"
        ));
    }
    let mut entry = toml_edit::InlineTable::new();
    entry.insert("package", package.clone().into());
    entry.insert("version", written.clone().into());
    deps.insert(&alias, toml_edit::value(entry));
    std::fs::write(&manifest, doc.to_string())
        .map_err(|e| format!("could not write `bynk.toml`: {e}"))?;
    Ok(AddedDependency {
        alias,
        package,
        version: written,
    })
}

/// The `[dependencies]` entry `bynk add` wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedDependency {
    pub alias: String,
    pub package: String,
    pub version: String,
}

impl std::fmt::Display for AddedDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} = {{ package = \"{}\", version = \"{}\" }}",
            self.alias, self.package, self.version
        )
    }
}

/// The `[registries]` of the manifest at `root`.
pub(crate) fn registries_of(root: &Path) -> Vec<bynk_project::RegistryConfig> {
    crate::fetch::read_manifest(root)
        .and_then(|doc| {
            doc.get("registries")
                .and_then(|v| v.as_table())
                .and_then(|t| bynk_project::manifest::registries_from(t).ok())
        })
        .unwrap_or_default()
}

pub(crate) fn read_config(dir: &Path) -> Result<Config, String> {
    let bytes = std::fs::read(dir.join("config.json")).map_err(|_| {
        format!(
            "`{}` isn't a registry: it has no `config.json` (run `bynk registry init`)",
            dir.display()
        )
    })?;
    Config::parse(&bytes)
}

pub(crate) fn signing_key(path: &Path) -> Result<SigningKey, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("could not read the key `{}`: {e}", path.display()))?;
    SigningKey::from_pkcs8(&bytes)
}

pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("could not write the key: {e}"))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("could not write the key: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// The default key path for registry `id`: `./<id>.key`.
pub fn default_key_path(id: &str) -> PathBuf {
    PathBuf::from(format!("{id}.key"))
}
