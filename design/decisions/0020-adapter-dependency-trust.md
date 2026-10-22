# 0020 — Adapter npm-dependency trust policy

- **Status:** Superseded by the `adapter-npm-approval` ADR (#2296): a fetched dependency's adapter's npm
  packages enter a build only once the project approves them in `bynk.lock`.
- **Spec:** §8.6 records the pinning floor

## Context
Third-party library adapters are a goal, and an adapter's `requires` is a
supply-chain surface — a malicious adapter could declare any dependency.

## Decision (direction only)
Declared dependencies are pinned (0013) and surfaced for review. The full
allow-list / confirmation policy — what a consumer must approve before an
adapter's dependencies enter their `package.json` — is **not yet decided**.

## Consequences
Until settled, reviewing an adapter's `binding … requires { … }` line is the
trust boundary.
