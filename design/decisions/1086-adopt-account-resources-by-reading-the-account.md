# 1086 — An R2 bucket or D1 database already on the account is adopted by reading the account, not by an error code

- **Status:** Accepted (v0.812)

**Context.** Back-half paydown 1D.4b (#4056) moves R2's bucket create and D1's database create and by-name lookup from wrangler to Cloudflare's REST API, under ADR "Account resources need API credentials". Under wrangler, "already there" was read from its error text: R2's "already exists, and you own it" (code 10004) and D1's "A database with that name already exists". #4056's Decision C proposed reading the codes instead: 10004 for a bucket that is ours, 10006 for one that is taken, and D1's own "already exists" code, to be confirmed against the API reference. The reference does not support this:

- R2's error-code page (https://developers.cloudflare.com/r2/api/error-codes/) has no 10004. It documents 10006 as `NoSuchBucket` (404). Its only conflict code is 10073 `BucketConflict` (409, "Bucket name already exists"), which does not say whose bucket it is.
- D1's API error table (https://developers.cloudflare.com/d1/observability/debug-d1/) documents no "already exists" code. The create reference (https://developers.cloudflare.com/api/resources/d1/subresources/database/methods/create/) lists none.
- D1's list takes a `name` search (https://developers.cloudflare.com/api/resources/d1/subresources/database/methods/list/) without saying whether it matches exactly. Its `result_info` has no `total_pages`.

**Decision.**

- **The trigger.** A create that Cloudflare refused (`CloudflareError::is_refusal`: a 4xx that is not a denied token or a 429) is followed by a read of the account by name. A denied token, a rate limit, a server error or a transport failure is never followed by one.
- **R2.** `GET /r2/buckets/{name}`, in the bucket's jurisdiction, answers 200 only for a bucket this account holds. So a 200 accepts the bucket as this project's, and the ledger records "adopted the R2 bucket … already on the account". A 404, or any other failure, leaves the create's own error. Another account's bucket therefore stays a failure, whatever code the create carried.
- **D1.** `GET /d1/database?name=…` is paged while pages are full and matched on `name ==` exactly, so a longer name the search also finds is never adopted. A match is adopted, as P4a did, and recorded as "adopted the D1 database … already on the account". The jurisdiction rule of #3820 still refuses to record an adopted database as pinned. No match leaves the create's own error. A failed lookup is added to that error.
- **Policy stays in the kinds.** `create_r2_bucket` and `create_d1` decide over the typed error and two new account reads, `r2_owned` and `d1_find`. `d1_find` replaces `d1_list` and its stdout parser.
- **Credentials.** The pre-flight of ADR "Account resources need API credentials" now covers an unrecorded bucket or database too. The kinds are checked in the order KV, R2, D1, and the first that needs the variables reports. A recorded bucket or database is reused without a call.

**Consequences.** Adoption no longer depends on an undocumented code, or on wording that could change, and a code the reference later documents changes nothing. The cost is one extra read on a refused create. The real codes Cloudflare answers with are recorded by the live check, but the outcome does not depend on them. `wrangler_resource_ops` falls from 12 to 9: `D1Create`, `D1List` and `R2Create` leave `OpKind`, and with them the last of wrangler's R2 and D1 text matching and the `parse_d1_id` and `parse_d1_listed_id` parsers.
