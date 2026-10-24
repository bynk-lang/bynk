# 0612 — The first-party registry is extracted to its own repository before going live

- **Status:** Accepted (v0.403.4)

**Context.** ADR 0591 put the hosted registry in this repository at
`services/registry/` under the track's extraction rules E1–E5, "extracted after
go-live". Track §1.3 named the trigger (Q7): go-live, then two consecutive
`bynk-std` releases that needed no change for the service. Keeping it here meant
a capability landed with its first use, a protocol change landed in one review,
and a compiler change that broke the service failed the PR that made it. On
2026-10-24 the maintainer required the service to live in its own repository in
the `bynk-lang` organisation before it goes live. The accepted proposal is
#2415; the extraction before go-live, the dropped trigger and the repository's
name were the maintainer's calls.

**Decision.**

- **A. Extract before go-live, to `bynk-lang/registry`; Q7's trigger is
  dropped.** R7 precedes R6 (#2413). This amends ADR 0591's "extracted after
  go-live", and track §1.3, §8 (D11), §9 (D1, D11), §10, §11 Q7 and §13.
- **B. The history comes along.** `services/registry/`'s commits were split
  out of `main` (at 709159f72, whose tree the split matches exactly) and pushed
  as `bynk-lang/registry`'s first `main`. Their messages keep citing this
  repository's issues, which the new README says.
- **C. A pinned toolchain release.** `bynk-toolchain` names one final `bynk`
  release (first v0.403.3). CI and deploys install its `bynk`, `bynkc` and
  `bynk_registry_core-<tag>.wasm`, each checked against its `SHA256SUMS`;
  nothing is built from this repository's source. Bumping it is a one-line PR
  whose CI is the proof. A toolchain-enforced pin (a `bynk.toml` key) was
  rejected for now: it is a feature of its own, not needed to extract.
- **D. A non-blocking canary here.** `registry-canary.yml` runs daily and on
  dispatch: it builds `main`'s toolchain and admission module and runs the
  service's own checks against `bynk-lang/registry`'s `main`. A failure opens
  or updates one `registry-canary` issue; a pass closes it. A required check
  was rejected, because a red service would block unrelated compiler PRs on
  another repository's state; no canary was rejected, because breaks would
  surface only at the next pin bump.
- **E. The deploy guard judges the registry's own tags.** Only a `v<semver>`
  tag with a published release on `bynk-lang/registry`'s `main` deploys (a
  candidate to staging only). The toolchain is what that commit pins, and must
  be a published, final `bynk` release; the guard pins its digests. D11's
  agreement between the deployed service and its toolchain is now held by the
  pin, not by a shared tag.
- **F. The order of the move.** The repository was created and its history
  pushed; a ruleset (no deletion or force push, PRs only, then `CI green`
  required) was added after the push it would have blocked; the import PR
  (bynk-lang/registry#1) went green on v0.403.3 and was merged; only then did
  this repository delete `services/registry/`, its `registry-service` CI job
  and `deploy-registry.yml`.
- **G. Kept here.** `bynk-registry-core` and its parity checks, the
  `registry-core-wasm` release asset the service installs, `bynk registry
  conformance`, the reference server, the protocol spec, and the track's spine
  and proposals.
- **H. The extraction rules.** E1–E4 are now true by construction and live in
  `bynk-lang/registry`'s README as its rules. E5 (no registry dependencies)
  stays forever, checked by its CI.

**Consequences.** A capability the service needs lands here, ships in a
release, and the service bumps its pin; service changes are PRs on
`bynk-lang/registry`. A compiler regression that would break it is reported by
the canary within a day instead of blocking a merge. The service's GitHub
environments, secrets, tag ruleset and immutable releases (R6's setup) belong to
`bynk-lang/registry`. The changelog's earlier rows that name
`services/registry/` stay as written: they describe what shipped then.
