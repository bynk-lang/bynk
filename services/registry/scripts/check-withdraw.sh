#!/usr/bin/env bash
# The operator routes, end to end (R5a.2, #2397): publish two versions with the
# real client and export them, take one down with an operator token, then check
# what everyone sees — the protocol's read checks (`read.withdrawn`), a fresh
# client failing as `bynk.registry.withdrawn`, the publish API's `410`/`409`,
# the backup without the archive, and the log — and restore an organisation.
#
#   BYNK=<path to bynk> scripts/check-withdraw.sh
#
# Needs the admission module (scripts/admission-module.sh) and python3.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
bynk="${BYNK:-bynk}"
read -r -a wrangler <<< "${WRANGLER:-wrangler}"
work="$(mktemp -d)"
trap 'kill "${dev_pid:-}" 2>/dev/null || true; wait "${dev_pid:-}" 2>/dev/null || true; rm -rf "$work"' EXIT
export BYNK_HOME="$work/home"
org=conformance-fixture
reason="Removed after a malware report (check-withdraw)"

[[ -f "$here/src/registry/bynk_registry_core.wasm" ]] || {
  echo "the admission module isn't built: run scripts/admission-module.sh" >&2
  exit 1
}

fail() {
  echo "check-withdraw: $*" >&2
  echo "--- bynk dev log ---" >&2
  tail -60 "$work/dev.log" >&2
  exit 1
}

# 1. An empty registry, a publish token, and three operator tokens: one that
# can `withdraw`, one that can `operate`, and one that can do neither.
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
withdrawer="op_withdraw_$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
operator="op_operate_$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
tokens="$(python3 - "$work/tokens.toml" "$withdrawer" "$operator" <<'PY'
import hashlib, json, sys, tomllib
table = tomllib.load(open(sys.argv[1], "rb"))["token"]
for token, can in ((sys.argv[2], ["withdraw"]), (sys.argv[3], ["operate"])):
    table.append({"sha256": hashlib.sha256(token.encode()).hexdigest(), "organisations": [], "can": can})
print(json.dumps(table, separators=(",", ":")))
PY
)"
persist="$work/persist"
(cd "$work" && "$here/seed/seed.sh" "$work/reg" --bucket registry-store-store --local --persist-to "$persist")

start_dev() {
  rm -f "$work/ready.json"
  "$bynk" dev "$here" --ready-json "$work/ready.json" -- \
    --persist-to "$persist" --test-scheduled \
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
  [[ -s "$work/ready.json" ]] || fail "bynk dev never became ready"
  url="$(grep -o 'http://[^"]*' "$work/ready.json" | head -1)"
}
stop_dev() {
  kill "$dev_pid" 2>/dev/null || true
  wait "$dev_pid" 2>/dev/null || true
  dev_pid=
}
backup_has() { # backup_has <key>: whether the backup bucket holds it
  "${wrangler[@]}" r2 object get "registry-store-backup/$1" --local --persist-to "$persist" \
    --file "$work/backup.out" >/dev/null 2>&1
}
export_pass() { # tick the export until a pass finishes
  for _ in $(seq 1 10); do
    curl -s -o /dev/null "$url/__scheduled?cron=17+*+*+*+*"
    if curl -s "$url/_status/export" | python3 -c 'import json,sys; s=json.load(sys.stdin); sys.exit(0 if s["phase"] == "idle" and s["last"].get("value") else 1)'; then
      return 0
    fi
  done
  fail "no export pass finished in 10 ticks"
}
start_dev

# 2. Publish 0.1.0 and 0.2.0; a consumer locks 0.1.0 and has its tree.
routing="$(printf '[registries.first-party]\nindex = "%s/"\norganisations = ["%s"]\nkey = "%s"\n' "$url" "$org" "$root")"
pkg="$work/client/greeting"
mkdir -p "$pkg/src"
printf '[package]\nname = "%s/greeting"\nversion = "0.1.0"\n\n%s\n' "$org" "$routing" >"$pkg/bynk.toml"
printf 'commons greeting\n\nfn greet(name: String) -> String {\n  "hello \\(name)"\n}\n' >"$pkg/src/greeting.bynk"
export BYNK_REGISTRY_FIRST_PARTY_TOKEN="$full"
(cd "$pkg" && "$bynk" publish) >/dev/null 2>&1 || fail "bynk publish 0.1.0 failed"
cp -r "$pkg" "$work/client/greeting-0.1.0"
sed -i 's/^version = "0.1.0"/version = "0.2.0"/' "$pkg/bynk.toml"
(cd "$pkg" && "$bynk" publish) >/dev/null 2>&1 || fail "bynk publish 0.2.0 failed"
app="$work/client/app"
mkdir -p "$app/src"
printf '[project]\nname = "app"\n\n%s\n[dependencies]\ngreeting = { package = "%s/greeting", version = "~0.1" }\n' "$routing" "$org" >"$app/bynk.toml"
printf 'commons app\n\nuses greeting.greeting\n\nfn welcome() -> String {\n  greet("you")\n}\n' >"$app/src/app.bynk"
(cd "$app" && "$bynk" check) >/dev/null 2>&1 || fail "a consumer couldn't resolve 0.1.0"
grep -q 'version = "0.1.0"' "$app/bynk.lock" || fail "the consumer's lock doesn't name 0.1.0"
hex="$(curl -s "$url/$org/greeting" | python3 -c '
import base64, json, sys
for l in sys.stdin:
    line = json.loads(l)
    if line["version"] == "0.1.0":
        print(base64.b64decode(line["checksum"][7:]).hex())')"
