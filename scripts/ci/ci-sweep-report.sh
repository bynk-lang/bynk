#!/usr/bin/env bash
# Report-building half of .github/workflows/ci-sweep.yml (#3143).
#
#   ci-sweep-report.sh   # findings on stdin, one per line, each ending in
#                        # `<!-- ci-sweep:SHA -->`
#
# Reports each SHA once on a single "CI runs missing" tracking issue: it opens
# the issue, or comments on the open one, skipping SHAs the issue body or its
# comments already carry. It only reports; it never re-runs anything.
# Needs `gh`, GH_REPO, GITHUB_SERVER_URL and GITHUB_RUN_ID. Kept out of the
# workflow YAML so xtask/tests/ci_sweep_report.rs can run it against a stub `gh`.
set -euo pipefail

findings=$(cat)
if [ -z "$findings" ]; then
  echo "Every open PR head and queue group has a ci.yml run."
  exit 0
fi
printf '%s\n' "$findings"

title="CI runs missing"
number=$(gh issue list --state open --author app/github-actions --search "in:title \"$title\"" --json number,title \
  --jq "[.[] | select(.title == \"$title\")][0].number // empty")
known=""
[ -z "$number" ] || known=$(gh issue view "$number" --json body,comments --jq '.body, (.comments[].body)')
fresh=""
while IFS= read -r line; do
  [ -n "$line" ] || continue
  marker=${line##*<!-- }
  grep -qF -- "<!-- $marker" <<<"$known" || fresh="$fresh$line"$'\n'
done <<<"$findings"
if [ -z "$fresh" ]; then
  echo "Already reported."
  exit 0
fi
run_url="$GITHUB_SERVER_URL/$GH_REPO/actions/runs/$GITHUB_RUN_ID"
body="${fresh}
Found by the [CI sweep]($run_url). It only reports; nothing was re-run."
if [ -n "$number" ]; then
  gh issue comment "$number" --body "$body"
else
  gh issue create --title "$title" --body "$body

GitHub sometimes drops the event that starts CI (#3119). Close this issue once the listed items are sorted out; the next finding opens a new one."
fi
