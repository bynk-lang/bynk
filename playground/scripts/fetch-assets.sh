#!/usr/bin/env bash
# Stage the compiler's browser artefacts from a published Bynk release into
# src/vendor/, instead of building the compiler (build-wasm.sh, build-grammar.sh).
# The playground then runs the release it is pinned to.
#
#   scripts/fetch-assets.sh [tag]      # default: the tag in playground/BYNK_VERSION
#
# The tarball is checked against the release's SHA256SUMS before it is unpacked.
# BYNK_RELEASE_BASE_URL overrides the download base (a file:// directory holding
# bynk-web-<tag>.tar.gz and SHA256SUMS, for testing a build before it is released).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
tag="${1:-}"
if [[ -z "$tag" ]]; then
  [[ -f "$here/BYNK_VERSION" ]] || { echo "no tag given and $here/BYNK_VERSION is missing" >&2; exit 1; }
  tag="$(tr -d '[:space:]' < "$here/BYNK_VERSION")"
fi
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "not a release tag: $tag" >&2; exit 1; }

name="bynk-web-$tag.tar.gz"
base="${BYNK_RELEASE_BASE_URL:-https://github.com/bynk-lang/bynk/releases/download/$tag}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/$name" "$base/$name"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
want="$(awk -v n="$name" '$2 == n { print $1 }' "$tmp/SHA256SUMS")"
[[ -n "$want" ]] || { echo "$name is not listed in SHA256SUMS" >&2; exit 1; }
got="$(sha256sum "$tmp/$name" | awk '{ print $1 }')"
[[ "$got" == "$want" ]] || { echo "checksum mismatch for $name: want $want, got $got" >&2; exit 1; }

rm -rf "$here/src/vendor"
mkdir -p "$here/src/vendor"
tar -xzf "$tmp/$name" -C "$here/src/vendor" --strip-components=1
# The web-tree-sitter runtime wasm comes from this app's own dependency.
cp -f "$here/node_modules/web-tree-sitter/web-tree-sitter.wasm" "$here/src/vendor/tree-sitter.wasm"
echo "staged Bynk $tag browser assets in src/vendor/"
