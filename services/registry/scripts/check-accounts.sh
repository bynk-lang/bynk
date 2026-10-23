#!/usr/bin/env bash
# Accounts, end to end (R3, #2363, decision F): serve the registry with a fake
# GitHub (scripts/fake-github.py) behind its device flow, sign two people in,
# claim organisations, add a member, mint publish tokens, and hold the service
# to the full `bynk registry conformance` run with an account-minted token.
# Then check that a revoked token, a blocked organisation and a reserved name
# are refused, and that an operator can claim a reserved name for someone.
# Last, the real client (R3.1, #2378): `bynk login`, `bynk org` and
# `bynk publish` with a token `bynk org token mint --save` kept — and, per
# organisation (R3.2, #2383), two organisations publishing on one registry.
#
#   BYNK=<path to bynk> scripts/check-accounts.sh
#
# Needs the admission module (scripts/admission-module.sh), curl and python3.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
bynk="${BYNK:-bynk}"
work="$(mktemp -d)"
trap 'kill "${dev_pid:-}" "${fake_pid:-}" 2>/dev/null || true; wait "${dev_pid:-}" "${fake_pid:-}" 2>/dev/null || true; rm -rf "$work"' EXIT
export BYNK_HOME="$work/home"
org=conformance-fixture

[[ -f "$here/src/registry/bynk_registry_core.wasm" ]] || {
  echo "the admission module isn't built: run scripts/admission-module.sh" >&2
  exit 1
}

fail() {
  echo "check-accounts: $*" >&2
  echo "--- bynk dev log ---" >&2
  tail -60 "$work/dev.log" >&2 || true
  exit 1
}
# `field <json> <key>`: one top-level field of a JSON object.
field() {
  python3 -c 'import json,sys; print(json.loads(sys.argv[1])[sys.argv[2]])' "$1" "$2"
}

# 1. A fake GitHub on a free port.
python3 "$here/scripts/fake-github.py" "$work/fake.port" &
fake_pid=$!
for _ in $(seq 1 50); do [[ -s "$work/fake.port" ]] && break; sleep 0.1; done
github="http://127.0.0.1:$(cat "$work/fake.port")"

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

# 4. Sign in through the device flow: start, poll while pending, then the token.
sign_in() {
  local started poll answer
  started="$(expect 200 POST /account/login -)"
  poll="$(field "$started" poll)"
  expect 400 POST /account/login/poll - "{\"poll\":\"$poll\"}" >/dev/null
  answer="$(expect 200 POST /account/login/poll - "{\"poll\":\"$poll\"}")"
  field "$answer" token
}
ann="$(sign_in)"
bob="$(sign_in)"
echo "check-accounts: ann and bob signed in through the device flow"

# 5. Organisations: claims are first come; members are added by login; a
# reserved name is refused, and an operator claims it for someone.
expect 201 POST "/account/orgs/$org" "$ann" '{"block":false}' >/dev/null
expect 409 POST "/account/orgs/$org" "$bob" '{"block":false}' >/dev/null
expect 201 POST /account/orgs/someone-else "$ann" '{"block":false}' >/dev/null
expect 200 PUT "/account/orgs/$org/members/bob" "$ann" >/dev/null
expect 403 PUT "/account/orgs/$org/members/cy" "$bob" >/dev/null
expect 403 POST /account/orgs/npm "$ann" '{"block":false}' >/dev/null
expect 201 POST /account/orgs/npm "$operator" '{"block":true,"owner":"cy"}' >/dev/null
echo "check-accounts: claims, members and reserved names behave"

# 6. Tokens: bob, a member, mints the run's token; ann mints a narrow one for
# another organisation (auth.scope).
full="$(field "$(expect 201 POST "/account/orgs/$org/tokens" "$bob" '{"can":["publish","yank"],"days":7,"note":"conformance"}')" token)"
narrow="$(field "$(expect 201 POST /account/orgs/someone-else/tokens "$ann" '{"can":["publish"],"days":7,"note":"narrow"}')" token)"
view="$(expect 200 GET "/account/orgs/$org" "$bob")"
python3 -c 'import json,sys; v=json.loads(sys.argv[1]); assert [m["login"] for m in v["members"]] == ["ann","bob"], v; assert len(v["tokens"]) == 1, v' "$view" ||
  fail "the organisation's view is wrong: $view"

# 7. Hold it to the protocol with the account-minted token.
BYNK_REGISTRY_CONFORMANCE_TOKEN="$full" NARROW="$narrow" \
  "$bynk" registry conformance "$url" --org "$org" --key "$root" --narrow-token-env NARROW ||
  fail "the conformance run failed with an account-minted token"

