---
level: patch
changelog: `bynk check` and the language server build the cross-context views once per project instead of once per context, so checking is roughly linear in project size (500 contexts: 7.5 s to 0.47 s per edit)
---
