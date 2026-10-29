#!/usr/bin/env bash
# British-English spelling lint for the Bynk Book.
#
# Scans the Book's Markdown for US spellings, ignoring fenced code blocks (so
# code identifiers are never flagged) and inline `code` spans. The changelog is
# skipped: its rows are written once at stamp time and quote identifiers,
# environment variables and issue titles verbatim. Exits non-zero, listing offenders, if any
# US spelling appears in prose. Edit the US:UK list below to extend it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)/src/content/docs/book"

# US spelling : suggested British spelling
WORDS="$(cat <<'EOF'
color:colour
colors:colours
behavior:behaviour
behaviors:behaviours
organize:organise
organized:organised
organizes:organises
organizing:organising
organization:organisation
organizations:organisations
initialize:initialise
initialized:initialised
initializes:initialises
initializing:initialising
initialization:initialisation
serialize:serialise
serialized:serialised
deserialize:deserialise
analyze:analyse
analyzed:analysed
center:centre
centered:centred
defense:defence
catalog:catalogue
canceled:cancelled
canceling:cancelling
modeling:modelling
modeled:modelled
labeled:labelled
labeling:labelling
fulfill:fulfil
favor:favour
favorite:favourite
normalize:normalise
optimize:optimise
optimized:optimised
recognize:recognise
specialize:specialise
summarize:summarise
minimize:minimise
maximize:maximise
artifact:artefact
gray:grey
traveled:travelled
EOF
)"

us_regex="$(printf '%s\n' "$WORDS" | cut -d: -f1 | paste -sd'|' -)"

found=0
while IFS= read -r file; do
  matches="$(awk -v re="$us_regex" '
    /^[[:space:]]*```/ { infence = !infence; next }
    infence { next }
    {
      line = $0
      gsub(/`[^`]*`/, " ", line)
      line = tolower(line)
      if (match(line, "(^|[^a-z])(" re ")([^a-z]|$)")) {
        print FILENAME ":" FNR ": " $0
      }
    }
  ' "$file")"
  if [ -n "$matches" ]; then
    echo "$matches"
    found=1
  fi
done < <(find "$ROOT" \( -name '*.md' -o -name '*.mdx' \) ! -path '*/reference/changelog.md' | sort)

if [ "$found" -ne 0 ]; then
  echo ""
  echo "British-English lint failed: replace the US spellings above with British forms."
  exit 1
fi
echo "British-English lint: OK — no US spellings in prose."
