---
level: patch
changelog: The VS Code project sidebar and live model views catch up with a slow analysis round after a save or an external change. When no diagnostics arrive to signal the round, they re-check on a backing-off schedule for about 30 s. A file open in the editor that changes on disk now also counts as an external change
---
