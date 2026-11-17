---
level: patch
changelog: `bynk deploy` no longer fails a first deploy when wrangler uploads the Worker and then reports `[code: 10007]` (a race in its follow-up lookup), if the account shows the upload landed as the Worker's only version; any other failure, and a Worker that already existed, stay fatal (#3562, G4-2)
probes: ts_writes
---

On the first deploy of a Worker name, `wrangler deploy` can upload the Worker and then
fail its follow-up lookup with `This Worker does not exist on your account [code:
10007]`, exiting non-zero though the upload landed (a race with the script's creation,
not something a Worker group causes). `bynk deploy` now treats that as success, and
only on evidence from Cloudflare itself: the ledger has no record of the Worker,
wrangler said `[code: 10007]`, the account had no script of that name before the push,
and it has exactly one version after it. It needs `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID` to ask; without them, or on any doubt, the failure stays as
wrangler reported it, and so does every other failure.

Wrangler's lookup runs after the upload and before its triggers step (workers.dev,
routes, custom domains, crons), so after forgiving the failure `bynk deploy` runs
`wrangler triggers deploy` for the Worker, and fails the deploy, saying the Worker was
uploaded and to run it again, if that step fails.
