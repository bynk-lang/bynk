---
level: minor
probes: emit_diagnostics, diagnostic_coverage
changelog: Packaging slice 2b — git dependencies (`{ package, git, rev }`) are fetched by the user's own hardened `git` into a per-user content-addressed store, and `bynk.lock` pins each to a commit and a sha256 checksum of its files; `bynk fetch` and `bynk update` are new, `--locked` and `--offline` make `check`, `test`, `dev` and `deploy` refuse a lock change or a fetch, a workspace shares one lock, and `[dev-dependencies]` resolve for suites only and never reach a deployable build. `bynkc` and the editor read the lock and the store and never fetch
---

## ADR: the-lock-and-the-store
title: `bynk.lock` pins each git dependency to a commit and a canonical-tree sha256, kept in a per-user store
summary: Packaging slice 2b — the lock format; the canonical-tree checksum; the content-addressed, read-only store; verification before every `bynk` build

**Context.** Slice 2a (#2223) resolved path dependencies, which need no lock: they are the author's own
working copy. A git dependency is someone else's code at a moving `rev`, so a build is reproducible only if
something records which commit it was and what its files were (track D5, D7, §5.2).

**Decision.**

- **The lock.** `bynk.lock` (version 1) has one `[[package]]` per resolved package, in a fixed order: `name`,
  `version` (its `[package] version`, when declared), `source` and `dependencies`, and for a git package a
  `checksum`. A git `source` is `git+<url>?rev=<rev>#<commit>` — what was asked for and what it resolved to;
  a path package is `path+<dir>` and is never hashed. A lock with no `version`, or another version, is
  refused (`bynk.package.lock_invalid`) rather than read as empty, as `bynk.deploy.lock` refuses one.
- **Holding.** A locked git package keeps its commit while its manifest `{ git, rev }` is unchanged; a branch
  that moves upstream moves the build only through `bynk update`. Changing `rev` re-resolves.
- **The checksum** is `sha256-<base64>` over the package's canonical tree: every regular file except under
  `.git`, in byte-wise sorted `/`-separated path order, each contributing `path \0 length \0 bytes`. It is
  computed on a checkout made with `core.autocrlf=false` and `core.symlinks=false`, so it is independent of
  git's object format and the platform; symbolic links contribute nothing.
- **The store** is `$BYNK_HOME/store/sha256/<hex>/` (`BYNK_HOME` defaulting to `$XDG_CACHE_HOME/bynk`, else
  `~/.cache/bynk`): an entry is staged, hashed, renamed into place and made read-only. Before any `bynk`
  command compiles, each locked git package's entry is re-hashed against the lock
  (`bynk.package.checksum_mismatch`). `sha2` is a dependency of the `bynk` CLI only; the compile reads
  checksums and never hashes. FNV-1a stays for contract hashes.

**Consequences.** A committed lock plus a warm store reproduces a build with no network. `bynk vendor`
(copying the store into the project) is deferred to slice 4.

## ADR: the-fetch-boundary
title: Only the `bynk` CLI fetches; the compile reads the lock and the store
summary: Track D4 made concrete — `bynk` resolves, fetches and locks before compiling; `bynkc`, the language server and the playground never fetch; `--locked` and `--offline` refuse rather than repair

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

## ADR: one-lock-per-workspace
title: A workspace has one `bynk.lock`, and one version of each package
summary: The lock lives at the workspace root and covers every member's graph; two members reaching one package at two revisions is `bynk.package.version_conflict`

**Context.** Track §4.2 gives a workspace one root lock; #2223's comment point 8 deferred whole-workspace
resolution to this slice.

**Decision.** A member's lock is its workspace root's `bynk.lock`, resolved across every member's
manifests, so members agree on one commit of each package; reaching one package at two revisions (or from
git and a path) is `bynk.package.version_conflict`. Each member's graph is still compiled per member. A
standalone project's lock sits beside its `bynk.toml`.

**Consequences.** Running any `bynk` command in any member keeps the whole workspace's lock current.

## ADR: dev-dependencies-reach-only-suites
title: `[dev-dependencies]` resolve with the graph, reach only suites, and never a deployable build
summary: The project's own `[dev-dependencies]` are resolved and locked; only a suite (its `uses` or its target) may reach one, else `bynk.package.dev_dependency_in_source`; a build that compiles no suites compiles none of them

**Context.** Slices 1 and 2a refused a non-empty `[dev-dependencies]` until this slice.

**Decision.** Only the project's (or member's) own `[dev-dependencies]` are read — a dependency's are its
own business. They resolve and lock with the rest of the graph. A reference through a dev-only alias is
allowed only in a suite; from a commons, context or adapter it is `bynk.package.dev_dependency_in_source`.
Slice 2a's reach-closure roots at suites only when the build checks them (`tests`), so a deployable build
compiles, and emits, no dev-dependency unit.

**Consequences.** A suite can target a dev-dependency's unit directly (`suite fx.sample`).
