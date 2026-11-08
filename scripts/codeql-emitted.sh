#!/usr/bin/env bash
# CodeQL over the TypeScript the compiler EMITS (#255).
#
# Compiles every `examples/*` project and every canary under
# `.github/codeql/bynk-emitted/canaries/` for the `workers` target into a
# staging dir, builds one JavaScript/TypeScript CodeQL database over it, and
# runs the default `javascript-code-scanning` suite plus the Bynk query pack
# (`.github/codeql/bynk-emitted/`): the stock SSRF and open-redirect queries
# re-run with models of the emitted Workers runtime in scope.
#
# The canaries are deliberately vulnerable. Each must be flagged by the query
# `canaries/expected.tsv` names, at a location inside its own emitted tree; if
# one is not, the models have stopped seeing Bynk's runtime and a clean scan of
# the examples means nothing, so this exits 1.
#
# Out of band only: run nightly by codeql-emitted.yml, never on a PR, the merge
# queue or a release.
#
# Usage: scripts/codeql-emitted.sh [OUT_DIR]
#   OUT_DIR  where to write stage/, db/ and the SARIF (default: a fresh temp dir)
# Env:
#   CODEQL   the codeql CLI (default: codeql on PATH)
#   BYNK     the bynk driver (default: bynk on PATH)
#   THREADS  evaluator threads (default: 0 = one per core)
#
# Outputs, in OUT_DIR:
#   results.sarif           everything, canaries included
#   results-examples.sarif  canary results removed: what gets uploaded, so the
#                           deliberate bugs never become open alerts
#   summary.md              per-canary verdict + per-query counts on the examples
set -euo pipefail

cd "$(dirname "$0")/.."
repo=$PWD
pack_root=.github/codeql/bynk-emitted
canaries=$pack_root/canaries
CODEQL=${CODEQL:-codeql}
BYNK=${BYNK:-bynk}
THREADS=${THREADS:-0}

out=${1:-$(mktemp -d "${TMPDIR:-/tmp}/codeql-emitted.XXXXXX")}
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$out/stage
db=$out/db
rm -rf "$stage" "$db"
mkdir -p "$stage/examples" "$stage/canaries"

compile() { # compile <project dir> <dest>
  "$BYNK" compile "$1" -o "$2" --target workers >/dev/null
}

echo "== compiling (workers target) into $stage"
for ex in examples/*/; do
  ex=${ex%/}
  [ -f "$ex/bynk.toml" ] || continue
  compile "$ex" "$stage/examples/$(basename "$ex")"
done
while IFS=$'\t' read -r name _; do
  case $name in '' | '#'*) continue ;; esac
  compile "$canaries/$name" "$stage/canaries/$name"
done <"$canaries/expected.tsv"
# Source maps and debug sidecars are noise to the extractor.
find "$stage" \( -name '*.map' -o -name '*.bynkdbg.json' \) -delete

echo "== building the database"
"$CODEQL" database create "$db" --language=javascript-typescript \
  --source-root="$stage" --build-mode=none --threads="$THREADS" --overwrite >/dev/null

echo "== analysing"
"$CODEQL" database analyze "$db" \
  codeql/javascript-queries:codeql-suites/javascript-code-scanning.qls \
  "$repo/$pack_root/queries" \
  --format=sarif-latest --sarif-category=emitted-ts \
  --output="$out/results.sarif" --threads="$THREADS" >/dev/null

# Uploaded copy: drop results located in the canaries' trees.
jq '.runs |= map(.results |= map(select(
      (.locations[0].physicalLocation.artifactLocation.uri // "") | startswith("canaries/") | not)))' \
  "$out/results.sarif" >"$out/results-examples.sarif"

# Verdict. Not `{ … } | tee`: a pipeline runs the block in a subshell, which
# would swallow `fail=1`.
fail=0
{
  echo "## Canaries"
  echo
  echo "| canary | expected query | flagged |"
  echo "|---|---|---|"
  while IFS=$'\t' read -r name rule; do
    case $name in '' | '#'*) continue ;; esac
    hits=$(jq --arg r "$rule" --arg p "canaries/$name/" '[.runs[].results[]
        | select(.ruleId == $r)
        | select(.locations[0].physicalLocation.artifactLocation.uri | startswith($p))] | length' \
      "$out/results.sarif")
    if [ "$hits" -gt 0 ]; then
      echo "| \`$name\` | \`$rule\` | yes ($hits) |"
    else
      echo "| \`$name\` | \`$rule\` | **NO** |"
      fail=1
    fi
  done <"$canaries/expected.tsv"
  echo
  echo "## Findings on examples/* (canaries excluded)"
  echo
  n=$(jq '[.runs[].results[]] | length' "$out/results-examples.sarif")
  if [ "$n" -eq 0 ]; then
    echo "None."
  else
    echo "| query | count |"
    echo "|---|---|"
    jq -r '[.runs[].results[].ruleId] | group_by(.) | map("| `\(.[0])` | \(length) |") | .[]' \
      "$out/results-examples.sarif"
  fi
} >"$out/summary.md"
cat "$out/summary.md"

if [ "$fail" -ne 0 ]; then
  echo "codeql-emitted: a canary was not flagged; the models no longer see Bynk's runtime (see $out/summary.md)" >&2
  exit 1
fi
echo "codeql-emitted: every canary flagged. Output in $out"
