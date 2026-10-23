#!/usr/bin/env bash
# The deploy workflow's ref guard (first-party registry R5.3, #2392; track
# D11): production deploys only a published release, from a tag on `main`; a
# release candidate may go to staging, never to production.
#
#   scripts/deploy-guard.sh check <tag>
#       Checks the tag's shape, then gathers the facts from GitHub with `gh`
#       (GH_TOKEN; the repository is GITHUB_REPOSITORY, default
#       bynk-lang/bynk) and decides. Prints, for $GITHUB_OUTPUT:
#         production=true|false   whether the tag may also go to production
#         sha=<commit>            the commit the tag named when checked: the
#                                 jobs deploy this, not the tag, which can move
#         archive_sha256=<hex>    the release's toolchain archive and
#         module_sha256=<hex>     admission module, as SHA256SUMS listed them
#                                 when checked: a later replaced asset fails
#   scripts/deploy-guard.sh decide <tag> <release> <on-main>
#       The decision alone, on given facts (scripts/test-deploy-guard.sh):
#       <release> is `published`, `prerelease`, `draft` or `missing`;
#       <on-main> is `yes` or `no`.
#
# A refusal exits 1 and says why on stderr.
set -euo pipefail

target=x86_64-unknown-linux-gnu

refuse() {
  echo "deploy-guard: $*" >&2
  exit 1
}

# `shape <tag>`: `final` or `candidate`, or a refusal.
shape() {
  if [[ "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo final
  elif [[ "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-rc(\.?[0-9]+)?$ ]]; then
    echo candidate
  else
    refuse "\`$1\` isn't a release tag (vX.Y.Z, or vX.Y.Z-rc.N for staging)"
  fi
}

decide() {
  local tag="$1" release="$2" on_main="$3" kind
  kind="$(shape "$tag")" || exit 1
  case "$release" in
    published) ;;
    prerelease) [[ "$kind" == candidate ]] || refuse "\`$tag\`'s release is marked a pre-release, but its tag isn't a candidate" ;;
    draft) refuse "\`$tag\`'s release is still a draft" ;;
    missing) refuse "\`$tag\` has no GitHub release" ;;
    *) refuse "unknown release state \`$release\`" ;;
  esac
  [[ "$on_main" == yes ]] || refuse "\`$tag\` isn't on \`main\` (D11: releases tag main)"
  if [[ "$kind" == candidate ]]; then echo "production=false"; else echo "production=true"; fi
}

# `sum <SHA256SUMS file> <asset>`: the asset's listed digest, exactly matched.
sum() {
  awk -v name="$2" '$2 == name { print $1; found = 1 } END { exit !found }' "$1" ||
    refuse "SHA256SUMS doesn't list \`$2\`"
}

check() {
  local tag="$1" repo="${GITHUB_REPOSITORY:-bynk-lang/bynk}" release sha status on_main=no sums
  shape "$tag" >/dev/null || exit 1
  release="$(gh release view "$tag" --repo "$repo" --json isDraft,isPrerelease \
    --jq 'if .isDraft then "draft" elif .isPrerelease then "prerelease" else "published" end' 2>/dev/null)" ||
    release=missing
  # Pin the commit now: everything after is checked, and deployed, by SHA.
  sha="$(gh api "repos/$repo/commits/refs/tags/$tag" --jq .sha 2>/dev/null)" || refuse "no tag \`$tag\`"
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]] || refuse "\`$tag\` didn't resolve to a commit"
  # On main: main is that commit, or ahead of it and behind it by nothing.
  status="$(gh api "repos/$repo/compare/$sha...main" --jq .status 2>/dev/null)" || status=missing
  [[ "$status" == ahead || "$status" == identical ]] && on_main=yes
  decide "$tag" "$release" "$on_main"
  sums="$(mktemp)"
  gh release download "$tag" --repo "$repo" --pattern SHA256SUMS --output "$sums" --clobber 2>/dev/null ||
    refuse "\`$tag\`'s release has no SHA256SUMS"
  # A refusal inside $(…) exits only the subshell: each is checked here.
  local archive module
  archive="$(sum "$sums" "bynk-$tag-$target.tar.gz")" || exit 1
  module="$(sum "$sums" "bynk_registry_core-$tag.wasm")" || exit 1
  rm -f "$sums"
  echo "sha=$sha"
  echo "archive_sha256=$archive"
  echo "module_sha256=$module"
}

case "${1:-}" in
  check) [[ $# == 2 ]] || refuse "usage: deploy-guard.sh check <tag>"; check "$2" ;;
  decide) [[ $# == 4 ]] || refuse "usage: deploy-guard.sh decide <tag> <release> <on-main>"; decide "$2" "$3" "$4" ;;
  *) refuse "usage: deploy-guard.sh check <tag> | decide <tag> <release> <on-main>" ;;
esac
