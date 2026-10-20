# 0544 — A workspace is a virtual root that groups projects, with no identity

- **Status:** Accepted (v0.374)

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
