---
level: patch
changelog: The Workers guide no longer says an uploaded source map maps a fault's logged stack: Cloudflare applies an uploaded map only to an uncaught exception, and a Worker catches every fault to answer the `500`, so the frames in `wrangler tail` stay `index.js:L:C`; it points to `bynk dev --inspect`, pausing on caught exceptions, to see a fault at its `.bynk` line, and corrects the emitter's comments to match (#2192).
---
