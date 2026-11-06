#!/bin/bash
# A pinned release, given without its leading v, installs exactly that tag for
# the binaries it ships.
set -e

source dev-container-features-test-lib

check "bynk is 0.330.4" bash -c "bynk --version | grep -Fq '0.330.4'"
# The published release may predate the rename (bynkc-lsp only); either name will do.
check "a language server (bynk-lsp or bynkc-lsp) is executable on PATH" bash -c "test -x \"\$(command -v bynk-lsp)\" || test -x \"\$(command -v bynkc-lsp)\""
check "bynk doctor --strict" bash -c "bynk doctor --strict"

reportResults
