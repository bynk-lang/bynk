---
level: minor
changelog: Packaging slice 1 — `bynk.toml` gains `[package]` (an `organisation/package` identity, with `exports` checked by `bynk check`), `[dependencies]` and `[dev-dependencies]` (checked, then refused until resolution ships), `[registries]` (checked configuration) and `[workspace]`, whose root runs `bynk check`, `fmt` and `test` across its members
---

## ADR: shareable-unit-set
title: A package shares commons and adapters, never a context
summary: The shareable-unit set — `[package] exports` lists commons and adapters the project declares; contexts are never shared

**Context.** The packaging track (`design/tracks/packaging.md`, spine #843, settled in #2199) makes a
Bynk project shareable as a package. Its D1 keeps the spine's answer to "what can a package share?",
and slice 1 (#2207) is where the manifest first names the shared units. Two facts force the answer.
A **context** is a deployment unit: its Worker, its secrets and every deploy-ledger key derive from
its name and belong to the project that deploys it (`deployed_worker_name`, `bynk.deploy.lock`). A
**commons** or **adapter** is code and a host boundary: it has no deployed identity of its own, so it
can be compiled into any consumer. Behavioural library units (agents a consumer mixes into its own
context) are a later question, gated on the agent-capability-encapsulation track (track slice 6).

**Decision.**

- **The set.** A package shares **commons** and **adapters** (with their bindings and `requires`).
  A **context is never shareable**, so no dependency ever produces a Worker. A package may still hold
  contexts, for examples or its own tests; they are simply never exported.
- **`exports`.** `[package] exports` lists the shared units by their dotted names. Absent, it means
  every commons and adapter the package declares. Its shape (a list of unit names) is checked with
  the rest of the manifest, as a `ProjectPathsError`. Whether each entry names a commons or adapter
  the project declares needs the unit table, so `bynk check` checks it once units are known, as two
  coded project-level errors attributed to `bynk.toml`: `bynk.project.export_context` for a context,
  and `bynk.project.export_unknown` for a name the project doesn't declare (a first-party unit
  included), with a did-you-mean note. This is the one packaging-manifest rule that is a coded
  diagnostic (#2207 decision F).
- **One reading.** `bynk_project::read_package_config` reads `[package]` from the overlay's
  `bynk.toml` or the one on disk, as `[unstable]` is read (ADR 0468 D4). Both `run_checks`
  (`bynkc`, `bynk check`) and `analyse_project` (the language server) run the check after grouping,
  so the build and the editor agree.

**Consequences.**

- Adding a context to a package never changes what its consumers can reach.
- Under a `BYNK_BYNKC` override pinned to a `bynkc` older than this slice, `[package]` is an unknown
  table and the check fails loudly; it is never silently skipped.
- Slice 2 enforces the set from the consumer's side: only exported (or, absent `exports`, every
  shareable) unit of a dependency resolves through its alias. Slice 6's behavioural units extend the
  set by their own ADR.

## ADR: package-identity-without-renaming
title: Package identity is `organisation/package` in the manifest, and no unit is renamed
summary: Package identity without renaming — `[package] name` is `org/package`; unit names stay as declared; dependency units are reached as `<alias>.<unit>`; names are restricted to what they map to

**Context.** The spine's slice 0 planned a breaking rename of every unit to
`organisation.package.unit`. The track re-settled it (D2, §3.3): a rename would change every deployed
Worker's name and orphan its ledger entries, it contradicts the standard library's "unbundling renames
no unit" commitment (P-3), and package scoping buys the same collision safety. Slice 1 (#2207) is where
the identity first appears in `bynk.toml`, and its review added the naming rules that make an alias
always writable in source (decision G).

**Decision.**

- **Identity.** A shareable project declares `[package] name = "organisation/package"`. The
  organisation is the unit of ownership and registry routing; the package is the unit of versioning.
  `[package] version`, when present, is strict semver 2.0 (publishing will require it);
  `description`, `license` and `repository` are free strings.
- **No renaming.** A unit keeps the dotted name it declares in source, unique within its package. A
  consumer reaches a dependency's unit as `<alias>.<unit>`, where the alias is the dependency's key
  in `[dependencies]`. The first-party surface keeps its reserved full names (`bynk`,
  `bynk.cloudflare`; ADR 0012) and is not an alias.
- **`[project]` and `[package]` are exclusive.** A manifest with both is refused, so a project never
  has two names. `[project]` stays for applications, display-only; `bynk new` still writes it.
- **Names map to where they are used (decision G).**
  - A **package segment** matches `[a-z][a-z0-9_]*`, at most 64 characters, and isn't a Bynk
    keyword: an alias defaults to it (track §3.3) and is written in source, where `-` lexes as minus.
    A hyphenated name is refused with the underscore spelling suggested. Widening later to a
    `-`→`_` mapping would add names and break none; narrowing would break names, so the rule starts
    strict.
  - An **organisation segment** matches `[a-z][a-z0-9-]*`, at most 64 characters: it never appears
    in source, only in manifests, the lock and registry URLs, and matching GitHub-style names helps
    claim-blocking. The organisation `bynk` is reserved for first-party packages.
  - An **alias** matches the package-segment grammar, isn't a keyword, and isn't `bynk`.
  - A **registry name** matches `[a-z][a-z0-9-]*` and isn't `bynk`, which `mirror-of = "bynk"`
    already means. Its token variable is `BYNK_REGISTRY_<NAME>_TOKEN`, the name uppercased with `-`
    as `_`; since a registry name holds no `_`, the mapping is injective and no two registries share
    a variable.
- **Slice 1 validates, nothing resolves.** `[dependencies]` and `[dev-dependencies]` entries are
  shape-checked (exactly one of `version`, `git` with `rev`, or `path`; a well-formed `package`; a
  pinned `version`) and a non-empty table is then refused until slice 2 can resolve it, rather than
  accepted and ignored (#1665). Validating version-range syntax is slice 2's, with the resolver that
  reads ranges. `[registries]` is checked configuration (https-only index, one registry per
  organisation, one-level mirrors, an `ed25519:` key pin, never a credential) that nothing reads
  until slice 4.

**Consequences.**

- No deployed Worker or ledger key changes for any existing project.
- An alias colliding with the first segment of a consumer's own unit needs the unit table and an
  alias that can be used, so its check lands in slice 2.
- Manifest errors stay `ProjectPathsError` messages (#2207 decision F); the dotted names in the track
  doc (`project_and_package`) name variants, not diagnostic codes.

## ADR: workspaces
title: A workspace is a virtual root that groups projects, with no identity
summary: Workspaces — a `[workspace]` root holds only `[workspace]`/`[registries]`; members are globs minus `exclude`; `check`, `fmt` and `test` run across members, `dev`/`deploy` refuse at the root

**Context.** The spine listed `workspace` as an identity tier; the track (§3.2, §4.2) made it a build
grouping only, since a package's identity must survive moving between repositories. Slice 1 (#2207,
decision C) gives `[workspace]` its first behaviour so that a parsed-but-inert table doesn't recreate
the silence #1665 removed.

**Decision.**

- **A virtual root.** A workspace root's `bynk.toml` holds `[workspace]` and may hold `[registries]`;
  any other table, and any source at the root (`src/` or a `.bynk` file), is refused. A root is
  never a project, and `bynkc` refuses to build one.
- **Members.** `members` patterns, minus `exclude` patterns, relative to the root. A component may
  use `*` and `?`; `*` matches within one directory level, and `**` is refused. A hidden directory
  matches only a component that itself starts with `.`. A literal entry that doesn't exist, a member
  with no `bynk.toml`, and a workspace that matches no member are errors.
- **Commands.** At the root, `bynk check`, `bynk fmt` and `bynk test` run over every member in path
  order; every member runs, and the command fails if any does. `rich` output and `bynk test`'s human
  output head each member with `── <path> ──`; `--format short` prints no headers, with paths
  relative to where the command ran; `bynk fmt` names files the same way. `bynk test --format json`,
  `--output` and `--against` each belong to one project and are refused at a root. `bynk dev` and
  `bynk deploy` refuse at a root ("run inside a member"); deploy state stays per member. Inside a
  member every command behaves as in a standalone project.
- **Detection from a member.** A project belongs to the workspace above it whose `[workspace]`
  parses, has no unknown table, and lists it after pattern expansion; mere ancestry never counts,
  and a broken unrelated parent counts as absent. It is used only to refuse a member's own
  `[registries]` (registries are workspace-wide) and a member's own `[workspace]` (no nesting).
- **No identity.** A workspace doesn't appear in any name, lock entry or deployed resource.

**Consequences.**

- No existing project changes: `[workspace]` was refused before this slice.
- The language server still doesn't call the whole-manifest check (by design, `paths.rs`); it
  serves each member as a project, and a root opened in an editor has no sources.
- Slice 2 adds the workspace's shared root `bynk.lock` and path dependencies between members (#2207
  decision B moved the lock skeleton there).