[[ -n "$hex" ]] || fail "the index has no 0.1.0"

# The backup holds the archive before the takedown.
export_pass
stop_dev
backup_has "dl/sha256/$hex" || fail "the export didn't copy 0.1.0's archive"
start_dev

# 3. Only a `withdraw` token takes a version down.
withdraw() { # withdraw <token> <version> <reason>: the status, body in $work/withdraw.json
  curl -s -o "$work/withdraw.json" -w '%{http_code}' -X POST "$url/_operator/withdraw/$org/greeting/$2" \
    -H "authorization: Bearer $1" -H 'content-type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"reason": sys.argv[1]}))' "$3")"
}
[[ "$(withdraw bynk_unknown 0.1.0 "$reason")" == 401 ]] || fail "an unknown token withdrew a version"
[[ "$(withdraw "$full" 0.1.0 "$reason")" == 403 ]] || fail "a publish-and-yank token withdrew a version"
[[ "$(withdraw "$operator" 0.1.0 "$reason")" == 403 ]] || fail "an operate token withdrew a version"
[[ "$(withdraw "$withdrawer" 0.1.0 "$(printf 'two\nlines')")" == 400 ]] || fail "a two-line reason was accepted"
status="$(withdraw "$withdrawer" 0.1.0 "  $reason  ")"
[[ "$status" == 200 ]] || fail "the takedown answered $status: $(cat "$work/withdraw.json")"
python3 - "$work/withdraw.json" "$reason" <<'PY' || fail "the takedown's answer is wrong: $(cat "$work/withdraw.json")"
import json, sys
a = json.load(open(sys.argv[1]))
assert a["archive"] == "deleted", a
assert a["version"] == "0.1.0" and a["reason"] == sys.argv[2], a
PY
[[ "$(withdraw "$withdrawer" 0.1.0 "$reason")" == 409 ]] || fail "a repeated takedown wasn't 409"
echo "check-withdraw: an operator took $org/greeting 0.1.0 down"

# 4. What everyone sees: the line yanked and withdrawn, its archive 404, and
# the protocol's read checks pass.
python3 - "$reason" <<PY || fail "the index file doesn't carry the withdrawal: $(curl -s "$url/$org/greeting")"
import json, sys, urllib.request
lines = [json.loads(l) for l in urllib.request.urlopen("$url/$org/greeting").read().decode().splitlines() if l.strip()]
down = [l for l in lines if l["version"] == "0.1.0"][0]
assert down["yanked"] is True and down["withdrawn"] == sys.argv[1], down
live = [l for l in lines if l["version"] == "0.2.0"][0]
assert live["yanked"] is False and "withdrawn" not in live, live
PY
[[ "$(curl -s -o /dev/null -w '%{http_code}' "$url/dl/sha256/$hex")" == 404 ]] || fail "the withdrawn archive is still served"
"$bynk" registry conformance "$url" --read-only --package "$org/greeting" --key "$root" >"$work/conformance.out" 2>&1 ||
  { cat "$work/conformance.out" >&2; fail "the read checks failed after the takedown"; }
grep -q "read.withdrawn" "$work/conformance.out" || fail "the read checks didn't run read.withdrawn"

# 5. A consumer without the tree fails as `bynk.registry.withdrawn`, quoting the
# reason; one with the tree in its store keeps building.
(cd "$app" && BYNK_HOME="$work/fresh-home" "$bynk" check) >"$work/fresh.out" 2>&1 &&
  fail "a fresh client built against a withdrawn version"
grep -q "bynk.registry.withdrawn" "$work/fresh.out" || { cat "$work/fresh.out" >&2; fail "the failure isn't bynk.registry.withdrawn"; }
grep -q "$reason" "$work/fresh.out" || { cat "$work/fresh.out" >&2; fail "the failure doesn't quote the reason"; }
(cd "$app" && "$bynk" check) >/dev/null 2>&1 || fail "a locked build with the tree in its store stopped building"
echo "check-withdraw: a fresh client fails as bynk.registry.withdrawn; a stored tree keeps building"

