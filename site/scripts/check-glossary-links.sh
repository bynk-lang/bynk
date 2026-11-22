#!/usr/bin/env bash
# Glossary first-use linking lint for the Bynk Book (advisory).
#
# Scans the Book's **/*.md (ignoring fenced code blocks) for glossary terms that
# appear on a page which carries NO link to that term's glossary entry. It is a
# guide for catching up first-use linking page by page — it is ADVISORY: it
# prints findings and exits 0. Set GLOSSARY_LINK_STRICT=1 to exit non-zero when
# there are findings (opt-in, e.g. for a focused check).
#
# It deliberately does not auto-link: glossary terms are common words and would
# false-positive on substrings and code. A human decides each first use.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)/src/content/docs"
GLOSSARY_REL="reference/glossary.md" # under the Reference since slice 5

# slug ::: case-insensitive(1/0) ::: regex ::: display
TERMS="$(cat <<'EOF'
term-opaque-type:::1:::opaque type:::opaque type
term-refined-type:::1:::refined type:::refined type
term-sum-type:::1:::sum type:::sum type
term-branded-type:::1:::branded type:::branded type
term-refinement-predicate:::1:::refinement predicate:::refinement predicate
term-project-vs-legacy-mode:::1:::legacy mode:::project vs legacy mode
term-agent:::1:::agents?:::agent
term-context:::1:::contexts?:::context
term-service:::1:::services?:::service
term-capability:::1:::capabilit(y|ies):::capability
term-provider:::1:::providers?:::provider
term-commit:::1:::commits?:::commit
term-record:::1:::records?:::record
term-enum:::1:::enums?:::enum
term-admission:::1:::admission:::admission
term-zeroable:::1:::zeroable:::zeroable
term-effect:::0:::Effect:::Effect
term-result-option:::0:::Result|Option:::Result / Option
term-mock:::0:::Mock\\[:::Mock[T]
term-of-unsafe:::0:::\\.(of|unsafe)([^A-Za-z]|$):::.of / .unsafe
EOF
)"

# Does the term's regex appear in prose (outside fenced code blocks)?
prose_has() {
  awk -v re="$2" -v ci="$3" '
    /^[[:space:]]*```/ { infence = !infence; next }
    infence { next }
    {
      line = (ci == "1") ? tolower($0) : $0
      pat  = (ci == "1") ? "(^|[^a-z])(" tolower(re) ")([^a-z]|$)" : re
      if (match(line, pat)) { found = 1; exit }
    }
    END { exit(found ? 0 : 1) }
  ' "$1"
}

total=0
while IFS= read -r file; do
  rel="${file#"$ROOT"/}"
  [ "$rel" = "$GLOSSARY_REL" ] && continue
  findings=""
  while IFS= read -r term; do
    [ -z "$term" ] && continue
    slug="${term%%:::*}"
    rest="${term#*:::}"; ci="${rest%%:::*}"
    rest="${rest#*:::}"; re="${rest%%:::*}"; display="${rest##*:::}"
    if prose_has "$file" "$re" "$ci"; then
      if ! grep -qF "${GLOSSARY_REL##*/}#${slug}" "$file"; then
        findings="${findings}    ${display}  (#${slug})"$'\n'
      fi
    fi
  done <<< "$TERMS"
  if [ -n "$findings" ]; then
    echo "$rel:"
    printf '%s' "$findings"
    total=$((total + 1))
  fi
done < <(find "$ROOT/book" "$ROOT/reference" -name '*.md' | sort)

echo ""
echo "Glossary-link lint: $total page(s) have glossary terms with no first-use link."
echo "(Advisory — link the first occurrence on each page to ${GLOSSARY_REL}#term-<slug>.)"

if [ -n "${GLOSSARY_LINK_STRICT:-}" ] && [ "$total" -ne 0 ]; then
  exit 1
fi
exit 0
