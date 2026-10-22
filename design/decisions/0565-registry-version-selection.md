# 0565 — A registry package resolves to the highest version every range admits, without backtracking

- **Status:** Accepted (v0.383)

**Context.** Range syntax validation was moved to this slice (#2223, #2240); track D6 is single-version
resolution.

**Decision.** A `version` is a Cargo-style semver range (`^1.4`, `~1.4.2`, `=1.4.2`, comparator lists; a bare
`1.4` is `^1.4`), validated as the manifest is read. Each registry package resolves to the highest non-yanked
version satisfying every range reaching it — from the project, workspace members and other packages — whose
`toolchain` this `bynk` meets; a chosen version's own dependencies add ranges, and resolution repeats until
none appears. The locked version wins while it satisfies every range, even when yanked; `bynk update`
re-chooses. No version satisfying every range is `bynk.package.version_conflict`, naming each requirer and
range. There is no backtracking search.

**Consequences.** An unsolvable-looking graph that a backtracking resolver could solve is reported, not solved;
that is a recorded follow-on if real graphs need it.
