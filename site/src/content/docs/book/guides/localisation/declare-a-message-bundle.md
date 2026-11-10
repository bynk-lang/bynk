---
title: Declare a message bundle
---
A **message bundle** is a `messages` block per locale, declared inside a
`commons`. The compiler turns the set into a lookup and generates a
`render(tag, msg)` for that commons. For the model behind it, see
[Understand localisation](/book/guides/localisation/understand-localisation/).

## The smallest bundle

```bynk
commons shop.messages

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
	"cart.empty" => "Your basket is empty."
	"cart.greeting" => "Hello, {name}!"
}
```

Three things are load-bearing:

- **`messages` is a commons item.** Declaring one inside a `context` or
  `adapter` is `bynk.messages.outside_commons` — a bundle is pure, stateless
  data, which is what `commons` means.
- **`@reference` marks the reference locale**, and exactly one block per
  commons may carry it. It is the locale every other one is checked against,
  and the fallback when a requested locale isn't declared.
- **Both `uses` lines are required.** `bynk.locale` supplies `render`'s
  fallback and the `message`/`with*` builders; `bynk.locale.types` supplies
  `LocaleTag` and `Message`, which the generated `render`'s own signature
  names. Omitting either is `bynk.messages.missing_locale_dependency`.

Each entry is `"code" => "template"` — both sides plain string literals. Codes
are yours to structure; a dotted, hierarchical convention (`cart.empty`) reads
well and sorts well, but nothing enforces it.

## Adding a locale

Add a block per language. Only the reference block carries `@reference`:

```bynk
commons shop.messages

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
	"cart.empty" => "Your basket is empty."
	"cart.greeting" => "Hello, {name}!"
}

messages "fr" {
	"cart.empty" => "Votre panier est vide."
	"cart.greeting" => "Bonjour, {name} !"
}
```

The compiler now holds you to the reference:

- Drop `"cart.empty"` from `fr` and you get `bynk.messages.incomplete`, one
  diagnostic per missing code, anchored at the `fr` block.
- Rename `{name}` to `{prenom}` in the French template and you get
  `bynk.messages.placeholder_mismatch` — the *names* must agree, because they
  are the keys the caller supplies.

Placeholder **order** is free. `"Hello, {name}, you are {age}"` and
`"{age} ans, bonjour {name}"` agree: translations routinely reorder to suit the
target language's grammar, and only the set is compared.

A locale may be declared once. Two blocks with the same tag is
`bynk.resolve.duplicate_message_locale`.

The tag is a `LocaleTag` string literal, so region- and script-bearing tags are
declared like any other — `messages "pt-BR"`, `messages "zh-Hans-CN"`:

```bynk
commons shop.messages

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
	"cart.empty" => "Your basket is empty."
}

messages "pt-BR" {
	"cart.empty" => "A sua cesta está vazia."
}
```

A tag that isn't a valid `LocaleTag` — a bare word like `messages "klingon"`, or
a mis-cased `messages "pt-br"` — is `bynk.messages.invalid_locale_tag`, caught at
compile time rather than reaching the locale-aware runtime as an invalid tag.
`LocaleTag`'s pattern mandates canonical casing (`pt-BR`, not `pt-br`), so a
locale has one spelling across the bundle.

`LocaleTag` admits the full well-formed BCP-47 shape, not just
`language[-Script][-REGION]`: variants (`messages "ca-valencia"`,
`messages "de-CH-1996"`), extensions (`messages "en-US-u-ca-buddhist"`), and
private-use subtags (`messages "de-CH-x-phonebk"`, or the standalone
`messages "x-custom"`) are all declarable. Grandfathered/irregular tags like
`i-klingon` and `en-GB-oed` don't fit this productive grammar and remain
`bynk.messages.invalid_locale_tag`.

## Rendering

A `messages` block gives its own commons a generated
`render(tag: LocaleTag, msg: Message) -> String`. Use it alongside the builders
from `bynk.locale`:

```bynk,fragment
commons shop.messages

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
	"cart.greeting" => "Hello, {name}!"
}

messages "fr" {
	"cart.greeting" => "Bonjour, {name} !"
}

---
Renders `code` for `tag`, substituting `name`.
---
fn greet(tag: LocaleTag, code: String, name: String) -> String {
	render(tag, withText(message(code), "name", name))
}
```

`greet("fr", "cart.greeting", "Ada")` returns `Bonjour, Ada !`;
`greet("de", …)` falls back to the reference locale's `Hello, Ada!`, because
`de` isn't declared.

## Calling it from a context

Wrap the rendering in a function inside the bundle's own commons — as `greet`
above does — and call *that* from your context:

```bynk,fragment
context shop.web

uses bynk.locale.types
uses shop.messages
consumes bynk { Locale }

service api from http {
	on GET("/hello/:name") (name: String) -> Effect[HttpResult[String]] given Locale {
		let tag <- Locale.current()
		Ok(greet(tag, "cart.greeting", name))
	}
}
```

Note what the context `uses`: `bynk.locale.types` (for `LocaleTag`, which
`Locale.current()` returns) and the bundle's commons. It does **not**
`uses bynk.locale`.

> **That omission is required, not stylistic.** `bynk.locale` exports a
> `render`, and so does every message-bundle commons — a context that `uses`
> both hits `bynk.uses.name_conflict` on the shared name, whether or not it ever
> calls either. Keeping the `render`/`message`/`with*` calls inside the bundle's
> own commons, behind a wrapper function, is the pattern that avoids it.

## What a bundle exports

Alongside `render`, a bundle's generated module exports its declared locale set
and its reference tag, for the compiler's own wiring:

```ts
export const __messagesReferenceLocale: LocaleTag;
export const __messagesLocales: readonly LocaleTag[];
```

The `__` prefix is one no Bynk identifier can spell, so these never collide with
a declaration of your own (#1697). On the Cloudflare platform they are what
`Locale.current()` negotiates an
inbound `Accept-Language` header against, wired up automatically when a context
has exactly one detectable bundle. A context reaching two or more bundles has
no single answer to negotiate against and is reported as ambiguous.

## Splitting across files

A commons may span several files (its declarations merge), so one locale per
file is a natural layout for a large bundle:

```
src/shop/messages/en.bynk    -- commons shop.messages, the @reference block
src/shop/messages/fr.bynk    -- commons shop.messages, the fr block
```

The `@reference`, completeness and placeholder checks all run across the merged
commons, not per file.

**See also:**
[Understand localisation](/book/guides/localisation/understand-localisation/),
[Format with ICU](/book/guides/localisation/format-with-icu/),
[Diagnostics](/book/reference/diagnostics/).