# 6. Nothing revives it: the same tree is 410, an un-yank 409.
(cd "$work/client/greeting-0.1.0" && "$bynk" publish) >"$work/republish.out" 2>&1 &&
  fail "the withdrawn version was published again"
grep -q "withdrawn" "$work/republish.out" || { cat "$work/republish.out" >&2; fail "the republish wasn't refused as withdrawn"; }
(cd "$pkg" && "$bynk" yank --undo "$org/greeting" 0.1.0) >"$work/unyank.out" 2>&1 &&
  fail "the withdrawn version was un-yanked"
grep -q "withdrawn" "$work/unyank.out" || { cat "$work/unyank.out" >&2; fail "the un-yank wasn't refused as withdrawn"; }
[[ "$(curl -s -o /dev/null -w '%{http_code}' "$url/dl/sha256/$hex")" == 404 ]] || fail "the republish put the archive back"

# 7. The log, for `withdraw` tokens only.
[[ "$(curl -s -o /dev/null -w '%{http_code}' "$url/_operator/takedowns" -H "authorization: Bearer $operator")" == 403 ]] ||
  fail "an operate token read the takedown log"
curl -s "$url/_operator/takedowns" -H "authorization: Bearer $withdrawer" | python3 -c '
import hashlib, json, sys
log = json.load(sys.stdin)["takedowns"]
assert len(log) == 1, log
t = log[0]
assert t["package"] == sys.argv[1] and t["version"] == "0.1.0" and t["archive"] == "deleted", t
assert t["tokenId"] == hashlib.sha256(sys.argv[2].encode()).hexdigest(), t
' "$org/greeting" "$withdrawer" || fail "the takedown log is wrong"

# 8. The takedown deleted the backup's copy too. (The end-of-pass sweep that
# catches an export tick racing a takedown is a unit test.)
stop_dev
backup_has "dl/sha256/$hex" && fail "the backup still holds the withdrawn archive"
echo "check-withdraw: the backup's copy is gone"

# 9. Restore an organisation from an export, as an operator: restore one into
# a fresh registry, export it, then restore the export's very bytes into
# another fresh registry.
persist="$work/orgs"
(cd "$work" && "$here/seed/seed.sh" "$work/reg" --bucket registry-store-store --local --persist-to "$persist") >/dev/null
start_dev
restore() { # restore <token> <org> <curl data argument>: the status
  curl -s -o "$work/restore.json" -w '%{http_code}' -X PUT "$url/_operator/orgs/$2" \
    -H "authorization: Bearer $1" -H 'content-type: application/json' --data-binary "$3"
}
state='{"name":"restored-org","owner":{"id":1,"login":"ann"},"members":[{"id":1,"login":"ann"},{"id":2,"login":"bob"}],"blocked":false,"trusted":[{"id":"p1","repository":"ann/money","repositoryId":"4242","workflow":".github/workflows/release.yml","environment":"release","can":["publish"]}]}'
[[ "$(restore "$withdrawer" restored-org "$state")" == 403 ]] || fail "a withdraw token restored an organisation"
status="$(restore "$operator" restored-org "$state")"
[[ "$status" == 201 ]] || fail "the restore answered $status: $(cat "$work/restore.json")"
[[ "$(restore "$operator" restored-org "$state")" == 409 ]] || fail "a restore overwrote a claimed organisation"
export_pass
stop_dev
"${wrangler[@]}" r2 object get "registry-store-backup/agents/orgs/restored-org.json" --local --persist-to "$persist" \
  --file "$work/restored-org.json" >/dev/null 2>&1 || fail "the export has no restored-org"
persist="$work/orgs-again"
(cd "$work" && "$here/seed/seed.sh" "$work/reg" --bucket registry-store-store --local --persist-to "$persist") >/dev/null
start_dev
status="$(restore "$operator" restored-org "@$work/restored-org.json")"
[[ "$status" == 201 ]] || fail "restoring the export's bytes answered $status: $(cat "$work/restore.json")"
curl -s "$url/account/orgs/restored-org" -H "authorization: Bearer $operator" | python3 -c '
import json, sys
v = json.load(sys.stdin)
assert v["owner"]["login"] == "ann" and [m["login"] for m in v["members"]] == ["ann", "bob"], v
[p] = v["trusted"]
assert p["id"] == "p1" and p["repositoryId"] == "4242" and p["workflow"] == ".github/workflows/release.yml", v
' || fail "the organisation restored from its export isn't as exported"
echo "check-withdraw: an operator restored an organisation from the export's own bytes"
