#!/usr/bin/env bash
# Build the grammar wasm from source into src/vendor/ (the in-repo path; the
# pinned-release path is fetch-assets.sh). The grammar build itself lives in
# the repo's scripts/ so the release can package it too.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"

BYNK_GRAMMAR_OUT_DIR="$here/src/vendor" bash "$here/../scripts/build-grammar-wasm.sh"
# The web-tree-sitter runtime wasm, shipped alongside. It comes from this app's
# own dependency, so it is not part of the release assets.
# (0.26 renamed the runtime wasm file from tree-sitter.wasm to web-tree-sitter.wasm;
# we keep our own vendored name, referenced by tshighlight.ts's locateFile.)
cp -f "$here/node_modules/web-tree-sitter/web-tree-sitter.wasm" "$here/src/vendor/tree-sitter.wasm"
