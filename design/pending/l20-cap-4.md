---
level: minor
changelog: The editor shows capability parameters: hover on a call names the instance each parameter is bound to, completion after `given Store = ` offers only instances of the parameter's kind, signature help shows `given Store: Kv`, and the architecture model records each binding; the book gains the guide *Write a library that uses a capability* (experimental, behind `bynk.given.parameters`)
---

## ADR: capability-parameters-tooling
title: The editor shows the instance a capability parameter is bound to, and offers only instances of its kind
summary: How slice 4 of capability parameters surfaces bindings in hover, completion, signature help and the architecture model

**Context.** #3959, slice 4 of the capability-parameters track (`design/tracks/capability-parameters.md`, §13). Slices 1 to 3 made capability parameters declarable, bindable, forwardable and testable. A binding is the one thing that decides which instance a library reaches, so the editor has to make it visible where the call is written, and must never choose it (track §11).

**Decision.** (A) *Signatures.* A function's signature, in hover, completion details and signature help, shows each capability parameter with its bound (`given Store: Kv, Logger`). The bindings are not ordinary parameters, so signature help's parameter ranges still count the arguments only. (B) *Hover.* Hover on the callee of a call that binds appends *Bound at this call*, one line per binding (`Store` is bound to `Sessions`); hover on a binding's parameter name says whose parameter it is and what it binds. Both read the call from the source text around the cursor, as signature help does. (C) *Completion.* After `given Store = `, completion offers every candidate the caller holds of the parameter's bound kind: its unit's instances of that kind, the bare kind where the kind allows one (`Kv`), and in a commons the enclosing function's own parameters of the kind (forwarding). Nothing of another kind is offered, and nothing is pre-selected. (D) *Architecture model.* Each handler's facts gain its calls' bindings (function, parameter, instance, site), carried into the model and the wire form as `bindings`, omitted when empty, so a diagram can draw which instance a library reaches. The add-binding fix is slice 1's (`bynk.given.binding_missing`, machine-applicable only with one candidate).

**Consequences.** An existing project's editor answers and wire form are unchanged: each addition appears only where a capability parameter is declared or bound. The editor extension draws the bindings once it reads the new field.
