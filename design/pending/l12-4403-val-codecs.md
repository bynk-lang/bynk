---
level: patch
probes: ts_writes
raises: ts_writes — the emitter spells a qualified type's codec helper names (`__serialise_lib_x_Status`) with a `format!` in `codec_names`, as the other codec names are spelt (#4403)
changelog: A test suite can name a used commons' type through a `uses` qualifier (`Val[lib.x.Status]`, `Val[Holder]` over a record of qualified field types), and the wire codecs of a record whose fields are written through a `uses` qualifier (`lib.x.Status`, or `X.Status` under an alias) are bound, so a Workers build no longer emits an unbound `__serialise_lib.x.Status` (#4403)
---
