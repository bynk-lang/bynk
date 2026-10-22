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
    let newest = &config.keys.last().expect("parse checks").key;
    if key.public() != *newest {
        return Err(format!(
            "`{}` isn't registry `{}`'s current key",
            key_file.display(),
            config.id
        ));
    }
    let text = std::fs::read_to_string(package_dir.join("bynk.toml")).map_err(|e| {
        format!(
            "`{}` has no readable `bynk.toml`: {e}",
            package_dir.display()
        )
    })?;
    bynk_project::check_manifest_str(&text)
        .map_err(|e| format!("its `bynk.toml` is invalid: {e}"))?;
    let doc: toml::Table = text
        .parse()
        .map_err(|_| "its `bynk.toml` isn't TOML".to_string())?;
    let package = doc
        .get("package")
        .and_then(|v| v.as_table())
        .and_then(|t| package_config_from(t).ok())
        .ok_or("it isn't a package: its `bynk.toml` has no `[package]`")?;
    let version = package
        .version
        .clone()
        .ok_or("its `[package]` has no `version`, which a registry needs")?;
    let name = format!("{}/{}", package.org, package.package);
    let mut deps = BTreeSet::new();
    for table in ["dependencies"] {
        for spec in crate::fetch::specs(&doc, table) {
            match spec.source {
                DependencySource::Version(range) => {
                    deps.insert(Requirement {
                        package: spec.package,
                        range,
                    });
                }
                _ => {
                    return Err(format!(
                        "its dependency `{}` is a path or git dependency, which a registry refuses — publish it, and depend on it by `version`",
                        spec.alias
                    ));
                }
            }
        }
    }
    let index_path = dir.join(&package.org).join(&package.package);
    let existing = std::fs::read_to_string(&index_path).unwrap_or_default();
    for raw in existing.lines().filter(|l| !l.trim().is_empty()) {
        let line = IndexLine::parse(raw)?;
        if line.version == version {
            return Err(format!(
                "`{name}` {version} is already in the registry — its index is append-only; bump `version`"
            ));
        }
    }
    let (bytes, checksum) =
        archive::pack(package_dir).map_err(|e| format!("could not pack the package: {e}"))?;
    if config.dl.contains("://") {
        return Err(format!(
            "registry `{}` keeps its archives at `{}`, which isn't a directory this command can write",
            config.id, config.dl
        ));
    }
    let hex = bynk_project::lock::checksum_hex(&checksum).expect("a checksum pack wrote");
    let archive_path = dir.join(&config.dl).join("sha256").join(&hex);
    std::fs::create_dir_all(archive_path.parent().unwrap())
        .map_err(|e| format!("could not write the archive: {e}"))?;
    std::fs::write(&archive_path, bytes)
        .map_err(|e| format!("could not write the archive: {e}"))?;
    let mut line = IndexLine {
        name: name.clone(),
        version: version.clone(),
        checksum,
        deps: deps.into_iter().collect(),
        npm: npm_of(package_dir, &doc),
        toolchain: None,
        yanked: false,
        sig: String::new(),
    };
    line.sig = key.sign(&line.payload(&config.id));
    std::fs::create_dir_all(index_path.parent().unwrap())
        .map_err(|e| format!("could not write the index: {e}"))?;
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&line.render());
    out.push('\n');
    std::fs::write(&index_path, out).map_err(|e| format!("could not write the index: {e}"))?;
    Ok(format!("{name} {version}"))
}

/// `bynk registry yank`: set (or, with `undo`, clear) `yanked` on one line.
pub fn yank(dir: &Path, package: &str, version: &str, undo: bool) -> Result<(), String> {
    let index_path = dir.join(package);
    let text = std::fs::read_to_string(&index_path)
        .map_err(|_| format!("the registry has no package `{package}`"))?;
    let mut found = false;
    let mut out = String::new();
    for raw in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut line = IndexLine::parse(raw)?;
        if line.version == version {
            line.yanked = !undo;
            found = true;
        }
        out.push_str(&line.render());
        out.push('\n');
    }
    if !found {
        return Err(format!("the registry has no `{package}` {version}"));
    }
    std::fs::write(&index_path, out).map_err(|e| format!("could not write the index: {e}"))
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

fn read_config(dir: &Path) -> Result<Config, String> {
    let bytes = std::fs::read(dir.join("config.json")).map_err(|_| {
        format!(
            "`{}` isn't a registry: it has no `config.json` (run `bynk registry init`)",
            dir.display()
        )
    })?;
    Config::parse(&bytes)
}

fn signing_key(path: &Path) -> Result<SigningKey, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("could not read the key `{}`: {e}", path.display()))?;
    SigningKey::from_pkcs8(&bytes)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
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
