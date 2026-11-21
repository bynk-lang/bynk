---
level: patch
changelog: The `bynk` driver's own messages on stderr carry a label after who is speaking, so they take the colour palette: `error:` (red) when the command then fails, as in `bynk: error: not inside a Bynk project`, `warning:` (amber) when it carries on in a degraded state, and `note:` (blue) when you need to act; a coded `error[…]:` label and a workspace member's messages (`bynk: packages/ok: error: …`) are coloured too. The wording of these messages changed, so a script or CI filter that matches one may need the label added; exit codes and machine-readable output are unchanged
---
