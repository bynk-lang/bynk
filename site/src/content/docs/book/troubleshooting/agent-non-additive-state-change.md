---
title: "`bynk.agent.non_additive_state_change`"
---
```text
[bynk.agent.non_additive_state_change] Error: agent `Cart`'s stored state changed
in a way deployed data cannot be read through — field(s) retyped: prefs
(prefs.dark added without a default)
```

## What it means

`bynk.schema.lock` records the shape of every agent class's stored state. A build
compares your `store` fields with that record, and refuses a change that records
already on disk could not be read through. Adding a `store` field, changing only a
refinement (a warning), and adding a record field **with a default** anywhere in a
stored value are all accepted. Anything else is this error.

The error sits on the agent's name and lists what broke, grouped by cause:

| Clause | Cause |
|---|---|
| `field(s) removed: owner` | A `store` field is gone. |
| `field(s) retyped: prefs (…)` | A stored field's value changed. The parentheses name each position that broke. |
| `field(s) retyped: items` | The same, with no position: the lock predates positions (see [older locks](#a-lock-written-by-an-older-toolchain)). |
| `field(s) changed storage kind: n` | A field moved between storage kinds, such as `Cell` and `Map`. |

A renamed field is a removed field plus an added one, so it appears as `removed`.
When exactly one removed and one added field share a stored shape, a note names
them as a likely rename.

### Reading the positions

Each clause in parentheses is a path from the `store` field to the place that
changed, followed by what happened there:

| Clause | Meaning |
|---|---|
| `one (one.dark added without a default)` | A record field was added with no default, so a record stored before it cannot decode. |
| `prefs.theme removed` | A recorded record field, variant or element position no longer exists. |
| `prefs.size retyped` | The position is still there but now holds a different type. |
| `prefs.size lost its default` | A record field that had a default no longer has one, so a record stored without it would not decode. |
| `last?\|Paused variant added` | A sum type gained a variant, which is refused like any other change to a sum. |

The path is built from these segments:

| Segment | Meaning | Example |
|---|---|---|
| `.field` | A record field, or a variant's payload field. | `prefs.theme`, `last?\|Added.sku` |
| `[]` | The element of a `List` or `Set`. | `many[]`, `many[].size` |
| `?` | The value inside an `Option`. | `last?` |
| `{}` | The value of a `Map` nested inside another type. | `index{}` |
| `\|Variant` | One variant of a sum type, or `\|Ok` / `\|Err` of a `Result`. | `last?\|Paused` |
| `#0`, `#1` | The first and second type argument of a storage kind that takes two. For `Map[K, V]`, `#0` is the key and `#1` the value. | `byId#1.size` |

The first segment is always the `store` field's name. A field with a single type
argument (`Cell[Prefs]`, `Log[Prefs]`) has no `#` segment.

## Example

With the lock recording `Prefs` as `{ theme: String }`, adding a field with no
default is refused. This is the real output of that build:

```text
[bynk.agent.non_additive_state_change] Error: agent `Cart`'s stored state changed
in a way deployed data cannot be read through — field(s) retyped: prefs
(prefs.dark added without a default)
Note: if this class was never deployed, delete its `[agents."shop.Cart"]` entry
from `bynk.schema.lock` to record the new shape as a fresh baseline; if it was
deployed, widen rather than narrow, or stage the change through a new field
(ADR 0124 D3), or declare a `migrate from N` step that carries the old data
forward
```

```bynk,fragment
context shop

type Prefs = { theme: String, dark: Bool }   // `dark` has no default

agent Cart {
  key id: String
  store prefs: Cell[Prefs] = Prefs { theme: "light", dark: false }

  on call ping() -> Effect[Int] {
    1
  }
}
```

## Fix

In order of preference:

1. **Give an added record field a default.** A stored record that lacks the field
   decodes to the default, so nothing is orphaned and the build accepts it:

   ```bynk,fragment
   type Prefs = { theme: String, dark: Bool = false }
   ```

   See [Add an option without breaking callers](/book/guides/program-structure/add-an-option/)
   for field defaults.

2. **Keep a field and add a new one, rather than renaming it.** A rename is a
   removal plus an addition, and the deployed value would read as the new field's
   zero. Leave the old `store` field, add the new one, and move over in code. The
   same goes for a retyped field: widen with a new field rather than narrow an
   existing one.

3. **Write a `migrate from N` step.** The step carries the stored record from
   version `N`, as the lock recorded it, to the current shape. For a rename, the
   error's note spells out the step to write:

   ```bynk,fragment
   context shop

   type CartV1 = { owner: String }

   agent Cart {
     key id: String
     store ownerName: Cell[String]

     migrate from 1 (old: CartV1) {
       ownerName := old.owner
     }

     on call ping() -> Effect[Int] {
       1
     }
   }
   ```

   See [Migrations](/book/reference/agents/#migrations).

4. **For a class that was never deployed, delete its entry from the lock.** Remove
   the `[agents."<context>.<Agent>"]` table (for example `[agents."shop.Cart"]`)
   from `bynk.schema.lock`; the next build records the new shape as a fresh
   baseline. Do this only when no deployment holds data for the class, since the
   lock is what protects stored data.

`bynk check` and `bynk test` reconcile against the lock without writing it; only
a build records a new version.

## A lock written by an older toolchain

A `bynk.schema.lock` written before positions were recorded has no `paths` for its
fields. Against such a lock the first change inside a stored value is judged as a
retype of the whole field, and the error carries this note:

> this lock predates per-path shapes, so a change inside a stored value can't be
> judged field by field: build once with the definition the lock records, before
> this change, so the lock records which fields have defaults; a record field
> added with a default is then additive

Revert the change, build once so the lock records the positions (the version does
not change), then reapply it. A record field added with a default is then accepted.

## Related

- Reference: [agents, "Changes that need no migration"](/book/reference/agents/#changes-that-need-no-migration)
- Specification: [§5.4.3 Rehydration validation](/book/spec/static-semantics/#543-rehydration-validation-v097)
- Guide: [Add an option without breaking callers](/book/guides/program-structure/add-an-option/)
- Guide: [Build a stateful agent](/book/guides/agents-and-state/stateful-agent/)
