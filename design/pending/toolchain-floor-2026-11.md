---
level: patch
changelog: The wrangler minimum is now 4.162.0, the newest, and the Containers floor of 4.136.0 is gone, since the general minimum is past it; `postal-mime` and `yaml` are bumped in the runtime tests
breaking: `bynk doctor`, `bynk dev` and `bynk deploy` now want wrangler 4.162.0 or newer: an older one is warned about, and `bynk deploy` refuses to push a context with an agent on it (`bynk: wrangler X is older than 4.162.0 …`); upgrade with `npm install -g wrangler@4`.
---
