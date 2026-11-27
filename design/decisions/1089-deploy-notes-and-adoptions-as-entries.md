# 1089 — The adoption checks, the account notes and the forward warnings are entries of the JSON plan

- **Status:** Accepted (v0.813.1)

**Context.** #3797, part 3b. Parts 1 to 3 made the plan document carry `refusals` and `warnings`. Three things `--plan` still said on stderr alone: the adoption checks of `[deploy.vectorize]` and `[deploy.containers]` (a refusal, and a note for an entry that adopts nothing under this environment), what checking the plan against the account found (`account_notes`), and the warnings for a `Forward` destination Cloudflare would refuse.

**Decision.** (A) `apply_adoptions` returns the notes it printed on success and the refusal on failure, each as an entry (`bynk.deploy.adoption_unreadable`, `adoption_unmatched`, `adoption_partial`), and the stop carries the upload order like the other post-order stops. (B) The plan takes the account notes (`bynk.deploy.account_note`) and the forward warnings (`bynk.deploy.email_forward_destination`) as `warnings` before it is printed, so the document has them; the stderr text and its order are unchanged. (C) The reference table lists the new codes and says the account checks and confirmations of a run that applies are not entries, since `--plan` never reaches them.

**Consequences.** Additive for a JSON consumer. The account notes carry the text of the note and no subject fields, because they are free-form lines of several kinds; a client matches on the code and shows the message. `bynk revert --format json` verdicts and `bynk.cloudflare.credentials_missing` follow in part 4.
