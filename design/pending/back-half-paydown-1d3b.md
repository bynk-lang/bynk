---
level: patch
changelog: Internal restructuring of `bynk deploy`, `bynk revert`, `bynk migrate`, `bynk dev` and `bynk tail`; every wrangler call goes through one Cloudflare module, with no change to plans, messages, provisioning or `bynk.deploy.lock`
probes: cloudflare_client_sites
---
