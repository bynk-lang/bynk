# 1098 — The revert plan's vocabulary is documented, and missing credentials are a structured error under JSON

- **Status:** Accepted (v0.820.1)

**Context.** #3797, part 4. The revert plan has been machine-readable since it gained `--format json`, but the reference said only "`json` is for CI": the check names, the verdict words and the `bynk.revert.*` codes were pinned by a golden test and nowhere a client could read. A revert also needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` before it reads anything, and without them a JSON client got an empty stdout and a line of prose on stderr.

**Decision.** (A) The reference documents the plan: its fields, the two `verdict` words (`ok`, `refused`), the `code` a refusal carries, and a table of each `check` with what it looks at and its code. (B) A new registered code, `bynk.cloudflare.credentials_missing`, is the `error.code` of a document a revert writes to stdout under `--format json` when a credential is unset; `missing` names each unset variable and `message` is the stderr text. The stderr line and the exit status do not change, and a run in the short format prints no document.

**Consequences.** Additive for a JSON consumer. Only `bynk revert` writes the document: a deploy that applies has already printed its plan when the account checks run, so its credential failures stay on stderr, and `bynk deploy --plan` reads no account without credentials and says so as a warning.
