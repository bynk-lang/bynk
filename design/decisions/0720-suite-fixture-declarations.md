# 0720 — A suite declares test-only fixture files, read from fixtures/

- **Status:** Accepted (v0.497)

**Context.** A test that needs a realistic input had to write it as an escaped string literal in the case. The one file-reading declaration, `embed` (#2387), is production-only: a suite cannot declare one, its file must live in `embeds/` (the folder that ships in the Worker), and it is capped at 256 KiB. The workarounds, an `embed` in the target or a payload pasted into a production commons, leak test data into the deployable (#2841). Different contracts are different declarations (ADR 0608 F).

**Decision.**
- **A new declaration.** `fixture name = "fixtures/…"` in a suite body, with `fixture` a contextual keyword (an identifier everywhere else), parsed beside `env`. It is not `embed` allowed in suites.
- **Folder.** The path is relative to the project root and inside `fixtures/`, mirroring `embeds/` and `migrations/`. The path rules are an `embed`'s (`bynk-check`'s `unsafe_asset_path_reason`), so a declaration cannot reach `.env` or a source file; the folder differs (`bynk.fixture.outside_fixtures_dir`, `unsafe_path`, `missing_file`, `unreadable_file`).
- **Type and content.** A `String`. The content rules are reused from `embed` (`embedded_text`: UTF-8, no NUL, BOM dropped, newlines normalised), reported under `bynk.fixture.invalid_encoding` and `binary_content`. `Bytes` and typed fixtures are later increments.
- **Limits.** 4 MiB a file (`bynk.fixture.too_large`) and no project total: a fixture is never deployed, so the 1 MiB bundle budget does not apply.
- **Scope.** The declaring suite (one `SuiteDecl`, as helpers and `stub` are), at every tier. Its name is a case-scope value: a repeat is `bynk.fixture.duplicate`, a clash with a name in scope `bynk.resolve.name_conflict`. A fixture nothing mentions is the warning `bynk.fixture.unused`.
- **Mechanics.** The checker adds a fixture to the suite's per-suite view as the `String` constant it already knows an `embed` to be, so name resolution, typing and lowering are unchanged. The text is read once per file (`CompileOptions::fixture_files`, listed lazily like `embeds/`) and written once into the test module as a module-level `string` constant; each runner binds the suite's names to it. The deployable is built without suites, so it never contains a fixture, and is byte-identical with or without one.
- **Tooling.** A fixture is indexed as a constant in the suite's namespace, so hover, go-to-definition, references and semantic tokens work; completion offers the suite's fixtures and the files under `fixtures/` in the declaration; `bynk dev` and the language server watch `fixtures/`.

**Consequences.** A real captured payload can drive a test without inlining or shipping it. A fixture's text is in a test module once, however many runners and suites read it. Hover shows the path and "test-only; never deployed" but not the file's size, which hover cannot read. Sharing a fixture across suites waits for a test-only module.
