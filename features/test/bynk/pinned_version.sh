#!/bin/bash
# A pinned release, given without its leading v, installs exactly that tag for
# all three binaries.
set -e

source dev-container-features-test-lib

check "bynk is 0.330.4" bash -c "bynk --version | grep -Fq '0.330.4'"
check "bynkc is 0.330.4" bash -c "bynkc --version | grep -Fq '0.330.4'"
check "bynkc-lsp is executable on PATH" bash -c "test -x \"\$(command -v bynkc-lsp)\""
check "bynk doctor --strict" bash -c "bynk doctor --strict"

reportResults
