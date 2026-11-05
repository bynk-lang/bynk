---
level: patch
changelog: "`bynk-grammar` keeps rule order itself, so `serde_json`'s `preserve_order` is off workspace-wide and the editor-contract key sort is gone"
---

Only `bynk-grammar` enabled `preserve_order` on the normal/dev graph (tree-sitter enables it for its build script only), and the release binaries are already built without it, so nothing shipped changes.
