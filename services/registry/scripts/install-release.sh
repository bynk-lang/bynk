#!/usr/bin/env bash
# Install a released toolchain and admission module for a deploy (R5.3,
# #2392; track D11): the tag's `bynk` and `bynkc` for x86_64 Linux, and its
# `bynk_registry_core` module. Nothing is built from source.
#
#   GH_TOKEN=… ARCHIVE_SHA256=<hex> MODULE_SHA256=<hex> \
#     scripts/install-release.sh <tag> <bin dir>
#
# Each asset must match the digest given, which the deploy guard read from the
# release's SHA256SUMS when it checked the tag: an asset replaced since then
# fails here. The binaries go in <bin dir> (put it on PATH); the module goes
# beside the adapter that includes it, src/registry/bynk_registry_core.wasm.
set -euo pipefail
tag="$1"
bin="$2"
: "${ARCHIVE_SHA256:?the archive's digest, from the deploy guard}"
: "${MODULE_SHA256:?the module's digest, from the deploy guard}"
here="$(cd "$(dirname "$0")/.." && pwd)"
repo="${GITHUB_REPOSITORY:-bynk-lang/bynk}"
target=x86_64-unknown-linux-gnu
archive="bynk-$tag-$target.tar.gz"
module="bynk_registry_core-$tag.wasm"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

gh release download "$tag" --repo "$repo" --dir "$work" --pattern "$archive" --pattern "$module"
verify() { # verify <asset> <expected digest>
  [[ -f "$work/$1" ]] || { echo "install-release: \`$tag\` has no \`$1\`" >&2; exit 1; }
  local got
  got="$(sha256sum "$work/$1" | cut -d' ' -f1)"
  [[ "$got" == "$2" ]] || {
    echo "install-release: \`$1\` is $got, not the $2 the guard checked" >&2
    exit 1
  }
}
verify "$archive" "$ARCHIVE_SHA256"
verify "$module" "$MODULE_SHA256"
mkdir -p "$bin"
tar -xzf "$work/$archive" -C "$work"
install -m 0755 "$work/bynk-$tag-$target/bynk" "$work/bynk-$tag-$target/bynkc" "$bin/"
install -m 0644 "$work/$module" "$here/src/registry/bynk_registry_core.wasm"
echo "install-release: bynk $tag and its admission module, as the guard checked them"
