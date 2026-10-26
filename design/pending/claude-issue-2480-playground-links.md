---
level: minor
changelog: "`vscode-bynk` adds **Copy Playground Link** and **Open in Playground** for a selection or a single-file program, on desktop and on the web (#2480)"
---

## ADR: playground-links-in-the-fragment
title: Playground links carry the code, compressed, in the URL fragment
summary: A link is the selection in the playground's own fragment format, capped in size, never truncated; projects share a selection only

**Context.** Sharing a snippet for help, a bug report or teaching meant
copying and pasting by hand. The playground (`bynk-lang/playground`, ADR 0614)
already opens a program from its URL: ADR 0140 D5 fixed the format as
`#base64url(deflate-raw(utf8(source)))`, and By Example's "Open in playground"
links (`site/src/lib/examples.mjs`) emit it. No playground change is needed.

**Decision.**

- **A. The fragment, with a cap.** `vscode-bynk` builds the same link, reusing
  that format byte for byte. The code stays in the fragment, which a browser does
  not send to a server. Above 16,384 characters of link the command says the
  snippet is too large; it never truncates, because a cut link would open a
  different program. The link is made only when the user runs a command.
- **B. Projects share a selection.** A single-file program (no `bynk.toml`
  above the file) is shared whole when nothing is selected. A file in a project
  depends on its sibling units, so there the command asks for a selection. A
  selection that names a unit from a `uses` clause it doesn't carry gets a
  warning, since it may not compile alone.
- **C. One implementation for both hosts.** The commands use only the VS Code
  API and Compression Streams, so the desktop and web entries register the same
  module.

**Consequences.** Sharing is one command on desktop and on the web. The
fragment format is now duplicated in the extension and the site; the
site's encoder and the extension's are tested to agree. A selection that
omits its unit header is shared as is, and the playground reports what it
finds. The link's size is bounded by the cap, not by the playground, which
we do not control here. Hover, completion, semantic tokens and signature
help are unchanged.
