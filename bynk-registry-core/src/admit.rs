//! Admission (first-party registry track §3.4): the rules every registry —
//! the reference server, `bynk registry add`, and the hosted service — applies
//! to an uploaded archive, in memory.
//!
//! [`admit`] unpacks the upload with the hardened [`unpack`], reads its own
//! `bynk.toml` (which must name a `[package]` with a `version`, and, when the
//! caller names one, the very `org/package` and version the request named),
//! refuses path and git dependencies, collects the npm packages its adapters
//! require, and returns the canonical re-packed archive with its checksum.
//! What it does not do: sign, check the index for an existing version, or write
//! anything. The caller owns that I/O.

use std::collections::BTreeSet;

use bynk_project::lock::adapter_requires;
use bynk_project::manifest::{DependencySource, dependencies_from, package_config_from};

use crate::archive::{Tree, pack, unpack};

/// A dependency or npm requirement in an index line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Requirement {
    pub package: String,
    pub range: String,
}

/// Why a registry refused an upload: a stable `code` for the publish API's
/// JSON error, and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

/// An upload that passed admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
    /// `org/package`.
    pub name: String,
    pub version: String,
    /// The canonical archive: the upload's tree, re-packed.
    pub archive: Vec<u8>,
    /// The canonical-tree checksum of `archive` (`sha256-<base64>`).
    pub checksum: String,
    /// Its registry dependencies, by `version` range.
    pub deps: Vec<Requirement>,
    /// The npm packages its adapters require.
    pub npm: Vec<Requirement>,
}

fn refuse(message: impl Into<String>) -> Refusal {
    Refusal {
        code: "invalid_package",
        message: message.into(),
    }
}

/// Admit `upload`. When `expected` is `Some((name, version))`, the package must
/// be exactly that.
pub fn admit(upload: &[u8], expected: Option<(&str, &str)>) -> Result<Admitted, Refusal> {
    let tree = unpack(upload).map_err(|e| refuse(e.message))?;
    let text = tree
        .get("bynk.toml")
        .and_then(|b| std::str::from_utf8(b).ok())
        .ok_or_else(|| refuse("the package has no `bynk.toml`"))?;
    bynk_project::check_manifest_str(text)
        .map_err(|e| refuse(format!("its `bynk.toml` is invalid: {e}")))?;
    let doc: toml::Table = text
        .parse()
        .map_err(|_| refuse("its `bynk.toml` isn't TOML"))?;
    let package = doc
        .get("package")
        .and_then(|v| v.as_table())
        .and_then(|t| package_config_from(t).ok())
        .ok_or_else(|| refuse("it isn't a package: its `bynk.toml` has no `[package]`"))?;
    let version = package
        .version
        .clone()
        .ok_or_else(|| refuse("its `[package]` has no `version`, which a registry needs"))?;
    let name = format!("{}/{}", package.org, package.package);
    if let Some((want_name, want_version)) = expected
        && (want_name != name || want_version != version)
    {
        return Err(refuse(format!(
            "the upload is `{name}` {version}, but the request named `{want_name}` {want_version}"
        )));
    }
    let mut deps = BTreeSet::new();
    for spec in specs(&doc, "dependencies") {
        match spec.source {
            DependencySource::Version(range) => {
                deps.insert(Requirement {
                    package: spec.package,
                    range,
                });
            }
            _ => {
                return Err(refuse(format!(
                    "its dependency `{}` is a path or git dependency, which a registry refuses — publish it, and depend on it by `version`",
                    spec.alias
                )));
            }
        }
    }
    let (archive, checksum) = pack(&tree);
    Ok(Admitted {
        name,
        version,
        archive,
        checksum,
        deps: deps.into_iter().collect(),
        npm: npm_of(&tree, &doc),
    })
}

fn specs(doc: &toml::Table, table: &'static str) -> Vec<bynk_project::manifest::DependencySpec> {
    doc.get(table)
        .and_then(|v| v.as_table())
        .and_then(|t| dependencies_from(table, t).ok())
        .unwrap_or_default()
}

