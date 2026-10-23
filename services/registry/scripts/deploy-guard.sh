#!/usr/bin/env bash
# The deploy workflow's ref guard (first-party registry R5.3, #2392; track
# D11): production deploys only a published release, from a tag on `main`; a
# release candidate may go to staging, never to production.
#
#   scripts/deploy-guard.sh check <tag>
#       Gathers the facts from GitHub with `gh` (GH_TOKEN; the repository is
#       GITHUB_REPOSITORY, default bynk-lang/bynk), then decides. Prints `production=true|false` for
#       $GITHUB_OUTPUT: whether the tag may also go to production.
#   scripts/deploy-guard.sh decide <tag> <release> <on-main>
#       The decision alone, on given facts (scripts/test-deploy-guard.sh):
#       <release> is `published`, `prerelease`, `draft` or `missing`;
#       <on-main> is `yes` or `no`.
#
# A refusal exits 1 and says why on stderr.
set -euo pipefail

refuse() {
  echo "deploy-guard: $*" >&2
  exit 1
}

decide() {
  local tag="$1" release="$2" on_main="$3" candidate=
  [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
    if [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-rc(\.?[0-9]+)?$ ]]; then candidate=1; else
      refuse "\`$tag\` isn't a release tag (vX.Y.Z, or vX.Y.Z-rc.N for staging)"
    fi
  case "$release" in
    published) ;;
    prerelease) [[ -n "$candidate" ]] || refuse "\`$tag\`'s release is marked a pre-release, but its tag isn't a candidate" ;;
    draft) refuse "\`$tag\`'s release is still a draft" ;;
    missing) refuse "\`$tag\` has no GitHub release" ;;
    *) refuse "unknown release state \`$release\`" ;;
  esac
  [[ "$on_main" == yes ]] || refuse "\`$tag\` isn't on \`main\` (D11: releases tag main)"
  if [[ -n "$candidate" ]]; then echo "production=false"; else echo "production=true"; fi
}

check() {
  local tag="$1" release on_main=no
  release="$(gh release view "$tag" --json isDraft,isPrerelease \
    --jq 'if .isDraft then "draft" elif .isPrerelease then "prerelease" else "published" end' 2>/dev/null)" ||
    release=missing
  # On main: main is the tag's commit, or ahead of it and behind it by nothing.
  local repo="${GITHUB_REPOSITORY:-bynk-lang/bynk}" status
  status="$(gh api "repos/$repo/compare/$tag...main" --jq .status 2>/dev/null)" || status=missing
  [[ "$status" == ahead || "$status" == identical ]] && on_main=yes
  decide "$tag" "$release" "$on_main"
}

case "${1:-}" in
  check) [[ $# == 2 ]] || refuse "usage: deploy-guard.sh check <tag>"; check "$2" ;;
  decide) [[ $# == 4 ]] || refuse "usage: deploy-guard.sh decide <tag> <release> <on-main>"; decide "$2" "$3" "$4" ;;
  *) refuse "usage: deploy-guard.sh check <tag> | decide <tag> <release> <on-main>" ;;
esac
