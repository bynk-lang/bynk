---
title: Document your code
---
**Goal:** write documentation that the tooling shows wherever a name appears.

Bynk has two kinds of comment, and they do different jobs:

- A **line comment**, `// …`, is a note for whoever edits the code. It runs to
  the end of the line, and the tools ignore it.
- A **doc** is documentation. It attaches to the declaration (or member) that
  follows it. Hover shows it, and so do completion and the editor's
  **Bynk: Show Documentation** page. A doc comes in two shapes of one marker:
  - a **one-line doc**, `--- …`, for a sentence;
  - a **doc-block**, `---` on its own line, then the text, then `---`, for
    anything longer.

Use `//` for remarks about how the code works, and a doc for what a reader of
the API needs to know. Dashes only ever mean documentation, and `//` only ever
means a comment.

## Document a declaration

Put a doc-block directly above the declaration. The markers are lines of dashes
on their own, and everything between them is the doc:

```bynk
---
A repository, as `owner/name`.
---
type Repo = String

---
The number of open issues in `repo`, or zero when it has none.
---
fn openIssues(repo: Repo) -> Int { 0 }
```

Any declaration can carry one: a type, a function, a capability and its
operations, a service, an agent and their handlers, an actor, an event, a
message bundle. Hover on `Repo` anywhere it is used, and you see its signature
with this doc below it.

## Write a one-line doc

Most docs are a sentence. For those, a doc-block spends three lines on one.
Write a **one-line doc** instead: `---`, a space, then the text, at the start
of a line.

```bynk
--- A repository, as `owner/name`.
type Repo = String

--- The number of open issues in `repo`, or zero when it has none. ---
fn openIssues(repo: Repo) -> Int { 0 }
```

You may close the line with the same dashes it opened with, as the second doc
does; the closer is not part of the doc, and `bynk fmt` leaves it off. A
`---` is a doc only at the start of a line: after code on the same line, it is
three `-` operators.

A doc that needs a second line is a doc-block. Two docs directly above one
declaration is the error `bynk.parse.multiple_docs`, whether they are two
one-line docs or one of each, and its fix joins two one-line docs into a
block. `bynk fmt` keeps each doc in the shape you wrote: it never turns a
one-line doc into a doc-block, or the other way round.

## Document fields and variants

A record is often a wire contract, and each field deserves its own
explanation. Put a doc above the field:

```bynk
--- One canary result, exactly the body of `POST /runs`.
type Run = {
	--- The repository, as `owner/name`.
	repo: String,
	--- The Bynk release the checks ran against.
	version: String,
	// Only an editor needs this note; hover never shows it.
	passed: Int,
}
```

Hover on `repo`, where it is declared, where it is read (`run.repo`), or where
it is set in a record literal, and you see its doc. Completion shows it too,
and the documentation page lists each documented field under its type.

Variants take docs the same way, in both sum forms, and so does each field of a
variant's payload and each entry of a message bundle:

```bynk
type Outcome = enum {
	--- Every canary check passed.
	Pass,
	--- At least one check failed; the run URL has the details.
	Fail,
}

type Shape =
--- A circle.
| Circle(
	--- The radius, in pixels.
	radius: Int,
)
| Dot
```

A `//` comment above a field stays a comment. Only a doc documents. Function
and handler parameters are documented in the function's own doc.

## Document a module

A doc-block **above the unit header** documents the unit itself. This is its
*module doc*:

```bynk
---
The report the org canary posts, and how versions are ordered.
---
commons compat.model

---
A repository, as `owner/name`.
---
type Repo = String
```

The module doc shows when you hover the unit's name: in its own header, in a
`uses compat.model`, or in a `consumes` of a context. It is also the opening
paragraph of the unit's **Show Documentation** page.

### Turn a header comment into a module doc

