# 0553 — `bynk.lock` pins each git dependency to a commit and a canonical-tree sha256, kept in a per-user store

- **Status:** Accepted (v0.379)

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
