#!/usr/bin/env bash
# The publish API, end to end (R2, #2336, decision F): build an empty signed
# registry with the `bynk registry` tooling (an offline root, an online
# delegate, an `api`), seed `bynk dev`'s local bucket with its `config.json`,
# serve the service with the delegate and two operator tokens as secrets, and
# hold it to the protocol with a full `bynk registry conformance` run.
#
#   BYNK=<path to bynk> scripts/check-publish.sh
#
# Needs the admission module (scripts/admission-module.sh) and python3.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
bynk="${BYNK:-bynk}"
work="$(mktemp -d)"
trap 'kill "${dev_pid:-}" 2>/dev/null || true; wait "${dev_pid:-}" 2>/dev/null || true; rm -rf "$work"' EXIT
export BYNK_HOME="$work/home"
org=conformance-fixture

[[ -f "$here/src/registry/bynk_registry_core.wasm" ]] || {
  echo "the admission module isn't built: run scripts/admission-module.sh" >&2
  exit 1
}

# 1. An empty registry: a root kept offline, a delegate the service signs
# with, and the publish API at `api` beside the index.
root="$("$bynk" registry init "$work/reg" --id bynk-fixture --key-out "$work/root.key" | grep -o 'ed25519:[^"]*')"
public="$("$bynk" registry delegate "$work/reg" --key "$work/root.key" --delegate-out "$work/online.key" 2>&1 | grep -o 'ed25519:[A-Za-z0-9+/=]*' | head -1)"
python3 - "$work/reg/config.json" <<'PY'
import json, sys
p = sys.argv[1]
c = json.load(open(p))
c["api"] = "api"
open(p, "w").write(json.dumps(c, indent=2) + "\n")
PY

# 2. Two operator tokens: one for the conformance organisation, and a narrow
# one for another (`auth.scope`). The service holds only their hashes.
full="$("$bynk" registry token "$work/tokens.toml" --org "$org" 2>/dev/null | tail -1)"
narrow="$("$bynk" registry token "$work/tokens.toml" --org someone-else 2>/dev/null | tail -1)"
tokens="$(python3 - "$work/tokens.toml" <<'PY'
import json, sys, tomllib
print(json.dumps(tomllib.load(open(sys.argv[1], "rb"))["token"], separators=(",", ":")))
PY
)"

# 3. Seed `bynk dev`'s local bucket with the registry (just its config.json).
persist="$work/persist"
(cd "$work" && "$here/seed/seed.sh" "$work/reg" --bucket registry-store-store --local --persist-to "$persist")

# 4. Serve it with its secrets, and wait until it answers.
"$bynk" dev "$here" --ready-json "$work/ready.json" -- \
  --persist-to "$persist" \
  --var "REGISTRY_SIGNING_KEY:$(base64 -w0 <"$work/online.key")" \
  --var "REGISTRY_SIGNING_PUBLIC:$public" \
  --var "REGISTRY_TOKENS:$tokens" \
  >"$work/dev.log" 2>&1 &
dev_pid=$!
for _ in $(seq 1 120); do
  [[ -s "$work/ready.json" ]] && break
  kill -0 "$dev_pid" 2>/dev/null || { cat "$work/dev.log" >&2; exit 1; }
  sleep 1
done
[[ -s "$work/ready.json" ]] || { echo "bynk dev never became ready" >&2; cat "$work/dev.log" >&2; exit 1; }
url="$(grep -o 'http://[^"]*' "$work/ready.json" | head -1)"

if [[ -n "${BYNK_HOLD:-}" ]]; then
  # Debugging: leave the service up with its tokens, until interrupted.
  echo "url=$url full=$full narrow=$narrow work=$work"
  wait "$dev_pid"
fi

# 5. Hold it to the protocol: a full run publishes, retries, conflicts,
# refuses, yanks and resolves a throwaway package.
status=0
BYNK_REGISTRY_CONFORMANCE_TOKEN="$full" NARROW="$narrow" \
  "$bynk" registry conformance "$url" --org "$org" --key "$root" --narrow-token-env NARROW || status=$?
if [[ $status -ne 0 ]]; then
  echo "--- bynk dev log ---" >&2
  tail -60 "$work/dev.log" >&2
fi
exit $status
