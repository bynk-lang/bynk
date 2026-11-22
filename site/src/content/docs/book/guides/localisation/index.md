---
title: Localisation
---
Handlers produce a stable message **code** plus typed values; a **message
bundle** turns that into text in a particular language. Adding a language
touches the bundle and nothing else.

**Understand**
- [Understand localisation](/book/guides/localisation/understand-localisation/)

**Do**
- [Declare a message bundle](/book/guides/localisation/declare-a-message-bundle/)
- [Format with ICU](/book/guides/localisation/format-with-icu/)

**Worked example:** [`examples/greeter-i18n`](https://github.com/bynk-lang/bynk/tree/main/examples/greeter-i18n)
— an HTTP service that negotiates a locale and renders from a three-language
bundle.

**See also:** [First-party `bynk` capabilities](/reference/bynk-capabilities/),
[Diagnostics](/reference/diagnostics/).
