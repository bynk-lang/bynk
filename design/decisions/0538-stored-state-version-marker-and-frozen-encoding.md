# 0538 — Stored agent state carries an in-record `__v` schema-version marker, and its encoding is part of the 1.0 data contract

- **Status:** Accepted (v0.372)

**Context.** Slice S1 of the state-migrations track writes a schema version into each agent's stored state, so loading knows which recorded shape it is reading. Once deployed agents have written it, the marker's place and meaning are permanent. That is why this decision lands with S0, ahead of the code (`design/tracks/state-migrations.md` §3.2, §3.7, settled by #2189). Stored state is the boundary encoding that `__encode<Agent>State` writes (ADR 0124, #1649). That is emitted TypeScript, which the 1.0 definition otherwise lets improve within 1.x.

**Decision.**

1. *Place.* The marker is a reserved field, `__v`, inside the record stored under the `"state"` key. It holds the class's lock `version` (the first ADR above) and is written on every commit. The marker and the data it describes are one value in one `put`, so no crash can separate them.
2. *Meaning.* An absent `__v` means version 1, the shape the lock baselined. A stored version above the running build's version means the deploy was rolled back. That faults as `RehydrationViolation` with cause `NewerSchema`, never decodes the record, and never writes it back.
3. *The name cannot collide (#2204 Decision D, settled in implementation).* The proposal planned a `bynk.store.reserved_name` check. It is unnecessary: an identifier is `[A-Za-z][A-Za-z0-9_]*`, so no `store` field can start with `_`. A test pins the guarantee. If identifiers ever admit a leading `_`, a reservation check must land with that change.
4. *The encoding is frozen, with one allowance (maintainer decision on #2189).* The encoding of each storable type, as `__encode<Agent>State` writes it, is part of the 1.0 data-compatibility promise. A 1.x release may change an encoding only by bumping every affected class's version and shipping the compiler-generated step that re-encodes stored records. Otherwise the change is a 2.0 event. The accepted cost is that legacy decoders are carried through 1.x, down to the oldest floor a deployment might hold. The stricter rule, no encoding change within 1.x, was rejected because it would leave a wrong 1.0 encoding stuck until 2.0.
5. *The 1.0 definition records it.* `design/bynk-1.0-definition.md` gains this carve-out beside the cross-context wire format's, and Gate 3 links its track.

**Consequences.** S1 builds the marker against a settled place, meaning and rollback rule. Every future change to the boundary serialiser now needs a migration story for stored state, not only for the wire. A release that changes an encoding says so in its cumulative migration note (`bynk-release-discipline.md`).
