#!/usr/bin/env bash
# The scheduled export and a restore drill, end to end (R5.2, #2391): publish
# with the real client, drive `bynk dev`'s cron until an export pass finishes,
# then restore a fresh bucket from the backup alone and hold the restored
# registry to the protocol's read checks.
#
#   BYNK=<path to bynk> scripts/check-export.sh
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

[[ -f "$here/src/registry/bynk_registry_core.wasm" ]] || {
  echo "the admission module isn't built: run scripts/admission-module.sh" >&2
  exit 1
}

fail() {
  echo "check-export: $*" >&2
  echo "--- bynk dev log ---" >&2
  tail -60 "$work/dev.log" >&2
  exit 1
}

# 1. An empty registry and an operator token, seeded into `bynk dev`'s bucket.
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
tokens="$(python3 - "$work/tokens.toml" <<'PY'
import json, sys, tomllib
print(json.dumps(tomllib.load(open(sys.argv[1], "rb"))["token"], separators=(",", ":")))
PY
)"
(cd "$work" && "$here/seed/seed.sh" "$work/reg" --bucket registry-store-store --local --persist-to "$work/persist")

start_dev() { # start_dev <persist dir>: serve it, with cron triggers callable
  rm -f "$work/ready.json"
  "$bynk" dev "$here" --ready-json "$work/ready.json" -- \
    --persist-to "$1" --test-scheduled \
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
start_dev "$work/persist"

# 2. Publish a package with the real client.
routing() { printf '[registries.first-party]\nindex = "%s/"\norganisations = ["%s"]\nkey = "%s"\n' "$url" "$org" "$root"; }
pkg="$work/client/greeting"
mkdir -p "$pkg/src"
printf '[package]\nname = "%s/greeting"\nversion = "0.1.0"\n\n%s\n' "$org" "$(routing)" >"$pkg/bynk.toml"
printf 'commons greeting\n\nfn greet(name: String) -> String {\n  "hello \\(name)"\n}\n' >"$pkg/src/greeting.bynk"
(cd "$pkg" && BYNK_REGISTRY_FIRST_PARTY_TOKEN="$full" "$bynk" publish) >/dev/null 2>&1 || fail "bynk publish failed"

# `last <json status>`: the last finished pass, `{}` when none (an `Option`
# is `{"kind", "value"}` in the service's JSON).
last() {
  python3 -c 'import json,sys; l=json.loads(sys.argv[1])["last"]; print(json.dumps(l.get("value", {})))' "$1"
}

# 3. Tick the export until a pass finishes: start, copy the bucket's one page,
# export the (no) organisations.
finished=
for _ in $(seq 1 10); do
  curl -s -o /dev/null "$url/__scheduled?cron=17+*+*+*+*"
  status="$(curl -s "$url/_status/export")"
  if [[ "$(curl -s "$url/_status/export" | python3 -c 'import json,sys; print(json.load(sys.stdin)["phase"])')" == idle && "$(last "$status")" != "{}" ]]; then
    finished=1
    break
  fi
done
[[ -n "$finished" ]] || fail "no export pass finished in 10 ticks: $status"
python3 - "$(last "$status")" <<'PY' || fail "the pass didn't copy what was published: $status"
import json, sys
last = json.loads(sys.argv[1])
assert last["failures"] == 0 and last["packages"] == 1 and last["copied"] >= 3, last
PY
echo "check-export: a pass copied the bucket and the package's lines: $(python3 -c 'import json,sys; l=json.loads(sys.argv[1]); print(l["objects"], "objects,", l["copied"], "copied")' "$(last "$status")")"

# A second tick within the day starts nothing.
curl -s -o /dev/null "$url/__scheduled?cron=17+*+*+*+*"
[[ "$(curl -s "$url/_status/export" | python3 -c 'import json,sys; print(json.load(sys.stdin)["phase"])')" == idle ]] ||
  fail "a pass started again within a day"
stop_dev

# 4. Restore: read the backup bucket alone (config.json, the index file, and
# the archives its lines name) into a directory, and seed a fresh bucket.
get() { # get <key>: the backup's object, into $work/restored/<key>
  mkdir -p "$(dirname "$work/restored/$1")"
  "${wrangler[@]}" r2 object get "registry-store-backup/$1" --local --persist-to "$work/persist" \
    --file "$work/restored/$1" >/dev/null 2>&1 || fail "the backup has no $1"
}
get config.json
get "$org/greeting"
get "agents/packages/$org/greeting.json"
for key in $(python3 - "$work/restored/$org/greeting" <<'PY'
import base64, json, sys
for line in open(sys.argv[1]):
    if line.strip():
        print("dl/sha256/" + base64.b64decode(json.loads(line)["checksum"][7:]).hex())
PY
); do get "$key"; done
python3 - "$work/restored" "$org/greeting" <<'PY' || fail "the exported lines don't match the index file"
import json, sys
root, name = sys.argv[1], sys.argv[2]
exported = json.load(open(f"{root}/agents/packages/{name}.json"))
indexed = [json.loads(l) for l in open(f"{root}/{name}") if l.strip()]
assert [l["checksum"] for l in exported] == [l["checksum"] for l in indexed], (exported, indexed)
PY
rm -rf "$work/restored/agents"
(cd "$work" && "$here/seed/seed.sh" "$work/restored" --bucket registry-store-store --local --persist-to "$work/fresh") >/dev/null
start_dev "$work/fresh"

# 5. The restored registry serves what was published, verified as any client
# would, and a consumer resolves and locks it.
"$bynk" registry conformance "$url" --read-only --package "$org/greeting" --key "$root" >/dev/null ||
  fail "the restored registry failed the read checks"
app="$work/client/app"
mkdir -p "$app/src"
printf '[project]\nname = "app"\n\n%s\n[dependencies]\ngreeting = { package = "%s/greeting", version = "^0.1" }\n' "$(routing)" "$org" >"$app/bynk.toml"
printf 'commons app\n\nuses greeting.greeting\n\nfn welcome() -> String {\n  greet("you")\n}\n' >"$app/src/app.bynk"
(cd "$app" && "$bynk" check) >/dev/null 2>&1 || fail "a consumer couldn't resolve from the restored registry"
echo "check-export: a fresh bucket restored from the backup alone serves and resolves $org/greeting"
