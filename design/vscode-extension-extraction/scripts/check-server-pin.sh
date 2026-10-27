#!/usr/bin/env bash
# Check that `bynkServerVersion` names a compiler release this extension can
# actually use: published (not a draft) and carrying everything the extension
# downloads — SHA256SUMS, the editor contract, the web assets, and the server
# and toolchain binaries for every target.
#
# Usage: scripts/check-server-pin.sh [--strict]
#
# A failure to *ask* GitHub (an API error, a rate limit) is a warning by
# default so a GitHub outage never turns an unrelated PR red; `--strict` (the
# release workflow) makes it an error. Needs `gh` and a token (GH_TOKEN).
set -euo pipefail

repo="bynk-lang/bynk"
targets="aarch64-apple-darwin aarch64-unknown-linux-gnu x86_64-apple-darwin x86_64-pc-windows-msvc x86_64-unknown-linux-gnu"

strict=0
[ "${1:-}" = "--strict" ] && strict=1

cd "$(dirname "$0")/.."
pin="$(sed -nE 's/^  "bynkServerVersion": "([^"]+)".*/\1/p' package.json)"
if [ -z "$pin" ]; then
	echo "::error::no bynkServerVersion in package.json"
	exit 1
fi

if ! out="$(gh release view "$pin" --repo "$repo" --json isDraft,assets \
	-q '"draft=\(.isDraft)", .assets[].name' 2>&1)"; then
	if printf '%s\n' "$out" | grep -q "release not found"; then
		echo "::error::bynkServerVersion $pin is not a published release of $repo"
		exit 1
	fi
	level=warning
	[ "$strict" = 1 ] && level=error
	echo "::$level::could not ask GitHub about release $pin: $out"
	exit "$strict"
fi

fail=0
if printf '%s\n' "$out" | grep -qx "draft=true"; then
	echo "::error::release $pin is a draft, so its assets can't be downloaded"
	fail=1
fi
wants="SHA256SUMS bynk-editor-contract-$pin.json bynk-web-$pin.tar.gz"
for target in $targets; do
	exe=""
	case "$target" in *windows*) exe=".exe" ;; esac
	for tool in bynkc-lsp bynk bynkc; do
		wants="$wants $tool-$target$exe"
	done
done
for want in $wants; do
	printf '%s\n' "$out" | grep -qx "$want" ||
		{ echo "::error::release $pin has no $want asset"; fail=1; }
done
[ "$fail" = 0 ] && echo "server pin $pin: a published release with everything the extension downloads"
exit "$fail"
