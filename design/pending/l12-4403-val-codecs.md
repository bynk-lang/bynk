---
level: patch
changelog: A test suite can name a used commons' type through a `uses` qualifier (`Val[lib.x.Status]`, `Val[Holder]` over a record of qualified field types), and the wire codecs of a record whose fields are written through a `uses` qualifier (`lib.x.Status`, or `X.Status` under an alias) are bound, so a Workers build no longer emits an unbound `__serialise_lib.x.Status` (#4403)
---
