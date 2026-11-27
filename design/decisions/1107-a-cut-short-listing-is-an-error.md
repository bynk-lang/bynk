# 1107 — An account listing cut short before its end is an error, never a shorter list

- **Status:** Accepted (v0.828.1)

**Context.** 1D.4c (#4568) bounded the Vectorize index listing, which the API reference documents no paging for: at most `MAX_PAGES` (100) pages, and a stop at a page that brings no new name. A listing stopped that way still answered with the names it had seen. An index past the stop then read as absent. Today that fails safe (its create is refused and reported), but a later `--prune` reading "absent" as "forget it" would leave a live index unrecorded. 1D.4d (#4582) already made the queues lookup fail in the same case.

**Decision.** When the listing is stopped while the account still says there is more (`total_pages` beyond the page, or a full page with none given), by the cap or by a repeated page, `vectorize_names` fails with `the account's Vectorize index listing could not be read to its end (<why>), so which indexes exist is not known`. A short or empty last page is the listing's end, and its names are the answer, as before.

**Consequences.** No lookup over an account listing answers "absent" for what it could not see. A deploy against a listing that misbehaves this way stops, saying why, rather than going on with a partial view. The request sequences are unchanged.
