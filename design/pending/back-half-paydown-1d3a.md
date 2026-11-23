---
level: patch
changelog: Internal restructuring of `bynk deploy`; the resource kinds reach Cloudflare through one operation trait, with no change to plans, provisioning, messages or `bynk.deploy.lock`
probes: cloudflare_client_sites, tool_spawn_sites
---
