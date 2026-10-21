# 0554 — Only the `bynk` CLI fetches; the compile reads the lock and the store

- **Status:** Accepted (v0.379)

**Context.** Standard-library P-1 keeps builds hermetic; track §6.4 puts the network in one place.

**Decision.**

- `bynk check`, `test`, `dev`, `deploy`, and the new `fetch` and `update`, resolve the project's (or its
  workspace's) manifests against `bynk.lock` first: a locked package missing from the store is fetched at its
  commit, a new or changed one is resolved from its `rev`, and the lock is rewritten when the graph changed.
- `bynk_project::resolve_graph` maps a git dependency to its store entry through the lock, read overlay-first
  as the manifests are. With no lock entry or no store entry it reports one `bynk.package.not_fetched`
  naming `bynk fetch` — and the references through that alias are dropped, not re-reported as unknown units.
  `bynkc`, the language server and the playground therefore never fetch.
- `--locked` fails (`bynk.package.lock_out_of_date`) on any lock change; `--offline` fails on any fetch.
  Both refuse; neither repairs.
- **Git is hardened** (track §6.3): the user's own `git`, with `protocol.allow=never` and only `https` and
  `ssh` re-allowed (`file` only from the project's own manifests), `core.hooksPath=/dev/null`, no submodules,
  `GIT_TERMINAL_PROMPT=0`, `--` before positional arguments, and a checkout by `read-tree` +
  `checkout-index`, which run no hooks. A URL or revision that looks like an option, or an `ext::`/`fd::`
  transport, is refused (`bynk.package.unsafe_source`). Bynk passes no credentials. A fetched package's own
  path dependencies and `file:` git dependencies are never honoured (`bynk.dependency.path_outside_root`).

**Consequences.** `bynk doctor` reports `git` for a project with git dependencies. A failed clone or a
missing revision is `bynk.package.git_failed`.
