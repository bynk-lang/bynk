#!/usr/bin/env bash
# C4c (#2940, Decision B): run a command, and stop it early if a sibling job of
# this workflow run concludes `failure`.
#
#   run-unless-sibling-fails.sh "<sibling job name>" <command> [args...]
#
# GitHub can cancel a whole run but not one job, and a `needs:` edge from the
# system job to the unit job would serialise them (a full extra compile on the
# green path). So the system job runs in parallel and this watcher polls the
# run's jobs API; once the sibling has failed the result is red whatever the
# system tier says, and its runner minutes are better spent elsewhere.
#
# Needs GH_TOKEN, GITHUB_REPOSITORY and GITHUB_RUN_ID (all set by Actions) and
# `actions: read`. A lookup error is ignored (the watcher only ever stops a
# command early; it never makes a passing one fail). POLL_SECONDS (default 20)
# is the interval.
set -u

sibling="$1"
shift
export SIBLING_JOB="$sibling"

setsid "$@" &
cmd_pid=$!
stopped=0
marker="$(mktemp)"
rm -f "$marker"

errlog="$(mktemp)"
echo "sibling watcher: polling '$sibling' every ${POLL_SECONDS:-20}s"
(
  while kill -0 "$cmd_pid" 2>/dev/null; do
    sleep "${POLL_SECONDS:-20}"
    state="$(gh api "repos/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}/jobs?per_page=100" \
      --jq '[.jobs[] | select(.name == env.SIBLING_JOB) | .conclusion] | any(. == "failure")' 2>"$errlog" || true)"
    # A lookup that keeps failing would otherwise be invisible: say so once.
    if [ -s "$errlog" ] && [ -z "${warned:-}" ]; then
      echo "::warning::sibling watcher cannot read the run's jobs, so it cannot stop this job early: $(head -c 300 "$errlog")"
      warned=1
    fi
    if [ "$state" = "true" ]; then
      : > "$marker"
      # The command leads its own process group (setsid), so this reaches the
      # whole nextest tree.
      kill -TERM -- "-$cmd_pid" 2>/dev/null || kill -TERM "$cmd_pid" 2>/dev/null
      exit 0
    fi
  done
) &
watcher_pid=$!

wait "$cmd_pid"
status=$?
kill "$watcher_pid" 2>/dev/null
wait "$watcher_pid" 2>/dev/null

if [ -e "$marker" ]; then
  rm -f "$marker"
  echo "::error::stopped: the '$sibling' job failed, so this run is red regardless. Fix that job first."
  exit 1
fi
exit "$status"