/// The npm packages the adapters in `tree` require, for its index line: the
/// `.bynk` files under the package's source roots (`[paths] include`, or
/// `src`, never a `tests` tree), skipping hidden directories.
fn npm_of(tree: &Tree, doc: &toml::Table) -> Vec<Requirement> {
    let roots = source_roots(doc);
    let mut out = BTreeSet::new();
    for (path, bytes) in tree.iter() {
        if !path.ends_with(".bynk") {
            continue;
        }
        let Some(rest) = roots.iter().find_map(|root| under(path, root)) else {
            continue;
        };
        // A hidden directory below a root is skipped; the file itself isn't.
        let mut parts: Vec<&str> = rest.split('/').collect();
        parts.pop();
        if parts.iter().any(|p| p.starts_with('.')) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        if let Some((_, requires)) = adapter_requires(text) {
            for (package, range) in requires {
                out.insert(Requirement { package, range });
            }
        }
    }
    out.into_iter().collect()
}

/// `path` relative to `root` (`""` is the package root), when it is inside it.
fn under<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    if root.is_empty() {
        Some(path)
    } else {
        path.strip_prefix(root)?.strip_prefix('/')
    }
}

/// The package's source roots as normalised `/`-relative prefixes: its
/// `[paths] include`, or `src`, without any `tests` tree. An absolute entry or
/// one climbing out of the package has no files in the archive and is dropped.
fn source_roots(doc: &toml::Table) -> Vec<String> {
    let include: Vec<String> = match doc
        .get("paths")
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("include"))
    {
        Some(toml::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Some(toml::Value::String(s)) => vec![s.clone()],
        _ => vec!["src".to_string()],
    };
    let mut out: Vec<String> = Vec::new();
    for entry in include {
        let entry = entry.replace('\\', "/");
        if entry.rsplit('/').find(|p| !p.is_empty()) == Some("tests") {
            continue;
        }
        let Some(root) = normalise(&entry) else {
            continue;
        };
        if !out.contains(&root) {
            out.push(root);
        }
    }
    out
}

/// A relative `/`-path with `.` and `..` resolved; `None` if absolute or it
/// climbs out of the package.
fn normalise(path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains(':') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(files: &[(&str, &str)]) -> Vec<u8> {
        let tree: Tree = files
            .iter()
            .map(|(p, b)| ((*p).to_string(), b.as_bytes().to_vec()))
            .collect();
        pack(&tree).0
    }

    const MANIFEST: &str = "[package]\nname = \"acme/probe\"\nversion = \"1.2.3\"\n";

    #[test]
    fn a_good_package_is_admitted_with_its_canonical_archive() {
        let up = upload(&[("bynk.toml", MANIFEST), ("src/a.bynk", "commons a\n")]);
        let got = admit(&up, Some(("acme/probe", "1.2.3"))).unwrap();
        assert_eq!(
            (got.name.as_str(), got.version.as_str()),
            ("acme/probe", "1.2.3")
        );
        assert_eq!(got.archive, up);
        assert!(got.deps.is_empty() && got.npm.is_empty());
    }

    #[test]
    fn a_mismatched_junk_or_path_dependency_upload_is_refused() {
        let good = upload(&[("bynk.toml", MANIFEST)]);
        for (up, want) in [
            (good.clone(), Some(("acme/other", "1.2.3"))),
            (good, Some(("acme/probe", "9.9.9"))),
            (b"not an archive".to_vec(), None),
            (
                upload(&[(
                    "bynk.toml",
                    &format!(
                        "{MANIFEST}[dependencies]\nh = {{ package = \"acme/h\", path = \"../h\" }}\n"
                    ),
                )]),
                None,
            ),
        ] {
            let err = admit(&up, want).unwrap_err();
            assert_eq!(err.code, "invalid_package");
        }
    }

    #[test]
    fn source_roots_follow_paths_include_and_skip_tests() {
        let doc: toml::Table =
            "[paths]\ninclude = [\"./src/\", \"lib\", \"tests\", \"../x\", \"/abs\"]\n"
                .parse()
                .unwrap();
        assert_eq!(source_roots(&doc), ["src", "lib"]);
        let doc: toml::Table = "[paths]\ninclude = \".\"\n".parse().unwrap();
        assert_eq!(source_roots(&doc), [""]);
    }
}
