//! Guard: #1673 — the VS Code extension's `bynkServerVersion` pin names the last
//! *shipped* release, never a version that hasn't been released yet.
//!
//! The pin used to be rewritten to the workspace version on every increment by
//! `scripts/bump-version.sh`, so between releases it named a GitHub Release that
//! did not exist, and a freshly packaged VSIX could not download its server.
//! It is now moved only by `release.yml`'s `server-pin` job, after the release
//! is published. Whether that release exists needs the network, so
//! `scripts/check-server-pin.sh` checks it, in ci.yml's `server-pin` job. This offline half runs everywhere, including the
//! stamp's "Validate the stamped tree" step: the pin is a plain `vX.Y.Z`, it is
//! never ahead of the workspace, the bump script doesn't move it, and
//! `scripts/next-server-pin.sh` (release.yml's mover) only moves it forward, to
//! a release tag.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(path: &str) -> String {
    let path = repo().join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

/// `"X.Y.Z"` as numbers, or `None` if it isn't exactly three numeric parts.
fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.split('.').map(|p| p.parse::<u64>().ok());
    let parsed = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(parsed)
}

/// The string value of the first `"key": "…"` line in `json`.
fn json_str<'a>(json: &'a str, key: &str) -> &'a str {
    let needle = format!("\"{key}\": \"");
    let start = json
        .find(&needle)
        .unwrap_or_else(|| panic!("no `{key}` in vscode-bynk/package.json"))
        + needle.len();
    &json[start..start + json[start..].find('"').unwrap()]
}

fn workspace_version() -> (u64, u64, u64) {
    let toml = read("Cargo.toml");
    let line = toml
        .lines()
        .find(|l| l.starts_with("version = "))
        .expect("workspace version");
    parse(line.trim_start_matches("version = ").trim_matches('"')).expect("X.Y.Z workspace version")
}

#[test]
fn the_server_pin_is_a_released_version_not_ahead_of_the_workspace() {
    let pkg = read("vscode-bynk/package.json");
    let pin = json_str(&pkg, "bynkServerVersion");
    let Some(v) = pin.strip_prefix('v').and_then(parse) else {
        panic!(
            "bynkServerVersion is `{pin}`; it must be a release tag `vX.Y.Z` (a \
             pre-release tag is never the pin)"
        );
    };
    let ws = workspace_version();
    assert!(
        v <= ws,
        "bynkServerVersion {pin} is ahead of the workspace version {}.{}.{}, so it \
         names a release that can't exist yet. The pin is the last shipped \
         release; release.yml's server-pin job moves it after a release.",
        ws.0,
        ws.1,
        ws.2
    );
}

/// A scratch directory under the target dir, emptied first.
#[cfg(unix)]
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("server-pin")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copy `rel` (a file or a directory tree) from the repo to the same relative
/// path under `dest`.
#[cfg(unix)]
fn copy_in(rel: &str, dest: &std::path::Path) {
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        if from.is_dir() {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                copy(&entry.path(), &to.join(entry.file_name()));
            }
        } else {
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(from, to).unwrap_or_else(|e| panic!("copy {from:?}: {e}"));
        }
    }
    copy(&repo().join(rel), &dest.join(rel));
}

/// The bump script must leave the pin alone: rewriting it to the workspace
/// version is exactly the coupling #1673 removed. This runs the real script on
/// a copy of every file it edits, with `cargo`, `npm` and `node` stubbed out
/// (they only regenerate lockfiles and print a summary), and checks the
/// outcome, so the guard holds however a future rewrite is spelled.
#[test]
#[cfg(unix)]
fn the_bump_script_does_not_move_the_server_pin() {
    use std::os::unix::fs::PermissionsExt;

    let tree = scratch("bump");
    for rel in [
        "scripts/bump-version.sh",
        "Cargo.toml",
        "vscode-bynk/package.json",
        "tree-sitter-bynk/package.json",
        "site/src/content/docs",
        "site/public/llms.txt",
    ] {
        copy_in(rel, &tree);
    }
    for entry in std::fs::read_dir(repo()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        if name.starts_with("bynk") && repo().join(&name).join("README.md").is_file() {
            copy_in(&format!("{name}/README.md"), &tree);
        }
    }
    let bin = tree.join("stub-bin");
    std::fs::create_dir_all(&bin).unwrap();
    for tool in ["cargo", "npm", "node"] {
        let path = bin.join(tool);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let before = read("vscode-bynk/package.json");
    let pin = json_str(&before, "bynkServerVersion").to_string();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = std::process::Command::new("bash")
        .arg(tree.join("scripts/bump-version.sh"))
        .arg("9999.0.0")
        .env("PATH", path)
        .output()
        .expect("run bump-version.sh");
    assert!(
        out.status.success(),
        "bump-version.sh failed on the scratch tree. If it now edits a file this \
         test doesn't copy in, add it above.\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let after = std::fs::read_to_string(tree.join("vscode-bynk/package.json")).unwrap();
    assert_eq!(
        json_str(&after, "version"),
        "9999.0.0",
        "non-vacuity: the bump did move the extension's own version"
    );
    assert_eq!(
        json_str(&after, "bynkServerVersion"),
        pin,
        "scripts/bump-version.sh moved bynkServerVersion. The pin names the last \
         shipped release, and only release.yml's server-pin job moves it."
    );
}

/// `scripts/next-server-pin.sh`, which release.yml's `server-pin` job runs
/// with the stamp App's push token in reach: run against each tag shape on a
/// copy of `package.json`, with the pin at `v0.290.0`.
#[test]
#[cfg(unix)]
fn next_server_pin_moves_only_forward_to_a_release_tag() {
    let dir = scratch("next");
    let pkg = dir.join("package.json");
    let original = read("vscode-bynk/package.json");
    let current = json_str(&original, "bynkServerVersion");
    let seeded = original.replacen(
        &format!("\"bynkServerVersion\": \"{current}\""),
        "\"bynkServerVersion\": \"v0.290.0\"",
        1,
    );
    // (tag, exit ok, stdout, the pin afterwards)
    let cases = [
        ("v0.291.0", true, "move=true", "v0.291.0"),
        ("v0.290.10", true, "move=true", "v0.290.10"),
        ("v0.290.0", true, "move=false", "v0.290.0"),
        ("v0.289.9", true, "move=false", "v0.290.0"),
        ("v0.300.0-rc1", true, "move=false", "v0.290.0"),
        ("v0.291.0+b1", false, "", "v0.290.0"),
        ("v0.291.0rc1", false, "", "v0.290.0"),
        ("latest", false, "", "v0.290.0"),
    ];
    for (tag, ok, stdout, pin_after) in cases {
        std::fs::write(&pkg, &seeded).unwrap();
        let out = std::process::Command::new("bash")
            .arg(repo().join("scripts/next-server-pin.sh"))
            .arg(tag)
            .arg(&pkg)
            .output()
            .expect("run next-server-pin.sh");
        assert_eq!(out.status.success(), ok, "{tag}: exit status");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            stdout,
            "{tag}: output"
        );
        let after = std::fs::read_to_string(&pkg).unwrap();
        assert_eq!(
            json_str(&after, "bynkServerVersion"),
            pin_after,
            "{tag}: pin"
        );
    }
}
