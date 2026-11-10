# YAML conformance corpus

A subset of the [yaml-test-suite](https://github.com/yaml/yaml-test-suite)
(data tag `data-2022-01-17`, commit `6e6c296ae9c9d2d5c4134b4b64d01b29ac19ff6f`),
MIT licensed (see `LICENSE`). Each directory holds one case's `in.yaml`, and, for a
case the parser accepts, its expected `in.json`.

`manifest.json` classifies every case drawn from the suite:

- `pass`: a valid single-document case the parser accepts; its result must equal
  `in.json` exactly.
- `refused`: a valid case outside the supported subset (anchors, aliases, tags,
  complex keys, multiple documents, and a few exotic layouts); it must stay refused,
  with the recorded `kind`, never partially decoded.
- `lenient`: a case the suite marks *invalid* that the parser still accepts. Each is
  a known leniency about indentation or comment placement, not a wrong value; the
  list is a ratchet, so a case leaves it only by being refused.

Not drawn from the suite: cases with several documents or no `in.json`.
Regenerate the manifest only after reviewing the diff: a case moving from `pass`
to anything else is a regression.
