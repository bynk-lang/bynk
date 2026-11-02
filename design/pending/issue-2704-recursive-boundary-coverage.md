---
level: patch
changelog: Regression tests pin the recursive-type nesting bound on `/_bynk/call/` arguments, on a subscriber's event payload decoder, and on cycles that cross files, a commons and a context boundary, and check that a codec and its callers agree on the `depth` parameter (#2704)
---
