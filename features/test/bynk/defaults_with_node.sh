#!/bin/bash
# The proposal's acceptance check (#2103): with the Node feature and the
# feature's defaults, `bynk doctor --strict` is all green.
set -e

source dev-container-features-test-lib

check "bynk on PATH" bash -c "bynk --version"
check "bynkc on PATH" bash -c "bynkc --version"
check "bynkc-lsp is executable on PATH" bash -c "test -x \"\$(command -v bynkc-lsp)\""
check "wrangler 4" bash -c "wrangler --version | grep -Eq '(^|[^0-9])4\.[0-9]+'"
check "tsc 7" bash -c "tsc --version | grep -Eq 'Version 7\.'"
check "bynk doctor --strict" bash -c "bynk doctor --strict"

reportResults
