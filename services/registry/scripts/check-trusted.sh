#!/usr/bin/env bash
# Trusted publishing, end to end (R4, #2381, decision E): serve the registry
# with a fake GitHub (scripts/fake-github.py) and a fake GitHub Actions OIDC
# issuer (scripts/fake-oidc.mjs) behind the `Workload` actor's `trustFrom`. An
# owner registers a trusted publisher; a token with the right repository id,
# workflow and environment exchanges for a publish token, and the full
# `bynk registry conformance` run passes with it. Then every mismatch is
# refused: workflow, repository id, environment, audience, expiry, a blocked
# organisation, and a removed publisher.
#
#   BYNK=<path to bynk> scripts/check-trusted.sh
#
# Needs the admission module (scripts/admission-module.sh), curl, python3 and node.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
bynk="${BYNK:-bynk}"
work="$(mktemp -d)"
trap 'kill "${dev_pid:-}" "${fake_pid:-}" "${oidc_pid:-}" 2>/dev/null || true; wait "${dev_pid:-}" "${fake_pid:-}" "${oidc_pid:-}" 2>/dev/null || true; rm -rf "$work"' EXIT
export BYNK_HOME="$work/home"
org=conformance-fixture

[[ -f "$here/src/registry/bynk_registry_core.wasm" ]] || {
  echo "the admission module isn't built: run scripts/admission-module.sh" >&2
  exit 1
}

fail() {
  echo "check-trusted: $*" >&2
  echo "--- bynk dev log ---" >&2
  tail -60 "$work/dev.log" >&2 || true
  exit 1
}
# `field <json> <key>`: one top-level field of a JSON object.
field() {
  python3 -c 'import json,sys; print(json.loads(sys.argv[1])[sys.argv[2]])' "$1" "$2"
}

# 1. A fake GitHub and a fake OIDC issuer, each on a free port.
python3 "$here/scripts/fake-github.py" "$work/fake.port" &
fake_pid=$!
node "$here/scripts/fake-oidc.mjs" "$work/oidc.port" &
oidc_pid=$!
for _ in $(seq 1 50); do [[ -s "$work/fake.port" && -s "$work/oidc.port" ]] && break; sleep 0.1; done
github="http://127.0.0.1:$(cat "$work/fake.port")"
oidc="http://127.0.0.1:$(cat "$work/oidc.port")"

# 2. An empty registry with a delegate and an `api`, and one operator token
# that can `operate` (and nothing else).
root="$("$bynk" registry init "$work/reg" --id bynk-fixture --key-out "$work/root.key" | grep -o 'ed25519:[^"]*')"
public="$("$bynk" registry delegate "$work/reg" --key "$work/root.key" --delegate-out "$work/online.key" 2>&1 | grep -o 'ed25519:[A-Za-z0-9+/=]*' | head -1)"
python3 - "$work/reg/config.json" <<'PY'
import json, sys
p = sys.argv[1]
c = json.load(open(p))
c["api"] = "api"
open(p, "w").write(json.dumps(c, indent=2) + "\n")
PY
operator="bynk_$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
operator_table="$(python3 -c 'import hashlib,json,sys; print(json.dumps([{"sha256": hashlib.sha256(sys.argv[1].encode()).hexdigest(), "organisations": [], "can": ["operate"]}]))' "$operator")"
persist="$work/persist"
(cd "$work" && "$here/seed/seed.sh" "$work/reg" --bucket registry-store-store --local --persist-to "$persist")

# 3. Serve it, with the fake GitHub as GitHub.
"$bynk" dev "$here" --ready-json "$work/ready.json" -- \
  --persist-to "$persist" \
  --var "REGISTRY_SIGNING_KEY:$(base64 -w0 <"$work/online.key")" \
  --var "REGISTRY_SIGNING_PUBLIC:$public" \
  --var "REGISTRY_TOKENS:$operator_table" \
  --var "GITHUB_CLIENT_ID:fake-client" \
  --var "GITHUB_BASE:$github" \
  --var "GITHUB_API_BASE:$github" \
  --var "REGISTRY_OIDC_TRUST:{\"issuer\":\"$oidc\",\"jwks\":\"$oidc/jwks\"}" \
  >"$work/dev.log" 2>&1 &
dev_pid=$!
for _ in $(seq 1 120); do
  [[ -s "$work/ready.json" ]] && break
  kill -0 "$dev_pid" 2>/dev/null || { cat "$work/dev.log" >&2; exit 1; }
  sleep 1
done
[[ -s "$work/ready.json" ]] || fail "bynk dev never became ready"
url="$(grep -o 'http://[^"]*' "$work/ready.json" | head -1)"

