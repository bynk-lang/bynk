#!/usr/bin/env bash
# The read path, end to end (R1, #2334, decision D): build a signed registry
# with the `bynk registry` tooling (an offline root, a delegate, two versions of
# `conformance-fixture/hello`, one yanked), seed `bynk dev`'s local bucket with
# it, serve the service, and hold it to the protocol with
# `bynk registry conformance --read-only`.
#
#   BYNK=<path to bynk> scripts/check-read-path.sh
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
bynk="${BYNK:-bynk}"
work="$(mktemp -d)"
# `bynk dev` stops every `wrangler dev` and `workerd` it started when it is
# sent SIGTERM (bynk-cli.md, `bynk dev` step 6), so killing it is enough.
trap 'kill "${dev_pid:-}" 2>/dev/null || true; wait "${dev_pid:-}" 2>/dev/null || true; rm -rf "$work"' EXIT
export BYNK_HOME="$work/home"

# 1. A signed registry, built offline.
"$bynk" registry init "$work/reg" --id bynk-fixture --key-out "$work/root.key" >/dev/null
"$bynk" registry delegate "$work/reg" --key "$work/root.key" --delegate-out "$work/online.key" 2>/dev/null
for v in 1.0.0 1.1.0; do
  pkg="$work/pkgs/hello-$v"
  mkdir -p "$pkg/src"
  printf '[package]\nname = "conformance-fixture/hello"\nversion = "%s"\n' "$v" > "$pkg/bynk.toml"
  printf 'commons greet\n\nfn hello() -> String {\n  "hello %s"\n}\n' "$v" > "$pkg/src/greet.bynk"
  "$bynk" registry add "$work/reg" "$pkg" --key "$work/online.key" 2>/dev/null
done
"$bynk" registry yank "$work/reg" conformance-fixture/hello 1.0.0 2>/dev/null

# 2. Seed `bynk dev`'s local bucket. `bynk dev` runs `wrangler dev` in local
# mode; both sides share one persistence directory.
persist="$work/persist"
# From the scratch directory, so wrangler's local cache (which records the
# machine's network details) is written there and never into the repository.
(cd "$work" && "$here/seed/seed.sh" "$work/reg" --bucket registry-store-store --local --persist-to "$persist")

# 3. Serve it, and wait until it answers.
"$bynk" dev "$here" --ready-json "$work/ready.json" -- --persist-to "$persist" >"$work/dev.log" 2>&1 &
dev_pid=$!
for _ in $(seq 1 120); do
  [[ -s "$work/ready.json" ]] && break
  kill -0 "$dev_pid" 2>/dev/null || { cat "$work/dev.log" >&2; exit 1; }
  sleep 1
done
[[ -s "$work/ready.json" ]] || { echo "bynk dev never became ready" >&2; cat "$work/dev.log" >&2; exit 1; }
url="$(grep -o 'http://[^"]*' "$work/ready.json" | head -1)"

# 4. Hold it to the protocol.
"$bynk" registry conformance "$url" --read-only --package conformance-fixture/hello
