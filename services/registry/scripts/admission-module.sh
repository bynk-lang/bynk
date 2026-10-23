#!/usr/bin/env bash
# Build the admission module (#2336, decision A): `bynk-registry-core` as a
# standalone wasm32 module, copied beside the adapter that includes it. The
# module is built, never committed (src/registry/bynk_registry_core.wasm is
# git-ignored); run this once after a fresh clone, and again after a change to
# the admission or signing rules. On extraction (track §1.3) this fetches the
# release's module instead.
#
#   scripts/admission-module.sh
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$here/../.." && pwd)"
BYNK_REGISTRY_CORE_OUT_DIR="$here/src/registry" "$repo/scripts/build-registry-core.sh"
