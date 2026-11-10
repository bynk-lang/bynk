---
title: Format with ICU
---
A bare `{name}` placeholder substitutes a value as-is. That is not enough for
real text: "1 items" is wrong, and so is a date formatted the American way for a
German reader. Message templates therefore accept four more placeholder forms,
drawn from [ICU MessageFormat](https://unicode-org.github.io/icu/userguide/format_parse/messages/),
which read the *value* of the argument rather than just printing it.

Bynk carries no CLDR data of its own. Plural-category selection and
number/date formatting are delegated to the JavaScript host's own `Intl`
object, which every target platform already ships.

## Plural

```bynk
commons shop.messages

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
	"cart.count" => "{n, plural, one {# item} other {# items}}"
}
```

Arms are CLDR plural categories — `zero`, `one`, `two`, `few`, `many`, `other`
— and **`other` is mandatory**. Inside an arm, a bare `#` substitutes the
argument's own value, formatted for the locale.

Which categories a language actually uses is a runtime fact, so declare the
ones your translation needs. Polish needs three:

```bynk
commons shop.messages

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
	"cart.count" => "{n, plural, one {# item} other {# items}}"
}

messages "pl" {
	"cart.count" => "{n, plural, one {# produkt} few {# produkty} other {# produktów}}"
}
```

An arm a locale never selects is inert, not an error — the compiler does not
try to predict which categories a language uses, because that would mean
carrying the CLDR data it deliberately doesn't.

Supply the argument as `withWhole` (an `Int`) or `withNum` (a `Float`).

## Select

For a fixed set of variants that isn't a plural — grammatical gender, a plan
tier, a status:

```bynk
commons shop.messages

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
	"review.liked" => "{g, select, male {He} female {She} other {They}} liked this."
}
```

Keys are arbitrary, `other` is mandatory and is used for any value no arm
names. Supply the argument as `withText`.

## Number

```bynk
commons shop.messages

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
	"stats.views" => "{n, number} views"
	"stats.rounded" => "{n, number, integer} views"
	"stats.share" => "{n, number, percent} of readers"
}
```

Three forms: bare (the locale's default decimal formatting), `integer` (no
fraction digits) and `percent`. Supply as `withWhole` or `withNum`.

## Date

```bynk
commons shop.messages

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
	"order.placed" => "Placed {at, date}"
	"order.placed_long" => "Placed {at, date, long}"
}
```

Bare, or one of `short`, `medium`, `long`, `full`. Supply as `withMoment` (an
`Instant`) — a `date` placeholder given anything else falls back to its literal
`{name}` text rather than guessing.

## Literal braces, hashes and apostrophes

Inside a `plural` or `select` arm, `{`, `}` and `#` are syntax. To use one as
text, wrap it in single quotes; to write a literal apostrophe, double it. This
is ICU's own quoting convention:

| Written | Renders as |
|---|---|
| `'{'` · `'}'` | `{` · `}` |
| `'#'` | `#` (rather than the argument's value) |
| `''` | `'` |

```bynk
commons shop.messages

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
	"cart.count" => "{n, plural, one {it''s # item} other {it''s # items}}"
	"syntax.hint" => "{n, plural, other {use '{'name'}' to substitute}}"
}
```

Outside an arm, none of this applies — an ordinary template is literal text
plus its placeholders.

## What the compiler checks

- Every template parses as valid ICU
  (`bynk.messages.malformed_icu_syntax`) — unbalanced arm braces, an unknown
  format keyword, a missing `other` arm, `#` outside a plural arm, or a
  repeated arm key.
- A placeholder shared between the reference locale and another must have the
  same **format kind** in both (`bynk.messages.format_mismatch`). A UI can't
  sanely be plural-aware in one language and plain in another.

The placeholder *name* check from
[Declare a message bundle](/book/guides/localisation/declare-a-message-bundle/)
still applies, and is separate: names are compared as a set,
format kinds are compared per name.

## Not supported

Each of these is **diagnosed**, not silently mishandled — you get
`bynk.messages.malformed_icu_syntax` rather than surprising output:

| Construct | Status |
|---|---|
| `selectordinal` | Not supported. |
| `plural`'s `offset:` | Not supported. |
| `plural`'s exact-value arms (`=0`, `=1`) | Not supported. |
| CLDR skeleton strings (`{n, number, ::currency/GBP}`) | Not supported — only the fixed style keywords above. |
| A second `{arg, …}` dispatch nested inside an arm | Not supported — one dispatch level per placeholder; an arm's body is literal text and `#` only. |

One more limit worth knowing: nothing checks that a call site building a
`Message` supplies the `MessageArg` variant a template's format implies. A
`{n, plural, …}` given a `withText` value falls back to the literal `{n}` at
render time rather than failing the build.

**See also:**
[Understand localisation](/book/guides/localisation/understand-localisation/),
[Declare a message bundle](/book/guides/localisation/declare-a-message-bundle/),
[Diagnostics](/book/reference/diagnostics/).
