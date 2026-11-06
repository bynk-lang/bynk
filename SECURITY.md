# Security Policy

## Supported versions

Bynk is pre-1.0 and ships from a single moving release line: only the **latest
released version** receives security fixes. There are no long-term-support or
back-ported release branches. If you are affected by a security issue, upgrade
to the most recent release first.

| Release           | Supported          |
| ----------------- | ------------------ |
| Latest release    | :white_check_mark: |
| Older releases    | :x:                |

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues,
pull requests, or discussions.**

Report privately through GitHub's private vulnerability reporting:

1. Open <https://github.com/bynk-lang/bynk/security/advisories/new> — or go to the
   repository's **Security** tab and click **Report a vulnerability** (under
   *Advisories*).
2. Fill in the advisory form with as much detail as you can.

This opens a private channel visible only to the maintainers. If you have not
used private reporting before, GitHub documents the reporter's side of it at
<https://docs.github.com/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability>.

If private vulnerability reporting is unavailable to you, open a minimal public
issue that says only *"security issue — requesting a private contact"* (no
details) so a maintainer can reach out.

Please include, where possible:

- The affected component (e.g. the `bynk` compiler, an emitted Workers
  artifact, the `bynk-lsp` language server, or the VS Code extension).
- A description of the issue and its impact.
- Steps to reproduce, or a proof-of-concept.
- Any known mitigations or workarounds.

## Our commitment

- We will acknowledge your report within a reasonable timeframe.
- We will keep you informed as we investigate and work on a fix.
- We will credit you in the advisory once a fix is released, unless you ask to
  remain anonymous.

Thank you for helping keep Bynk and its users safe.
