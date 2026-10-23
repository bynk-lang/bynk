#!/usr/bin/env bash
# The deploy guard's decisions (R5.3, #2392), on given facts.
#
#   scripts/test-deploy-guard.sh
set -euo pipefail
guard="$(cd "$(dirname "$0")" && pwd)/deploy-guard.sh"
failed=0

accepts() { # accepts <production=…> <tag> <release> <on-main>
  local want="$1" got
  shift
  if got="$("$guard" decide "$@" 2>/dev/null)" && [[ "$got" == "$want" ]]; then
    echo "  pass  accepts $* ($want)"
  else
    echo "  FAIL  accepts $*: got \`${got:-a refusal}\`, wanted \`$want\`"
    failed=1
  fi
}
refuses() { # refuses <tag> <release> <on-main>
  if "$guard" decide "$@" >/dev/null 2>&1; then
    echo "  FAIL  refuses $*: it was accepted"
    failed=1
  else
    echo "  pass  refuses $*"
  fi
}

accepts production=true v1.2.3 published yes
accepts production=false v1.2.3-rc.1 prerelease yes
accepts production=false v1.2.3-rc1 published yes
refuses main published yes
refuses refs/heads/main published yes
refuses v1.2 published yes
refuses v1.2.3-beta.1 prerelease yes
refuses v1.2.3 missing yes
refuses v1.2.3 draft yes
refuses v1.2.3-rc.1 draft yes
refuses v1.2.3 prerelease yes
refuses v1.2.3 published no
refuses v1.2.3-rc.1 prerelease no

exit "$failed"
