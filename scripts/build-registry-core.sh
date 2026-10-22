#!/usr/bin/env bash
# Build bynk-registry-core to a standalone wasm32 module (#2321, decision E):
# a plain C ABI (alloc / dealloc / admit), no wasm-bindgen and no glue JS, so
# the output is the single file `bynk_registry_core.wasm`.
#
# BYNK_REGISTRY_CORE_OUT_DIR overrides where it lands (default
# target/registry-core). wasm-opt (binaryen) shrinks it when available; the
# module is correct without it, but the CI size budget is judged after it.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
out="${BYNK_REGISTRY_CORE_OUT_DIR:-$repo/target/registry-core}"

cd "$repo"
cargo build --locked --target wasm32-unknown-unknown -p bynk-registry-core --release
mkdir -p "$out"
cp "$repo/target/wasm32-unknown-unknown/release/bynk_registry_core.wasm" "$out/bynk_registry_core.wasm"

if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz -o "$out/bynk_registry_core.wasm" "$out/bynk_registry_core.wasm"
  echo "wasm-opt -Oz applied"
else
  echo "wasm-opt not found; the module is unoptimised" >&2
fi
echo "$out/bynk_registry_core.wasm: $(wc -c <"$out/bynk_registry_core.wasm") bytes"
