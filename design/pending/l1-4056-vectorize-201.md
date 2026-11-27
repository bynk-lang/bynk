---
level: patch
changelog: A first `bynk deploy` of a Vectorize index succeeds again: Cloudflare answers the index create with `201 Created`, which the REST client read as a failure (`Cloudflare answered 201 []`) although the index was made, so each run got one index further. Any 2xx with `success: true` is now a success
---
