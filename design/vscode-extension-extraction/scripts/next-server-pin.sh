#!/usr/bin/env bash
# Move the extension's server pin to a just-published compiler release tag,
# when it should move (#1673). Called by pin-bump.yml.
#
# The pin names the last shipped *release*, so:
#   - a pre-release tag (`v1.2.3-rc1`) never becomes the pin;
#   - the pin never moves backwards (a re-run, or a late tag of an older
#     version, is a no-op).
# Any other tag shape is refused outright.
#
# Usage: scripts/next-server-pin.sh TAG [PACKAGE_JSON]
# Edits PACKAGE_JSON (default package.json) in place when the pin moves, and
# prints `move=true` or `move=false` on stdout (a GITHUB_OUTPUT line).
set -euo pipefail

tag="${1:?usage: scripts/next-server-pin.sh TAG [PACKAGE_JSON]}"
pkg="${2:-$(dirname "$0")/../package.json}"

if ! printf '%s' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
	echo "::error::$tag is not a vX.Y.Z[-pre] release tag" >&2
	exit 1
fi
pin="$(sed -nE 's/^  "bynkServerVersion": "([^"]+)".*/\1/p' "$pkg")"
if [ -z "$pin" ]; then
	echo "::error::no bynkServerVersion line in $pkg" >&2
	exit 1
fi

if [ "${tag#*-}" != "$tag" ]; then
	echo "::notice::$tag is a pre-release; the pin stays at $pin" >&2
	echo "move=false"
elif [ "$(printf '%s\n%s\n' "$pin" "$tag" | sort -V | tail -n1)" = "$pin" ]; then
	echo "::notice::the pin is already $pin (not behind $tag)" >&2
	echo "move=false"
else
	sed -i.bak -E 's/^(  "bynkServerVersion": )"[^"]+"/\1"'"$tag"'"/' "$pkg"
	rm -f "$pkg.bak"
	echo "::notice::moving the pin $pin -> $tag" >&2
	echo "move=true"
fi
