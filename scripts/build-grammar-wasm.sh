#!/usr/bin/env bash
# Build tree-sitter-bynk to a wasm grammar for web-tree-sitter, and stage it with
# its highlight query, for the release's bynk-web tarball (bynk-lang/playground's
# syntax highlighting; scripts/package-web-assets.sh). Uses the repo-local
# tree-sitter CLI (`npm ci` in tree-sitter-bynk/ first); `build --wasm` fetches its
# own wasi-sdk, or falls back to emscripten/docker.
# BYNK_GRAMMAR_OUT_DIR overrides where the files land (default target/web-assets).
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
out="${BYNK_GRAMMAR_OUT_DIR:-$repo/target/web-assets}"
grammar="$repo/tree-sitter-bynk"

mkdir -p "$out"
cd "$grammar"
# Emits tree-sitter-bynk.wasm in CWD.
"$grammar/node_modules/.bin/tree-sitter" build --wasm
mv -f tree-sitter-bynk.wasm "$out/tree-sitter-bynk.wasm"
cp -f "$grammar/queries/highlights.scm" "$out/highlights.scm"
echo "grammar wasm + highlights.scm staged in $out"
