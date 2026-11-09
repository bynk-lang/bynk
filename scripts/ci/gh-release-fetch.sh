#!/usr/bin/env bash
# Download one GitHub release asset through the authenticated REST API and
# check it against the sha256 digest GitHub records for it.
#
#   gh-release-fetch.sh <owner/repo> <tag> <asset-name> <out-dir>
#   gh-release-fetch.sh --latest <owner/repo> <tag-prefix>
#
# The first form writes <out-dir>/<asset-name>. The second prints the newest
# non-draft, non-prerelease tag that starts with <tag-prefix> (for a version
# range like nextest's 0.9 line).
#
# Why not the `github.com/<repo>/releases/download/...` URL: that is the
# anonymous web route, and CI runners share their egress IPs, so it answers
# 504s (and 429s) when GitHub is shedding unauthenticated load: tool installs
# and npm postinstalls failed that way across CI on 2026-11-09. This goes via
# api.github.com/repos/<repo>/releases/assets/<id> with the job's token instead.
# curl drops the Authorization header when the API redirects to the asset
# host (a different host), so the token never leaves api.github.com.
#
# Needs GH_TOKEN (the job's `github.token` is enough: every repo read here is
# public), curl, sha256sum or shasum, and jq or python3. Not `gh`: the
# self-hosted Linux runners have none (see run-unless-sibling-fails.sh).
set -euo pipefail

die() { echo "::error::gh-release-fetch: $*" >&2; exit 1; }
: "${GH_TOKEN:?gh-release-fetch: GH_TOKEN must be set (pass github.token)}"

# --retry alone retries timeouts and 408/429/5xx, not a 403: a token out of
# rate limit stays out for the hour, so retrying that only wastes time.
api() {
  curl -fsSL --proto '=https' --tlsv1.2 --retry 5 --retry-delay 3 \
    -H "Authorization: Bearer $GH_TOKEN" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "$@"
}

# json <query-name> [arg]: read JSON on stdin and answer one of the two
# questions this script asks, with jq when present, else python3. On Windows
# both write CRLF, and a trailing \r on the digest fails the sha256 compare
# with two identical-looking hashes; strip it.
json() {
  json_raw "$@" | tr -d '\r'
}
json_raw() {
  if command -v jq >/dev/null 2>&1; then
    case "$1" in
      asset) jq -r --arg n "$2" '.assets[] | select(.name == $n) | "\(.id) \(.digest // "")"' ;;
      tags) jq -r '.[] | select((.draft | not) and (.prerelease | not)) | .tag_name' ;;
    esac
  elif command -v python3 >/dev/null 2>&1; then
    python3 -I -c '
import json, sys
data = json.load(sys.stdin)
if sys.argv[1] == "asset":
    for a in data["assets"]:
        if a["name"] == sys.argv[2]:
            print(a["id"], a.get("digest") or "")
else:
    for r in data:
        if not r["draft"] and not r["prerelease"]:
            print(r["tag_name"])
' "$@"
  else
    die "needs jq or python3"
  fi
}

if [[ "${1:-}" == "--latest" ]]; then
  repo="${2:?usage: gh-release-fetch.sh --latest <owner/repo> <tag-prefix>}"
  prefix="${3:?usage: gh-release-fetch.sh --latest <owner/repo> <tag-prefix>}"
  # The newest 100 releases reach past any one tool's recent line even in a
  # monorepo like rustsec/rustsec. They come in creation order, so a backport
  # on an older line could come first: take the highest by version sort (only
  # purely numeric remainders, so a `-rc` or odd tag never wins).
  listing=$(api -H "Accept: application/vnd.github+json" \
              "https://api.github.com/repos/$repo/releases?per_page=100") ||
    die "could not list $repo's releases"
  version=$(json tags <<<"$listing" \
            | awk -v p="$prefix" 'index($0, p) == 1 { print substr($0, length(p) + 1) }' \
            | grep -E '^[0-9]+(\.[0-9]+)*$' | sort -V | tail -n1 || true)
  [[ -n "$version" ]] || die "no release of $repo with a tag starting $prefix"
  printf '%s%s\n' "$prefix" "$version"
  exit 0
fi

repo="${1:?usage: gh-release-fetch.sh <owner/repo> <tag> <asset> <out-dir>}"
tag="${2:?usage: gh-release-fetch.sh <owner/repo> <tag> <asset> <out-dir>}"
asset="${3:?usage: gh-release-fetch.sh <owner/repo> <tag> <asset> <out-dir>}"
out="${4:?usage: gh-release-fetch.sh <owner/repo> <tag> <asset> <out-dir>}"

release=$(api -H "Accept: application/vnd.github+json" \
            "https://api.github.com/repos/$repo/releases/tags/$tag") ||
  die "could not look up $repo release $tag"
read -r id digest < <(json asset "$asset" <<<"$release") || true
[[ -n "${id:-}" ]] || die "$repo $tag has no asset named $asset"
# GitHub records a sha256 digest for every release asset; without one there
# is nothing to check the download against, so refuse rather than skip.
[[ "${digest:-}" == sha256:* ]] || die "$repo $tag: $asset has no sha256 digest to verify against"

mkdir -p "$out"
echo "downloading $repo $tag $asset (authenticated)"
# No anonymous fallback: CI downloads go only through the authenticated API.
# If the token's rate limit is spent (403/429), fail loudly rather than
# quietly retrying on the route this script exists to avoid.
api -H "Accept: application/octet-stream" -o "$out/$asset" \
  "https://api.github.com/repos/$repo/releases/assets/$id" ||
  die "could not download $repo $tag $asset through the API (rate limit spent, or the asset is gone)"

if command -v sha256sum >/dev/null 2>&1; then
  got=$(sha256sum "$out/$asset" | cut -d' ' -f1)
else
  got=$(shasum -a 256 "$out/$asset" | cut -d' ' -f1)
fi
[[ "$got" == "${digest#sha256:}" ]] ||
  die "$asset: sha256 $got does not match the release's ${digest#sha256:}"
echo "verified sha256 $got"
