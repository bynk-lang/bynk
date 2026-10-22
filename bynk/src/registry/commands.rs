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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bynk_project::lock::adapter_requires;
use bynk_project::manifest::{DependencySource, package_config_from};

use super::archive;
use super::protocol::{Config, IndexLine, KeyEntry, Requirement, SigningKey};

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

/// Why a registry refused an upload: a stable `code` for the publish API's
/// JSON error, and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub code: &'static str,
    pub message: String,
}

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
/// The archive is unpacked with the hardened `unpack`, its own `bynk.toml`
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
    let staging = private_staging()
        .map_err(|e| refuse("internal", format!("could not stage the upload: {e}")))?;
    let result = admit_staged(dir, config, key, upload, expected, &staging);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn admit_staged(
    dir: &Path,
    config: &Config,
    key: &SigningKey,
    upload: &[u8],
    expected: Option<(&str, &str)>,
    staging: &Path,
) -> Result<Admitted, Refusal> {
    archive::unpack(upload, staging).map_err(|e| refuse("invalid_package", e.message))?;
    let text = std::fs::read_to_string(staging.join("bynk.toml"))
        .map_err(|_| refuse("invalid_package", "the package has no `bynk.toml`"))?;
    bynk_project::check_manifest_str(&text).map_err(|e| {
        refuse(
            "invalid_package",
            format!("its `bynk.toml` is invalid: {e}"),
        )
    })?;
    let doc: toml::Table = text
        .parse()
        .map_err(|_| refuse("invalid_package", "its `bynk.toml` isn't TOML"))?;
    let package = doc
        .get("package")
        .and_then(|v| v.as_table())
        .and_then(|t| package_config_from(t).ok())
        .ok_or_else(|| {
            refuse(
                "invalid_package",
                "it isn't a package: its `bynk.toml` has no `[package]`",
            )
        })?;
    let version = package.version.clone().ok_or_else(|| {
        refuse(
            "invalid_package",
            "its `[package]` has no `version`, which a registry needs",
        )
    })?;
    let name = format!("{}/{}", package.org, package.package);
    if let Some((want_name, want_version)) = expected
        && (want_name != name || want_version != version)
    {
        return Err(refuse(
            "invalid_package",
            format!(
                "the upload is `{name}` {version}, but the request named `{want_name}` {want_version}"
            ),
        ));
    }
    let mut deps = BTreeSet::new();
    for spec in crate::fetch::specs(&doc, "dependencies") {
        match spec.source {
            DependencySource::Version(range) => {
                deps.insert(Requirement {
                    package: spec.package,
                    range,
                });
            }
            _ => {
                return Err(refuse(
                    "invalid_package",
                    format!(
                        "its dependency `{}` is a path or git dependency, which a registry refuses — publish it, and depend on it by `version`",
                        spec.alias
                    ),
                ));
            }
        }
    }
    let (bytes, checksum) = archive::pack(staging)
        .map_err(|e| refuse("internal", format!("could not re-pack the package: {e}")))?;
    let index_path = dir.join(&package.org).join(&package.package);
    let existing = std::fs::read_to_string(&index_path).unwrap_or_default();
    for raw in existing.lines().filter(|l| !l.trim().is_empty()) {
        let line = IndexLine::parse(raw).map_err(|e| refuse("internal", e))?;
        if line.version == version {
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
    write_replacing(&archive_path, &bytes)
        .map_err(|e| refuse("internal", format!("could not write the archive: {e}")))?;
    let mut line = IndexLine {
        name,
        version,
        checksum,
        deps: deps.into_iter().collect(),
        npm: npm_of(staging, &doc),
        toolchain: None,
        yanked: false,
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
/// A new, empty directory only this process can enter, under the system temp
/// directory: created exclusively under a random name (never reused, never
/// followed if something already sits there), and `0700` on Unix.
fn private_staging() -> std::io::Result<PathBuf> {
    let rng = ring::rand::SystemRandom::new();
    for _ in 0..16 {
        let mut raw = [0u8; 16];
        ring::rand::SecureRandom::fill(&rng, &mut raw)
            .map_err(|_| std::io::Error::other("no randomness"))?;
        let name: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let path = std::env::temp_dir().join(format!("bynk-admit-{name}"));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free staging directory name"))
}

pub(crate) fn write_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Refuse a key that isn't the registry's current (newest) key.
fn current_key(config: &Config, key: &SigningKey, key_file: &Path) -> Result<(), String> {
    let newest = &config.keys.last().expect("parse checks").key;
    if key.public() != *newest {
        return Err(format!(
            "`{}` isn't registry `{}`'s current key",
            key_file.display(),
            config.id
        ));
    }
    Ok(())
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

/// `bynk add`: add `package` (`org/package`, optionally `@range`) to the
/// manifest at `project`, choosing the highest version its registry offers.
/// Returns the line written.
pub fn add_dependency(project: &Path, spec: &str, alias: Option<&str>) -> Result<String, String> {
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
    Ok(format!(
        "{alias} = {{ package = \"{package}\", version = \"{written}\" }}"
    ))
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

/// The npm packages the adapters at `package_dir` require, for its index line.
fn npm_of(package_dir: &Path, doc: &toml::Table) -> Vec<Requirement> {
    let mut out = BTreeSet::new();
    for root in bynk_project::packages::package_source_roots(package_dir, doc) {
        for file in bynk_project::discover_bynk_files(&root, &[]).unwrap_or_default() {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            if let Some((_, requires)) = adapter_requires(&text) {
                for (package, range) in requires {
                    out.insert(Requirement { package, range });
                }
            }
        }
    }
    out.into_iter().collect()
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
