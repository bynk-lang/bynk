---
level: patch
changelog: A lexical error no longer hides the rest of its file's diagnostics (#2807)
probes: emit_diagnostics
---

## ADR: lexer-error-recovery
title: The lexer recovers from a lexical error and reports them all
summary: Skip the offending text, keep lexing, drop the syntax errors it causes, and keep the formatter strict

**Context.** `tokenize` returned the tokens or the first lexical error, so one stray
character, or a half-typed one in the editor, hid every other diagnostic in its file
(review §6.3). #1663 recovered from syntax errors, not lexical ones.

**Decision.** The lexer has a recovering entry point, `tokenize_recovering`, that
returns the tokens it could read and every lexical error. It skips a bad character, a
malformed or out-of-range literal, an unclosed doc-block opener and a `///`, and a
string literal: an unterminated one to the end of its line, a malformed one that closes through its closing delimiter. No error token is added to `TokenKind`: a
skipped span yields no token, so no parser or IDE match grows an arm. The strict
`tokenize` family is unchanged (the first error, no tokens). The parse cache, the
project path, the single-file editor path and `check_single_file` use the recovering
lexer and merge its errors with the recovering parser's, checking the declarations that
survive as for a syntax error (#1710). A syntax error that starts inside a lexical
error's span, or at the token after it, is the gap's echo and is dropped
(`without_lex_echoes`). The formatter keeps the strict lexer: a file with any lexical
error is refused, since formatting around the gap would delete the user's text. Token-based
editor features (hover, completion, symbols) keep the strict lexer too and still need a
clean lex.

**Consequences.** An editor shows the stray-character squiggle with the file's other
errors. A skipped literal can leave a checker echo (a `()` body where the literal stood);
that is accepted for now. Making the token-based features recover is a separate change.
