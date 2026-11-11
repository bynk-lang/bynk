#!/usr/bin/env bash
# Install binaryen (wasm-opt) from its pinned GitHub release, downloaded
# through the authenticated API (gh-release-fetch.sh, which says why) and
# checked against the sha256 recorded below, then put its bin/ on PATH for
# later steps (GITHUB_PATH).
#
#   install-binaryen.sh
#
# Replaces `apt-get install binaryen`: on 2026-11-11 the hosted runners could
# not resolve Ubuntu's package mirrors for three hours, and that one step
# failed every Rust-touching PR and emptied the merge queue.
#
# The version is the one that apt step installed (binaryen 120 on the Ubuntu
# 26.04 image), so wasm-opt's output, and the size budgets measured after it,
# do not move. To bump it, change VERSION and the digest together: the digest
# is the asset's published `<asset>.sha256`. Only the platform CI uses is
# recorded; add a case to add one.
set -euo pipefail

VERSION=120

die() { echo "::error::install-binaryen: $*" >&2; exit 1; }
: "${GITHUB_PATH:?install-binaryen adds to GITHUB_PATH}"
: "${RUNNER_TEMP:?install-binaryen unpacks into RUNNER_TEMP}"
fetch="$(dirname "$0")/gh-release-fetch.sh"

case "${RUNNER_OS:-}-${RUNNER_ARCH:-}" in
  Linux-X64)
    arch_os=x86_64-linux
    sha256=ddb097af51d1bdb17300d986b0de7d97422f1933dedb4c9eda3510e0bf4076cc ;;
  *) die "no recorded binaryen $VERSION digest for ${RUNNER_OS:-?}-${RUNNER_ARCH:-?}" ;;
esac
asset="binaryen-version_$VERSION-$arch_os.tar.gz"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
"$fetch" WebAssembly/binaryen "version_$VERSION" "$asset" "$work" "$sha256"

dest="$RUNNER_TEMP/binaryen-$VERSION"
rm -rf "$dest"
mkdir -p "$dest"
tar -xzf "$work/$asset" -C "$dest" --strip-components=1
[[ -x "$dest/bin/wasm-opt" ]] || die "binaryen $VERSION has no bin/wasm-opt"

# Check the binary runs and is the pinned release before anything relies on it.
got="$("$dest/bin/wasm-opt" --version)"
[[ "$got" == *"version $VERSION"* ]] || die "wasm-opt reports '$got', expected version $VERSION"
echo "$dest/bin" >> "$GITHUB_PATH"
echo "installed $got -> $dest/bin"
