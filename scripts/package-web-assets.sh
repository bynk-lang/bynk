#!/usr/bin/env bash
# Package the browser artefacts of one release: the bynk-wasm compiler module +
# its wasm-bindgen glue and types, and the tree-sitter grammar wasm + highlight
# query. The playground (bynk-lang/playground) consumes this tarball at a pinned
# version instead of building the compiler itself; the release attaches it to
# the GitHub Release. ci.yml's web-assets job checks it holds what the playground
# stages.
#
#   scripts/package-web-assets.sh <tag> [out-dir]
#
# <tag> is the release tag (v0.382.2); the asset is bynk-web-<tag>.tar.gz, one
# top-level directory of the same name, in out-dir (default: the repo root).
# Needs what build-wasm.sh and build-grammar-wasm.sh need (wasm32 target,
# wasm-bindgen-cli at the Cargo.lock version, `npm ci` in tree-sitter-bynk/).
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
tag="${1:?usage: package-web-assets.sh <tag> [out-dir]}"
dest="$(mkdir -p "${2:-$repo}" && cd "${2:-$repo}" && pwd)"
name="bynk-web-$tag"
stage="$(mktemp -d)/$name"
mkdir -p "$stage"

BYNK_WASM_OUT_DIR="$stage" bash "$repo/scripts/build-wasm.sh" --release
BYNK_GRAMMAR_OUT_DIR="$stage" bash "$repo/scripts/build-grammar-wasm.sh"
printf '%s\n' "$tag" > "$stage/VERSION"
cp "$repo/LICENSE-MIT" "$repo/LICENSE-APACHE" "$stage/"

tar -C "$(dirname "$stage")" -czf "$dest/$name.tar.gz" "$name"
echo "packaged $dest/$name.tar.gz"
