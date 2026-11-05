; #2958: a multi-line string's language tag (`"""sql`, `r"""regex`) names the
; language of its body. The body is injected as that language; interpolation
; holes and escapes are children of the body, so they are left out of the
; injected text. A tag no editor knows is a plain string.
; Doc-block contents are plain text.

((string_literal
   (string_tag) @injection.language
   (string_body) @injection.content))

((raw_string
   (string_tag) @injection.language
   (raw_string_body) @injection.content))
