#!/usr/bin/env bash
# Set the single repo version across every versioned manifest.
#
# The repo carries one version while everything lives together (see
# design/README.md "Versioning & release"). The sites that must agree:
#
#   - Cargo.toml  [workspace.package] version
#   - Cargo.toml  in-workspace dependency requirements (path + version)
#   - vscode-bynk/package.json        version (NOT bynkServerVersion: that pin
#     names the last *shipped* release, and release.yml's server-pin job moves
#     it once a release is published — #1673)
#   - tree-sitter-bynk/package.json   version
#   - the lockfiles (Cargo.lock, both package-lock.json)
#   - the Bynk Book's current-version banners (the Book, MAJOR.MINOR) —
#     guarded by bynkc/tests/doc_version.rs (Workstream 0)
#
# The release workflow's verify job refuses a tag that doesn't match all of
# them, so run this in the increment PR and land the bump with the increment.
#
# Usage: scripts/bump-version.sh X.Y.Z
set -euo pipefail

ver="${1:?usage: scripts/bump-version.sh X.Y.Z}"
if ! printf '%s' "$ver" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
	echo "error: '$ver' is not a bare X.Y.Z version" >&2
	exit 1
fi

cd "$(dirname "$0")/.."

# Cargo workspace: [workspace.package] version + the in-workspace dependency
# requirements ({ path = "...", version = "..." }).
sed -i.bak -E \
	-e 's/^version = "[^"]+"/version = "'"$ver"'"/' \
	-e 's/(path = "[^"]+", version = )"[^"]+"/\1"'"$ver"'"/' \
	Cargo.toml
rm Cargo.toml.bak
cargo update --workspace --quiet

# npm manifests. Targeted sed (not `npm version`/JSON rewrite) so the bump
# never reformats the files. The extension's bynkServerVersion pin is left
# alone: it names a release that exists, which this version does not yet.
sed -i.bak -E 's/^(  "version": )"[^"]+"/\1"'"$ver"'"/' \
	vscode-bynk/package.json tree-sitter-bynk/package.json
rm vscode-bynk/package.json.bak tree-sitter-bynk/package.json.bak

# Sync the npm lockfiles to the new manifest versions. These calls exist to
# *rewrite* the lockfiles after the version bump, so they cannot use the
# reproducible `npm ci` form (which consumes the very lockfile they produce);
# `--package-lock-only` touches no node_modules and `--ignore-scripts` runs no
# install scripts. Scorecard Pinned-Dependencies flags these as unpinned npm
# invocations, but there is no hash/lockfile pin for a lockfile-generating call.
# Accepted risk, #828.
(cd vscode-bynk && npm install --package-lock-only --ignore-scripts >/dev/null)
(cd tree-sitter-bynk && npm install --package-lock-only --ignore-scripts >/dev/null)

# The book's current-version banners track MAJOR.MINOR (patches are
# non-language). Only these "current version" banners move — never the
# historical "introduced in vX" feature markers. `bynkc/tests/doc_version.rs`
# fails CI if any banner drifts from the released version.
mm="${ver%.*}"
book="site/src/content/docs/book"
# The Tooling index moved to the Developer Documentation surface (slice 5);
# its "currently v…" banner lives there now.
docs="site/src/content/docs/docs"
sed -i.bak -E "s/currently v[0-9]+\.[0-9]+/currently v$mm/" \
	"$book/index.md" "$docs/tooling/index.md"
sed -i.bak -E "s/written against v[0-9]+\.[0-9]+/written against v$mm/" \
	"$book/about/versioning-and-roadmap.md"
sed -i.bak -E "s/written against \*\*v[0-9]+\.[0-9]+\*\*/written against **v$mm**/" \
	"$book/reference/changelog.md"
sed -i.bak -E "s/current version, v[0-9]+\.[0-9]+/current version, v$mm/" \
	"$book/spec/scope.md" "$book/spec/appendix-version-history.md" "$book/spec/index.md"
find "$book" "$docs" -name '*.bak' -delete

# The LLM primer states the current version too (#2464), so an agent reading it
# knows which toolchain its syntax matches. `doc_version.rs` pins it as well.
sed -i.bak -E "s/currently v[0-9]+\.[0-9]+/currently v$mm/" site/public/llms.txt
rm -f site/public/llms.txt.bak

# The crate READMEs show a `[dependencies]` example pinned to the current
# MAJOR.MINOR (e.g. `bynk-syntax = "0.66"`); keep those in step with the release
# so they don't drift. Only the in-workspace `bynk-*` dep lines are rewritten.
sed -i.bak -E "s/^([[:space:]]*bynk[a-z-]* = )\"[0-9]+\.[0-9]+\"/\1\"$mm\"/" \
	bynk*/README.md
find . -maxdepth 2 -name 'README.md.bak' -delete

# Note: site/public/llms-full.txt is no longer regenerated here — it is a build
# artifact, produced by the site's `prebuild` npm hook on every `npm run build`
# (CI + deploy), never committed.

echo "bumped to $ver:"
grep -m1 '^version' Cargo.toml
node -p '"vscode-bynk        " + require("./vscode-bynk/package.json").version + " (server pin " + require("./vscode-bynk/package.json").bynkServerVersion + ")"'
node -p '"tree-sitter-bynk   " + require("./tree-sitter-bynk/package.json").version'
echo "docs banners      v$mm"
