#!/usr/bin/env bash
# Seed the registry's bucket from a `bynk registry` directory (first-party
# registry track R1, #2334, decision C).
#
# The directory is built — and signed — offline with the `bynk registry`
# tooling (`init`, `delegate`, `add --key <delegate>`); this script only copies
# it. It carries no keys. Every file becomes an object keyed by its path
# relative to the directory, so the bucket mirrors the static layout exactly:
# `config.json`, `<org>/<package>`, `dl/sha256/<hex>`. Dot-directories (a
# `.git` checkout, say) are skipped.
#
#   seed.sh <registry-dir> --bucket <name>                       # a real bucket
#   seed.sh <registry-dir> --bucket <name> --local --persist-to <dir>
#                                                                # `bynk dev`'s local bucket
set -euo pipefail

usage() { echo "usage: seed.sh <registry-dir> --bucket <name> [--local --persist-to <dir>]" >&2; exit 2; }

[[ $# -ge 1 ]] || usage
dir="$1"; shift
bucket=""; local_args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --bucket) bucket="${2:-}"; shift 2 ;;
    --local) local_args+=(--local); shift ;;
    --persist-to) local_args+=(--persist-to "${2:-}"); shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$bucket" ]] || usage
[[ -f "$dir/config.json" ]] || { echo "seed.sh: \`$dir\` isn't a registry (no config.json)" >&2; exit 1; }

wrangler="${WRANGLER:-wrangler}"
count=0
while IFS= read -r -d '' file; do
  key="${file#"$dir"/}"
  "$wrangler" r2 object put "$bucket/$key" --file "$file" "${local_args[@]}" >/dev/null
  count=$((count + 1))
done < <(find "$dir" -name '.*' -prune -o -type f -print0 | sort -z)
echo "seed.sh: seeded $count object(s) into \`$bucket\`"
