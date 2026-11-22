# 0886 — The documentation's prose is CC BY 4.0 and its snippets MIT, as the blog's; Bynk stays MIT or Apache-2.0

- **Status:** Accepted (v0.640.40)

**Context.** Everything in bynk-lang/bynk was dual-licensed MIT or
Apache-2.0, the documentation's prose included. The blog licensed its posts
CC BY 4.0 and its code MIT. The two sites now share one shell, and its footer
states the licence (web presence slice 8, #4025), so the difference showed.
The licences also do different jobs. Bynk's dual licence lets it be used
where one of the two is incompatible with a project's other licences. The
blog's split keeps the writing attributed while letting a developer copy a
snippet into their own work without attribution terms.

**Decision.** The documentation follows the blog:

- The prose of the pages in `site/src/content/` (the Book, Bynk by Example
  and the Reference) is licensed CC BY 4.0 (`LICENSE-CC-BY-4.0.md`).
- Code snippets in the documentation (the code blocks in those pages, and
  `site/src/snippets/`) are licensed MIT.
- Everything else, the toolchain, the runtime, the standard library,
  `examples/`, the tests and the site's own code, stays MIT or Apache-2.0, at
  the user's option.

The README's "Licence" section and contribution clause, `CONTRIBUTING.md`,
and each site's footer say which licence covers which part. Contributions
are taken on the same terms. The whole history of the repository was
authored by its maintainer, bots and Claude sessions, so relicensing the
existing prose needs no one else's agreement.

**Consequences.** A reader can quote or adapt the documentation with
attribution, and copy any snippet under MIT, on either site, on the same
terms. Code quoted from `examples/` stays dual-licensed, which already
includes MIT. A future outside contribution to the documentation's prose is
CC BY 4.0, which the contribution clause now says.