# 8. Revoking the token stops it; a blocked organisation refuses publishes;
# signing out ends the session.
id="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["tokens"][0]["id"])' "$view")"
# A stranger (cy, signed in) is told the organisation isn't there.
expect 404 DELETE "/account/orgs/$org/tokens/$id" "$(sign_in)" >/dev/null
expect 200 DELETE "/account/orgs/$org/tokens/$id" "$ann" >/dev/null
status="$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "authorization: Bearer $full" -H 'content-type: application/vnd.bynk.tree-v1' --data-binary x "$url/api/$org/probe/0.0.1")"
[[ "$status" == 401 ]] || fail "a revoked token was answered $status, not 401"
expect 201 POST /account/orgs/held "$ann" '{"block":true}' >/dev/null
held="$(field "$(expect 201 POST /account/orgs/held/tokens "$ann" '{"can":["publish"],"days":1,"note":""}')" token)"
status="$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "authorization: Bearer $held" -H 'content-type: application/vnd.bynk.tree-v1' --data-binary x "$url/api/held/probe/0.0.1")"
[[ "$status" == 403 ]] || fail "a blocked organisation answered $status, not 403"
expect 204 DELETE /account/session "$bob" >/dev/null
expect 401 GET "/account/orgs/$org" "$bob" >/dev/null
echo "check-accounts: revoked tokens, blocked names and signed-out sessions are refused"

# 9. The real client (R3.1, #2378): sign in with `bynk login` (the fake signs
# in ann again), claim an organisation, add a member, mint a token straight
# into the credentials file, and `bynk publish` with it.
export XDG_CONFIG_HOME="$work/config"
client_org=client-org
labs_org=client-labs
# A `<org>/greeting` package, with both organisations routed to the service.
greeting() {
  local dir="$work/client/$1"
  mkdir -p "$dir/src"
  printf '[package]\nname = "%s/greeting"\nversion = "0.1.0"\n\n[registries.first-party]\nindex = "%s/"\norganisations = ["%s", "%s"]\nkey = "%s"\n' \
    "$1" "$url" "$client_org" "$labs_org" "$root" >"$dir/bynk.toml"
  printf 'commons greeting\n\nfn greet(name: String) -> String {\n  "hello \\(name)"\n}\n' >"$dir/src/greeting.bynk"
}
greeting "$client_org"
greeting "$labs_org"
pkg="$work/client/$client_org"
(cd "$pkg" && "$bynk" login --registry first-party) 2>"$work/login.err" ||
  fail "bynk login failed: $(cat "$work/login.err")"
grep -q 'as ann' "$work/login.err" || fail "bynk login didn't sign ann in: $(cat "$work/login.err")"
(cd "$pkg" && "$bynk" org claim "$client_org") || fail "bynk org claim failed"
(cd "$pkg" && "$bynk" org member add "$client_org" bob) || fail "bynk org member add failed"
(cd "$pkg" && "$bynk" org token mint "$client_org" --days 1 --note client --save) || fail "bynk org token mint --save failed"
(cd "$pkg" && "$bynk" org show "$client_org") >"$work/show.txt" || fail "bynk org show failed"
grep -q 'members: ann (#101), bob (#102)' "$work/show.txt" || fail "bynk org show is wrong: $(cat "$work/show.txt")"
(cd "$pkg" && "$bynk" publish) || fail "bynk publish with the saved token failed"
# R3.2 (#2383): a second organisation's token goes in its own slot, and each
# package publishes with its organisation's token — no variable, no `--force`.
(cd "$pkg" && "$bynk" org claim "$labs_org") || fail "bynk org claim $labs_org failed"
(cd "$pkg" && "$bynk" org token mint "$labs_org" --days 1 --note client --save) || fail "bynk org token mint $labs_org --save failed"
grep -q "^\[registries.first-party.organisations.$labs_org\]" "$XDG_CONFIG_HOME/bynk/credentials.toml" ||
  fail "the $labs_org token isn't in its own slot"
(cd "$work/client/$labs_org" && "$bynk" publish) || fail "bynk publish for $labs_org with its own saved token failed"
(cd "$pkg" && "$bynk" logout --registry first-party) || fail "bynk logout failed"
(cd "$pkg" && "$bynk" org show "$client_org") 2>/dev/null && fail "bynk org show worked after logout"
echo "check-accounts: the real client signed in, claimed, minted saved tokens for two organisations, published for each and signed out"
