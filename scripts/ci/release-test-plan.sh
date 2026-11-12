#!/usr/bin/env bash
# #3391: decide which OS legs of release.yml's `test` matrix still need to run.
#
#   release-test-plan.sh   # env: GH_TOKEN REPO SHA EVENT; writes `matrix` to
#                          # $GITHUB_OUTPUT and a note to $GITHUB_STEP_SUMMARY
#
# A release tag must point at a commit already on `main` (release.yml's
# `verify`), so ci.yml has normally run the suite on that exact commit. This
# looks those runs up and drops the legs they already passed. It FAILS CLOSED:
# a leg is dropped only on an explicit `success`; no run, a cancelled or failed
# job, a skipped one or a lookup error all leave the leg in the matrix. A
# manual dispatch always runs every leg.
#
# What counts as "the suite already passed on this OS" (see ci.yml's `test`,
# `test-os` and `test-system`):
#
#   - Linux: `Test suite (ubuntu-latest)` (unit + integration tiers) AND
#     `System tests (Linux)` (the system tier, with workerd required). Together
#     they are the whole `ci` profile the release runs.
#   - Windows and macOS: their own `Test suite (<os>)` leg AND the Linux pair.
#     Off Linux ci.yml runs `ci-os`, the `ci` profile minus the corpus-wide
#     tsc/node passes, which are OS-independent and run on Linux in the system
#     tier. So an OS leg only stands in for the release's leg if Linux passed
#     those too.
#
#     The release used to be (with the nightly) the one gate that ran those
#     tsc/node passes (tsc_verify, behaviour_fixtures, examples,
#     example_tests_behaviour) on Windows and macOS, so a non-Linux leg is
#     dropped only if the LATEST nightly (`schedule` run on main, the full `ci`
#     profile on every OS) also passed its own `Test suite (<os>)` leg. The
#     nightly is not for the tagged commit and may be up to a day old: it
#     guards the OS-specific behaviour of those passes, not this commit's code.
#     No nightly, a failed or in-progress one, or a lookup error keeps the leg.
#
# Which run answers, per job (for the commit's own legs): the commit's `push` run on main, then its
# `merge_group` run (the queue passes the Linux legs and the push run then
# skips them). The first run that has the job decides: a leg that ran and did
# not succeed on the push run is never overridden by the queue's earlier pass.
# A `chore(stamp):` commit by the stamp bot changes no code and its run
# deliberately skips legs its parent passed (see ci.yml's `test-os` step), so
# for that commit the parent's runs are consulted for any job its own runs
# lack. Kept out of the workflow YAML so xtask/tests/release_test_plan.rs can
# run it against a stub `gh`.
set -uo pipefail

: "${REPO:?}" "${SHA:?}" "${EVENT:?}"
all='["ubuntu-latest","macos-latest","windows-latest"]'

emit() {
  local matrix=$1 note=$2
  echo "Release test plan: $note (run: $matrix)"
  if [ -n "${GITHUB_OUTPUT:-}" ]; then echo "matrix=$matrix" >> "$GITHUB_OUTPUT"; fi
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '### Release test plan\n\n%s\n\nLegs run: `%s`\n' "$note" "$matrix" >> "$GITHUB_STEP_SUMMARY"
  fi
  exit 0
}

if [ "$EVENT" != "push" ]; then
  emit "$all" "not a tag push; every leg runs."
fi

# `ok` when the run's job passed, `ran` when it ran and did not, `none` when
# the run or the job is absent or the lookup failed. A job skipped by the
# run's own gating counts as absent.
job_state() {
  run_state "head_sha=$1&event=$2&branch=main" "$3"
}

# As `job_state`, for the newest run matching the query string `$1`.
run_state() {
  local query=$1 name=$2 run_id
  run_id="$(gh api "repos/$REPO/actions/workflows/ci.yml/runs?$query&per_page=1" \
    --jq '.workflow_runs[0].id // empty' 2>/dev/null || true)"
  [ -n "$run_id" ] || { echo none; return; }
  gh api "repos/$REPO/actions/runs/$run_id/jobs?per_page=100" \
    --jq "[.jobs[] | select(.name == \"$name\" and .conclusion != \"skipped\") | .conclusion]
          | if length == 0 then \"none\" elif all(. == \"success\") then \"ok\" else \"ran\" end" \
    2>/dev/null || echo none
}

candidates=("$SHA")
commit="$(gh api "repos/$REPO/commits/$SHA" \
  --jq '[(.commit.message | split("\n")[0]), .commit.author.name, (.parents[0].sha // "")] | @tsv' 2>/dev/null || true)"
IFS=$'\t' read -r subject author parent <<< "$commit"
if [[ "${subject:-}" == "chore(stamp):"* ]] && [ "${author:-}" = "bynk-stamp[bot]" ] && [ -n "${parent:-}" ]; then
  candidates+=("$parent")
fi

green() {
  local name=$1 sha event state
  for sha in "${candidates[@]}"; do
    for event in push merge_group; do
      state="$(job_state "$sha" "$event" "$name")"
      case "$state" in
        ok) return 0 ;;
        ran) return 1 ;;
      esac
    done
  done
  return 1
}

linux_ok=0
if green "Test suite (ubuntu-latest)" && green "System tests (Linux)"; then linux_ok=1; fi

run='[]'
for os in ubuntu-latest macos-latest windows-latest; do
  ok=0
  if [ "$linux_ok" = "1" ]; then
    if [ "$os" = "ubuntu-latest" ]; then
      ok=1
    elif green "Test suite ($os)" \
      && [ "$(run_state "event=schedule&branch=main" "Test suite ($os)")" = "ok" ]; then
      ok=1
    fi
  fi
  if [ "$ok" = "0" ]; then run="$(jq -c --arg os "$os" '. + [$os]' <<< "$run")"; fi
done

if [ "$run" = "[]" ]; then
  emit "$run" "every leg already passed on $SHA in ci.yml; the suite is not re-run."
elif [ "$run" = "$all" ]; then
  emit "$run" "no complete green ci.yml run found for $SHA; every leg runs."
else
  emit "$run" "the other legs already passed on $SHA in ci.yml."
fi