A file often opens with a `//` comment that says what the module is for. The
[compat-board](https://github.com/bynk-lang/compat-board) project's files began
that way:

```bynk
commons compat.model

// The report the org canary posts, and how versions are ordered.

type Repo = String
```

That comment is invisible to the tools. To make it the module doc, move it above
the header and turn it into a doc-block, as in the example above. The text says
what the module is, so it belongs in the module doc. Keep a `//` comment for a
note that only an editor of the file needs.

### One module doc per unit

A context (or a commons) can be split across several files in its directory,
each opening with the same header. **At most one of those files may carry the
module doc.** A unit's doc is one piece of prose. If two files each had one, the
tools would have to join them in some file order, and the result would read
oddly. So two or more is the error `bynk.project.duplicate_module_doc`, which
names every file that carries one.

Pick the file that best introduces the unit, and keep the doc there:

```bynk
---
The shop: carts and checkout.
---
context shop

type CartId = String
```

Every other file of `context shop` opens with the bare header, or a `//`
comment above it:

```bynk
// Payment handling for the shop.
context shop

type Amount = Int
```

Hover and the documentation page find the doc in whichever file has it. The
page for any file of `shop` shows the whole context: the one module doc, then
every file's declarations.

A `suite` keeps its own doc, which describes the suite, so this rule doesn't
apply to it.

## Write Markdown in a doc

A doc is Markdown. Headings, lists, emphasis, links and code all render on
hover and on the documentation page. A `[Name]` link to another declaration
becomes a link you can follow.

To show an example in a doc, put it in a code fence. This guide uses `~~~`
fences; backtick fences work the same:

```bynk
---
The major part of a release version. For example, for

~~~text
0.309.4
~~~

it is `0`.
---
fn major(version: String) -> Int { 0 }
```

Keep code in a fence. If a doc-block's text, with its code fences set aside,
parses as declarations, the compiler rejects it
(`bynk.parse.doc_block_contains_code`): that is almost always code that was
turned into documentation by mistake.

### Put a rule or `---` lines inside a doc

A line of dashes on its own is a doc-block **marker**, never a divider. So for
a Markdown thematic break inside a doc, write `***` or `___`, which Markdown
reads the same way:

```bynk
---
Pick the board's colour for an outcome.

***

Green when every check passed.
---
fn colour(passed: Bool) -> String { if passed { "green" } else { "red" } }
```

A block closes only at a marker with **as many dashes as the one that opened
it**, as a Markdown code fence does. So to put a `---` line itself inside a
doc, such as a Bynk example with its own doc-block, open and close the block
with a longer marker:

```bynk
----
An example of a documented outcome:

~~~bynk
---
Whether the canary checks passed.
---
type Outcome = String
~~~
----
fn colour(passed: Bool) -> String { if passed { "green" } else { "red" } }
```

The `---` lines inside are content, because only a `----` line closes this
block. [Spec §3.3.2](/book/spec/lexical-grammar/#332-doc-blocks) has the exact
rule.

## Write examples that stay true {#examples}

A fenced `bynk` block in a doc-block is checked by `bynk test`, so an example
can't fall out of step with the code it documents. Each one is type-checked as a
`case` in the scope of the unit that declares the doc, so it can call that
unit's functions and use `expect`:

````text
---
Doubles a number.

```bynk
expect double(2) == 4
```
---
fn double(n: Int) -> Int { n * 2 }
````

A mistake in the example is reported at the line it's on in the doc, and fails
the test run. Mark a deliberately partial example `bynk ignore` (or
`bynk,ignore`) to skip it. Blocks in other languages, and `bynk fragment` or
`bynk fail` blocks, are left alone. A one-line doc (`--- text`) can't hold a
fence, so an example goes in a doc-block. See [Doc examples](/book/reference/testing/#doc-examples).

## `//`, `--- text` or a `---` block?

| You want to… | Write |
|---|---|
| Explain a declaration, a member or a unit in a sentence | a `--- text` one-line doc above it |
| Explain it at more length, or with structure: lists, code, headings | a `---` doc-block above it |
| Leave a note for someone editing the code | a `//` line comment |
| Separate sections of a file | `//` comments, such as a `// -----` ruler and a `// Helpers` heading |
| Disable code temporarily | `//` at the start of each line (VS Code's **Toggle Line Comment**) |

Don't use a line of dashes as a divider. It opens a doc-block, so a pair of
dividers around code would turn that code into documentation. The compiler
rejects that:

```bynk,fail=bynk.parse.doc_block_contains_code
--------------------------------
fn helper() -> Int { 1 }
--------------------------------
fn total() -> Int { 2 }
```

Write a `//` ruler instead:

```bynk
// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------
fn helper() -> Int { 1 }
```

## Migrate from `--` and `--|`

Before #2203, a comment was `--` and a one-line doc was `--|`. Both are errors
now (`bynk.lex.legacy_comment`, `bynk.lex.legacy_doc_line`), so an old file
never quietly changes meaning. Run `bynk fmt` over the project to rewrite them,
or use **Fix All** in VS Code. A run of `--|` lines was one doc; it has no
one-line spelling, so turn it into a doc-block by hand.

`bynk fmt` refuses a file (`bynk.fmt.mixed_comment_syntax`) when a line in it
reads differently under the old and new rules, rather than guess what you
meant: an old `--- Helpers ---` divider, which now reads as a doc, or a new
`--- text` doc beside a stray `--` comment. Rewrite the line it names, or fix
the `--` comments first, and format again.

**See also:** [Spec §3.3 — Comments and
doc-blocks](/book/spec/lexical-grammar/#33-comments-and-doc-blocks),
[Spec §5.1a — Module documentation](/book/spec/static-semantics/#module-docs).
