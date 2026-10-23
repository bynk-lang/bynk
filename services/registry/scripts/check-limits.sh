#!/usr/bin/env bash
# Abuse limits, end to end (R5.1, #2390): serve the service under `bynk dev`,
# whose rate limiting counts as Cloudflare's does, and check that each limit
# answers `429 rate_limited` once it is spent, and that an organisation past
# its quota gets `413 quota_exceeded` from the real client.
#
#   BYNK=<path to bynk> scripts/check-limits.sh
#
# Needs the admission module (scripts/admission-module.sh) and python3.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
bynk="${BYNK:-bynk}"
work="$(mktemp -d)"
trap 'kill "${dev_pid:-}" 2>/dev/null || true; wait "${dev_pid:-}" 2>/dev/null || true; rm -rf "$work"' EXIT
export BYNK_HOME="$work/home"
org=conformance-fixture
tiny=tiny-fixture

[[ -f "$here/src/registry/bynk_registry_core.wasm" ]] || {
  echo "the admission module isn't built: run scripts/admission-module.sh" >&2
  exit 1
}

fail() {
  echo "check-limits: $*" >&2
  echo "--- bynk dev log ---" >&2
  tail -60 "$work/dev.log" >&2
  exit 1
}

# 1. An empty registry, operator tokens for two organisations, and a quota of
# 10 bytes for one of them.
root="$("$bynk" registry init "$work/reg" --id bynk-fixture --key-out "$work/root.key" | grep -o 'ed25519:[^"]*')"
public="$("$bynk" registry delegate "$work/reg" --key "$work/root.key" --delegate-out "$work/online.key" 2>&1 | grep -o 'ed25519:[A-Za-z0-9+/=]*' | head -1)"
python3 - "$work/reg/config.json" <<'PY'
import json, sys
p = sys.argv[1]
c = json.load(open(p))
c["api"] = "api"
open(p, "w").write(json.dumps(c, indent=2) + "\n")
PY
full="$("$bynk" registry token "$work/tokens.toml" --org "$org" 2>/dev/null | tail -1)"
small="$("$bynk" registry token "$work/tokens.toml" --org "$tiny" 2>/dev/null | tail -1)"
# A token of its own for spending the write limit: the publishes above count
# against theirs.
spender="$("$bynk" registry token "$work/tokens.toml" --org "$org" 2>/dev/null | tail -1)"
tokens="$(python3 - "$work/tokens.toml" <<'PY'
import json, sys, tomllib
print(json.dumps(tomllib.load(open(sys.argv[1], "rb"))["token"], separators=(",", ":")))
PY
)"
persist="$work/persist"
(cd "$work" && "$here/seed/seed.sh" "$work/reg" --bucket registry-store-store --local --persist-to "$persist")

"$bynk" dev "$here" --ready-json "$work/ready.json" -- \
  --persist-to "$persist" \
  --var "REGISTRY_SIGNING_KEY:$(base64 -w0 <"$work/online.key")" \
  --var "REGISTRY_SIGNING_PUBLIC:$public" \
  --var "REGISTRY_TOKENS:$tokens" \
  --var "REGISTRY_QUOTAS:[{\"org\":\"$tiny\",\"bytes\":10}]" \
  >"$work/dev.log" 2>&1 &
dev_pid=$!
for _ in $(seq 1 120); do
  [[ -s "$work/ready.json" ]] && break
  kill -0 "$dev_pid" 2>/dev/null || { cat "$work/dev.log" >&2; exit 1; }
  sleep 1
done
[[ -s "$work/ready.json" ]] || fail "bynk dev never became ready"
url="$(grep -o 'http://[^"]*' "$work/ready.json" | head -1)"

# 2. The quota: the real client publishes into the organisation with room, and
# is refused past the 10-byte quota of the other.
routing="$(printf '[registries.first-party]\nindex = "%s/"\norganisations = ["%s", "%s"]\nkey = "%s"\n' "$url" "$org" "$tiny" "$root")"
publish() { # publish <org> <token>: the real client, publishing <org>/greeting 0.1.0
  local pkg="$work/client/$1"
  mkdir -p "$pkg/src"
  printf '[package]\nname = "%s/greeting"\nversion = "0.1.0"\n\n%s\n' "$1" "$routing" >"$pkg/bynk.toml"
  printf 'commons greeting\n\nfn greet(name: String) -> String {\n  "hello \\(name)"\n}\n' >"$pkg/src/greeting.bynk"
  (cd "$pkg" && BYNK_REGISTRY_FIRST_PARTY_TOKEN="$2" "$bynk" publish)
}
publish "$org" "$full" >/dev/null 2>&1 || fail "a publish within the default quota was refused"
publish "$tiny" "$small" >/dev/null 2>"$work/quota.err" && fail "a publish past the quota was accepted"
grep -q "quota_exceeded" "$work/quota.err" || { cat "$work/quota.err" >&2; fail "the refusal wasn't quota_exceeded"; }
echo "check-limits: a publish past its organisation's quota is 413 quota_exceeded"

# `spend <count> <limit> <curl args…>`: send up to <count> requests and check
# that `429 rate_limited` comes, and not before <limit> of them. A request is
# counted in fixed windows, so one that straddles a window may take up to
# twice the limit.
spend() {
  local count="$1" limit="$2" n status
  shift 2
  for n in $(seq 1 "$count"); do
    status="$(curl -s -o "$work/last.json" -w '%{http_code}' "$@")"
    if [[ "$status" == 429 ]]; then
      grep -q '"rate_limited"' "$work/last.json" || fail "a 429 without rate_limited: $(cat "$work/last.json")"
      ((n > limit)) || fail "rate-limited after $n requests, within the limit of $limit"
      return 0
    fi
  done
  fail "no 429 in $count requests (limit $limit): $*"
}

# 3. A token's writes: 60 a minute. Junk bodies, refused by admission, still count.
spend 130 60 -X PUT -H "authorization: Bearer $spender" -H 'content-type: application/vnd.bynk.tree-v1' \
  --data-binary junk "$url/api/$org/limited/0.0.1"
echo "check-limits: a token past 60 writes a minute is 429 rate_limited"

# 4. The account API from one IP: 60 a minute.
spend 130 60 -H 'cf-connecting-ip: 10.9.9.9' "$url/account/orgs/$org"
echo "check-limits: an IP past 60 account requests a minute is 429 rate_limited"

# 5. Writes from one IP, with no token: the 600-a-minute flood guard.
spend 1300 600 -X PUT -H 'cf-connecting-ip: 10.8.8.8' -H 'content-type: application/vnd.bynk.tree-v1' \
  --data-binary junk "$url/api/$org/flood/0.0.1"
echo "check-limits: an IP past 600 writes a minute is 429 rate_limited"
