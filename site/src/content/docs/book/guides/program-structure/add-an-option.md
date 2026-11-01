---
title: Add an option without breaking callers
---
**Goal:** give an operation a new, optional setting without changing every call
that already uses it.

Bynk has no optional parameters and no overloading, so a function's arity is
fixed. Optional settings go in an **options record** beside the plain
operation, and each option's **field default** says what happens when a caller
doesn't set it.

## Start with the plain operation

```bynk
commons shop.export

type Format = enum { Csv, Json }

fn export(rows: List[String]) -> String {
  exportWith(rows, ExportOptions {})
}

fn exportWith(rows: List[String], options: ExportOptions) -> String {
  let body = rows.fold("", (acc, r) => if acc == "" { r } else { "\(acc)\(options.separator)\(r)" })
  match options.format {
    Csv => body
    Json => "[\(body)]"
  }
}

type ExportOptions = {
  separator: String = ",",
  format: Format = Format.Csv,
}
```

Each default reproduces `export`, so `exportWith(rows, ExportOptions {})` is
exactly `export(rows)`, and a caller sets only what it changes:
`exportWith(rows, ExportOptions { format: Json })`.

## Add the next option

A later setting is one more field, **with a default**:

```bynk,fragment
type ExportOptions = {
  separator: String = ",",
  format: Format = Format.Csv,
  header: Option[String] = None,
}
```

Every existing `ExportOptions { … }` literal still compiles, and so does every
stored or sent `ExportOptions` that predates the field: a body without the
`header` key decodes as `None`.

A field added **without** a default is a breaking change: every literal must
now give it (`bynk.resolve.missing_field`). Leave a field undefaulted only when
there is no neutral choice and every caller must decide.

## Things to know

- **A default is a fixed value.** It can't call a function or read anything
  (`bynk.types.bad_field_default`). Work that needs a computed value belongs in
  the operation, keyed off an `Option` field.
- **Changing a default changes behaviour** for every literal that left the field
  out and every body decoded without it. Treat it as you would changing what the
  plain operation does.
- **An `Option` field still needs `= None`** to be optional in a literal.

**See also:** [Field defaults](/book/reference/types/#field-defaults),
[Spec §5.2b](/book/spec/static-semantics/#field-defaults).
