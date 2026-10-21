#!/usr/bin/env bash
# Check that the VS Code extension's server pin names a GitHub Release a fresh
# VSIX can actually download its language server from (#1673).
#
# `bynkServerVersion` (vscode-bynk/package.json) names the last *shipped*
# release; `src/server.ts` downloads `SHA256SUMS` and `bynkc-lsp-<target>[.exe]`
# from it. So the release must exist, must not be a draft (a draft's assets are
# not publicly downloadable), and must carry the sums file and a server binary
# for every target release.yml builds (its `binaries` matrix is the list).
#
# From TOOLCHAIN_FROM on, a release also carries the raw `bynk-<target>` and
# `bynkc-<target>` binaries the extension downloads as its managed toolchain
# (vscode-bynk/src/toolchain.ts), so a pin at or after it must have those too.
# An earlier pin predates them and is held to the server only: requiring them
# there would fail the release that first publishes them, since that release's
# own check runs against the previous release's pin.
#
# Usage: scripts/check-server-pin.sh [--strict]
#
# A failure to *ask* GitHub (an API error, a rate limit) is a warning by
# default, so a GitHub outage never turns an unrelated PR red; `--strict` (the
# release `verify` job) makes it an error, since a release must not ship blind.
# Needs `gh` with a token (GH_TOKEN) that can read the repo's releases.
set -euo pipefail

TOOLCHAIN_FROM="0.379.2"

strict=0
[ "${1:-}" = "--strict" ] && strict=1

cd "$(dirname "$0")/.."
repo="${GITHUB_REPOSITORY:-bynk-lang/bynk}"

pin="$(sed -nE 's/^  "bynkServerVersion": "([^"]+)".*/\1/p' vscode-bynk/package.json)"
if [ -z "$pin" ]; then
	echo "::error::no bynkServerVersion in vscode-bynk/package.json"
	exit 1
fi
# True when version $1 (MAJOR.MINOR.PATCH, optional leading v) is >= $2.
at_least() {
	[ "$(printf '%s\n%s\n' "${1#v}" "$2" | sort -t. -k1,1n -k2,2n -k3,3n | head -n1)" = "$2" ]
}

tools="bynkc-lsp"
if at_least "$pin" "$TOOLCHAIN_FROM"; then
	tools="bynkc-lsp bynk bynkc"
fi
targets="$(sed -nE 's/^ *target: ([a-z0-9_-]+) *$/\1/p' .github/workflows/release.yml)"
if [ -z "$targets" ]; then
	echo "::error::found no build targets in release.yml's binaries matrix"
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
for target in $targets; do
	exe=""
	case "$target" in *windows*) exe=".exe" ;; esac
	wants="SHA256SUMS"
	for tool in $tools; do
		wants="$wants $tool-$target$exe"
	done
	for want in $wants; do
		printf '%s\n' "$out" | grep -qx "$want" ||
			{ echo "::error::release $pin has no $want asset"; fail=1; }
	done
done
[ "$fail" = 0 ] && echo "server pin $pin: a published release with SHA256SUMS and ${tools// /, } for each target"
exit "$fail"