# `call <method> <path> <token|-> [json]`: the status, then the body, on two lines.
call() {
  local auth=()
  [[ "$3" != "-" ]] && auth=(-H "authorization: Bearer $3")
  if [[ $# -ge 4 ]]; then
    curl -s -o "$work/body" -w '%{http_code}' -X "$1" "${auth[@]}" -H 'content-type: application/json' --data "$4" "$url$2"
  else
    curl -s -o "$work/body" -w '%{http_code}' -X "$1" "${auth[@]}" "$url$2"
  fi
  echo
  cat "$work/body"
}
expect() { # expect <status> <method> <path> <token> [json]
  local want="$1"; shift
  local got; got="$(call "$@")"
  [[ "$(head -1 <<<"$got")" == "$want" ]] || fail "$2 $3: expected $want, got $got"
  tail -n +2 <<<"$got"
}

# 4. Sign in through the device flow, and claim the organisations.
sign_in() {
  local started poll answer
  started="$(expect 200 POST /account/login -)"
  poll="$(field "$started" poll)"
  expect 400 POST /account/login/poll - "{\"poll\":\"$poll\"}" >/dev/null
  answer="$(expect 200 POST /account/login/poll - "{\"poll\":\"$poll\"}")"
  field "$answer" token
}
ann="$(sign_in)"
expect 201 POST "/account/orgs/$org" "$ann" '{"block":false}' >/dev/null
expect 201 POST /account/orgs/someone-else "$ann" '{"block":false}' >/dev/null
expect 201 POST /account/orgs/held "$ann" '{"block":false}' >/dev/null

# 5. Register trusted publishers: the repository resolves to GitHub's id.
publisher='{"repository":"conformance-fixture/widgets","workflow":".github/workflows/release.yml","environment":"release","can":["publish","yank"]}'
registered="$(expect 201 POST "/account/orgs/$org/trusted" "$ann" "$publisher")"
[[ "$(field "$registered" repositoryId)" == 777 ]] || fail "the publisher wasn't bound to the repository's id: $registered"
held_publisher="$(expect 201 POST /account/orgs/held/trusted "$ann" "$publisher")"
expect 400 POST "/account/orgs/$org/trusted" "$ann" '{"repository":"conformance-fixture/widgets","workflow":"release.yml","environment":null,"can":["publish"]}' >/dev/null
expect 400 POST "/account/orgs/$org/trusted" "$ann" '{"repository":"conformance-fixture/missing","workflow":".github/workflows/release.yml","environment":null,"can":["publish"]}' >/dev/null
echo "check-trusted: trusted publishers registered by repository id"

# `oidc_token <json claims>`: a token from the fake issuer.
oidc_token() {
  curl -s -X POST -H 'content-type: application/json' --data "$1" "$oidc/token"
}
claims() { # claims <repository_id> <workflow file> <environment> [extra json]
  printf '{"sub":"repo:conformance-fixture/widgets:environment:%s","repository":"conformance-fixture/widgets","repository_id":"%s","job_workflow_ref":"conformance-fixture/widgets/%s@refs/tags/v1","environment":"%s","ref":"refs/tags/v1"%s}' \
    "$3" "$1" "$2" "$3" "${4:-}"
}
exchange() { # exchange <status> <org> <jwt>
  expect "$1" POST /account/trusted/exchange "$3" "{\"org\":\"$2\"}"
}

# 6. The exchange: a matching token gets a 15-minute publish token.
good="$(oidc_token "$(claims 777 .github/workflows/release.yml release)")"
full="$(field "$(exchange 201 "$org" "$good")" token)"
narrow="$(field "$(expect 201 POST /account/orgs/someone-else/tokens "$ann" '{"can":["publish"],"days":1,"note":"narrow"}')" token)"
echo "check-trusted: a matching GitHub Actions token exchanged for a publish token"

# 7. Hold it to the protocol with the exchanged token.
BYNK_REGISTRY_CONFORMANCE_TOKEN="$full" NARROW="$narrow" \
  "$bynk" registry conformance "$url" --org "$org" --key "$root" --narrow-token-env NARROW ||
  fail "the conformance run failed with an exchanged token"
exchange 403 "$org" "$good" >/dev/null # a token is exchanged once

# 8. Every mismatch is refused.
exchange 403 "$org" "$(oidc_token "$(claims 777 .github/workflows/ci.yml release)")" >/dev/null
exchange 403 "$org" "$(oidc_token "$(claims 999 .github/workflows/release.yml release)")" >/dev/null
exchange 403 "$org" "$(oidc_token "$(claims 777 .github/workflows/release.yml staging)")" >/dev/null
exchange 401 "$org" "$(oidc_token "$(claims 777 .github/workflows/release.yml release ',"aud":"another-registry"')")" >/dev/null
exchange 401 "$org" "$(oidc_token "$(claims 777 .github/workflows/release.yml release ',"ttl":-600')")" >/dev/null
exchange 401 "$org" "not-a-token" >/dev/null
expect 200 PUT /account/orgs/held/blocked "$ann" '{"blocked":true}' >/dev/null
exchange 403 held "$good" >/dev/null
id="$(field "$registered" id)"
expect 200 DELETE "/account/orgs/$org/trusted/$id" "$ann" >/dev/null
exchange 403 "$org" "$good" >/dev/null
echo "check-trusted: other workflows, repositories, environments, audiences, expired and replayed tokens, blocked names and removed publishers are refused"
