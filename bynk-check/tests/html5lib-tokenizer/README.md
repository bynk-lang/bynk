# html5lib-tests: the tokeniser corpus

The `tokenizer/` tests of [html5lib-tests](https://github.com/html5lib/html5lib-tests) at commit
`c5495c539ca8a79c3c5107f5f14010bcff9a35d5`, vendored for the typed-HTML tokeniser's conformance test
(`../html_tokeniser_conformance.rs`, typed-HTML track §11.2, #3927). MIT licensed: see `LICENSE`.

`entities.test`, `namedEntities.test` and `numericEntities.test` are left out: every case in them
is a character reference, which the tokeniser never decodes (a reference does not change the
state, so the conformance test skips any input with a `&`), and `namedEntities.test` alone is
1.1 MB.

Refresh by copying the same files from a newer commit and updating the hash above.
