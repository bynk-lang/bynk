---
level: patch
changelog: "A registry client refuses an index line whose `toolchain` or `withdrawn` is present but not a string (or, for `toolchain`, `null`) — a serialiser's tagged `Option` included — instead of reading it as absent, so `bynk registry conformance` fails such a registry rather than passing `read.withdrawn` for want of a withdrawn line"
---
