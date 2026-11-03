# 0719 — A case compares a value with a stored snapshot, and only the driver writes it

- **Status:** Accepted (v0.496)

**Context.** Asserting a large structured value meant a field-by-field `expect` or a
full expected literal, neither reviewable when the value is big nor stable when its
shape grows (#2842, found dogfooding a webhook receiver whose decoded payloads were
asserted a field at a time). Bynk had no snapshot or golden mechanism for user tests:
the only bless mechanism, `BYNK_BLESS=1`, is the compiler's own. Only the compiler
knows a value's type, and so its canonical encoding (`Json.encode`, `Option`s and sums
in their tagged form); a JavaScript-side snapshot library would see the emitted
representation instead.

**Decision.** `expect <value> matches snapshot "<name>"` is a form of `expect`. It is
a clause on the `expect` statement (`ExpectStmt.snapshot`), not a new expression, and
`matches` and `snapshot` are contextual words, like `faults`: ordinary names
elsewhere.

- **Where snapshots live (A).** `snapshots/<suite file path>.snap.json` under the
  project root, one file per suite file: the file's path from the project root,
  without `.bynk`. The file is one JSON object keyed by case name, then snapshot
  name; keys are sorted at every level and the text is indented two spaces, with a
  closing newline, so a change is a readable diff.
- **Reading (B).** The driver reads `snapshots/` and hands the text to the build,
  which inlines each suite file's snapshots into its test module. The harness reads
  nothing at test time (as the asset harness embeds its files). Only
  `bynk test --update-snapshots` writes, from the driver: the emitted runner reports,
  per snapshot file, its counts and the file's new text, and the driver writes after
  the run, only if the runner reported.
- **A missing snapshot (C).** It fails, so CI never passes on an unreviewed
  snapshot. Under the flag it is written, and counted *written*.
- **Encoding (D).** The value is encoded as `Json.encode` encodes it (the encode
  lowering is shared with the `Json.encode` call, so the two cannot drift), then
  parsed back and printed with sorted keys. A value that is not `Codable` is
  `bynk.test.snapshot_not_codable`. The name is a string literal
  (`bynk.test.snapshot_name_not_literal`), unique within its case
  (`bynk.test.snapshot_duplicate_name`), so the compiler knows every snapshot a
  suite takes.
- **Obsolete snapshots (E).** A snapshot whose case or name is gone, or whose file no
  suite takes a snapshot into, is reported as obsolete and deleted under the flag.
  Obsolescence is decided from the suite's source, not from the cases that ran, so a
  run filtered with `--case` or `--suite` neither loses nor reports the snapshots of
  the cases it skipped, and a filtered update keeps their stored values.
- **Reporting.** The written, updated and obsolete counts are carried in every
  output format: a last `snapshots` object on the `--format json` document and on
  the NDJSON `run-end` (absent for a project with no snapshot), and a `snapshots:`
  line in the `rich` output. An obsolete snapshot does not fail a run.
- **Scope.** A snapshot is taken in a `case` of a unit suite. It is refused in a
  `property` (`bynk.test.snapshot_in_property`) and in a `system`-tier case or an
  integration or system suite (`bynk.test.snapshot_needs_unit_suite`), which are
  emitted through other harnesses and compared against a real runtime.
  `--update-snapshots` is refused with `--against`, `--smoke`, `--coverage`,
  `--inspect` and `--no-run`: a snapshot is recorded from an in-process run of the
  project's own suites.

**Consequences.** The language gains a golden-file mechanism that reviews in the pull
request like code. The first run of a new snapshot fails until recorded, which is the
point: nothing passes unreviewed. A value with an instant or a random id is unstable;
`Clock` and `Random` are already deterministic under test, so stubbing them is the
answer, and a redaction option can come later. The test module carries the snapshot
runtime and the stored text only when a suite takes a snapshot, so every other
module is byte-for-byte unchanged. Tooling follows the new form: the formatter keeps
the clause, `matches snapshot` is offered after `expect <value>`, the two words get
the `keyword` semantic token, and hovering a claim shows the stored snapshot (or
that none is recorded). Snapshots in integration and system suites, and a redaction
option, are left for later.
