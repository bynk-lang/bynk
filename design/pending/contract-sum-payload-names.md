---
level: minor
changelog: A service contract's canonical form names a sum variant's payload fields (`A(x: Int)`), so renaming one changes the contract hash
---

## ADR: contract-sum-payload-field-names
title: A service contract names sum payload fields
summary: Contract mode renders a variant payload as sorted `name: type`, as the storage modes already do

**Context.** The wire format keys a sum's payload by field name
(`{"kind":"Circle","radius":0.5}`), but `Mode::Contract` rendered a payload as
its types only, positionally. `| A(x: Int)` and `| A(y: Int)` therefore had the
same `service_normal_form` and `contract_hash`, so renaming a payload field was a
wire break that neither the `X-Bynk-Contract` check nor the deploy skew gate
(ADR 0491) could see. Records already name and sort their fields, and the storage
renderings already render sum payloads as sorted `name: type` (#2204).

**Decision.** In every mode a variant payload renders as `name: type`, sorted by
that text, so `A(x: Int, y: String)` and `A(y: String, x: Int)` agree (JSON
object order is not observable) while a rename differs. The compatibility tree
(`contract_compat`) carries named payload `Field`s, the ledger text form encodes a
payload as `F(name, type)`, and the classifier compares payloads by name: any
difference in the set of payload field names is breaking (it errs towards
breaking, since it does not model payload defaults), and fields that match are
compared with the ordinary `flows` rule. The wire-format reference's normal-form
description is updated to match.

**Consequences.** Every contract whose services reach a sum type with a payload
rehashes once. Deployed contracts therefore skew against a build from the new
toolchain, so the first deploy after upgrading needs a whole-project deploy, as
with ADR 0200 and the contract-skew guide describe for v0.177. Ledger shapes
written by the old text form no longer parse and are treated as unclassified until
that deploy records them again. Contracts with no sum payload are unaffected.
