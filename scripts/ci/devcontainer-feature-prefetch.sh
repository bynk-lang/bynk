#!/usr/bin/env bash
# Fill the Dev Container feature's local release cache before
# `devcontainer features test`, so the scenarios install Bynk without the
# anonymous github.com release route (it answers 504s under load: the feature
# test failed that way on 2026-11-09).
#
#   devcontainer-feature-prefetch.sh
#
# Writes features/src/bynk/ci-release-cache/ (gitignored): `latest` holds the
# tag GitHub marks latest, and `<tag>/` holds SHA256SUMS and this runner's
# archive for that tag and for every version a scenario in
# features/test/bynk/scenarios.json pins. The CLI copies the feature folder,
# cache included, into each scenario's build, and install.sh then reads only the
# cache. Only release assets go in it: no token reaches the image.
#
# Every download is authenticated and digest-checked by gh-release-fetch.sh.
# Needs GH_TOKEN (the job's github.token), curl and jq.
set -euo pipefail

: "${GH_TOKEN:?devcontainer-feature-prefetch: GH_TOKEN must be set (pass github.token)}"

REPO="bynk-lang/bynk"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
FETCH="$ROOT/scripts/ci/gh-release-fetch.sh"
CACHE="$ROOT/features/src/bynk/ci-release-cache"
SCENARIOS="$ROOT/features/test/bynk/scenarios.json"

die() { echo "::error::devcontainer-feature-prefetch: $*" >&2; exit 1; }

case "$(uname -m)" in
  x86_64 | amd64) target="x86_64-unknown-linux-gnu" ;;
  aarch64 | arm64) target="aarch64-unknown-linux-gnu" ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac

# The tag GitHub itself marks latest: what the feature's anonymous
# releases/latest redirect resolves to for an end user.
latest=$(curl -fsSL --proto '=https' --tlsv1.2 --retry 5 --retry-delay 3 \
           -H "Authorization: Bearer $GH_TOKEN" \
           -H "Accept: application/vnd.github+json" \
           -H "X-GitHub-Api-Version: 2022-11-28" \
           "https://api.github.com/repos/$REPO/releases/latest" | jq -r .tag_name) ||
  die "could not resolve the latest release of $REPO"
[[ "$latest" == v[0-9]* ]] || die "unexpected latest tag '$latest'"

# The pinned versions, normalised to a tag the way install.sh does it.
mapfile -t pinned < <(jq -r '.[].features.bynk.version // empty' "$SCENARIOS" |
                        sed -e '/^latest$/d' -e 's/^v//' -e 's/^/v/' | sort -u)

rm -rf "$CACHE"
mkdir -p "$CACHE"
printf '%s\n' "$latest" > "$CACHE/latest"
for tag in "$latest" "${pinned[@]}"; do
  [[ -f "$CACHE/$tag/SHA256SUMS" ]] && continue
  for asset in SHA256SUMS "bynk-$tag-$target.tar.gz"; do
    "$FETCH" "$REPO" "$tag" "$asset" "$CACHE/$tag"
  done
done
echo "release cache: latest=$latest; tags: $(cd "$CACHE" && ls -d v*/ | tr -d / | tr '\n' ' ')"
