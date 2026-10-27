# 0636 — The editor manages dependencies and the registry only through the driver

- **Status:** Accepted (v0.425)

**Context.** The driver already adds, fetches and publishes registry
dependencies (`bynk add`, `bynk fetch`, `bynk publish`), but none of it was
reachable from the editor, and `bynk.toml` gave no sign of a newer version. A
registry is a static, signed, per-package index with no listing or free-text
search endpoint, and routing, trust (the pinned or locked key), tokens and
caching all live in the driver's `Client`. The first-party registry is not live
yet (#2413), so the work has to cope with registries routed in `[registries]`
alone.

**Decision.** (A) The extension never speaks HTTP to a registry. It asks the
driver: `bynk search <org/package> --format json` (an exact lookup returning
every version and the newest usable one), `bynk deps --format json` (the
manifest's dependencies joined to `bynk.lock`, offline), and `--format json` on
`add`, `fetch` and `publish`, whose documents include the file list a publish
would send. (B) Upgrade lenses in `bynk.toml` are on by default
(`bynk.dependencies.upgradeLenses`), look each package up at most once per
session in the background, and show nothing when the lookup fails, so an
offline registry never blocks or nags. (C) **Bynk: Publish…** is plan-first:
`bynk publish --dry-run --format json` runs first and its package identity,
version, registry, checksum and file list are shown in a modal confirmation; only
its **Publish** button runs the real publish.

**Consequences.** Auth, trust and caching stay in one place, and the editor
needs no registry client. "Search" is an exact `org/package` lookup until a
registry offers a listing, so there is no discovery by keyword and no completion
of package names in `[dependencies]` (deferred). Against the first-party
registry, the features degrade to the driver's `bynk.registry.unavailable`
message until it is live.
