#!/usr/bin/env bash
# Build the bynk-wasm crate to wasm32 and generate the browser glue with
# wasm-bindgen (in-browser track, slice 4). Pass `--release` for a small,
# wasm-opt-ready artefact; default is debug (fast, large) for local iteration.
# BYNK_WASM_OUT_DIR overrides where the glue + module land (default
# target/web-assets); the VS Code web extension (vscode-bynk `npm run
# build:wasm`, #2102) and the release's web assets (scripts/package-web-assets.sh,
# which bynk-lang/playground builds from) each stage their copy with it.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
out="${BYNK_WASM_OUT_DIR:-$repo/target/web-assets}"

cd "$repo"
if [[ "${1:-}" == "--release" ]]; then
  profile="release"
  cargo build --target wasm32-unknown-unknown -p bynk-wasm --release
else
  profile="debug"
  cargo build --target wasm32-unknown-unknown -p bynk-wasm
fi
wasm-bindgen --target web \
  --out-dir "$out" \
  "$repo/target/wasm32-unknown-unknown/$profile/bynk_wasm.wasm"

# Release: shrink with wasm-opt when available (the Q3 size budget). Optional —
# the artefact is correct without it.
if [[ "$profile" == "release" ]] && command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz -o "$out/bynk_wasm_bg.wasm" "$out/bynk_wasm_bg.wasm"
  echo "wasm-opt -Oz applied"
fi
echo "wasm glue + module staged in $out ($profile)"
