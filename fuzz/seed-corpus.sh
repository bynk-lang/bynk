#!/usr/bin/env sh
# Seed the fuzz corpora from the fixture corpus (#516): every `.bynk` source
# in the test fixtures, the examples, and the compiled doc surfaces. Real
# programs let the fuzzer start from deep in the grammar instead of
# rediscovering `commons` one byte at a time.
set -eu
cd "$(dirname "$0")"

for target in parse compile; do
  mkdir -p "corpus/$target"
done

i=0
find ../bynk-tests/tests/fixtures ../examples -name '*.bynk' -type f | while read -r f; do
  i=$((i + 1))
  for target in parse compile; do
    cp "$f" "corpus/$target/seed-$i.bynk"
  done
done

echo "seeded: $(ls corpus/parse | wc -l) inputs per target"
