# 0611 — The first-party registry's operator tombstone

- **Status:** Accepted (v0.403.2)

**Context.** Track D8 settles takedowns as an operator-only tombstone: the line
stays, yanked, with an unsigned `withdrawn` reason, and the archive is deleted.
ADR 0593 (R5a.1) put `withdrawn` in the protocol, taught the client to fail as
`bynk.registry.withdrawn`, and gave the directory tool `bynk registry withdraw`.
The hosted service already refused to republish (`410`) or un-yank (`409`) a
withdrawn version, but nothing in it could make one, and the directory tool
can't reach an R2 bucket behind a `PackageIndex` agent. The review of R5.2
(#2391) also deferred restoring organisations to this slice. The accepted
proposal is #2397; who may withdraw, the log's visibility, the restore and the
quota refund were settled with the maintainer before filing.

**Decision.**

- **A. A `withdraw` capability on operator tokens.** An entry in
  `REGISTRY_TOKENS` whose `can` lists `withdraw` (with `organisations: []`) may
  take any version down. It is separate from `operate`, so the token that
  settles name disputes can't delete archives, and the reverse. Account and
  publish tokens are never operator tokens (`401`); an operator token without
  the capability is `403`.
- **B. The routes are the service's, outside the protocol.** `POST
  /_operator/withdraw/<org>/<package>/<version>` `{"reason"}`, `GET
  /_operator/takedowns` and `PUT /_operator/orgs/<org>` live in a new
  `service operator`, beside `/_status/export`, not under `/api/`: ADR 0593 F
  keeps withdrawal out of the protocol and track §14 rules out protocol changes.
  The prefix starts with `_`, which no organisation name can, so no operator
  route shadows the read path's `/<org>/<package>` (the proposal's `/operator/`
  would have hidden a package `operator/takedowns`).
  They share the account API's per-IP limit. The conformance suite and the
  protocol page are unchanged.
- **C. The takedown.** `PackageIndex.withdraw(version, reason)` sets `yanked`
  and `withdrawn` and rewrites the index file inside the call; re-signing
  nothing. The route then deletes `dl/sha256/<hex>` from the store and the
  backup, and gives the organisation's `OrgUsage` the archive's size back
  (`release`, floored at 0). The reason follows the directory tool's rules
  (trimmed, non-empty, at most 500 characters, no control characters; `400
  invalid_reason`). A withdrawal is final: a repeat is `409 withdrawn`, except
  that a repeat finishes an interrupted takedown, as ADR 0593 D's directory
  tool does, answering `200` with `archive: "finished"`. It deletes an archive
  left in either bucket. It refunds the bytes when the store still held the
  archive: the refund follows the store's delete, so that archive was never
  refunded. It logs a takedown the log doesn't hold yet.
- **D. No other line shares a withdrawn tree.** The directory tool scans every
  index for another line naming the same tree. Here admission requires the
  tree's `bynk.toml` to name exactly the line's package and version, and the
  checksum covers that file, so no two lines share a tree. The service checks
  only within the package (free inside the agent), and never lists the bucket.
- **E. Two races are closed.** A same-tree `PUT` that passed `outlook` before
  the takedown and stored its archive after it deletes that archive when
  `append` answers `Withdrawn` (safe by D). An export tick that read the archive
  before the takedown could copy it back into the backup, so each export pass
  ends by deleting every archive the log names from the backup.
- **F. An operator-only log.** A `Takedowns` agent appends one record per
  takedown that changed something: when, the acting token's sha256, the
  package, version, checksum, reason and the archive's fate (`deleted`,
  `missing`, `shared`, `finished`). `GET /_operator/takedowns` reads it, newest
  first, with a `withdraw` token; the export writes it to
  `agents/takedowns.json`. The store bucket gains no keys.
- **G. Organisation restore.** `PUT /_operator/orgs/<org>` with an `operate`
  token takes the export's `agents/orgs/<org>.json`. It restores an unclaimed
  name only (`409 claimed` otherwise). A repeat of the very state the name
  holds answers `200` and re-runs the idempotent indexing below, so an
  interrupted restore can be finished. It refuses an export of another name or
  with no owner (`400`), re-registers each trusted publisher with its
  `TrustedRepository`, and lists the name in the `Directory`. Publish tokens
  were never exported; members mint new ones.
- **H. Index lines in the protocol's shape (amends ADR 0597).** The service wrote each line with
  `Json.encode` (ADR 0597), whose `Option` is the tagged `{"kind": …}` form.
  A client reads `toolchain` and `withdrawn` only as strings, so a withdrawn
  line would have read as merely yanked, and `read.withdrawn` would have passed
  for want of a withdrawn line. `renderLine` writes `toolchain` as `null` or a
  string and `withdrawn` as a string present only when set; `parseIndex` reads
  both forms, so index files already written are adopted unchanged. This
  reverses ADR 0597's deviation "the index file is the codec's JSON": its
  `toolchain: null` held only because the codec's `{"kind": "None"}` also reads
  as absent.

**Consequences.** A takedown is one request, and what it leaves is checkable:
CI's `scripts/check-withdraw.sh` takes a version down and checks the
conformance read checks, a fresh client failing as `bynk.registry.withdrawn`
with the reason, a stored tree still building, the `410` and `409`, the backup
without the archive, and the log; and restores an organisation. A tree already
in a user's store keeps building, by design. A takedown doesn't reach
off-account copies of the backup, which the README says to apply it to. **R6
constraint:** when archives are cached at the edge, a takedown must purge the
archive there, or archives can't be cached as immutable. The publish API's own
JSON answers (the `201` line) keep the codec's form: clients verify the index,
not the answer.
