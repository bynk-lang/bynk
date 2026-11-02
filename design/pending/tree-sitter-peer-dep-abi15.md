---
level: patch
changelog: The `tree-sitter-bynk` npm package declares the `tree-sitter` runtime it can load. Its parser is generated at ABI 15, but the peer dependency still said `^0.21`, and that runtime only loads ABI 14 and below, so `parser.setLanguage` threw. The peer range is now `^0.25` (#15).
---
