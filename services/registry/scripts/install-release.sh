#!/usr/bin/env bash
# Install a released toolchain and admission module for a deploy (R5.3,
# #2392; track D11): the tag's `bynk` and `bynkc` for x86_64 Linux, and its
# `bynk_registry_core` module, each checked against the release's SHA256SUMS.
# Nothing is built from source.
#
#   GH_TOKEN=… scripts/install-release.sh <tag> <bin dir>
#
# The binaries go in <bin dir> (put it on PATH); the module goes beside the
# adapter that includes it, src/registry/bynk_registry_core.wasm.
set -euo pipefail
tag="$1"
bin="$2"
here="$(cd "$(dirname "$0")/.." && pwd)"
repo="${GITHUB_REPOSITORY:-bynk-lang/bynk}"
target=x86_64-unknown-linux-gnu
archive="bynk-$tag-$target.tar.gz"
module="bynk_registry_core-$tag.wasm"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

gh release download "$tag" --repo "$repo" --dir "$work" \
  --pattern SHA256SUMS --pattern "$archive" --pattern "$module"
for asset in "$archive" "$module"; do
  [[ -f "$work/$asset" ]] || { echo "install-release: \`$tag\` has no \`$asset\`" >&2; exit 1; }
  grep -qE "  $asset\$" "$work/SHA256SUMS" || { echo "install-release: SHA256SUMS doesn't list \`$asset\`" >&2; exit 1; }
done
(cd "$work" && grep -E "  ($archive|$module)\$" SHA256SUMS | sha256sum --check --strict -) || {
  echo "install-release: \`$tag\`'s assets don't match its SHA256SUMS" >&2
  exit 1
}
mkdir -p "$bin"
tar -xzf "$work/$archive" -C "$work"
install -m 0755 "$work/bynk-$tag-$target/bynk" "$work/bynk-$tag-$target/bynkc" "$bin/"
install -m 0644 "$work/$module" "$here/src/registry/bynk_registry_core.wasm"
echo "install-release: bynk $tag and its admission module, checked against SHA256SUMS"
