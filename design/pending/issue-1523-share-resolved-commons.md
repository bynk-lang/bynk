---
level: patch
changelog: The checker shares a unit's parsed declarations and agents by reference instead of deep-cloning them for every agent and handler, so a project check is up to a third faster (500 contexts: a language-server edit 521 ms to 344 ms)
---
