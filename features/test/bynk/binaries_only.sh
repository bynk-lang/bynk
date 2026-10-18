#!/bin/bash
# Without Node, and with the npm tools switched off, the feature still installs
# the binaries and the compile floor holds (bare `bynk doctor` exits 0).
set -e

source dev-container-features-test-lib

check "bynk on PATH" bash -c "bynk --version"
check "bynkc on PATH" bash -c "bynkc --version"
check "bynkc-lsp is executable on PATH" bash -c "test -x \"\$(command -v bynkc-lsp)\""
check "no wrangler installed" bash -c "! command -v wrangler"
check "bynk doctor (compile floor)" bash -c "bynk doctor"

reportResults
