# 0543 — Package identity is `organisation/package` in the manifest, and no unit is renamed

- **Status:** Accepted (v0.374)

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
