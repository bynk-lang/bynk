---
title: Diagnostic index
---
<!-- GENERATED FILE — do not edit by hand.
     Source: bynk-syntax/src/diagnostics.rs (`render_markdown`).
     Regenerate with: BYNK_BLESS=1 cargo test -p xtask --test docs_consistency diagnostics_registry:: -->

Every diagnostic code the compiler can emit, with a one-line summary of the cause, grouped by category. Each code links to its own page: what the code means, how to fix it and an example where an explanation is written, and the Book pages that explain it. `bynk explain <code>` prints a code's explanation in a terminal, and in VS Code the diagnostic's **Explain** quick fix (or **Bynk: Explain Diagnostic Code…**) shows the same text in a panel, offline.

There are **898** codes in total.

## Actors

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.actor.bearer_identity_not_string_constructible`](/book/reference/diagnostics/bynk.actor.bearer_identity_not_string_constructible/) | A `Bearer` actor's identity is not a string-constructible type. |  | — |
| [`bynk.actor.bearer_missing_secret`](/book/reference/diagnostics/bynk.actor.bearer_missing_secret/) | A `Bearer` actor does not name its signing secret. |  | — |
| [`bynk.actor.binder_shadows_param`](/book/reference/diagnostics/bynk.actor.binder_shadows_param/) | A `by` actor binder collides with a handler parameter of the same name. |  | — |
| [`bynk.actor.by_on_agent`](/book/reference/diagnostics/bynk.actor.by_on_agent/) | A `by` actor clause was placed on an agent `on call` handler, which has no actor. |  | — |
| [`bynk.actor.duplicate_sum_scheme`](/book/reference/diagnostics/bynk.actor.duplicate_sum_scheme/) | Two peers in a multi-actor sum share an authentication scheme. |  | — |
| [`bynk.actor.foreign_context_name_unkeyable`](/book/reference/diagnostics/bynk.actor.foreign_context_name_unkeyable/) | A context declaring a `Foreign` actor has a name no call key can be named from. |  | — |
| [`bynk.actor.foreign_duplicate_secret`](/book/reference/diagnostics/bynk.actor.foreign_duplicate_secret/) | A `Foreign` actor's secret is also another actor's secret in the same context. |  | — |
| [`bynk.actor.foreign_identity_unsupported`](/book/reference/diagnostics/bynk.actor.foreign_identity_unsupported/) | A `Foreign` actor declares an `identity`. |  | — |
| [`bynk.actor.foreign_missing_secret`](/book/reference/diagnostics/bynk.actor.foreign_missing_secret/) | A `Foreign` actor does not name the secret its calls are signed with. |  | — |
| [`bynk.actor.foreign_name_is_context_name`](/book/reference/diagnostics/bynk.actor.foreign_name_is_context_name/) | A `Foreign` actor's name could be read as a context's name. |  | — |
| [`bynk.actor.foreign_reserved_secret`](/book/reference/diagnostics/bynk.actor.foreign_reserved_secret/) | A `Foreign` actor's secret uses a `BYNK_CALL_` edge key's name. |  | — |
| [`bynk.actor.foreign_secret_not_string`](/book/reference/diagnostics/bynk.actor.foreign_secret_not_string/) | A `Foreign` actor's secret is not a string. |  | — |
| [`bynk.actor.identity_not_sealed`](/book/reference/diagnostics/bynk.actor.identity_not_sealed/) | An actor identity type is not a context-ownable (sealed) value type. |  | — |
| [`bynk.actor.missing_by_on_http`](/book/reference/diagnostics/bynk.actor.missing_by_on_http/) | An HTTP handler lacks the required `by` actor clause. |  | — |
| [`bynk.actor.oidc_identity_not_string_constructible`](/book/reference/diagnostics/bynk.actor.oidc_identity_not_string_constructible/) | An `Oidc` actor's identity is not a string-constructible type. |  | — |
| [`bynk.actor.oidc_missing_audience`](/book/reference/diagnostics/bynk.actor.oidc_missing_audience/) | An `Oidc` actor does not name its `audience`. |  | — |
| [`bynk.actor.oidc_missing_issuer`](/book/reference/diagnostics/bynk.actor.oidc_missing_issuer/) | An `Oidc` actor does not name its `issuer`. |  | — |
| [`bynk.actor.oidc_missing_jwks`](/book/reference/diagnostics/bynk.actor.oidc_missing_jwks/) | An `Oidc` actor does not name its `jwks` endpoint. |  | — |
| [`bynk.actor.oidc_not_in_sum`](/book/reference/diagnostics/bynk.actor.oidc_not_in_sum/) | An `Oidc` actor appears as a member of a multi-actor sum. |  | — |
| [`bynk.actor.oidc_trust_from_is_secret`](/book/reference/diagnostics/bynk.actor.oidc_trust_from_is_secret/) | An `Oidc` actor's `trustFrom` names an environment variable that an actor uses as its `secret`. |  | — |
| [`bynk.actor.oidc_trust_from_not_string`](/book/reference/diagnostics/bynk.actor.oidc_trust_from_not_string/) | An `Oidc` actor's `trustFrom` isn't a string naming an environment variable. |  | — |
| [`bynk.actor.outside_context`](/book/reference/diagnostics/bynk.actor.outside_context/) | An `actor` was declared outside a context (e.g. in a commons). |  | — |
| [`bynk.actor.refinement_base_unsupported`](/book/reference/diagnostics/bynk.actor.refinement_base_unsupported/) | A refinement actor's base is not a `Bearer` actor (no claims to authorise against). |  | — |
| [`bynk.actor.refinement_in_sum`](/book/reference/diagnostics/bynk.actor.refinement_in_sum/) | A refinement actor appears as a member of a multi-actor sum. |  | — |
| [`bynk.actor.refinement_predicate_unsupported`](/book/reference/diagnostics/bynk.actor.refinement_predicate_unsupported/) | A refinement actor's `where` predicate is outside the closed claim-predicate set. |  | — |
| [`bynk.actor.scheme_arg_kind`](/book/reference/diagnostics/bynk.actor.scheme_arg_kind/) | A scheme config value other than `SignedCookie`'s is a duration or a bare name, which only `SignedCookie` takes. |  | — |
| [`bynk.actor.scheme_not_admissible`](/book/reference/diagnostics/bynk.actor.scheme_not_admissible/) | An actor's scheme is not admissible on this handler's protocol. |  | — |
| [`bynk.actor.signature_identity_unsupported`](/book/reference/diagnostics/bynk.actor.signature_identity_unsupported/) | A `Signature` actor declared an `identity`, which is not yet supported. |  | — |
| [`bynk.actor.signature_missing_header`](/book/reference/diagnostics/bynk.actor.signature_missing_header/) | A `Signature` actor does not name its signature header. |  | — |
| [`bynk.actor.signature_missing_secret`](/book/reference/diagnostics/bynk.actor.signature_missing_secret/) | A `Signature` actor does not name its signing secret. |  | — |
| [`bynk.actor.signature_requires_body`](/book/reference/diagnostics/bynk.actor.signature_requires_body/) | A `Signature` handler does not take a `body` parameter. |  | — |
| [`bynk.actor.signature_tolerance_without_timestamp`](/book/reference/diagnostics/bynk.actor.signature_tolerance_without_timestamp/) | A `Signature` actor set `tolerance` without a `timestamp` header. |  | — |
| [`bynk.actor.signed_cookie_bad_arg`](/book/reference/diagnostics/bynk.actor.signed_cookie_bad_arg/) | A `SignedCookie` config value has the wrong kind. |  | — |
| [`bynk.actor.signed_cookie_bad_name`](/book/reference/diagnostics/bynk.actor.signed_cookie_bad_name/) | A `SignedCookie` `cookie` is not a legal cookie name. |  | — |
| [`bynk.actor.signed_cookie_bad_ttl`](/book/reference/diagnostics/bynk.actor.signed_cookie_bad_ttl/) | A `SignedCookie` `ttl` is not a positive whole number of seconds. |  | — |
| [`bynk.actor.signed_cookie_duplicate_key`](/book/reference/diagnostics/bynk.actor.signed_cookie_duplicate_key/) | A `SignedCookie` config repeats a key. |  | — |
| [`bynk.actor.signed_cookie_identity_not_string_constructible`](/book/reference/diagnostics/bynk.actor.signed_cookie_identity_not_string_constructible/) | A `SignedCookie` actor's identity is missing or not a string-constructible type. |  | — |
| [`bynk.actor.signed_cookie_issue_needs_clock`](/book/reference/diagnostics/bynk.actor.signed_cookie_issue_needs_clock/) | `Actor.issue(identity)` on a `SignedCookie` actor is called where the handler does not list `Clock` in `given`. |  | — |
| [`bynk.actor.signed_cookie_issue_outside_handler`](/book/reference/diagnostics/bynk.actor.signed_cookie_issue_outside_handler/) | `Actor.issue(identity)` on a `SignedCookie` actor is called outside a service handler (in a provider or an agent). |  | — |
| [`bynk.actor.signed_cookie_missing_secret`](/book/reference/diagnostics/bynk.actor.signed_cookie_missing_secret/) | A `SignedCookie` actor does not name its signing secret. |  | — |
| [`bynk.actor.signed_cookie_missing_ttl`](/book/reference/diagnostics/bynk.actor.signed_cookie_missing_ttl/) | A `SignedCookie` actor does not declare a `ttl`. |  | — |
| [`bynk.actor.signed_cookie_not_in_sum`](/book/reference/diagnostics/bynk.actor.signed_cookie_not_in_sum/) | A `SignedCookie` actor appears as a member of a multi-actor sum. |  | — |
| [`bynk.actor.signed_cookie_public_cache`](/book/reference/diagnostics/bynk.actor.signed_cookie_public_cache/) | A `@cache(scope: public)` route is guarded by a `SignedCookie` actor. |  | — |
| [`bynk.actor.signed_cookie_read_directly`](/book/reference/diagnostics/bynk.actor.signed_cookie_read_directly/) | A typed `cookies` field reads the session cookie that a route's `SignedCookie` actor consumes. |  | — |
| [`bynk.actor.signed_cookie_secret_shared`](/book/reference/diagnostics/bynk.actor.signed_cookie_secret_shared/) | A `SignedCookie` actor's `secret` or `previous` is also used by another actor, or by itself. |  | — |
| [`bynk.actor.signed_cookie_ttl_long`](/book/reference/diagnostics/bynk.actor.signed_cookie_ttl_long/) | A `SignedCookie` `ttl` is longer than 30 days (warning). |  | Warning |
| [`bynk.actor.signed_cookie_unknown_key`](/book/reference/diagnostics/bynk.actor.signed_cookie_unknown_key/) | A `SignedCookie` config has a key outside the closed set. |  | — |
| [`bynk.actor.signed_cookie_unprefixed`](/book/reference/diagnostics/bynk.actor.signed_cookie_unprefixed/) | A `SignedCookie` `cookie` does not start with `__Host-` (warning). |  | Warning |
| [`bynk.actor.signed_cookie_write_directly`](/book/reference/diagnostics/bynk.actor.signed_cookie_write_directly/) | A cookie record's typed write names the session cookie that a `SignedCookie` actor consumes. |  | — |
| [`bynk.actor.signed_cookie_write_literal`](/book/reference/diagnostics/bynk.actor.signed_cookie_write_literal/) | An untyped `Cookie` literal is named like a `SignedCookie` actor's session cookie. |  | — |
| [`bynk.actor.sum_requires_binder`](/book/reference/diagnostics/bynk.actor.sum_requires_binder/) | A multi-actor sum `by` clause has no binder to match the resolved actor. |  | — |
| [`bynk.actor.unknown_actor`](/book/reference/diagnostics/bynk.actor.unknown_actor/) | A handler's `by` clause names an actor that is not declared. |  | — |
| [`bynk.actor.unknown_scheme`](/book/reference/diagnostics/bynk.actor.unknown_scheme/) | An actor declares an authentication scheme that is not compiler-known. |  | — |
| [`bynk.actor.unreachable_sum_arm`](/book/reference/diagnostics/bynk.actor.unreachable_sum_arm/) | A multi-actor sum has an arm unreachable after a catch-all (`None`) peer. |  | — |

## Adapters

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.adapter.consumes_context`](/book/reference/diagnostics/bynk.adapter.consumes_context/) | An `adapter` consumed a context; adapter dependencies are adapter-to-adapter. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.adapter.consumes_requires_selection`](/book/reference/diagnostics/bynk.adapter.consumes_requires_selection/) | An `adapter` used a whole-unit or aliased `consumes`; adapters must select capabilities with `consumes U { Cap, … }`. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.adapter.disallowed_item`](/book/reference/diagnostics/bynk.adapter.disallowed_item/) | An `adapter` declared a `service`, `agent`, or other item it may not contain. | [`adapter_decl`](/book/reference/grammar/#rule-adapter_decl) | — |
| [`bynk.adapter.duplicate_binding`](/book/reference/diagnostics/bynk.adapter.duplicate_binding/) | An `adapter` declared more than one `binding` clause. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| [`bynk.adapter.include_duplicate`](/book/reference/diagnostics/bynk.adapter.include_duplicate/) | An adapter `binding … includes [ … ]` list names one file more than once. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| [`bynk.adapter.include_kind`](/book/reference/diagnostics/bynk.adapter.include_kind/) | An adapter `binding … includes [ … ]` entry is not a `.wasm` file; only WebAssembly modules can be included. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| [`bynk.adapter.include_missing`](/book/reference/diagnostics/bynk.adapter.include_missing/) | An adapter `binding … includes [ … ]` entry names a file that could not be read. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| [`bynk.adapter.include_outside`](/book/reference/diagnostics/bynk.adapter.include_outside/) | An adapter `binding … includes [ … ]` path is absolute or climbs out of the adapter's directory with `..`. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| [`bynk.adapter.no_binding`](/book/reference/diagnostics/bynk.adapter.no_binding/) | An `adapter` declares an external provider but no `binding` module to supply it. | [`adapter_decl`](/book/reference/grammar/#rule-adapter_decl) | — |
| [`bynk.adapter.provider_has_body`](/book/reference/diagnostics/bynk.adapter.provider_has_body/) | A provider inside an `adapter` has a Bynk body; adapter providers must be external. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |

## Agent storage

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.store.annotation_kind_mismatch`](/book/reference/diagnostics/bynk.store.annotation_kind_mismatch/) | A storage annotation is used on a kind it does not apply to (e.g. `@ttl` on a `Map`). |  | — |
| [`bynk.store.annotation_unsupported`](/book/reference/diagnostics/bynk.store.annotation_unsupported/) | A known storage annotation that nothing acts on yet (`@bounded`) is used; the others (`@ttl`, `@retain`, `@indexed`) are functional. |  | — |
| [`bynk.store.cache_needs_clock`](/book/reference/diagnostics/bynk.store.cache_needs_clock/) | A handler performs a `Cache` operation (TTL expiry reads the clock) without declaring `given Clock`. |  | — |
| [`bynk.store.cache_ttl_required`](/book/reference/diagnostics/bynk.store.cache_ttl_required/) | A `Cache` field is missing its required `@ttl(<duration>)` annotation (a keyed store with no expiry is a `Map`). |  | — |
| [`bynk.store.kind_arity`](/book/reference/diagnostics/bynk.store.kind_arity/) | A storage kind was applied to the wrong number of type arguments (e.g. `Cell[A, B]`). |  | — |
| [`bynk.store.kind_unsupported`](/book/reference/diagnostics/bynk.store.kind_unsupported/) | A known storage kind (`Queue`) is used before the slice that supports it. |  | — |
| [`bynk.store.log_needs_clock`](/book/reference/diagnostics/bynk.store.log_needs_clock/) | A handler calls `Log.append` (which stamps the current time) without declaring `given Clock`. |  | — |
| [`bynk.store.unkeyable_key`](/book/reference/diagnostics/bynk.store.unkeyable_key/) | A store `Set` element or `Map`/`Cache` key is not value-keyable — it must be `String`, `Int`, or a refined/opaque type over them (#1680). |  | — |
| [`bynk.store.unknown_annotation`](/book/reference/diagnostics/bynk.store.unknown_annotation/) | A `store` field carries an annotation outside the closed `@indexed`/`@ttl`/`@retain`/`@bounded` set. |  | — |
| [`bynk.store.unknown_kind`](/book/reference/diagnostics/bynk.store.unknown_kind/) | A `store` field's type is not a known storage kind. |  | — |
| [`bynk.store.unknown_map_accessor`](/book/reference/diagnostics/bynk.store.unknown_map_accessor/) | A `store Map` field access is not one of its query accessors (`entries`/`keys`/`values`). |  | — |
| [`bynk.store.unknown_op`](/book/reference/diagnostics/bynk.store.unknown_op/) | A method called on a `store` field (`Map`, `Cache`, `Set`, `Cell` or `Log`) is not one of that kind's storage operations. |  | — |

## Agents

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.agent.class_conflict`](/book/reference/diagnostics/bynk.agent.class_conflict/) | An agent class has more than one fate: it is renamed twice, both renamed and removed, or renamed onto a class that already has its own recorded history in `bynk.schema.lock`. |  | — |
| [`bynk.agent.class_removed`](/book/reference/diagnostics/bynk.agent.class_removed/) | An agent class recorded in `bynk.schema.lock` is no longer declared, so a deployment's stored instances of it would be orphaned. If the class was never deployed, delete its `[agents]` entry from the lock; renaming or removing a deployed class gets its own declaration in a later state-migrations slice. |  | — |
| [`bynk.agent.construction_arity`](/book/reference/diagnostics/bynk.agent.construction_arity/) | An agent was constructed with the wrong number of key arguments. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| [`bynk.agent.handler_arity`](/book/reference/diagnostics/bynk.agent.handler_arity/) | An agent handler was called with the wrong number of arguments. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| [`bynk.agent.handler_not_found`](/book/reference/diagnostics/bynk.agent.handler_not_found/) | Called a handler the agent does not declare. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| [`bynk.agent.key_mismatch`](/book/reference/diagnostics/bynk.agent.key_mismatch/) | An agent key argument has the wrong type. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| [`bynk.agent.migrate_below_floor`](/book/reference/diagnostics/bynk.agent.migrate_below_floor/) | A `migrate` step reads a schema version below the agent's floor. A complete `bynk migrate sweep` raised the floor past it, so no stored instance can hold that version: the step emits nothing and can be deleted (warning). |  | Warning |
| [`bynk.agent.migrate_duplicate`](/book/reference/diagnostics/bynk.agent.migrate_duplicate/) | Two `migrate` steps in one agent read the same schema version. |  | — |
| [`bynk.agent.migrate_duplicate_assignment`](/book/reference/diagnostics/bynk.agent.migrate_duplicate_assignment/) | A `migrate` step assigns the same `store` field more than once; each field takes at most one value per step (record-literal semantics). |  | — |
| [`bynk.agent.migrate_impure`](/book/reference/diagnostics/bynk.agent.migrate_impure/) | A `migrate` step uses an effect, a capability, `?`, `expect`, or the agent's `key`; steps are pure and key-free so a migration sweep can run them by object ID. |  | — |
| [`bynk.agent.migrate_incomplete`](/book/reference/diagnostics/bynk.agent.migrate_incomplete/) | A `migrate` step does not account for every change between its version and the current shape: a removed field must be read (`old.f`) or dropped, and a retyped or re-kinded field must be assigned (or dropped, for a `Set`, `Log` or `Cache` field). |  | — |
| [`bynk.agent.migrate_shape_mismatch`](/book/reference/diagnostics/bynk.agent.migrate_shape_mismatch/) | A `migrate` step's declared old shape (or a `drop`) names a field the lock does not record at that version, or records with a different structure. |  | — |
| [`bynk.agent.migrate_unknown_version`](/book/reference/diagnostics/bynk.agent.migrate_unknown_version/) | A `migrate` step reads a schema version the lock does not record below the agent's current version. |  | — |
| [`bynk.agent.non_additive_state_change`](/book/reference/diagnostics/bynk.agent.non_additive_state_change/) | An agent's stored state changed in a way deployed data cannot be read through — a `store` field was removed, retyped (including inside a stored record or sum), or changed storage kind. A renamed field is a removed field plus an added one. If the class was never deployed, delete its `[agents]` entry from `bynk.schema.lock`; otherwise widen rather than narrow, or stage the change through a new field. |  | — |
| [`bynk.agent.outside_context`](/book/reference/diagnostics/bynk.agent.outside_context/) | An `agent` was declared outside a context. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| [`bynk.agent.refinement_changed`](/book/reference/diagnostics/bynk.agent.refinement_changed/) | A stored `store` field's refinement changed while its structure did not. A widening is safe; a narrowing faults on load for any stored value that no longer satisfies it (`RehydrationViolation`), so migrate the data first (warning). |  | Warning |
| [`bynk.agent.removed_live`](/book/reference/diagnostics/bynk.agent.removed_live/) | A `removed agent` declaration names a class that is still declared; removing a class deletes its stored data. |  | — |
| [`bynk.agent.replaces_live`](/book/reference/diagnostics/bynk.agent.replaces_live/) | `agent New replaces Old` names an `Old` that is still declared; a rename moves the old class's data to the new one. |  | — |
| [`bynk.agent.replaces_unknown`](/book/reference/diagnostics/bynk.agent.replaces_unknown/) | `agent New replaces Old` names an `Old` that `bynk.schema.lock` does not record for this context, so there is no recorded class to rename. |  | — |
| [`bynk.agent.return_not_effect`](/book/reference/diagnostics/bynk.agent.return_not_effect/) | An agent handler's return type is not an `Effect`. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| [`bynk.agent.state_name_conflict`](/book/reference/diagnostics/bynk.agent.state_name_conflict/) | A type is named `<Agent>State` beside agent `<Agent>`; that name is the agent's state record (its `Cell` fields), which its handlers and invariants see in place of the type. |  | — |
| [`bynk.agent.version_without_path`](/book/reference/diagnostics/bynk.agent.version_without_path/) | A schema version between the agent's floor and its current version has no way forward: no `migrate` step covers it and its path to the current shape is not entirely additive, so a stored record at that version faults on load with `NoMigrationPath` (warning). |  | Warning |
| [`bynk.agents.bad_state_initialiser`](/book/reference/diagnostics/bynk.agents.bad_state_initialiser/) | An agent `store` field initialiser is not a static value of the field's type. | [`store_field`](/book/reference/grammar/#rule-store_field) | — |
| [`bynk.agents.non_zeroable_state_field`](/book/reference/diagnostics/bynk.agents.non_zeroable_state_field/) | An agent `store` field has no initialiser and no implicit zero value. | [`store_field`](/book/reference/grammar/#rule-store_field) | — |

## Boundaries

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.boundary.ambiguous_type`](/book/reference/diagnostics/bynk.boundary.ambiguous_type/) | A value's type is declared, with different shapes, by more than one consumed context, so its fields cannot be read. |  | — |
| [`bynk.boundary.structural_mismatch`](/book/reference/diagnostics/bynk.boundary.structural_mismatch/) | Data crossing a context boundary did not match the expected shape. |  | — |

## Capabilities

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.capability.base_not_instantiable`](/book/reference/diagnostics/bynk.capability.base_not_instantiable/) | A derived capability's base is not a platform resource that can be instantiated. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| [`bynk.capability.derived_outside_context`](/book/reference/diagnostics/bynk.capability.derived_outside_context/) | A derived capability was declared outside a context. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| [`bynk.capability.instance_only`](/book/reference/diagnostics/bynk.capability.instance_only/) | A platform capability that is used only through named instances (`Service`, `R2`, `RateLimit`, `Queue`, `Analytics`, `D1`, `EdgeCache`) was used bare, in a `given` clause. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.capability.invalid_option`](/book/reference/diagnostics/bynk.capability.invalid_option/) | A derived capability's option is not a single positional literal of its kind (string, positive `Int`, or an offered window), is given twice, or is a value the kind refuses (an `@worker` that is not a well-formed Worker name outside the project, a malformed queue `@name`, an unknown `@jurisdiction`). | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| [`bynk.capability.missing_option`](/book/reference/diagnostics/bynk.capability.missing_option/) | A derived capability lacks an option its base requires, such as a `Service` instance's `@worker`. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| [`bynk.capability.not_provided`](/book/reference/diagnostics/bynk.capability.not_provided/) | A capability a context requires has no provider in it, on the Workers target or in a bundle with a composition root. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.capability.op_arity`](/book/reference/diagnostics/bynk.capability.op_arity/) | A capability operation was called with the wrong number of arguments. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| [`bynk.capability.outside_context`](/book/reference/diagnostics/bynk.capability.outside_context/) | A `capability` was declared outside a context. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| [`bynk.capability.provider_agent_cross_context`](/book/reference/diagnostics/bynk.capability.provider_agent_cross_context/) | On the Workers target, a context requires a capability whose provider, in another context, constructs that context's agents; the consumer's Worker would build the provider without a binding for those agents' Durable Objects. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.capability.unknown_base`](/book/reference/diagnostics/bynk.capability.unknown_base/) | A derived capability's base is not a capability the context consumes. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| [`bynk.capability.unknown_operation`](/book/reference/diagnostics/bynk.capability.unknown_operation/) | Referenced an operation the capability does not declare. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| [`bynk.capability.unknown_option`](/book/reference/diagnostics/bynk.capability.unknown_option/) | A derived capability carries an option its base does not take. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |

## Cloudflare

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.cloudflare.edge_cache_duplicate`](/book/reference/diagnostics/bynk.cloudflare.edge_cache_duplicate/) | A context declares a second `EdgeCache` instance; the edge cache is one setting for the whole Worker. |  | — |
| [`bynk.cloudflare.edge_cache_exported`](/book/reference/diagnostics/bynk.cloudflare.edge_cache_exported/) | An exported capability reaches an `EdgeCache` instance through its provider's `given`; a consuming context would build it in its own Worker, whose purge misses this one's cache. |  | — |
| [`bynk.cloudflare.edge_cache_in_agent`](/book/reference/diagnostics/bynk.cloudflare.edge_cache_in_agent/) | An agent handler reaches an `EdgeCache` instance, directly or through a provider's `given`; an agent's purge would not reach the Worker's cache. |  | — |
| [`bynk.cloudflare.edge_cache_in_queue`](/book/reference/diagnostics/bynk.cloudflare.edge_cache_in_queue/) | A queue consumer reaches an `EdgeCache` instance; purge from a service handler instead. |  | — |
| [`bynk.cloudflare.edge_cache_in_schedule`](/book/reference/diagnostics/bynk.cloudflare.edge_cache_in_schedule/) | An `on schedule` handler reaches an `EdgeCache` instance; a cron run's purge is accepted but clears nothing from the fetch cache, so purge from a service handler. |  | — |
| [`bynk.cloudflare.edge_cache_public_authenticated`](/book/reference/diagnostics/bynk.cloudflare.edge_cache_public_authenticated/) | In a context with `EdgeCache`, a `@cache(scope: public)` route is served by an actor that authenticates; its response would be shared at the edge with every caller. |  | — |

## Consumes

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.consumes.alias_conflict`](/book/reference/diagnostics/bynk.consumes.alias_conflict/) | Two `consumes` aliases collide. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.capability_name_clash`](/book/reference/diagnostics/bynk.consumes.capability_name_clash/) | Two flattened `consumes U { Cap }` capabilities collide, or one clashes with a local capability. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.capability_not_selected`](/book/reference/diagnostics/bynk.consumes.capability_not_selected/) | A capability a consumed unit exports is named (a `given` clause, a call, `provides`, a `stub`) in a file whose own `consumes` does not list it. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.name_conflict`](/book/reference/diagnostics/bynk.consumes.name_conflict/) | A `consumes` name collides with another name in scope. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.self_reference`](/book/reference/diagnostics/bynk.consumes.self_reference/) | A context `consumes` itself. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.service_arity`](/book/reference/diagnostics/bynk.consumes.service_arity/) | A consumed service was called with the wrong number of arguments. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.signature_type_unconsumed`](/book/reference/diagnostics/bynk.consumes.signature_type_unconsumed/) | A cross-context call reaches a service whose signature names a type the consumed context itself consumes from a third context, which the caller does not consume. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.target_is_commons`](/book/reference/diagnostics/bynk.consumes.target_is_commons/) | `consumes` targets a `commons` instead of a context. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.type_alias_capability`](/book/reference/diagnostics/bynk.consumes.type_alias_capability/) | A capability was aliased in `consumes U { Cap as Alias }`; only exported types can be aliased. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.type_alias_conflict`](/book/reference/diagnostics/bynk.consumes.type_alias_conflict/) | A `consumes U { T as Alias }` alias collides with a type or value in scope, or with another alias. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.type_alias_repeated`](/book/reference/diagnostics/bynk.consumes.type_alias_repeated/) | An exported type is aliased more than once, or is selected both bare and aliased, in `consumes U { … }`. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.type_alias_unknown_export`](/book/reference/diagnostics/bynk.consumes.type_alias_unknown_export/) | `consumes U { T as Alias }` names a type `U` does not export. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.type_not_selected`](/book/reference/diagnostics/bynk.consumes.type_not_selected/) | A type (or a bare variant of a sum) of a consumed unit is named in a file whose own `consumes` does not bring it in: it lists other names, or only another file of the unit consumes the unit. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.unit_not_consumed`](/book/reference/diagnostics/bynk.consumes.unit_not_consumed/) | A call to another unit's service is written in a file whose own `consumes` does not name that unit. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.unknown_context`](/book/reference/diagnostics/bynk.consumes.unknown_context/) | `consumes` names a context that does not exist. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.consumes.unknown_service`](/book/reference/diagnostics/bynk.consumes.unknown_service/) | Called a service the consumed context does not declare. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |

## Contexts

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.context.consumes_cycle`](/book/reference/diagnostics/bynk.context.consumes_cycle/) | Contexts form a `consumes` dependency cycle. |  | — |
| [`bynk.context.duplicate_binding`](/book/reference/diagnostics/bynk.context.duplicate_binding/) | A Worker would declare two bindings with the same name, such as an agent `Kv` and the `bynk.cloudflare` KV namespace (both `KV`). |  | — |
| [`bynk.context.external_construction`](/book/reference/diagnostics/bynk.context.external_construction/) | A context-owned type was constructed from outside that context. |  | — |
| [`bynk.context.external_decode`](/book/reference/diagnostics/bynk.context.external_decode/) | A context decoded a type another context owns and exports opaque: through `Json.decode`, `Json.decodeSql` or a `Codable` witness, or as the input of an HTTP, queue, event or WebSocket handler. Decoding creates a value, which only the owning context may do; its codec would check no more than the base type. |  | — |
| [`bynk.context.external_provider`](/book/reference/diagnostics/bynk.context.external_provider/) | A bodiless (external) provider was declared outside an `adapter`. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| [`bynk.context.opaque_inspection`](/book/reference/diagnostics/bynk.context.opaque_inspection/) | An opaquely-exported type was inspected from outside its context. |  | — |
| [`bynk.context.rebrand_construction`](/book/reference/diagnostics/bynk.context.rebrand_construction/) | A variant of a `uses`-sourced commons sum type was constructed directly inside a context, where the emitter's per-context rebrand leaves its constructors out of scope. |  | — |

## Contracts

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.contract.duplicate_name`](/book/reference/diagnostics/bynk.contract.duplicate_name/) | A function declares two contract clauses (`requires`/`ensures`) with the same name. |  | — |
| [`bynk.contract.impure_predicate`](/book/reference/diagnostics/bynk.contract.impure_predicate/) | A contract predicate uses an effectful or test-only construct; a contract clause must be pure. |  | — |
| [`bynk.contract.not_bool`](/book/reference/diagnostics/bynk.contract.not_bool/) | A contract predicate does not have type `Bool`. |  | — |
| [`bynk.contract.restated_by_test`](/book/reference/diagnostics/bynk.contract.restated_by_test/) | A `case`/`property` merely restates a contract clause already declared at the function; the test is redundant. |  | — |
| [`bynk.contract.result_in_requires`](/book/reference/diagnostics/bynk.contract.result_in_requires/) | A precondition (`requires`) references `result`; the return value is only in scope inside an `ensures`. |  | — |

## Cron

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.cron.bad_params`](/book/reference/diagnostics/bynk.cron.bad_params/) | A cron handler declares more than one parameter, or a non-`Int` one. | [`cron_handler`](/book/reference/grammar/#rule-cron_handler) | — |
| [`bynk.cron.duplicate_schedule`](/book/reference/diagnostics/bynk.cron.duplicate_schedule/) | Two cron handlers declare the same schedule. | [`cron_handler`](/book/reference/grammar/#rule-cron_handler) | — |
| [`bynk.cron.invalid_schedule`](/book/reference/diagnostics/bynk.cron.invalid_schedule/) | A cron expression is not five whitespace-separated fields. | [`cron_handler`](/book/reference/grammar/#rule-cron_handler) | — |
| [`bynk.cron.return_not_effect_result`](/book/reference/diagnostics/bynk.cron.return_not_effect_result/) | A cron handler does not return `Effect[Result[(), E]]`. | [`cron_handler`](/book/reference/grammar/#rule-cron_handler) | — |

## Deployment

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.deploy.agent_move_refused`](/book/reference/diagnostics/bynk.deploy.agent_move_refused/) | `bynk deploy` refused to put a context that hosts agents on a different Worker script than it was last deployed to (into or out of a `[deploy.workers.<key>]` group, between groups, or after a group is renamed or re-keyed): an agent's state belongs to its script, so it would start empty and the old data would stay behind. Start it fresh deliberately with `--accept-fresh-agent <context>`, which a move out of a group that stays cannot use. |  | — |
| [`bynk.deploy.asset_path_conflict`](/book/reference/diagnostics/bynk.deploy.asset_path_conflict/) | Two members of one `[deploy.workers.<key>]` group serve a fixed asset path (`at "/…"`) from different files, or one that would be a folder of the other; a group has one assets directory, so each fixed path names one file. |  | — |
| [`bynk.deploy.binding_collision`](/book/reference/diagnostics/bynk.deploy.binding_collision/) | Two members of one `[deploy.workers.<key>]` group declare different bindings under the same name; members share one `wrangler.toml`, so identical declarations share a binding and differing ones are refused. |  | — |
| [`bynk.deploy.context_in_two_groups`](/book/reference/diagnostics/bynk.deploy.context_in_two_groups/) | A context is listed in two `[deploy.workers.<key>]` groups; a context is deployed in one Worker only. |  | — |
| [`bynk.deploy.contract_skew`](/book/reference/diagnostics/bynk.deploy.contract_skew/) | A worker was compiled against a contract its live dependencies no longer provide, or no longer provides the contract its live callers were compiled against, so `bynk deploy --context` refuses to ship it. |  | — |
| [`bynk.deploy.duplicate_hostname`](/book/reference/diagnostics/bynk.deploy.duplicate_hostname/) | A route or custom domain is claimed twice across `[deploy.workers.<key>]` groups. |  | — |
| [`bynk.deploy.empty_group`](/book/reference/diagnostics/bynk.deploy.empty_group/) | A `[deploy.workers.<key>]` group lists no context. |  | — |
| [`bynk.deploy.foreign_contract_skew`](/book/reference/diagnostics/bynk.deploy.foreign_contract_skew/) | A build breaks the contract a declared foreign caller's client was generated at, as pinned in `bynk.foreign.toml`, so `bynk deploy` refuses to ship it. |  | — |
| [`bynk.deploy.group_name_conflict`](/book/reference/diagnostics/bynk.deploy.group_name_conflict/) | A `[deploy.workers.<key>]` group's key or script name is another group's, or is the Worker of a context in no group. |  | — |
| [`bynk.deploy.multiple_websocket_members`](/book/reference/diagnostics/bynk.deploy.multiple_websocket_members/) | More than one member of a `[deploy.workers.<key>]` group declares a `from websocket` service; an upgrade request names no path or service a group could route it by, so a group has at most one such member, which takes every upgrade request whatever the path. |  | — |
| [`bynk.deploy.not_a_context`](/book/reference/diagnostics/bynk.deploy.not_a_context/) | A `[deploy.workers.<key>]` group lists a unit that is not a `context`; only contexts are deployed as Workers. |  | — |
| [`bynk.deploy.route_conflict`](/book/reference/diagnostics/bynk.deploy.route_conflict/) | Two members of one `[deploy.workers.<key>]` group declare the same HTTP method and path; a Worker answers a route with one handler. |  | — |
| [`bynk.deploy.service_name_conflict`](/book/reference/diagnostics/bynk.deploy.service_name_conflict/) | Two members of one `[deploy.workers.<key>]` group declare a service of the same name; a Worker answers its members' services by name. |  | — |
| [`bynk.deploy.unknown_context`](/book/reference/diagnostics/bynk.deploy.unknown_context/) | A `[deploy.workers.<key>]` group lists a context the project does not declare. |  | — |
| [`bynk.deploy.worker_cycle`](/book/reference/diagnostics/bynk.deploy.worker_cycle/) | Workers bind to each other in a cycle that involves a `[deploy.workers.<key>]` group: a group folds its members into one Worker, which can close a cycle among contexts that are themselves acyclic. A Worker is uploaded after the Workers it binds to, so the cycle cannot be deployed. |  | — |
| [`bynk.deploy.workers_ignored_on_bundle`](/book/reference/diagnostics/bynk.deploy.workers_ignored_on_bundle/) | `bynk.toml` declares `[deploy.workers]` groups, which only a `workers` build uses; this build is a `bundle`, which has every context in one program and ignores them (warning). |  | Warning |

## Doc comments

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.doc.unresolved_link`](/book/reference/diagnostics/bynk.doc.unresolved_link/) | A `[Name]` doc link resolves to nothing in the unit's scope (editor hint only). |  | Warning |

## Effects

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.effect.bind_in_pure_context`](/book/reference/diagnostics/bynk.effect.bind_in_pure_context/) | An `<-` bind was used in a pure (non-effectful) context. | [`effect_let_stmt`](/book/reference/grammar/#rule-effect_let_stmt) | — |
| [`bynk.effect.bind_on_non_effect`](/book/reference/diagnostics/bynk.effect.bind_on_non_effect/) | An `<-` bind was applied to a non-`Effect` value. | [`effect_let_stmt`](/book/reference/grammar/#rule-effect_let_stmt) | — |
| [`bynk.effect.capability_in_pure_context`](/book/reference/diagnostics/bynk.effect.capability_in_pure_context/) | A capability was used in a pure context. |  | — |
| [`bynk.effect.cross_context_in_pure_context`](/book/reference/diagnostics/bynk.effect.cross_context_in_pure_context/) | A cross-context call was made in a pure context. |  | — |
| [`bynk.effect.do_discards_result`](/book/reference/diagnostics/bynk.effect.do_discards_result/) | A `do` statement was applied to an `Effect[Result[_, _]]`, which would drop an `Err`; bind it, write `let _ <- e` to discard it deliberately, or `do e.detach()` to stop waiting and have a failure reported. | [`do_stmt`](/book/reference/grammar/#rule-do_stmt) | — |
| [`bynk.effect.do_in_pure_context`](/book/reference/diagnostics/bynk.effect.do_in_pure_context/) | A `do` statement was used in a pure (non-effectful) context. | [`do_stmt`](/book/reference/grammar/#rule-do_stmt) | — |
| [`bynk.effect.do_on_non_effect`](/book/reference/diagnostics/bynk.effect.do_on_non_effect/) | A `do` statement was applied to a non-`Effect` value. | [`do_stmt`](/book/reference/grammar/#rule-do_stmt) | — |
| [`bynk.effect.fn_value_in_pure_context`](/book/reference/diagnostics/bynk.effect.fn_value_in_pure_context/) | An effectful function value was called in a pure context; like a capability call, it is legal only where the enclosing body is effectful. | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.effect.unbound_effect`](/book/reference/diagnostics/bynk.effect.unbound_effect/) | An `Effect` value in an effectful body was built but not bound with `<-`, sequenced with `do` or returned — in a plain `let`, a list element or a constructor payload — so it runs eagerly and unawaited (#1658). |  | — |

## Embedded text

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.embed.binary_content`](/book/reference/diagnostics/bynk.embed.binary_content/) | An `embed` names a file with a NUL byte, so it is binary, not text. |  | — |
| [`bynk.embed.duplicate`](/book/reference/diagnostics/bynk.embed.duplicate/) | An `embed` name is declared more than once in the same unit. |  | — |
| [`bynk.embed.invalid_encoding`](/book/reference/diagnostics/bynk.embed.invalid_encoding/) | An `embed` names a file that is not valid UTF-8. |  | — |
| [`bynk.embed.missing_file`](/book/reference/diagnostics/bynk.embed.missing_file/) | An `embed` names a file that is not in the project's `embeds/` folder. |  | — |
| [`bynk.embed.needs_project`](/book/reference/diagnostics/bynk.embed.needs_project/) | A single-file build met an `embed` declaration, which needs a project's `embeds/` folder. |  | — |
| [`bynk.embed.outside_context_or_commons`](/book/reference/diagnostics/bynk.embed.outside_context_or_commons/) | An `embed` declaration appears outside a context or commons. |  | — |
| [`bynk.embed.outside_embeds_dir`](/book/reference/diagnostics/bynk.embed.outside_embeds_dir/) | An `embed` names a file outside the project's `embeds/` folder. |  | — |
| [`bynk.embed.too_large`](/book/reference/diagnostics/bynk.embed.too_large/) | An `embed` names a file over 256 KiB, or the project's embedded files total over 1 MiB. |  | — |
| [`bynk.embed.unreadable_file`](/book/reference/diagnostics/bynk.embed.unreadable_file/) | An `embed` names a file in the project's `embeds/` folder that could not be read. |  | — |
| [`bynk.embed.unsafe_path`](/book/reference/diagnostics/bynk.embed.unsafe_path/) | An `embed` path is empty, absolute, or contains a `..` segment. |  | — |

## Events

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.event.bad_params`](/book/reference/diagnostics/bynk.event.bad_params/) | An `on event` handler declared the wrong number of parameters, or a second parameter whose type is not `EventEnvelope` — it takes the event payload and, optionally, the runtime envelope. |  | — |
| [`bynk.event.bad_schema_dispatch`](/book/reference/diagnostics/bynk.event.bad_schema_dispatch/) | A `via schema(...)` dispatch clause's argument is malformed — it must be a positive `Int` literal, a range of positive `Int` literals, or `_`. |  | — |
| [`bynk.event.bad_schema_version`](/book/reference/diagnostics/bynk.event.bad_schema_version/) | An event's `@schema(N)` annotation is malformed — `N` must be a single, positive, positional `Int` literal, and `@schema` may appear at most once on an event. |  | — |
| [`bynk.event.duplicate_handler`](/book/reference/diagnostics/bynk.event.duplicate_handler/) | A `from Events` service declares more than one `on event` handler; it has exactly one. | [`event_handler`](/book/reference/grammar/#rule-event_handler) | — |
| [`bynk.event.emit_not_an_event`](/book/reference/diagnostics/bynk.event.emit_not_an_event/) | `Events.emit[E]` named a type `E` that is declared in this context, but is not itself an `event` — only an `event` type may be emitted. |  | — |
| [`bynk.event.emit_outside_owner`](/book/reference/diagnostics/bynk.event.emit_outside_owner/) | `Events.emit[E]` named an event `E` not declared in the emitting context — only an event's declaring context may emit it. |  | — |
| [`bynk.event.handler_param_type_mismatch`](/book/reference/diagnostics/bynk.event.handler_param_type_mismatch/) | An `on event(e: T)` handler's declared parameter type does not match its `from Events(E)` header's event type. |  | — |
| [`bynk.event.non_additive_schema_change`](/book/reference/diagnostics/bynk.event.non_additive_schema_change/) | An event's field shape changed in a way the schema registry cannot evolve additively — a field was removed, retyped, added without a default, or lost a default it previously had. Give the new shape a new event type name, or make the change additive. |  | — |
| [`bynk.event.outside_context`](/book/reference/diagnostics/bynk.event.outside_context/) | An `event` was declared outside a context. |  | — |
| [`bynk.event.pattern_duplicate_field`](/book/reference/diagnostics/bynk.event.pattern_duplicate_field/) | A `from Events(E { ... })` subscription pattern listed the same field more than once. |  | — |
| [`bynk.event.pattern_type_mismatch`](/book/reference/diagnostics/bynk.event.pattern_type_mismatch/) | A `from Events(E { ... })` subscription pattern field's matched value is not compatible with that field's declared type. |  | — |
| [`bynk.event.pattern_unknown_field`](/book/reference/diagnostics/bynk.event.pattern_unknown_field/) | A `from Events(E { ... })` subscription pattern named a field that `E` does not declare. |  | — |
| [`bynk.event.pattern_unknown_variant`](/book/reference/diagnostics/bynk.event.pattern_unknown_variant/) | A `from Events(E { ... })` subscription pattern's variant value names a variant that does not exist on the field's declared sum type. |  | — |
| [`bynk.event.pattern_variant_payload`](/book/reference/diagnostics/bynk.event.pattern_variant_payload/) | A `from Events(E { ... })` subscription pattern's variant value names a variant that carries a payload — only nullary variants are admitted, since testing the tag alone would silently ignore the payload. |  | — |
| [`bynk.event.return_not_effect_unit`](/book/reference/diagnostics/bynk.event.return_not_effect_unit/) | An `on event` handler returns an `Effect` of something other than `()`; emission is fire-and-forget, so it returns `Effect[()]`. | [`event_handler`](/book/reference/grammar/#rule-event_handler) | — |
| [`bynk.event.schema_version_mismatch`](/book/reference/diagnostics/bynk.event.schema_version_mismatch/) | An event's `@schema(N)` annotation disagrees with the version the schema registry computes from the event's build history. |  | — |
| [`bynk.event.unknown_annotation`](/book/reference/diagnostics/bynk.event.unknown_annotation/) | An `event` declaration carried an `@`-annotation other than `@schema` — event annotations are a closed set. |  | — |
| [`bynk.event.unknown_subscription`](/book/reference/diagnostics/bynk.event.unknown_subscription/) | A `from Events(E)` subscription named `E`, which is not a declared event in this context or any consumed context. |  | — |

## Expectations

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.expect.faults_not_effect`](/book/reference/diagnostics/bynk.expect.faults_not_effect/) | A fault claim's subject (`expect <call> faults`) is not an effectful call: a value, or a call whose type is not `Effect[_]`. | [`faults_expr`](/book/reference/grammar/#rule-faults_expr) | — |
| [`bynk.expect.impure_predicate`](/book/reference/diagnostics/bynk.expect.impure_predicate/) | An `expect` predicate contains an effectful call (a sub-expression of type `Effect[_]`); a predicate must be pure. | [`expect_expr`](/book/reference/grammar/#rule-expect_expr) | — |
| [`bynk.expect.not_bool`](/book/reference/diagnostics/bynk.expect.not_bool/) | `expect` was given a non-`Bool` predicate. | [`expect_expr`](/book/reference/grammar/#rule-expect_expr) | — |
| [`bynk.expect.outside_case`](/book/reference/diagnostics/bynk.expect.outside_case/) | `expect` was used outside a `case` body. | [`expect_expr`](/book/reference/grammar/#rule-expect_expr) | — |

## Exports

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.exports.capability_not_provided`](/book/reference/diagnostics/bynk.exports.capability_not_provided/) | An exported capability has no provider in its context. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| [`bynk.exports.conflicting_visibility`](/book/reference/diagnostics/bynk.exports.conflicting_visibility/) | A type is exported with conflicting visibilities. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| [`bynk.exports.derived_capability`](/book/reference/diagnostics/bynk.exports.derived_capability/) | `exports capability` names a derived capability, whose instance stays with its context. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| [`bynk.exports.duplicate_export`](/book/reference/diagnostics/bynk.exports.duplicate_export/) | The same name is exported more than once. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| [`bynk.exports.duplicate_in_clause`](/book/reference/diagnostics/bynk.exports.duplicate_in_clause/) | A name appears twice in one `exports` clause. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| [`bynk.exports.undeclared_capability`](/book/reference/diagnostics/bynk.exports.undeclared_capability/) | `exports capability` names a capability that is not declared. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| [`bynk.exports.undeclared_type`](/book/reference/diagnostics/bynk.exports.undeclared_type/) | `exports` names a type that is not declared. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |

## Generics

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.generics.ambiguous_instantiation`](/book/reference/diagnostics/bynk.generics.ambiguous_instantiation/) | A generic function with an `Ord` bound is used as a value, and its type arguments cannot be determined from the type expected at that reference; annotate the expected type (constrained-generics slice 1). | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.generics.bound_on_capability_op`](/book/reference/diagnostics/bynk.generics.bound_on_capability_op/) | A generic capability operation's type parameter carries a bound other than `Key` or `Codable`; operations take only those two (constrained-generics slices 2 and 3). | [`capability_op`](/book/reference/grammar/#rule-capability_op) | — |
| [`bynk.generics.codable_op_return_not_first_party`](/book/reference/diagnostics/bynk.generics.codable_op_return_not_first_party/) | A user-declared capability operation's `Codable` type parameter appears in its return type. A typed read, the call site decoding the provider's reply as the instantiated payload, is accepted only for the standard library's own operations (#3458, lifting ADR 0696 decision 5). | [`capability_op`](/book/reference/grammar/#rule-capability_op) | — |
| [`bynk.generics.duplicate_type_param`](/book/reference/diagnostics/bynk.generics.duplicate_type_param/) | A `type` or `fn` declares the same type-parameter name more than once (v0.157, ADR 0183). |  | — |
| [`bynk.generics.generic_non_record`](/book/reference/diagnostics/bynk.generics.generic_non_record/) | A `type` declaration carries type parameters on a refined, opaque or alias body; only a record (`type Name[T] = { … }`) or sum (`type Name[T] = | … | …`) body may be generic (v0.157/#593, ADRs 0183/0197). | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| [`bynk.generics.generic_sum_embeds`](/book/reference/diagnostics/bynk.generics.generic_sum_embeds/) | A generic sum type carries an `embeds` clause; embedding into a generic sum is not supported (#593). | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| [`bynk.generics.internal_op_annotation`](/book/reference/diagnostics/bynk.generics.internal_op_annotation/) | A capability operation carries an annotation outside the standard library, or a first-party operation's is not `@onUndecodable(miss)` or `@onUndecodable(fault)`: the reply policy of a typed read is internal to first-party operations (#3458). | [`capability_op`](/book/reference/grammar/#rule-capability_op) | — |
| [`bynk.generics.method_on_generic_type`](/book/reference/diagnostics/bynk.generics.method_on_generic_type/) | A *static* method on a generic type never mentions one of the type's parameters in its parameter or return types, so no call could determine it from the arguments or the expected type. Mention the parameter in the signature, or make the method a free function (constrained-generics slice 4; instance methods are supported, #594). | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.generics.missing_codable_bound`](/book/reference/diagnostics/bynk.generics.missing_codable_bound/) | A type parameter is encoded or decoded with `Json.encode`/`Json.decode`, or passed to a `Codable`-bounded parameter of another generic or of a capability operation, but is not declared `Codable` (`[T: Codable]`, or `where A: Codable` for a receiver type's parameter). The diagnostic carries a machine-applicable fix that declares `Codable`, beside an existing `Eq`, which `Codable` does not imply (constrained-generics slice 3). | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.generics.missing_eq_bound`](/book/reference/diagnostics/bynk.generics.missing_eq_bound/) | A type parameter is compared with `==`/`!=`, or passed to an `Eq`-bounded parameter of another generic, but is not declared `Eq` (`[T: Eq]`, or `where A: Eq` for a receiver type's parameter). The diagnostic carries a machine-applicable fix (ADR 0430, amended by constrained-generics slice 0). | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.generics.missing_key_bound`](/book/reference/diagnostics/bynk.generics.missing_key_bound/) | A type parameter keys a `Map` or `Set` (a `Map` key, a `Set` element, a `distinct`/`groupBy`/join key), or is passed to a `Key`-bounded parameter of another generic, but is not declared `Key` (`[K: Key]`, or `where A: Key` for a receiver type's parameter; on the type for a generic type's own parameter). The diagnostic carries a machine-applicable fix that declares `Key`, replacing an existing `Eq`, which `Key` implies (constrained-generics slice 2). | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.generics.missing_ord_bound`](/book/reference/diagnostics/bynk.generics.missing_ord_bound/) | A type parameter is ordered (`<`, `<=`, `>`, `>=`, an ordering kernel key), or passed to an `Ord`-bounded parameter of another generic, but is not declared `Ord` (`[T: Ord]`, or `where A: Ord` for a receiver type's parameter). The diagnostic carries a machine-applicable fix that declares `Ord`, replacing an existing `Eq`, which `Ord` implies (constrained-generics slice 1). | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.generics.polymorphic_recursion`](/book/reference/diagnostics/bynk.generics.polymorphic_recursion/) | A generic type refers to itself with arguments other than its own type parameters, in order (`Weird[List[T]]` inside `Weird[T]`, or `Pair[B, A]` inside `Pair[A, B]`), or to a mutually recursive partner with an argument that wraps one of its own parameters (`Inner[List[T]]` inside `Outer[T]`). Such polymorphic recursion generates an unbounded set of instantiations and is refused at the declaration; a self-reference that passes the parameters through unchanged, and a partner applied to the declaration's own parameters or to types that mention none (of any arity), are accepted (#3192). |  | — |
| [`bynk.generics.recursive_generic_at_boundary`](/book/reference/diagnostics/bynk.generics.recursive_generic_at_boundary/) | A recursive generic type (a record or sum that transitively contains itself, through any wrapper or generic argument) is used where a codec is required: a service or agent handler signature, an agent store field or key, an event field, an actor identity type, a `Json.encode`/`decode` target or a `Codable` witness. It declares and works inside a context, but has no codec yet, so it cannot cross a boundary; when it is reached through another type, the note names the field path (#3192, ADR 0197). |  | — |
| [`bynk.generics.redundant_eq_bound`](/book/reference/diagnostics/bynk.generics.redundant_eq_bound/) | A type parameter is declared both `Eq` and `Ord` (or `Eq` and `Key`); `Ord` and `Key` imply `Eq`, so the `Eq` is redundant. The diagnostic carries a machine-applicable fix that removes it (constrained-generics slices 1 and 2, warning). | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | Warning |
| [`bynk.generics.type_arg_count`](/book/reference/diagnostics/bynk.generics.type_arg_count/) | A user-declared generic type is applied to the wrong number of type arguments, or a generic type is named without its `[…]` arguments (v0.157, ADR 0183). | [`applied_type_ref`](/book/reference/grammar/#rule-applied_type_ref) | — |
| [`bynk.generics.type_arg_mismatch`](/book/reference/diagnostics/bynk.generics.type_arg_mismatch/) | Inferred or explicit type arguments conflict, have the wrong arity, target a non-generic function, or a type parameter shadows a declared type. | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.generics.uninferable_type_arg`](/book/reference/diagnostics/bynk.generics.uninferable_type_arg/) | A generic function's type parameter could not be inferred from the arguments and was not given explicitly (`name[T](…)`); a bare generic function also cannot be passed as a value. | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.generics.unknown_constraint`](/book/reference/diagnostics/bynk.generics.unknown_constraint/) | A bound, or a `derives` entry, names something that is not a constraint; the constraints are `Eq`, `Ord`, `Key` and `Codable`. | [`type_param`](/book/reference/grammar/#rule-type_param) | — |
| [`bynk.generics.unknown_where_param`](/book/reference/diagnostics/bynk.generics.unknown_where_param/) | A method's `where` clause bounds a name that is not a type parameter of the method or of its receiver type. | [`where_clause`](/book/reference/grammar/#rule-where_clause) | — |

## Given capabilities

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.given.cross_context_unknown_capability`](/book/reference/diagnostics/bynk.given.cross_context_unknown_capability/) | `given B.Cap` or `consumes B { Cap }` names a capability the consumed context does not export. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.given.fn_outside_context`](/book/reference/diagnostics/bynk.given.fn_outside_context/) | A function in a commons (or a suite) declares `given`; only a context function can. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.given.fn_unsupported`](/book/reference/diagnostics/bynk.given.fn_unsupported/) | A `given` clause on a method, on a function that does not return `Effect[T]`, or naming `Events`. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.given.missing_for_call`](/book/reference/diagnostics/bynk.given.missing_for_call/) | A call to a function that declares `given` from a body that does not declare those capabilities. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.given.undeclared_capability`](/book/reference/diagnostics/bynk.given.undeclared_capability/) | A handler uses a capability it did not declare with `given`. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.given.unknown_capability`](/book/reference/diagnostics/bynk.given.unknown_capability/) | `given` names a capability that does not exist. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.given.unused_capability`](/book/reference/diagnostics/bynk.given.unused_capability/) | A `given` capability is never used (warning). | [`given_clause`](/book/reference/grammar/#rule-given_clause) | Warning |

## HTTP

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.http.body_kind_conflict`](/book/reference/diagnostics/bynk.http.body_kind_conflict/) | A handler carries both `@raw` and `@form`, which each fix the wire format of its one `body`. |  | — |
| [`bynk.http.body_on_get_or_delete`](/book/reference/diagnostics/bynk.http.body_on_get_or_delete/) | A GET or DELETE handler declares a `body` parameter. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cache_bad_immutable`](/book/reference/diagnostics/bynk.http.cache_bad_immutable/) | A `@cache` annotation's `immutable` is not a `Bool` literal. |  | — |
| [`bynk.http.cache_bad_max_age`](/book/reference/diagnostics/bynk.http.cache_bad_max_age/) | A `@cache` annotation's `maxAge` is missing or not a positive `Duration` literal. |  | — |
| [`bynk.http.cache_bad_scope`](/book/reference/diagnostics/bynk.http.cache_bad_scope/) | A `@cache` annotation's `scope` is not `public` or `private`. |  | — |
| [`bynk.http.cache_duplicate`](/book/reference/diagnostics/bynk.http.cache_duplicate/) | A handler carries more than one `@cache` annotation. |  | — |
| [`bynk.http.cache_max_age_fractional_seconds`](/book/reference/diagnostics/bynk.http.cache_max_age_fractional_seconds/) | A `@cache` annotation's `maxAge` is positive but does not resolve to a whole number of seconds. |  | — |
| [`bynk.http.cache_on_non_get`](/book/reference/diagnostics/bynk.http.cache_on_non_get/) | A `@cache` annotation is placed on a handler that is not `on http GET`. |  | — |
| [`bynk.http.cache_public_reads_cookies`](/book/reference/diagnostics/bynk.http.cache_public_reads_cookies/) | A `@cache(scope: public)` handler takes a `cookies` parameter, so a shared cache could replay one user's response to another. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cache_unknown_arg`](/book/reference/diagnostics/bynk.http.cache_unknown_arg/) | A `@cache` annotation has an argument outside the closed set (`maxAge`/`scope`/`immutable`). |  | — |
| [`bynk.http.cf_wrong_type`](/book/reference/diagnostics/bynk.http.cf_wrong_type/) | An HTTP handler's `cf` parameter is not typed `RequestCf`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookie_cross_site_without_credentials`](/book/reference/diagnostics/bynk.http.cookie_cross_site_without_credentials/) | A service attaches a `SameSite.Unrestricted` cookie but declares no `cors { credentials: true }`, so a cross-origin `fetch` can never send it back (warning). | [`http_handler`](/book/reference/grammar/#rule-http_handler) | Warning |
| [`bynk.http.cookie_domain_widens`](/book/reference/diagnostics/bynk.http.cookie_domain_widens/) | A cookie literal outside the `__Host-` prefix sets a `domain`, so every subdomain receives it (warning). | [`http_handler`](/book/reference/grammar/#rule-http_handler) | Warning |
| [`bynk.http.cookie_field_arity`](/book/reference/diagnostics/bynk.http.cookie_field_arity/) | A typed cookie write or deletion (`Record.field.cookie(v)`, `Record.field.expire()`) is given the wrong number of arguments. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookie_field_list`](/book/reference/diagnostics/bynk.http.cookie_field_list/) | A typed cookie write names a `List` field of the cookie record. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookie_field_not_cookie_record`](/book/reference/diagnostics/bynk.http.cookie_field_not_cookie_record/) | A typed cookie write names a record that is not the `cookies` record of an `on http` handler in this unit. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookie_field_unknown`](/book/reference/diagnostics/bynk.http.cookie_field_unknown/) | A typed cookie write names a field the cookie record does not have. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookie_host_prefix`](/book/reference/diagnostics/bynk.http.cookie_host_prefix/) | A `__Host-` cookie literal is not `secure`, sets a `domain`, or uses a `path` other than `/`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookie_partitioned_insecure`](/book/reference/diagnostics/bynk.http.cookie_partitioned_insecure/) | A `partitioned` cookie literal is not `secure`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookie_samesite_none_insecure`](/book/reference/diagnostics/bynk.http.cookie_samesite_none_insecure/) | A cookie literal with `sameSite: SameSite.Unrestricted` is not `secure`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookie_script_readable`](/book/reference/diagnostics/bynk.http.cookie_script_readable/) | A cookie literal sets `httpOnly: false`, so page scripts can read it (warning). | [`http_handler`](/book/reference/grammar/#rule-http_handler) | Warning |
| [`bynk.http.cookie_secure_prefix`](/book/reference/diagnostics/bynk.http.cookie_secure_prefix/) | A `__Secure-` cookie literal is not `secure`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookie_untyped_has_field`](/book/reference/diagnostics/bynk.http.cookie_untyped_has_field/) | An untyped `Cookie` literal is named like a field of a cookie record of the unit (a warning). | [`http_handler`](/book/reference/grammar/#rule-http_handler) | Warning |
| [`bynk.http.cookie_value_undecodable`](/book/reference/diagnostics/bynk.http.cookie_value_undecodable/) | An untyped `Cookie` literal's value is one the record field that reads that cookie would reject. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookies_field_not_stringy`](/book/reference/diagnostics/bynk.http.cookies_field_not_stringy/) | A field of an HTTP handler's `cookies` record has a type a cookie cannot carry. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookies_not_http`](/book/reference/diagnostics/bynk.http.cookies_not_http/) | A `RequestCookies` appears on a handler that is not an HTTP route. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.cookies_wrong_type`](/book/reference/diagnostics/bynk.http.cookies_wrong_type/) | An HTTP handler's `cookies` parameter is not a record type. |  | — |
| [`bynk.http.cors_invalid_field`](/book/reference/diagnostics/bynk.http.cors_invalid_field/) | A `cors` policy field (`headers`/`credentials`/`maxAge`) has the wrong value shape. |  | — |
| [`bynk.http.cors_invalid_origins`](/book/reference/diagnostics/bynk.http.cors_invalid_origins/) | A `cors` policy's `origins` is missing, empty, or not a list of string literals. |  | — |
| [`bynk.http.cors_not_http`](/book/reference/diagnostics/bynk.http.cors_not_http/) | A `cors { }` policy appears on a service that is not `from http`. |  | — |
| [`bynk.http.cors_unknown_field`](/book/reference/diagnostics/bynk.http.cors_unknown_field/) | A `cors { }` policy declares a field outside the closed set. |  | — |
| [`bynk.http.cors_wildcard_credentials`](/book/reference/diagnostics/bynk.http.cors_wildcard_credentials/) | A `cors` policy combines `credentials: true` with the wildcard origin `["*"]`. |  | — |
| [`bynk.http.duplicate_rejected`](/book/reference/diagnostics/bynk.http.duplicate_rejected/) | A `from http` service declares `on rejected` more than once. | [`rejected_handler`](/book/reference/grammar/#rule-rejected_handler) | — |
| [`bynk.http.duplicate_route`](/book/reference/diagnostics/bynk.http.duplicate_route/) | Two handlers share the same method and route. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.error_status_position`](/book/reference/diagnostics/bynk.http.error_status_position/) | An `ErrorStatus` name is used anywhere but as `RawError`'s first argument. |  | — |
| [`bynk.http.extra_param`](/book/reference/diagnostics/bynk.http.extra_param/) | A handler parameter is not a path parameter, `body`, `headers` or `query`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.form_body_not_record`](/book/reference/diagnostics/bynk.http.form_body_not_record/) | The `body` of a `@form` handler is not a record type. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.form_bool_without_default`](/book/reference/diagnostics/bynk.http.form_bool_without_default/) | A `Bool` field of a `@form` body has no default, so an unchecked checkbox (which a browser sends as nothing) is a `400` (warning). | [`http_handler`](/book/reference/grammar/#rule-http_handler) | Warning |
| [`bynk.http.form_duplicate`](/book/reference/diagnostics/bynk.http.form_duplicate/) | A handler carries more than one `@form` annotation. |  | — |
| [`bynk.http.form_field_not_stringy`](/book/reference/diagnostics/bynk.http.form_field_not_stringy/) | A field of a `@form` body record has a type a form cannot carry. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.form_on_bodyless`](/book/reference/diagnostics/bynk.http.form_on_bodyless/) | A `@form` or `@multipart` annotation is placed on a handler that takes no body. |  | — |
| [`bynk.http.form_on_sum_route`](/book/reference/diagnostics/bynk.http.form_on_sum_route/) | A `@form` handler's `by` clause is a sum of actors, whose wrapper reads the body as JSON itself. |  | — |
| [`bynk.http.form_unknown_arg`](/book/reference/diagnostics/bynk.http.form_unknown_arg/) | A `@form` annotation is given an argument; it takes none. |  | — |
| [`bynk.http.header_question_not_found`](/book/reference/diagnostics/bynk.http.header_question_not_found/) | `?` on `RequestHeaders.get(…)` in an `HttpResult` handler answers `404 NotFound` for a missing header, where `400` is meant (warning; auto-fixable to a `let … else` that answers `BadRequest`). |  | Warning |
| [`bynk.http.headers_wrong_type`](/book/reference/diagnostics/bynk.http.headers_wrong_type/) | An HTTP handler's `headers` parameter is not typed `RequestHeaders`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.invalid_path`](/book/reference/diagnostics/bynk.http.invalid_path/) | An HTTP route path is malformed. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.json_duplicate`](/book/reference/diagnostics/bynk.http.json_duplicate/) | A handler carries more than one `@json` annotation. |  | — |
| [`bynk.http.json_invalid_arg`](/book/reference/diagnostics/bynk.http.json_invalid_arg/) | A `@json` annotation's `options` argument is missing or is not `tagged` or `plain`. |  | — |
| [`bynk.http.json_invalid_field`](/book/reference/diagnostics/bynk.http.json_invalid_field/) | A `json { }` policy's `options` is not `tagged` or `plain`. |  | — |
| [`bynk.http.json_not_http`](/book/reference/diagnostics/bynk.http.json_not_http/) | A `json { }` policy appears on a service that is not `from http`. |  | — |
| [`bynk.http.json_on_non_http`](/book/reference/diagnostics/bynk.http.json_on_non_http/) | A `@json` annotation is placed on a handler that is not `on http`. |  | — |
| [`bynk.http.json_unknown_arg`](/book/reference/diagnostics/bynk.http.json_unknown_arg/) | A `@json` annotation has an argument outside the closed set (`options`). |  | — |
| [`bynk.http.json_unknown_field`](/book/reference/diagnostics/bynk.http.json_unknown_field/) | A `json { }` policy declares a field outside the closed set (`options`). |  | — |
| [`bynk.http.limit_bad_max_body`](/book/reference/diagnostics/bynk.http.limit_bad_max_body/) | A `@limit` annotation's `maxBody` is missing or not a positive `Int` literal. |  | — |
| [`bynk.http.limit_bad_max_file`](/book/reference/diagnostics/bynk.http.limit_bad_max_file/) | A `@limit` `maxFile` or `maxParts` is not a positive `Int` literal. |  | — |
| [`bynk.http.limit_duplicate`](/book/reference/diagnostics/bynk.http.limit_duplicate/) | A handler carries more than one `@limit` annotation. |  | — |
| [`bynk.http.limit_on_bodyless`](/book/reference/diagnostics/bynk.http.limit_on_bodyless/) | A `@limit` annotation is placed on a handler that takes no body (a GET or DELETE). |  | — |
| [`bynk.http.limit_on_non_multipart`](/book/reference/diagnostics/bynk.http.limit_on_non_multipart/) | A `@limit` `maxFile` or `maxParts` is placed on a route that is not `@multipart`. |  | — |
| [`bynk.http.limit_unknown_arg`](/book/reference/diagnostics/bynk.http.limit_unknown_arg/) | A `@limit` annotation has an argument outside the closed set (`maxBody`). |  | — |
| [`bynk.http.limits_invalid_field`](/book/reference/diagnostics/bynk.http.limits_invalid_field/) | A `limits` policy field (`maxBody`) has the wrong value shape. |  | — |
| [`bynk.http.limits_not_http`](/book/reference/diagnostics/bynk.http.limits_not_http/) | A `limits { }` policy appears on a service that is not `from http`. |  | — |
| [`bynk.http.limits_unknown_field`](/book/reference/diagnostics/bynk.http.limits_unknown_field/) | A `limits { }` policy declares a field outside the closed set. |  | — |
| [`bynk.http.multipart_body_unbounded`](/book/reference/diagnostics/bynk.http.multipart_body_unbounded/) | A `@multipart` route has no request-body cap (`@limit` or `limits { maxBody }`). |  | — |
| [`bynk.http.multipart_large_cap`](/book/reference/diagnostics/bynk.http.multipart_large_cap/) | A `@multipart` route's body cap is over 32 MiB; the body is buffered with a parsed copy beside it, in a Worker's 128 MB (warning). |  | Warning |
| [`bynk.http.path_param_not_stringy`](/book/reference/diagnostics/bynk.http.path_param_not_stringy/) | A path parameter's type is not constructible from a string. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.query_field_not_stringy`](/book/reference/diagnostics/bynk.http.query_field_not_stringy/) | A field of an HTTP handler's `query` record has a type a query string cannot carry. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.query_not_http`](/book/reference/diagnostics/bynk.http.query_not_http/) | A `RequestQuery` appears on a handler that is not an HTTP route. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.query_wrong_type`](/book/reference/diagnostics/bynk.http.query_wrong_type/) | An HTTP handler's `query` parameter is neither a record type nor `RequestQuery`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.raw_bad_content_type`](/book/reference/diagnostics/bynk.http.raw_bad_content_type/) | A `@raw` annotation's `contentType` is missing or not a `type/subtype` (or `"*/*"`) string literal. |  | — |
| [`bynk.http.raw_body_not_bytes`](/book/reference/diagnostics/bynk.http.raw_body_not_bytes/) | The `body` of a `@raw` handler is not typed `Bytes` or `Stream[Bytes]`. |  | — |
| [`bynk.http.raw_body_unbounded`](/book/reference/diagnostics/bynk.http.raw_body_unbounded/) | A `@raw` route has no request-body cap (no `@limit` and no service `limits`). |  | — |
| [`bynk.http.raw_duplicate`](/book/reference/diagnostics/bynk.http.raw_duplicate/) | A handler carries more than one `@raw` annotation. |  | — |
| [`bynk.http.raw_error_status`](/book/reference/diagnostics/bynk.http.raw_error_status/) | `RawError`'s first argument is not an `ErrorStatus` name (a 4xx or 5xx `HttpResult` variant name). |  | — |
| [`bynk.http.raw_on_bodyless`](/book/reference/diagnostics/bynk.http.raw_on_bodyless/) | A `@raw` annotation is placed on a handler that takes no body. |  | — |
| [`bynk.http.raw_unknown_arg`](/book/reference/diagnostics/bynk.http.raw_unknown_arg/) | A `@raw` annotation has an argument outside the closed set (`contentType`). |  | — |
| [`bynk.http.rejected_signature`](/book/reference/diagnostics/bynk.http.rejected_signature/) | An `on rejected` handler is not `on rejected(why: BoundaryRejection) -> HttpResult[()]` with no `by` and no `given`. | [`rejected_handler`](/book/reference/grammar/#rule-rejected_handler) | — |
| [`bynk.http.reserved_prefix`](/book/reference/diagnostics/bynk.http.reserved_prefix/) | A route uses a reserved prefix: `/_bynk/` (internal dispatch) or `/_assets/` (static assets). | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.reserved_route_syntax`](/book/reference/diagnostics/bynk.http.reserved_route_syntax/) | A route segment contains `*`, `{` or `}`, which are reserved for a future route form. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.return_not_effect_http_result`](/book/reference/diagnostics/bynk.http.return_not_effect_http_result/) | An HTTP handler does not return `Effect[HttpResult[T]]`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.security_invalid_field`](/book/reference/diagnostics/bynk.http.security_invalid_field/) | A `security` policy field (`hsts`/`nosniff`) has the wrong value shape. |  | — |
| [`bynk.http.security_not_http`](/book/reference/diagnostics/bynk.http.security_not_http/) | A `security { }` policy appears on a service that is not `from http`. |  | — |
| [`bynk.http.security_unknown_field`](/book/reference/diagnostics/bynk.http.security_unknown_field/) | A `security { }` policy declares a field outside the closed set. |  | — |
| [`bynk.http.stream_body_not_raw`](/book/reference/diagnostics/bynk.http.stream_body_not_raw/) | A `Stream[Bytes]` `body` is on a handler that is not `@raw`. |  | — |
| [`bynk.http.stream_body_on_sum_route`](/book/reference/diagnostics/bynk.http.stream_body_on_sum_route/) | A route with a streamed `body` has a `by` clause that is a sum of actors, whose wrapper reads the body itself. |  | — |
| [`bynk.http.stream_body_signature`](/book/reference/diagnostics/bynk.http.stream_body_signature/) | A route with a streamed `body` has a `Signature` actor, which verifies an HMAC over the whole body first. |  | — |
| [`bynk.http.unbound_path_param`](/book/reference/diagnostics/bynk.http.unbound_path_param/) | A `:name` route segment has no matching handler parameter. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.unknown_handler_annotation`](/book/reference/diagnostics/bynk.http.unknown_handler_annotation/) | A handler carries an annotation outside the closed set (`@cache`/`@limit`). |  | — |

## History properties

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.history.not_an_agent`](/book/reference/diagnostics/bynk.history.not_an_agent/) | A `for all run: History[T]` names a `T` that is not an agent — only an agent has handlers to sequence and reachable states to observe (testing track slice 7, ADR 0155). |  | — |
| [`bynk.history.not_generable`](/book/reference/diagnostics/bynk.history.not_generable/) | A `for all run: History[Agent]` targets an agent with a handler parameter whose type cannot be generated (e.g. a `Matches` refinement), so its call-history cannot be driven (testing track slice 7, ADR 0155). |  | — |
| [`bynk.history.outside_property`](/book/reference/diagnostics/bynk.history.outside_property/) | `History[Agent]` appears outside a `property`'s `for all` binding — it is a test-only generator, not a value type (testing track slice 7, ADR 0155). |  | — |
| [`bynk.history.restates_invariant`](/book/reference/diagnostics/bynk.history.restates_invariant/) | A history property merely re-checks a guarantee a declared `invariant`/`transition` already enforces on every reached state (testing track slice 7, ADR 0155). |  | — |

## Internal errors

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.internal.missing_checker_fact`](/book/reference/diagnostics/bynk.internal.missing_checker_fact/) | A compiler fault, not a mistake in the program: the code generator needed a fact the checker should have recorded (a type, a callee, an argument count, a field default) and found none. The diagnostic points at the construct being compiled; report it at https://github.com/bynk-lang/bynk/issues. |  | — |

## Lexer

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.lex.bad_escape`](/book/reference/diagnostics/bynk.lex.bad_escape/) | An invalid escape sequence in a string literal. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| [`bynk.lex.bad_unicode_escape`](/book/reference/diagnostics/bynk.lex.bad_unicode_escape/) | A `\u{…}` escape names a surrogate or a value above U+10FFFF. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| [`bynk.lex.decimal_literal_too_large`](/book/reference/diagnostics/bynk.lex.decimal_literal_too_large/) | A decimal literal has more than 100 fraction digits or more than 1000 characters, the most any `Decimal` may have (#2622). | [`decimal_literal`](/book/reference/grammar/#rule-decimal_literal) | — |
| [`bynk.lex.float_literal_overflow`](/book/reference/diagnostics/bynk.lex.float_literal_overflow/) | A float literal does not fit a finite 64-bit float. | [`float_literal`](/book/reference/grammar/#rule-float_literal) | — |
| [`bynk.lex.integer_overflow`](/book/reference/diagnostics/bynk.lex.integer_overflow/) | An integer literal is outside the `Int` range, the JS safe integers ±(2^53 − 1). | [`number_literal`](/book/reference/grammar/#rule-number_literal) | — |
| [`bynk.lex.interpolation_too_deep`](/book/reference/diagnostics/bynk.lex.interpolation_too_deep/) | A string interpolation `\(…)` nests deeper than the lexer's fixed limit. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| [`bynk.lex.legacy_comment`](/book/reference/diagnostics/bynk.lex.legacy_comment/) | A `--` line comment, the spelling before #2203; a comment is `//` (auto-fixable; `bynk fmt` rewrites it). |  | — |
| [`bynk.lex.legacy_doc_line`](/book/reference/diagnostics/bynk.lex.legacy_doc_line/) | A `--|` one-line doc, the spelling before #2203; a one-line doc is `--- text` (auto-fixable; `bynk fmt` rewrites it). |  | — |
| [`bynk.lex.multiline_close_not_own_line`](/book/reference/diagnostics/bynk.lex.multiline_close_not_own_line/) | A `"""` inside a multi-line string literal is not on a line of its own. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| [`bynk.lex.multiline_in_hole`](/book/reference/diagnostics/bynk.lex.multiline_in_hole/) | A multi-line string literal appears inside an interpolation hole, which must close on its own line. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| [`bynk.lex.multiline_indent`](/book/reference/diagnostics/bynk.lex.multiline_indent/) | A line of a multi-line string literal is indented less than its closing `"""`. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| [`bynk.lex.multiline_open`](/book/reference/diagnostics/bynk.lex.multiline_open/) | The opening `"""` of a multi-line string literal is not followed by a newline, or by a language tag (`"""sql`) and a newline. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| [`bynk.lex.slash_doc`](/book/reference/diagnostics/bynk.lex.slash_doc/) | A `///` doc comment first on its line; a Bynk doc is `--- text` or a `---` block (auto-fixable). |  | — |
| [`bynk.lex.unclosed_doc_block`](/book/reference/diagnostics/bynk.lex.unclosed_doc_block/) | A documentation block is not closed. |  | — |
| [`bynk.lex.unexpected_character`](/book/reference/diagnostics/bynk.lex.unexpected_character/) | An unexpected character in the source. |  | — |
| [`bynk.lex.unterminated_interpolation`](/book/reference/diagnostics/bynk.lex.unterminated_interpolation/) | An interpolation hole `\(…)` is not closed on its line. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| [`bynk.lex.unterminated_string`](/book/reference/diagnostics/bynk.lex.unterminated_string/) | A string literal is not terminated. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |

## Message bundles

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.messages.format_mismatch`](/book/reference/diagnostics/bynk.messages.format_mismatch/) | A code's placeholder is formatted as a different ICU kind (plain/plural/select/number/date) across declared locales. |  | — |
| [`bynk.messages.incomplete`](/book/reference/diagnostics/bynk.messages.incomplete/) | A locale is missing a code the reference locale declares. |  | — |
| [`bynk.messages.invalid_locale_tag`](/book/reference/diagnostics/bynk.messages.invalid_locale_tag/) | A `messages` block's locale tag is not a valid `LocaleTag` (e.g. `messages "xx"` where `xx` doesn't match the tag pattern). |  | — |
| [`bynk.messages.malformed_icu_syntax`](/book/reference/diagnostics/bynk.messages.malformed_icu_syntax/) | A message template's ICU placeholder syntax is invalid — unbalanced arm braces, an unknown format keyword, `#` outside a plural arm, a missing mandatory `other` arm, or an explicitly out-of-scope construct (`selectordinal`, `offset:`/`=N`, a CLDR skeleton). |  | — |
| [`bynk.messages.missing_locale_dependency`](/book/reference/diagnostics/bynk.messages.missing_locale_dependency/) | A commons declaring `messages` doesn't `uses bynk.locale` and/or `uses bynk.locale.types`, which its generated `render` and the types its signature names need. |  | — |
| [`bynk.messages.missing_reference`](/book/reference/diagnostics/bynk.messages.missing_reference/) | A message bundle has no `@reference` block. |  | — |
| [`bynk.messages.multiple_reference`](/book/reference/diagnostics/bynk.messages.multiple_reference/) | A message bundle has more than one `@reference` block. |  | — |
| [`bynk.messages.outside_commons`](/book/reference/diagnostics/bynk.messages.outside_commons/) | A `messages` declaration appears outside a commons. |  | — |
| [`bynk.messages.placeholder_mismatch`](/book/reference/diagnostics/bynk.messages.placeholder_mismatch/) | A locale's template for a code uses a different set of `{name}` placeholders than the reference locale's. |  | — |

## Observation

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.observe.impure_with`](/book/reference/diagnostics/bynk.observe.impure_with/) | A `with` predicate uses an effectful or test-only construct, or an effectful call (a sub-expression of type `Effect[_]`); it must be pure. |  | — |
| [`bynk.observe.not_a_seam`](/book/reference/diagnostics/bynk.observe.not_a_seam/) | An observation targets a capability the unit under test does not consume. |  | — |
| [`bynk.observe.trace_outside_test`](/book/reference/diagnostics/bynk.observe.trace_outside_test/) | `trace(Cap.op)` appears outside a `case` body. |  | — |
| [`bynk.observe.unknown_op`](/book/reference/diagnostics/bynk.observe.unknown_op/) | An observation names an operation the capability does not declare. |  | — |
| [`bynk.observe.with_not_bool`](/book/reference/diagnostics/bynk.observe.with_not_bool/) | A `with` predicate does not have type `Bool`. |  | — |

## Other

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.cell.invalid_target`](/book/reference/diagnostics/bynk.cell.invalid_target/) | A `:=` write targets something that is not a `store Cell` field. |  | — |
| [`bynk.cell.self_reference`](/book/reference/diagnostics/bynk.cell.self_reference/) | A `:=` right-hand side reads the cell being written (a read-modify-write); use `.update`. |  | — |
| [`bynk.codegen.tsc_failed`](/book/reference/diagnostics/bynk.codegen.tsc_failed/) | `tsc` rejected the TypeScript the compiler generated for a program that passed the checker. This is a bug in the Bynk compiler, not in the program: the diagnostic points at the Bynk code the failing generated line came from, with the TypeScript error attached (#3068). |  | — |
| [`bynk.container.build_missing`](/book/reference/diagnostics/bynk.container.build_missing/) | A `Container`'s `@build` directory holds no `Dockerfile`, so there is no image to build. |  | — |
| [`bynk.container.build_path`](/book/reference/diagnostics/bynk.container.build_path/) | A `Container`'s `@build` is not a directory inside the project: it is empty, absolute, uses `\`, or leaves the project with `..`. |  | — |
| [`bynk.container.output_outside_project`](/book/reference/diagnostics/bynk.container.output_outside_project/) | A build with a `Container` is written to an output directory outside the project, so `wrangler` cannot reach the container's Dockerfile from the Worker's output directory by a relative path inside the project. |  | — |
| [`bynk.d1.migration_invalid`](/book/reference/diagnostics/bynk.d1.migration_invalid/) | A `D1` migration does not apply to an empty in-memory SQLite, so the binding's literal SQL is not checked against its schema (warning). |  | Warning |
| [`bynk.d1.migration_name`](/book/reference/diagnostics/bynk.d1.migration_name/) | A file in the project's `migrations/` folder isn't `migrations/<database>/NNNN_<name>.sql`, or shares its number with another migration of the same database. |  | — |
| [`bynk.d1.migration_unreadable`](/book/reference/diagnostics/bynk.d1.migration_unreadable/) | A `D1` migration in the project's `migrations/` folder can't be read, or isn't UTF-8 text. |  | — |
| [`bynk.d1.migrations_unused`](/book/reference/diagnostics/bynk.d1.migrations_unused/) | A directory in the project's `migrations/` folder matches no `D1` instance's database name, so its migrations are never applied, usually because the capability was renamed (warning). |  | Warning |
| [`bynk.d1.select_field_missing`](/book/reference/diagnostics/bynk.d1.select_field_missing/) | A record read from a literal D1 statement has a field with no column of that name. For a `select[T]` / `firstAs[T]` row type, the column list the compiler writes from `T` does not prepare against the schema the binding's migrations build; for a `Json.decodeSql[T]` of the rows of a literal `all` / `first`, a required (non-`Option`) field of `T` has no result column. |  | — |
| [`bynk.d1.sql_invalid`](/book/reference/diagnostics/bynk.d1.sql_invalid/) | A literal D1 statement does not prepare against the schema the binding's migrations build: a syntax error, or an unknown table or column. |  | — |
| [`bynk.d1.sql_param_count`](/book/reference/diagnostics/bynk.d1.sql_param_count/) | A literal D1 statement uses a different number of bound parameters (`?N`) than the list literal passed with it holds. |  | — |
| [`bynk.dependency.path_outside_root`](/book/reference/diagnostics/bynk.dependency.path_outside_root/) | A path dependency declared by a dependency resolves outside the workspace; a dependency's own path dependencies are honoured only between members of one workspace. |  | — |
| [`bynk.detach.in_agent_handler`](/book/reference/diagnostics/bynk.detach.in_agent_handler/) | A `detach` was used in an agent handler, which has no execution context to keep a detached effect alive. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.detach.in_pure_context`](/book/reference/diagnostics/bynk.detach.in_pure_context/) | A `detach` was used in a pure (non-effectful) context. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.detach.store_op`](/book/reference/diagnostics/bynk.detach.store_op/) | A `detach` targets an agent store operation, which is part of the handler's atomic commit and cannot be detached. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.duration.literal_overflow`](/book/reference/diagnostics/bynk.duration.literal_overflow/) | A `Duration` literal (`<int>.<unit>`) exceeds the representable millisecond range. |  | — |
| [`bynk.email.bad_params`](/book/reference/diagnostics/bynk.email.bad_params/) | An email `on message` handler does not take exactly one `InboundEmail` parameter. |  | — |
| [`bynk.email.duplicate_address`](/book/reference/diagnostics/bynk.email.duplicate_address/) | Two `from email` services claim the same recipient address. |  | — |
| [`bynk.email.duplicate_handler`](/book/reference/diagnostics/bynk.email.duplicate_handler/) | A `from email` service declares more than one `on message` handler; it has exactly one. |  | — |
| [`bynk.email.invalid_address`](/book/reference/diagnostics/bynk.email.invalid_address/) | A `from email("…")` binding is not a recipient address a service can claim. |  | — |
| [`bynk.email.return_not_email_verdict`](/book/reference/diagnostics/bynk.email.return_not_email_verdict/) | An email `on message` handler does not return `Effect[EmailVerdict]`. |  | — |
| [`bynk.exec.unavailable`](/book/reference/diagnostics/bynk.exec.unavailable/) | A call to, or reference to, a function that hands work to the execution context (`detach`, `within` or `race`, directly or through the functions it calls) appears where no execution context exists: an agent handler, a WebSocket handler or a provider operation. |  | — |
| [`bynk.fault.todo`](/book/reference/diagnostics/bynk.fault.todo/) | A `todo(message)` placeholder: the code type-checks but faults when it runs. A non-failing warning, so `check --deny-warnings` refuses unfinished code in CI (warning). |  | Warning |
| [`bynk.fixture.binary_content`](/book/reference/diagnostics/bynk.fixture.binary_content/) | A suite's `fixture` names a file that contains a NUL byte, so it is binary, not text. | [`fixture_decl`](/book/reference/grammar/#rule-fixture_decl) | — |
| [`bynk.fixture.duplicate`](/book/reference/diagnostics/bynk.fixture.duplicate/) | A suite declares two `fixture`s of the same name. | [`fixture_decl`](/book/reference/grammar/#rule-fixture_decl) | — |
| [`bynk.fixture.invalid_encoding`](/book/reference/diagnostics/bynk.fixture.invalid_encoding/) | A suite's `fixture` names a file that is not valid UTF-8. | [`fixture_decl`](/book/reference/grammar/#rule-fixture_decl) | — |
| [`bynk.fixture.missing_file`](/book/reference/diagnostics/bynk.fixture.missing_file/) | A suite's `fixture` names a file that is not in the project's `fixtures/` folder. | [`fixture_decl`](/book/reference/grammar/#rule-fixture_decl) | — |
| [`bynk.fixture.outside_fixtures_dir`](/book/reference/diagnostics/bynk.fixture.outside_fixtures_dir/) | A suite's `fixture` names a file outside the project's `fixtures/` folder. | [`fixture_decl`](/book/reference/grammar/#rule-fixture_decl) | — |
| [`bynk.fixture.too_large`](/book/reference/diagnostics/bynk.fixture.too_large/) | A suite's `fixture` names a file over the 4 MiB limit for one fixture file. | [`fixture_decl`](/book/reference/grammar/#rule-fixture_decl) | — |
| [`bynk.fixture.unreadable_file`](/book/reference/diagnostics/bynk.fixture.unreadable_file/) | A suite's `fixture` names a listed file that could not be read. | [`fixture_decl`](/book/reference/grammar/#rule-fixture_decl) | — |
| [`bynk.fixture.unsafe_path`](/book/reference/diagnostics/bynk.fixture.unsafe_path/) | A suite's `fixture` path is empty, absolute or contains a `..` segment. | [`fixture_decl`](/book/reference/grammar/#rule-fixture_decl) | — |
| [`bynk.fixture.unused`](/book/reference/diagnostics/bynk.fixture.unused/) | A suite declares a `fixture` that none of its cases, properties or helpers uses (warning). | [`fixture_decl`](/book/reference/grammar/#rule-fixture_decl) | Warning |
| [`bynk.fmt.comment_loss`](/book/reference/diagnostics/bynk.fmt.comment_loss/) | The formatter would drop a comment, so it leaves the file unchanged. |  | — |
| [`bynk.fmt.mixed_comment_syntax`](/book/reference/diagnostics/bynk.fmt.mixed_comment_syntax/) | A file mixes the retired `--` comment or `--|` doc spellings with a line that reads differently under the old and new comment rules (#2203), so the formatter leaves it unchanged. |  | — |
| [`bynk.fmt.roundtrip`](/book/reference/diagnostics/bynk.fmt.roundtrip/) | The formatter's output does not re-parse to the same program, so it leaves the file unchanged. |  | — |
| [`bynk.held.branch_divergence`](/book/reference/diagnostics/bynk.held.branch_divergence/) | Branches of a conditional leave a held value (e.g. `Connection[F]`) in inconsistent ownership states — one consumes or stores it, another leaves it owned (§2.9.5, real-time track slice 2). |  | — |
| [`bynk.held.consume_on_borrow`](/book/reference/diagnostics/bynk.held.consume_on_borrow/) | A consuming operation (`close`/`put`/`take`) is called on a *borrowed* held reference — borrows admit only non-consuming operations like `send` (§2.9.3, real-time track slice 2). |  | — |
| [`bynk.held.leak`](/book/reference/diagnostics/bynk.held.leak/) | A held value (`Connection[F]`) is still owned at scope exit — it must be disposed (stored, closed, or transferred) before the handler or function returns (§2.9.1, real-time track slice 2). |  | — |
| [`bynk.held.query_accessor_on_held_map`](/book/reference/diagnostics/bynk.held.query_accessor_on_held_map/) | A key-aware query accessor (`.entries`/`.keys`/`.values`) is used on a held `Map[K, Connection]` — a held resource is iterated with the broadcast ops (`forEach`/`parTraverse`), not a key query. |  | — |
| [`bynk.held.unsupported_map_op`](/book/reference/diagnostics/bynk.held.unsupported_map_op/) | A held `Map[K, Connection]` is given an `update`/`upsert` — a held resource cannot be transformed by a `(Connection) -> Connection` function; use `put`/`get`/`remove` (real-time track slice 3b-ii). |  | — |
| [`bynk.held.unsupported_storage`](/book/reference/diagnostics/bynk.held.unsupported_storage/) | A held value (`Connection[F]`) is stored in a `Set`/`Log`/`Cache` — held values may only live in `Cell[Option[Connection]]` or `Map[K, Connection]` (§2.9.3, real-time track slice 2). |  | — |
| [`bynk.held.use_after_consume`](/book/reference/diagnostics/bynk.held.use_after_consume/) | A held value (`Connection[F]`) is used after a consuming operation (`close`/`put`/`take`) ended its lifetime (§2.9.2, real-time track slice 2). |  | — |
| [`bynk.index.bad_argument`](/book/reference/diagnostics/bynk.index.bad_argument/) | An `@indexed` argument is not a `by: <field>` label. |  | — |
| [`bynk.index.missing`](/book/reference/diagnostics/bynk.index.missing/) | A query filters a map by equality on a field that is not `@indexed` (a perf-hint warning). |  | Warning |
| [`bynk.index.unkeyable_key`](/book/reference/diagnostics/bynk.index.unkeyable_key/) | An `@indexed(by: k)` field is not value-keyable. |  | — |
| [`bynk.index.unknown_key`](/book/reference/diagnostics/bynk.index.unknown_key/) | An `@indexed(by: k)` field is not a field of the map's value type. |  | — |
| [`bynk.index.unused`](/book/reference/diagnostics/bynk.index.unused/) | A declared `@indexed(by: k)` is never used by an equality filter (a hygiene warning). |  | Warning |
| [`bynk.invariant.cross_agent_reference`](/book/reference/diagnostics/bynk.invariant.cross_agent_reference/) | An invariant predicate references another agent; invariants are per-agent. |  | — |
| [`bynk.invariant.duplicate_name`](/book/reference/diagnostics/bynk.invariant.duplicate_name/) | An agent declares two invariants with the same name. |  | — |
| [`bynk.invariant.impure_predicate`](/book/reference/diagnostics/bynk.invariant.impure_predicate/) | An invariant predicate uses an effectful or test-only construct. |  | — |
| [`bynk.invariant.not_bool`](/book/reference/diagnostics/bynk.invariant.not_bool/) | An invariant predicate does not have type `Bool`. |  | — |
| [`bynk.json.encode_plain_generic`](/book/reference/diagnostics/bynk.json.encode_plain_generic/) | `Json.encodePlain` is given a value whose type mentions a type parameter. A generic value encodes through its `Codable` witness, which always writes `Option` in the tagged form, so plain encoding is available only at a concrete type. |  | — |
| [`bynk.json.nested_option_plain`](/book/reference/diagnostics/bynk.json.nested_option_plain/) | A plain-JSON route's response type, or the argument of `Json.encodePlain`, contains an `Option[U]` whose payload `U` can itself encode as `null` (`Option[T]` or `()`), so `Some(U)` and `None` would both be written `null`. |  | — |
| [`bynk.lambda.unannotated_param`](/book/reference/diagnostics/bynk.lambda.unannotated_param/) | A lambda parameter has no type annotation in a position where no function type is expected to infer it from. | [`lambda_expr`](/book/reference/grammar/#rule-lambda_expr) | — |
| [`bynk.list.deprecated_function`](/book/reference/diagnostics/bynk.list.deprecated_function/) | A `bynk.list` free function (`map`/`filter`/`find`/`any`/`all`) is deprecated in favour of the `List` method form (warning; auto-fixable). |  | Warning |
| [`bynk.locale.multiple_message_bundles`](/book/reference/diagnostics/bynk.locale.multiple_message_bundles/) | A context consumes `Locale` but its direct `uses` reaches two or more message-bundle commons — there is no single bundle to negotiate against. |  | — |
| [`bynk.mysql.sql_invalid`](/book/reference/diagnostics/bynk.mysql.sql_invalid/) | A literal `MySql` statement does not parse with the MySQL dialect of a pure-Rust SQL parser, or is more than one command, in a project that has a `schema.sql` (warning: the parser is not MySQL's own, so it may refuse valid SQL). |  | Warning |
| [`bynk.mysql.sql_param_count`](/book/reference/diagnostics/bynk.mysql.sql_param_count/) | A literal `MySql` statement has a different number of `?` placeholders than the list literal passed with it holds, in a project that has a `schema.sql`. |  | — |
| [`bynk.par.invalid_statement`](/book/reference/diagnostics/bynk.par.invalid_statement/) | A `par { … }` block holds a statement other than `let x <- e` or `do e`: a refutable `let … else`, a plain `let`, `:=` or a store write, `expect`, `advance`, a nested `par`, or anything that is not a statement. | [`par_stmt`](/book/reference/grammar/#rule-par_stmt) | — |
| [`bynk.par.question_in_join`](/book/reference/diagnostics/bynk.par.question_in_join/) | A `?` appears inside a `par { … }` binding. `?` takes a `Result` value, not an effect, so a binding's own result is unwrapped after the block (`let user = userR?`), where the `?` lines run in source order; a `?` on an argument is computed before the block. | [`par_stmt`](/book/reference/grammar/#rule-par_stmt) | — |
| [`bynk.par.sibling_name`](/book/reference/diagnostics/bynk.par.sibling_name/) | A `par { … }` binding mentions a name another binding in the same block binds, or binds a name twice. The bindings run together, so none can read a sibling's result; the rule holds even where the name would resolve to an outer binding. | [`par_stmt`](/book/reference/grammar/#rule-par_stmt) | — |
| [`bynk.par.single_binding`](/book/reference/diagnostics/bynk.par.single_binding/) | A `par { … }` block has a single binding, which is a plain `let x <- e` with extra syntax (warning). | [`par_stmt`](/book/reference/grammar/#rule-par_stmt) | Warning |
| [`bynk.par.store_write_in_closure`](/book/reference/diagnostics/bynk.par.store_write_in_closure/) | A closure passed to `parTraverse`, `parTraverseAll` or `parTraverseTry` writes a `store` field (`put`, `remove`, `update`, `upsert`, `add`, `append` or `:=`). Its branches run concurrently, and a `get` then a `put` across them loses updates, so store writes stay sequential, as they do in a `par { … }` block. |  | — |
| [`bynk.postgres.sql_invalid`](/book/reference/diagnostics/bynk.postgres.sql_invalid/) | A literal `Postgres` statement does not parse with the Postgres dialect of a pure-Rust SQL parser, or is more than one command, in a project that has a `schema.sql` (warning: the parser is not Postgres' own, so it may refuse valid SQL). |  | Warning |
| [`bynk.postgres.sql_param_count`](/book/reference/diagnostics/bynk.postgres.sql_param_count/) | A literal `Postgres` statement uses a different number of parameters (its highest `$n`) than the list literal passed with it holds, in a project that has a `schema.sql`. |  | — |
| [`bynk.query.join_key_mismatch`](/book/reference/diagnostics/bynk.query.join_key_mismatch/) | A `joinOn`/`leftJoin` left and right key function return different types. |  | — |
| [`bynk.query.sum_needs_numeric`](/book/reference/diagnostics/bynk.query.sum_needs_numeric/) | A `sum`/`average` key function does not return a numeric type (`Int`, `Float`, or `Duration`; `sum` also takes a `Decimal`, `average` does not). |  | — |
| [`bynk.race.in_agent_handler`](/book/reference/diagnostics/bynk.race.in_agent_handler/) | `a.race(b)` was used in an agent handler, which has no execution context to hand the losing effect to. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.race.in_pure_context`](/book/reference/diagnostics/bynk.race.in_pure_context/) | `a.race(b)` was used in a pure (non-effectful) context. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.race.outside_handler`](/book/reference/diagnostics/bynk.race.outside_handler/) | `a.race(b)` was used outside a service handler's body and outside a function (in a provider operation, a WebSocket handler or a test `case` body), where no execution context exists. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.registry.account_refused`](/book/reference/diagnostics/bynk.registry.account_refused/) | `bynk login`, `bynk logout` or a `bynk org` command didn't go through: you aren't signed in or your session expired, the registry has no account API (or is read through a mirror), the sign-in was declined or its code expired, or the registry refused the request (a taken or reserved name, not the owner, an unknown member or token). |  | — |
| [`bynk.registry.bad_archive`](/book/reference/diagnostics/bynk.registry.bad_archive/) | A registry archive is malformed or hostile: not a `bynk-tree-v1` stream, a path that is absolute or climbs out of the package, an out-of-order or repeated file, or past the size limits. |  | — |
| [`bynk.registry.bad_signature`](/book/reference/diagnostics/bynk.registry.bad_signature/) | An index line isn't signed by a key the trusted key's rotation chain reaches, or its signed registry `id` or package `name` isn't the one asked for (a forged or replayed line). |  | — |
| [`bynk.registry.http_failed`](/book/reference/diagnostics/bynk.registry.http_failed/) | A registry couldn't be read: unreachable, an error status, a refused or missing token, a redirect from https to http, too many redirects, or a response past the size limit. |  | — |
| [`bynk.registry.key_mismatch`](/book/reference/diagnostics/bynk.registry.key_mismatch/) | A registry's keys don't lead from the key trusted for it (pinned in `[registries]`, or recorded in `bynk.lock`), or it now declares a different `id`. |  | — |
| [`bynk.registry.nonconformant`](/book/reference/diagnostics/bynk.registry.nonconformant/) | `bynk registry conformance` found a registry breaking the protocol: a check failed (a wrong status, an unsigned or mis-signed line, a checksum that isn't the tree sent, a refusal without its JSON body, …); each failure names the check and the protocol section it holds the registry to. |  | — |
| [`bynk.registry.publish_refused`](/book/reference/diagnostics/bynk.registry.publish_refused/) | `bynk publish` or `bynk yank` didn't go through: the project isn't a versioned package, it has a path or git dependency, its registry has no publish API (or is read through a mirror), or the registry refused the request (no or wrong token, an existing version, an invalid upload). |  | — |
| [`bynk.registry.source_mismatch`](/book/reference/diagnostics/bynk.registry.source_mismatch/) | `bynk.lock` locks a package to one registry, but its organisation now routes to another; a misrouted organisation fails rather than resolving a same-named package elsewhere. |  | — |
| [`bynk.registry.unavailable`](/book/reference/diagnostics/bynk.registry.unavailable/) | No registry serves a package: its organisation isn't listed by any `[registries]` entry (the first-party registry isn't live yet), the registry has no `config.json`, or it has no such package or archive. |  | — |
| [`bynk.registry.withdrawn`](/book/reference/diagnostics/bynk.registry.withdrawn/) | `bynk.lock` locks a registry version that its registry withdrew (a takedown: yanked, and its archive deleted), and its tree isn't in the store. |  | — |
| [`bynk.revert.agent_schema_older`](/book/reference/diagnostics/bynk.revert.agent_schema_older/) | `bynk revert` refused because the target writes an older agent schema version than the live one while stored instances exist, are not known to be absent, or the target is below the class's floor, so the older code would fault on newer records. |  | — |
| [`bynk.revert.class_changed`](/book/reference/diagnostics/bynk.revert.class_changed/) | `bynk revert` refused because a Durable Object class was added, removed or renamed between the live version and the target, and Cloudflare cannot roll back across a class migration. |  | — |
| [`bynk.revert.contract_skew`](/book/reference/diagnostics/bynk.revert.contract_skew/) | `bynk revert` refused because the target's contracts would no longer match another live Worker, judged as if every selected Worker were reverted together. |  | — |
| [`bynk.revert.database_schema_ahead`](/book/reference/diagnostics/bynk.revert.database_schema_ahead/) | `bynk revert` refused because the live version applied D1 migrations the target was not built against, or the applied migrations could not be read; migrations only move forward, so the older code may not run on the newer schema. |  | — |
| [`bynk.revert.facts_missing`](/book/reference/diagnostics/bynk.revert.facts_missing/) | `bynk revert` refused because the live version or the target version carries no deploy facts (`BYNK_DEPLOY_FACTS`), or the facts could not be read, so nothing can be checked against them. |  | — |
| [`bynk.revert.history_unplaceable`](/book/reference/diagnostics/bynk.revert.history_unplaceable/) | `bynk revert` refused for one Worker because its deployment history cannot be placed: it has no earlier deployment, the live upload is outside the listed history, a gradual deployment is in progress, or the history could not be read. |  | — |
| [`bynk.revert.id_unrecorded`](/book/reference/diagnostics/bynk.revert.id_unrecorded/) | `bynk revert` refused because the target binds a KV namespace, database or limiter that the deploy lock no longer records, usually because the resource was re-provisioned since. |  | — |
| [`bynk.revert.layout_older`](/book/reference/diagnostics/bynk.revert.layout_older/) | `bynk revert` refused because the target writes an older agent storage layout than the live one while stored instances exist or are not known to be absent, so the older code could not read the newer records. |  | — |
| [`bynk.revert.membership_changed`](/book/reference/diagnostics/bynk.revert.membership_changed/) | `bynk revert` refused because a `[deploy.workers.<key>]` group's target version held other contexts than the live one (a context moved into or out of the group since, or the version predates groups), so a rollback would take a member's code out of the Worker. |  | — |
| [`bynk.revert.secret_changed`](/book/reference/diagnostics/bynk.revert.secret_changed/) | `bynk revert` refused because a secret changed after the target was live, so the rollback could restore an old secret value that its peers no longer accept. |  | — |
| [`bynk.secrets.computed_name`](/book/reference/diagnostics/bynk.secrets.computed_name/) | A `bynk.Secrets` read names its secret with a computed expression rather than a literal, so `bynk deploy` cannot plan it (warning). |  | Warning |
| [`bynk.target.browser_bundle_only`](/book/reference/diagnostics/bynk.target.browser_bundle_only/) | The `browser` platform builds only the in-process `Bundle` topology; `--target workers` is not a browser build. |  | — |
| [`bynk.target.vendor_conflict`](/book/reference/diagnostics/bynk.target.vendor_conflict/) | One deployment unit's in-process closure uses platform-native capabilities from two mutually-exclusive platforms. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.target.vendor_required`](/book/reference/diagnostics/bynk.target.vendor_required/) | A deployment unit uses a platform-native capability but the build selects another `--platform`. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.visibility.private_in_public`](/book/reference/diagnostics/bynk.visibility.private_in_public/) | A public declaration's signature names a `private` type of its own unit, which an importer could neither name nor build. | [`visibility_modifier`](/book/reference/grammar/#rule-visibility_modifier) | — |
| [`bynk.visibility.private_item`](/book/reference/diagnostics/bynk.visibility.private_item/) | A reference from another unit names a `private` declaration, which is visible only within its own unit and that unit's suites. | [`visibility_modifier`](/book/reference/grammar/#rule-visibility_modifier) | — |
| [`bynk.visibility.private_messages`](/book/reference/diagnostics/bynk.visibility.private_messages/) | A `messages` block in a commons is `private`. A bundle whose `@reference` block is private is never negotiated by a context that uses the commons, and `private` on another block of a bundle whose reference is public has no effect (warning). | [`visibility_modifier`](/book/reference/grammar/#rule-visibility_modifier) | Warning |
| [`bynk.visibility.private_not_allowed`](/book/reference/diagnostics/bynk.visibility.private_not_allowed/) | `private` modifies a declaration it does not apply to: anything but a `fn`, `type`, `messages`, `asset`, `embed` or `const` in a commons or context. | [`visibility_modifier`](/book/reference/grammar/#rule-visibility_modifier) | — |
| [`bynk.wasm.panic`](/book/reference/diagnostics/bynk.wasm.panic/) | The in-browser compiler panicked internally (a compiler bug, reported as a diagnostic instead of crashing the page). |  | — |
| [`bynk.wasm.strip_failed`](/book/reference/diagnostics/bynk.wasm.strip_failed/) | The in-browser compiler could not strip the emitted TypeScript to JavaScript. |  | — |
| [`bynk.within.in_agent_handler`](/book/reference/diagnostics/bynk.within.in_agent_handler/) | `e.within(d)` was used in an agent handler, which has no execution context to hand the abandoned effect to. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.within.in_pure_context`](/book/reference/diagnostics/bynk.within.in_pure_context/) | `e.within(d)` was used in a pure (non-effectful) context. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.within.outside_handler`](/book/reference/diagnostics/bynk.within.outside_handler/) | `e.within(d)` was used outside a service handler's body and outside a function (in a provider operation, a WebSocket handler or a test `case` body), where no execution context exists. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.within.timed_out_not_in_scope`](/book/reference/diagnostics/bynk.within.timed_out_not_in_scope/) | `e.within(d)` yields `Result[_, TimedOut]`, but the unit does not consume `bynk`, so `TimedOut` is not in scope. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.ws.lifecycle_by_mismatch`](/book/reference/diagnostics/bynk.ws.lifecycle_by_mismatch/) | A WebSocket `on message`/`on close` handler's `by` clause does not name the same actor(s) as `on open`'s — the connection's principal is verified once, at `on open`, and carried for its life, so the lifecycle handlers must be the same single actor or the same actor-sum members (binder names may differ). |  | — |
| [`bynk.ws.message_frame_param`](/book/reference/diagnostics/bynk.ws.message_frame_param/) | A WebSocket `on message` handler does not have exactly one parameter of the service's inbound (`in:`) frame type — the decoded frame (real-time track slice 3b-iii). |  | — |
| [`bynk.ws.open_given_unsupported`](/book/reference/diagnostics/bynk.ws.open_given_unsupported/) | A WebSocket `on open` handler declares `given` capabilities — unsupported at v1, since on Workers the handler runs inside the connection-hosting Durable Object, which has no composition root to supply them (real-time track slice 3b). |  | — |
| [`bynk.ws.open_transfer_cross_file`](/book/reference/diagnostics/bynk.ws.open_transfer_cross_file/) | On the Workers target, a WebSocket `on open` handler transfers its `connection` to an agent declared in a different file of the same context — the open is hosted in the agent's Durable Object, which is emitted from the agent's own file, so the upgrade would never be handled; the service and the agent must share a file. |  | — |
| [`bynk.ws.open_transfer_key`](/book/reference/diagnostics/bynk.ws.open_transfer_key/) | On the Workers target, the key of a WebSocket `on open` handler's connection transfer is not a route parameter, a constant of the context or of a commons it uses, or a literal — the upgrade is routed to the hosting Durable Object before the body runs, so the edge must be able to evaluate the key on its own. |  | — |
| [`bynk.ws.open_transfer_shape`](/book/reference/diagnostics/bynk.ws.open_transfer_shape/) | A WebSocket `on open` handler does not transfer its `connection` into exactly one agent, so the Workers upgrade has no single Durable Object to route to (real-time track slice 3b). |  | — |
| [`bynk.ws.route_param_mismatch`](/book/reference/diagnostics/bynk.ws.route_param_mismatch/) | A WebSocket `on message`/`on close` route parameter does not match the `on open` parameter at the same position — route values are recovered positionally from the connection, so they must be a type-compatible prefix of the `on open` parameters (real-time track slice 3b-iii). |  | — |

## Packages

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.package.alias_shadows_unit`](/book/reference/diagnostics/bynk.package.alias_shadows_unit/) | A dependency alias in `[dependencies]` is also the first segment of one of the project's own units, so `<alias>.…` would be ambiguous. |  | — |
| [`bynk.package.checksum_mismatch`](/book/reference/diagnostics/bynk.package.checksum_mismatch/) | A git dependency's store entry, or a freshly fetched commit, doesn't hash to the checksum `bynk.lock` records; `bynk` refuses to build it. |  | — |
| [`bynk.package.dependency_cycle`](/book/reference/diagnostics/bynk.package.dependency_cycle/) | The package graph has a cycle: a dependency depends, directly or not, on itself or on the project. |  | — |
| [`bynk.package.dependency_not_found`](/book/reference/diagnostics/bynk.package.dependency_not_found/) | A path dependency's directory has no readable `bynk.toml`, its `bynk.toml` is invalid, or it has no `[package]`. |  | — |
| [`bynk.package.dev_dependency_in_source`](/book/reference/diagnostics/bynk.package.dev_dependency_in_source/) | A commons, context or adapter reaches a `[dev-dependencies]` alias, which only a suite may use. |  | — |
| [`bynk.package.duplicate_source`](/book/reference/diagnostics/bynk.package.duplicate_source/) | One package is reached from two different directories; a build holds one copy of each package. |  | — |
| [`bynk.package.git_failed`](/book/reference/diagnostics/bynk.package.git_failed/) | `git` couldn't clone, fetch or check out a git dependency, or the revision it names doesn't exist. |  | — |
| [`bynk.package.lock_invalid`](/book/reference/diagnostics/bynk.package.lock_invalid/) | `bynk.lock` can't be read: it isn't valid TOML, has no `version`, is a version this toolchain doesn't read, or locks a package to something that isn't a commit. |  | — |
| [`bynk.package.lock_out_of_date`](/book/reference/diagnostics/bynk.package.lock_out_of_date/) | `--locked` was given, and resolving the manifests would change `bynk.lock`. |  | — |
| [`bynk.package.name_mismatch`](/book/reference/diagnostics/bynk.package.name_mismatch/) | A path dependency's `package` names a different package than the directory's `[package] name`. |  | — |
| [`bynk.package.not_fetched`](/book/reference/diagnostics/bynk.package.not_fetched/) | A git dependency has no `bynk.lock` entry, or its store entry is missing; `bynk fetch` fetches it (refused under `--offline`). |  | — |
| [`bynk.package.unit_not_exported`](/book/reference/diagnostics/bynk.package.unit_not_exported/) | A `uses` or `consumes` reaches a unit its dependency declares but doesn't list in `[package] exports`. |  | — |
| [`bynk.package.unknown_unit`](/book/reference/diagnostics/bynk.package.unknown_unit/) | A `uses` or `consumes` names, through a dependency alias, a unit the dependency doesn't declare. |  | — |
| [`bynk.package.unsafe_source`](/book/reference/diagnostics/bynk.package.unsafe_source/) | A git dependency's URL or revision looks like a command-line option, or uses a transport other than `https`, `ssh` (or `file`, from the project's own manifests). |  | — |
| [`bynk.package.unsupported_in_dependency`](/book/reference/diagnostics/bynk.package.unsupported_in_dependency/) | A dependency declares an `asset` or an `embed`, which would resolve against the consuming project's `assets/` or `embeds/`; a per-package namespace for either isn't designed yet. |  | — |
| [`bynk.package.version_conflict`](/book/reference/diagnostics/bynk.package.version_conflict/) | One package is reached at two different git revisions (or from git and a path); a build, or a workspace, holds one copy of each package. |  | — |

## Parser

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.parse.bare_expression_statement`](/book/reference/diagnostics/bynk.parse.bare_expression_statement/) | An expression stands alone as a statement, followed by more of the block; a block has no expression statements, so its value would be discarded. Bind it with `let _ <-` (an effect) or `let _ =`. | [`block`](/book/reference/grammar/#rule-block) | — |
| [`bynk.parse.clause_after_statement`](/book/reference/diagnostics/bynk.parse.clause_after_statement/) | A `stub` or `env` clause came after a statement in a test case; they lead the case body. |  | — |
| [`bynk.parse.const_missing_type`](/book/reference/diagnostics/bynk.parse.const_missing_type/) | A `const` declaration has no type annotation; a constant's type is always written (`const Name: Type = …`). | [`const_decl`](/book/reference/grammar/#rule-const_decl) | — |
| [`bynk.parse.consumes_after_decls`](/book/reference/diagnostics/bynk.parse.consumes_after_decls/) | `consumes` appears after other declarations. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.parse.dangling_handler_annotation`](/book/reference/diagnostics/bynk.parse.dangling_handler_annotation/) | A handler-position annotation (e.g. `@cache`) is not followed by an `on` handler. |  | — |
| [`bynk.parse.doc_block_contains_code`](/book/reference/diagnostics/bynk.parse.doc_block_contains_code/) | A documentation block's content (outside Markdown code fences) parses as declarations, so a pair of dash dividers has turned code into documentation. |  | — |
| [`bynk.parse.duplicate_cors`](/book/reference/diagnostics/bynk.parse.duplicate_cors/) | A service declares more than one `cors { }` policy. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.parse.duplicate_json`](/book/reference/diagnostics/bynk.parse.duplicate_json/) | A service declares more than one `json { }` policy. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.parse.duplicate_limits`](/book/reference/diagnostics/bynk.parse.duplicate_limits/) | A service declares more than one `limits { }` policy. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.parse.duplicate_security`](/book/reference/diagnostics/bynk.parse.duplicate_security/) | A service declares more than one `security { }` policy. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.parse.empty_agent`](/book/reference/diagnostics/bynk.parse.empty_agent/) | An `agent` body is empty. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| [`bynk.parse.empty_capability`](/book/reference/diagnostics/bynk.parse.empty_capability/) | A `capability` body is empty. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| [`bynk.parse.empty_interpolation`](/book/reference/diagnostics/bynk.parse.empty_interpolation/) | An interpolation hole `\(…)` contains no expression. |  | — |
| [`bynk.parse.empty_match`](/book/reference/diagnostics/bynk.parse.empty_match/) | A `match` has no arms. | [`match_expr`](/book/reference/grammar/#rule-match_expr) | — |
| [`bynk.parse.empty_service`](/book/reference/diagnostics/bynk.parse.empty_service/) | A `service` body is empty. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.parse.event_pattern_empty`](/book/reference/diagnostics/bynk.parse.event_pattern_empty/) | A `from Events(E { ... })` subscription pattern listed no fields — use `from Events(E)` (no braces) for an unfiltered subscription. |  | — |
| [`bynk.parse.expected_agent_key`](/book/reference/diagnostics/bynk.parse.expected_agent_key/) | Expected a `key` declaration in an agent. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| [`bynk.parse.expected_agent_storage`](/book/reference/diagnostics/bynk.parse.expected_agent_storage/) | An agent declares no storage — it has no `store` fields. |  | — |
| [`bynk.parse.expected_base_type`](/book/reference/diagnostics/bynk.parse.expected_base_type/) | Expected a base type. | [`base_type`](/book/reference/grammar/#rule-base_type) | — |
| [`bynk.parse.expected_capability_op`](/book/reference/diagnostics/bynk.parse.expected_capability_op/) | Expected a capability operation. | [`capability_op`](/book/reference/grammar/#rule-capability_op) | — |
| [`bynk.parse.expected_expression`](/book/reference/diagnostics/bynk.parse.expected_expression/) | Expected an expression. |  | — |
| [`bynk.parse.expected_handler`](/book/reference/diagnostics/bynk.parse.expected_handler/) | Expected a handler. | [`handler`](/book/reference/grammar/#rule-handler) | — |
| [`bynk.parse.expected_item`](/book/reference/diagnostics/bynk.parse.expected_item/) | Expected a declaration. |  | — |
| [`bynk.parse.expected_predicate`](/book/reference/diagnostics/bynk.parse.expected_predicate/) | Expected a refinement predicate. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.parse.expected_provider_op`](/book/reference/diagnostics/bynk.parse.expected_provider_op/) | Expected a provider operation. | [`provider_op`](/book/reference/grammar/#rule-provider_op) | — |
| [`bynk.parse.expected_token`](/book/reference/diagnostics/bynk.parse.expected_token/) | Expected a specific token. |  | — |
| [`bynk.parse.expected_type`](/book/reference/diagnostics/bynk.parse.expected_type/) | Expected a type. |  | — |
| [`bynk.parse.expected_unit_header`](/book/reference/diagnostics/bynk.parse.expected_unit_header/) | Expected a `commons` or `context` header. |  | — |
| [`bynk.parse.expected_visibility`](/book/reference/diagnostics/bynk.parse.expected_visibility/) | Expected a visibility keyword. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| [`bynk.parse.exports_after_decls`](/book/reference/diagnostics/bynk.parse.exports_after_decls/) | `exports` appears after other declarations. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| [`bynk.parse.extra_tokens`](/book/reference/diagnostics/bynk.parse.extra_tokens/) | Unexpected tokens after an otherwise complete construct. |  | — |
| [`bynk.parse.generic_arg_count`](/book/reference/diagnostics/bynk.parse.generic_arg_count/) | Wrong number of generic type arguments. | [`generic_type_ref`](/book/reference/grammar/#rule-generic_type_ref) | — |
| [`bynk.parse.handler_in_agent`](/book/reference/diagnostics/bynk.parse.handler_in_agent/) | A protocol handler (`on GET`/`schedule`/`message`) was declared in an agent. | [`handler`](/book/reference/grammar/#rule-handler) | — |
| [`bynk.parse.invariant_after_handler`](/book/reference/diagnostics/bynk.parse.invariant_after_handler/) | An `invariant` was declared after a handler; invariants precede handlers. |  | — |
| [`bynk.parse.keyword_field_shorthand`](/book/reference/diagnostics/bynk.parse.keyword_field_shorthand/) | A reserved keyword was used as a field shorthand, which would bind a variable of that name. | [`identifier`](/book/reference/grammar/#rule-identifier) | — |
| [`bynk.parse.lambda_pattern_annotation`](/book/reference/diagnostics/bynk.parse.lambda_pattern_annotation/) | A lambda parameter with a record pattern carries a type annotation; the pattern's type name already gives the parameter's type. | [`lambda_param`](/book/reference/grammar/#rule-lambda_param) | — |
| [`bynk.parse.lambda_pattern_not_record`](/book/reference/diagnostics/bynk.parse.lambda_pattern_not_record/) | A lambda parameter is a pattern other than a record pattern; only an irrefutable record pattern may stand in for a name. | [`lambda_param`](/book/reference/grammar/#rule-lambda_param) | — |
| [`bynk.parse.lambda_pattern_refutable`](/book/reference/diagnostics/bynk.parse.lambda_pattern_refutable/) | A lambda parameter's record pattern can fail to match some value of its type, and a lambda has no `else` to fall back on. | [`lambda_param`](/book/reference/grammar/#rule-lambda_param) | — |
| [`bynk.parse.let_else_on_binding`](/book/reference/diagnostics/bynk.parse.let_else_on_binding/) | An `else` follows a plain `let name = …` binding, which matches every value, so the `else` could never run. | [`let_stmt`](/book/reference/grammar/#rule-let_stmt), [`effect_let_stmt`](/book/reference/grammar/#rule-effect_let_stmt) | — |
| [`bynk.parse.let_else_staircase`](/book/reference/diagnostics/bynk.parse.let_else_staircase/) | Nested `match`es on `Option`/`Result` that each carry on in one arm and give up in the other, which `let … else` writes flat. A hint with a machine-applicable rewrite; never fails a build and is hidden from the terminal (hint). |  | Hint |
| [`bynk.parse.let_pattern_annotation`](/book/reference/diagnostics/bynk.parse.let_pattern_annotation/) | A `let` with a pattern carries a type annotation; the pattern's names take their types from the value. | [`let_else_stmt`](/book/reference/grammar/#rule-let_else_stmt) | — |
| [`bynk.parse.let_pattern_without_else`](/book/reference/diagnostics/bynk.parse.let_pattern_without_else/) | A `let` binds a pattern but has no `else` saying what the block produces when the value does not match; only a record pattern that cannot fail may omit it. | [`let_else_stmt`](/book/reference/grammar/#rule-let_else_stmt) | — |
| [`bynk.parse.list_rest_not_last`](/book/reference/diagnostics/bynk.parse.list_rest_not_last/) | A `..` in a list pattern is not the final element; suffix forms such as `[.., last]` are not available. | [`list_pattern`](/book/reference/grammar/#rule-list_pattern) | — |
| [`bynk.parse.malformed_float_literal`](/book/reference/diagnostics/bynk.parse.malformed_float_literal/) | A float literal is missing a digit on one side of the `.` (`1.`, `.5`). | [`float_literal`](/book/reference/grammar/#rule-float_literal) | — |
| [`bynk.parse.match_ends_block`](/book/reference/diagnostics/bynk.parse.match_ends_block/) | A `match` ends the block it is in, because a block has no expression statements; a second `match` straight after it cannot follow. | [`block`](/book/reference/grammar/#rule-block) | — |
| [`bynk.parse.migrate_after_handler`](/book/reference/diagnostics/bynk.parse.migrate_after_handler/) | A `migrate` step is declared after an agent handler; migration steps precede the handlers. |  | — |
| [`bynk.parse.migrate_item`](/book/reference/diagnostics/bynk.parse.migrate_item/) | A `migrate` step's body holds something other than `field := value`, `let` or `drop field`. |  | — |
| [`bynk.parse.multiple_docs`](/book/reference/diagnostics/bynk.parse.multiple_docs/) | A declaration or member has two docs directly above it; a target has one doc, so a doc of more than one line is a `---` block. |  | — |
| [`bynk.parse.nesting_too_deep`](/book/reference/diagnostics/bynk.parse.nesting_too_deep/) | An expression or type nests deeper than the parser's fixed limit. |  | — |
| [`bynk.parse.no_remainder_operator`](/book/reference/diagnostics/bynk.parse.no_remainder_operator/) | `%` is not an operator; use `Int.rem` (the dividend's sign) or `Int.mod` (the divisor's). |  | — |
| [`bynk.parse.non_associative`](/book/reference/diagnostics/bynk.parse.non_associative/) | A non-associative operator was chained (e.g. `a == b == c`). | [`binary_expr`](/book/reference/grammar/#rule-binary_expr) | — |
| [`bynk.parse.orphan_doc_block`](/book/reference/diagnostics/bynk.parse.orphan_doc_block/) | A documentation block is not attached to a declaration (warning). |  | Warning |
| [`bynk.parse.policy_order`](/book/reference/diagnostics/bynk.parse.policy_order/) | A service's `cors`/`security`/`limits` policy is out of order or follows a handler; they open the body in the order `cors`, `security`, `limits`, `json`. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.parse.record_pattern_in_is_unparenthesised`](/book/reference/diagnostics/bynk.parse.record_pattern_in_is_unparenthesised/) | A record pattern after `is` is not parenthesised — write `x is (T { f, .. })`. | [`record_pattern`](/book/reference/grammar/#rule-record_pattern) | — |
| [`bynk.parse.record_pattern_without_rest`](/book/reference/diagnostics/bynk.parse.record_pattern_without_rest/) | A record pattern does not end with `..`; every record pattern is open. | [`record_pattern`](/book/reference/grammar/#rule-record_pattern) | — |
| [`bynk.parse.refined_pattern_inner`](/book/reference/diagnostics/bynk.parse.refined_pattern_inner/) | A refined pattern's inner form is something other than `_`. | [`refined_pattern`](/book/reference/grammar/#rule-refined_pattern) | — |
| [`bynk.parse.removed_send_arrow`](/book/reference/diagnostics/bynk.parse.removed_send_arrow/) | The `~>` send was removed; write `do e.detach()`. | [`do_stmt`](/book/reference/grammar/#rule-do_stmt) | — |
| [`bynk.parse.reserved_keyword`](/book/reference/diagnostics/bynk.parse.reserved_keyword/) | A reserved keyword was used as an identifier. | [`identifier`](/book/reference/grammar/#rule-identifier) | — |
| [`bynk.parse.self_outside_method`](/book/reference/diagnostics/bynk.parse.self_outside_method/) | `self` is the first parameter of a free function; only a method declaration, `fn Type.method(self, …)`, has a receiver. | [`self_expr`](/book/reference/grammar/#rule-self_expr) | — |
| [`bynk.parse.storage_after_phase`](/book/reference/diagnostics/bynk.parse.storage_after_phase/) | Agent storage (`state` / `store`) is declared after the invariants or handlers. |  | — |
| [`bynk.parse.transition_after_handler`](/book/reference/diagnostics/bynk.parse.transition_after_handler/) | A `transition` is declared after an agent handler; step invariants precede the handlers. |  | — |
| [`bynk.parse.unexpected_adapter`](/book/reference/diagnostics/bynk.parse.unexpected_adapter/) | An `adapter` appeared where it is not allowed. |  | — |
| [`bynk.parse.unexpected_context`](/book/reference/diagnostics/bynk.parse.unexpected_context/) | A `context` appeared where it is not allowed. | [`context_decl`](/book/reference/grammar/#rule-context_decl) | — |
| [`bynk.parse.unexpected_eof`](/book/reference/diagnostics/bynk.parse.unexpected_eof/) | Unexpected end of input. |  | — |
| [`bynk.parse.unexpected_suite`](/book/reference/diagnostics/bynk.parse.unexpected_suite/) | A `suite` appeared where it is not allowed. | [`suite_decl`](/book/reference/grammar/#rule-suite_decl) | — |
| [`bynk.parse.unknown_effect_method`](/book/reference/diagnostics/bynk.parse.unknown_effect_method/) | An unknown method on `Effect`. |  | — |
| [`bynk.parse.unknown_handler_kind`](/book/reference/diagnostics/bynk.parse.unknown_handler_kind/) | An unknown handler form (expected `call`, an HTTP method, `schedule`, or `message`). | [`handler`](/book/reference/grammar/#rule-handler) | — |
| [`bynk.parse.unknown_predicate`](/book/reference/diagnostics/bynk.parse.unknown_predicate/) | An unknown refinement predicate. | [`predicate_name`](/book/reference/grammar/#rule-predicate_name) | — |
| [`bynk.parse.unknown_tier`](/book/reference/diagnostics/bynk.parse.unknown_tier/) | A `case`/`suite` `as <tier>` clause names something other than `unit`, `integration`, or `system`. |  | — |
| [`bynk.parse.uses_after_decls`](/book/reference/diagnostics/bynk.parse.uses_after_decls/) | `uses` appears after other declarations. | [`uses_decl`](/book/reference/grammar/#rule-uses_decl) | — |
| [`bynk.parse.variant_name_case`](/book/reference/diagnostics/bynk.parse.variant_name_case/) | A sum-type or enum variant name is not capitalised. | [`sum_variant`](/book/reference/grammar/#rule-sum_variant), [`enum_type`](/book/reference/grammar/#rule-enum_type) | — |

## Project

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.namespace.reserved`](/book/reference/diagnostics/bynk.namespace.reserved/) | A user unit is named `bynk` or `bynk.*`; the `bynk` root is reserved for the toolchain. |  | — |
| [`bynk.project.contracts_lock_corrupt`](/book/reference/diagnostics/bynk.project.contracts_lock_corrupt/) | `bynk.contracts.lock` (the older service contracts a callee still accepts) is missing its version field, empty, truncated, unparseable, or of an unsupported version — restore it from version control rather than deleting it, since a callee that forgets the contracts it accepts refuses its live callers. |  | — |
| [`bynk.project.contracts_lock_oversized`](/book/reference/diagnostics/bynk.project.contracts_lock_oversized/) | `bynk.contracts.lock` has one service accepting more older contracts than the build keeps comfortably (warning). |  | Warning |
| [`bynk.project.contracts_lock_stale`](/book/reference/diagnostics/bynk.project.contracts_lock_stale/) | `bynk.contracts.lock` lists an older contract the service no longer accepts, because it is unreadable or no longer compatible with the contract the service has now; the entry is ignored (warning). |  | Warning |
| [`bynk.project.duplicate_compatibility_flag`](/book/reference/diagnostics/bynk.project.duplicate_compatibility_flag/) | `bynk.toml`'s `[workers] compatibility_flags` lists a flag twice, or one every Bynk Worker already has; the duplicate is dropped (warning). |  | Warning |
| [`bynk.project.duplicate_module_doc`](/book/reference/diagnostics/bynk.project.duplicate_module_doc/) | More than one file of a multi-file unit carries a module doc. |  | — |
| [`bynk.project.export_context`](/book/reference/diagnostics/bynk.project.export_context/) | `bynk.toml`'s `[package] exports` lists a context: a package shares commons and adapters, never a context, which belongs to the project that deploys it. |  | — |
| [`bynk.project.export_unknown`](/book/reference/diagnostics/bynk.project.export_unknown/) | `bynk.toml`'s `[package] exports` lists a name that isn't a commons or adapter the project declares. |  | — |
| [`bynk.project.file_and_directory`](/book/reference/diagnostics/bynk.project.file_and_directory/) | A unit exists as both a file and a directory. |  | — |
| [`bynk.project.inconsistent_commons_name`](/book/reference/diagnostics/bynk.project.inconsistent_commons_name/) | A source file's path does not match its declared name. |  | — |
| [`bynk.project.kind_conflict`](/book/reference/diagnostics/bynk.project.kind_conflict/) | A name is declared as both a commons and a context. |  | — |
| [`bynk.project.no_root`](/book/reference/diagnostics/bynk.project.no_root/) | No project root could be determined. |  | — |
| [`bynk.project.no_sources`](/book/reference/diagnostics/bynk.project.no_sources/) | The project contains no source files. |  | — |
| [`bynk.project.read_failed`](/book/reference/diagnostics/bynk.project.read_failed/) | A source file could not be read. |  | — |
| [`bynk.project.schema_registry_corrupt`](/book/reference/diagnostics/bynk.project.schema_registry_corrupt/) | `bynk.schema.lock` (the schema registry for events and agent state) is missing its version field, empty, truncated, or otherwise unparseable — restore it from version control rather than deleting it, since deleting it would silently re-baseline every event's and agent's history. |  | — |
| [`bynk.project.unstable_not_experimental`](/book/reference/diagnostics/bynk.project.unstable_not_experimental/) | `bynk.toml`'s `[unstable] allow` lists an item that nothing experimental lies in or under: it is stable now, so the entry can go (warning). |  | Warning |
| [`bynk.project.unstable_unknown_item`](/book/reference/diagnostics/bynk.project.unstable_unknown_item/) | `bynk.toml`'s `[unstable] allow` lists a path that names no standard-library unit, exported item or operation. |  | — |

## Properties (generative tests)

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.property.restates_refinement`](/book/reference/diagnostics/bynk.property.restates_refinement/) | A `property` merely re-checks a refinement its type already guarantees. | [`for_all`](/book/reference/grammar/#rule-for_all) | — |
| [`bynk.property.where_not_bool`](/book/reference/diagnostics/bynk.property.where_not_bool/) | A `for all ... where` filter does not type to `Bool`. | [`for_all`](/book/reference/grammar/#rule-for_all) | — |

## Providers

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.provider.agent_call_undeclared_capability`](/book/reference/diagnostics/bynk.provider.agent_call_undeclared_capability/) | A provider calls an agent handler that needs a capability the provider's `given` does not list. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| [`bynk.provider.dependency_cycle`](/book/reference/diagnostics/bynk.provider.dependency_cycle/) | Providers form a capability dependency cycle through `given`. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| [`bynk.provider.extra_operation`](/book/reference/diagnostics/bynk.provider.extra_operation/) | A `provides` block implements an operation not in the capability. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| [`bynk.provider.generic_op_requires_external`](/book/reference/diagnostics/bynk.provider.generic_op_requires_external/) | A Bynk-bodied `provides` implements a capability operation that declares its own type parameter — only an external (bodiless) provider can. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| [`bynk.provider.missing_operation`](/book/reference/diagnostics/bynk.provider.missing_operation/) | A `provides` block is missing a capability operation. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| [`bynk.provider.outside_context`](/book/reference/diagnostics/bynk.provider.outside_context/) | `provides` was declared outside a context. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| [`bynk.provider.signature_mismatch`](/book/reference/diagnostics/bynk.provider.signature_mismatch/) | A `provides` operation's signature does not match the capability. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| [`bynk.provider.unknown_capability`](/book/reference/diagnostics/bynk.provider.unknown_capability/) | `provides` names a capability that does not exist. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |

## Queue

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.queue.bad_params`](/book/reference/diagnostics/bynk.queue.bad_params/) | An `on message` handler does not take exactly one `message` parameter. | [`queue_handler`](/book/reference/grammar/#rule-queue_handler) | — |
| [`bynk.queue.duplicate_consumer`](/book/reference/diagnostics/bynk.queue.duplicate_consumer/) | Two `on message` handlers, in one context or in two, consume the same queue. | [`queue_handler`](/book/reference/grammar/#rule-queue_handler) | — |
| [`bynk.queue.invalid_name`](/book/reference/diagnostics/bynk.queue.invalid_name/) | A `from queue("…")` binding has an empty queue name, or one Cloudflare refuses. | [`queue_handler`](/book/reference/grammar/#rule-queue_handler) | — |
| [`bynk.queue.return_not_queue_result`](/book/reference/diagnostics/bynk.queue.return_not_queue_result/) | An `on message` handler does not return `Effect[QueueResult]`. | [`handler`](/book/reference/grammar/#rule-handler) | — |

## Record spread

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.record_spread.field_type_mismatch`](/book/reference/diagnostics/bynk.record_spread.field_type_mismatch/) | A record-spread override has the wrong type for the field. | [`record_spread`](/book/reference/grammar/#rule-record_spread) | — |
| [`bynk.record_spread.non_record_base`](/book/reference/diagnostics/bynk.record_spread.non_record_base/) | The base of a record spread is not a record. | [`record_spread`](/book/reference/grammar/#rule-record_spread) | — |
| [`bynk.record_spread.type_mismatch`](/book/reference/diagnostics/bynk.record_spread.type_mismatch/) | A record spread's base is a different record type. | [`record_spread`](/book/reference/grammar/#rule-record_spread) | — |
| [`bynk.record_spread.unknown_field`](/book/reference/diagnostics/bynk.record_spread.unknown_field/) | A record spread overrides a field the record does not have. | [`record_spread`](/book/reference/grammar/#rule-record_spread) | — |

## Refinement

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.refine.literal_violates`](/book/reference/diagnostics/bynk.refine.literal_violates/) | A literal does not satisfy the refined type's predicate. | [`refined_type`](/book/reference/grammar/#rule-refined_type) | — |

## Resolution

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.resolve.ambiguous_variant`](/book/reference/diagnostics/bynk.resolve.ambiguous_variant/) | A bare variant name is declared by several sum types in scope, and no expected type picks one. |  | — |
| [`bynk.resolve.arity_mismatch`](/book/reference/diagnostics/bynk.resolve.arity_mismatch/) | A function was called with the wrong number of arguments. | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.resolve.bad_const_initialiser`](/book/reference/diagnostics/bynk.resolve.bad_const_initialiser/) | A `const` initialiser is not a static value of the declared type: only literals, constructors over static values, `T.unsafe(lit)`, `List.empty()`/`Map.empty()`/`Set.empty()`, `Duration` literals and references to other constants are allowed. | [`const_decl`](/book/reference/grammar/#rule-const_decl) | — |
| [`bynk.resolve.const_cycle`](/book/reference/diagnostics/bynk.resolve.const_cycle/) | Constants refer to each other in a cycle, so none of them has a value. | [`const_decl`](/book/reference/grammar/#rule-const_decl) | — |
| [`bynk.resolve.const_name_case`](/book/reference/diagnostics/bynk.resolve.const_name_case/) | A constant's name is not upper-camel case (`MaxRetries`, not `maxRetries`). | [`const_decl`](/book/reference/grammar/#rule-const_decl) | — |
| [`bynk.resolve.const_outside_context_or_commons`](/book/reference/diagnostics/bynk.resolve.const_outside_context_or_commons/) | A `const` declaration appears outside a context or commons. | [`const_decl`](/book/reference/grammar/#rule-const_decl) | — |
| [`bynk.resolve.duplicate_actor`](/book/reference/diagnostics/bynk.resolve.duplicate_actor/) | Two actors share a name. |  | — |
| [`bynk.resolve.duplicate_agent`](/book/reference/diagnostics/bynk.resolve.duplicate_agent/) | Two agents share a name. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| [`bynk.resolve.duplicate_capability`](/book/reference/diagnostics/bynk.resolve.duplicate_capability/) | Two capabilities share a name. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| [`bynk.resolve.duplicate_const`](/book/reference/diagnostics/bynk.resolve.duplicate_const/) | Two constants share a name. | [`const_decl`](/book/reference/grammar/#rule-const_decl) | — |
| [`bynk.resolve.duplicate_field`](/book/reference/diagnostics/bynk.resolve.duplicate_field/) | A record, or one variant's payload, declares a field twice. | [`record_type`](/book/reference/grammar/#rule-record_type) | — |
| [`bynk.resolve.duplicate_field_init`](/book/reference/diagnostics/bynk.resolve.duplicate_field_init/) | A record construction initialises a field twice. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| [`bynk.resolve.duplicate_fn`](/book/reference/diagnostics/bynk.resolve.duplicate_fn/) | Two functions share a name. | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.resolve.duplicate_message_code`](/book/reference/diagnostics/bynk.resolve.duplicate_message_code/) | A message bundle declares the same code twice in one block. |  | — |
| [`bynk.resolve.duplicate_message_locale`](/book/reference/diagnostics/bynk.resolve.duplicate_message_locale/) | Two `messages` blocks in one bundle declare the same locale tag. |  | — |
| [`bynk.resolve.duplicate_method`](/book/reference/diagnostics/bynk.resolve.duplicate_method/) | Two methods share a name. | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.resolve.duplicate_param`](/book/reference/diagnostics/bynk.resolve.duplicate_param/) | A parameter name is repeated. | [`param`](/book/reference/grammar/#rule-param) | — |
| [`bynk.resolve.duplicate_provider`](/book/reference/diagnostics/bynk.resolve.duplicate_provider/) | A capability is provided more than once. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| [`bynk.resolve.duplicate_service`](/book/reference/diagnostics/bynk.resolve.duplicate_service/) | Two services share a name. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.resolve.duplicate_type`](/book/reference/diagnostics/bynk.resolve.duplicate_type/) | Two types share a name. | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| [`bynk.resolve.duplicate_variant`](/book/reference/diagnostics/bynk.resolve.duplicate_variant/) | A sum type declares a variant twice. | [`sum_type`](/book/reference/grammar/#rule-sum_type) | — |
| [`bynk.resolve.fn_without_call`](/book/reference/diagnostics/bynk.resolve.fn_without_call/) | A function was referenced without being called. |  | — |
| [`bynk.resolve.let_shadows_fn`](/book/reference/diagnostics/bynk.resolve.let_shadows_fn/) | A `let` binding shadows a function. | [`let_stmt`](/book/reference/grammar/#rule-let_stmt) | — |
| [`bynk.resolve.let_shadows_type`](/book/reference/diagnostics/bynk.resolve.let_shadows_type/) | A `let` binding shadows a type. | [`let_stmt`](/book/reference/grammar/#rule-let_stmt) | — |
| [`bynk.resolve.method_unknown_type`](/book/reference/diagnostics/bynk.resolve.method_unknown_type/) | A method is defined on an unknown type. |  | — |
| [`bynk.resolve.missing_field`](/book/reference/diagnostics/bynk.resolve.missing_field/) | A record construction omits a required field. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| [`bynk.resolve.name_conflict`](/book/reference/diagnostics/bynk.resolve.name_conflict/) | Two declarations share a name. |  | — |
| [`bynk.resolve.not_a_record_type`](/book/reference/diagnostics/bynk.resolve.not_a_record_type/) | Record syntax was used on a non-record type. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| [`bynk.resolve.opaque_record_construction`](/book/reference/diagnostics/bynk.resolve.opaque_record_construction/) | An opaque type was constructed with record syntax. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| [`bynk.resolve.param_as_function`](/book/reference/diagnostics/bynk.resolve.param_as_function/) | A value (such as a parameter) was called as a function. | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.resolve.recursive_record_field`](/book/reference/diagnostics/bynk.resolve.recursive_record_field/) | A record directly contains a field of its own type. | [`record_type`](/book/reference/grammar/#rule-record_type) | — |
| [`bynk.resolve.reserved_builtin_type`](/book/reference/diagnostics/bynk.resolve.reserved_builtin_type/) | A type declaration reuses a compiler-known built-in type name. | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| [`bynk.resolve.reserved_host_name`](/book/reference/diagnostics/bynk.resolve.reserved_host_name/) | A declaration is named `globalThis`, which the generated TypeScript uses to reach host globals. | [`type_decl`](/book/reference/grammar/#rule-type_decl), [`fn_decl`](/book/reference/grammar/#rule-fn_decl), [`agent_decl`](/book/reference/grammar/#rule-agent_decl), [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| [`bynk.resolve.reserved_payload_field`](/book/reference/diagnostics/bynk.resolve.reserved_payload_field/) | A sum variant declares a payload field named `kind`, the variant's wire discriminant. | [`sum_type`](/book/reference/grammar/#rule-sum_type) | — |
| [`bynk.resolve.self_outside_method`](/book/reference/diagnostics/bynk.resolve.self_outside_method/) | `self` referenced outside a method or handler. | [`self_expr`](/book/reference/grammar/#rule-self_expr) | — |
| [`bynk.resolve.type_as_function`](/book/reference/diagnostics/bynk.resolve.type_as_function/) | A type name was called as if it were a function. | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.resolve.type_in_expr`](/book/reference/diagnostics/bynk.resolve.type_in_expr/) | A type name was used where a value is expected. |  | — |
| [`bynk.resolve.unconsumed_context`](/book/reference/diagnostics/bynk.resolve.unconsumed_context/) | A context's service was called, or its capability named in `given`, without a `consumes` declaration. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.resolve.unknown_field`](/book/reference/diagnostics/bynk.resolve.unknown_field/) | A record construction names a field the record type does not declare. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| [`bynk.resolve.unknown_function`](/book/reference/diagnostics/bynk.resolve.unknown_function/) | Called a function that does not exist. | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.resolve.unknown_name`](/book/reference/diagnostics/bynk.resolve.unknown_name/) | Referenced a name that is not in scope. |  | — |
| [`bynk.resolve.unknown_static_member`](/book/reference/diagnostics/bynk.resolve.unknown_static_member/) | Referenced an unknown static member (e.g. `T.x`). | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| [`bynk.resolve.unknown_type`](/book/reference/diagnostics/bynk.resolve.unknown_type/) | Referenced a type that does not exist. |  | — |

## Services

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.service.duplicate_call_handler`](/book/reference/diagnostics/bynk.service.duplicate_call_handler/) | A service declares more than one `on call` handler; it has exactly one. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.service.missing_from`](/book/reference/diagnostics/bynk.service.missing_from/) | A `from`-less service has a handler other than `on call`. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.service.mixed_protocols`](/book/reference/diagnostics/bynk.service.mixed_protocols/) | A service mixes handler forms that do not match its `from <protocol>`. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.service.outside_context`](/book/reference/diagnostics/bynk.service.outside_context/) | A `service` was declared outside a context. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.service.return_not_effect`](/book/reference/diagnostics/bynk.service.return_not_effect/) | A service handler's return type is not an `Effect`. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.service.unknown_protocol`](/book/reference/diagnostics/bynk.service.unknown_protocol/) | A `from <protocol>` names an unknown protocol (e.g. a transport like Kafka). | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| [`bynk.service.unknown_via_clause`](/book/reference/diagnostics/bynk.service.unknown_via_clause/) | A `via <name>(...)` clause on a `from Events(...)` header named something other than `schema` — `via` clauses are a closed set, and only `via schema(...)` exists today. |  | — |
| [`bynk.service.websocket_header`](/book/reference/diagnostics/bynk.service.websocket_header/) | The `from websocket` header is malformed — it binds frame types as `websocket(in: <type>, out: <type>)` (real-time track slice 3). |  | — |
| [`bynk.service.websocket_multiple`](/book/reference/diagnostics/bynk.service.websocket_multiple/) | A context holds more than one `from websocket` service — at v1 the Workers upgrade routes by the `Upgrade: websocket` header alone, so one WebSocket service per context (real-time track slice 3b). |  | — |
| [`bynk.service.websocket_open_arity`](/book/reference/diagnostics/bynk.service.websocket_open_arity/) | A `from websocket` service must hold exactly one `on open` handler (the edge upgrade), and at most one `on message` (inbound) and one `on close` (real-time track slice 3/3b-iii). |  | — |
| [`bynk.service.websocket_param_not_stringy`](/book/reference/diagnostics/bynk.service.websocket_param_not_stringy/) | A `from websocket` service's `on open` parameter has a type not constructible from `String`; it arrives as a query-string value (#1657). |  | — |

## Stability

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.stability.experimental`](/book/reference/diagnostics/bynk.stability.experimental/) | An experimental standard-library item is used without an `[unstable] allow` entry in `bynk.toml` covering it. |  | — |

## Static assets

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.asset.duplicate`](/book/reference/diagnostics/bynk.asset.duplicate/) | An `asset` name is declared more than once in the same unit. |  | — |
| [`bynk.asset.invalid_at_path`](/book/reference/diagnostics/bynk.asset.invalid_at_path/) | An `asset`'s `at` path is not a URL path naming one file, or lies under `/_assets/` or `/_bynk/`, or is a file the platform reads as configuration. |  | — |
| [`bynk.asset.may_shadow_route`](/book/reference/diagnostics/bynk.asset.may_shadow_route/) | A parameterised HTTP route matches a fixed asset path, so it never sees requests for that path (a warning). |  | Warning |
| [`bynk.asset.missing_file`](/book/reference/diagnostics/bynk.asset.missing_file/) | An `asset` names a file that is not in the project's `assets/` folder. |  | — |
| [`bynk.asset.needs_project`](/book/reference/diagnostics/bynk.asset.needs_project/) | A single-file build met an `asset` declaration; assets need a project build. |  | — |
| [`bynk.asset.not_served`](/book/reference/diagnostics/bynk.asset.not_served/) | A deployable Bundle build met an `asset`; a Bundle ships no files, so its `.url` would not be served (a warning). |  | Warning |
| [`bynk.asset.outside_assets_dir`](/book/reference/diagnostics/bynk.asset.outside_assets_dir/) | An `asset` path does not name a file in the project's `assets/` folder. |  | — |
| [`bynk.asset.outside_context_or_commons`](/book/reference/diagnostics/bynk.asset.outside_context_or_commons/) | An `asset` declaration appears outside a context or commons. |  | — |
| [`bynk.asset.path_conflict`](/book/reference/diagnostics/bynk.asset.path_conflict/) | Two different asset files would be served at one fixed path by the same Worker. |  | — |
| [`bynk.asset.shadows_route`](/book/reference/diagnostics/bynk.asset.shadows_route/) | A fixed asset path equals a literal HTTP route, in the same context or in another member of the same `[deploy.workers.<key>]` group, which the platform-served asset makes unreachable. |  | — |
| [`bynk.asset.unreadable_file`](/book/reference/diagnostics/bynk.asset.unreadable_file/) | An `asset` names a file in the project's `assets/` folder that could not be read. |  | — |
| [`bynk.asset.unsafe_path`](/book/reference/diagnostics/bynk.asset.unsafe_path/) | An `asset` path is empty, absolute, or contains a `..` segment. |  | — |

## Stubs

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.stub.after_type`](/book/reference/diagnostics/bynk.stub.after_type/) | A test `stub … returns <value> after <d>` latency is not a `Duration`. |  | — |
| [`bynk.stub.at_system`](/book/reference/diagnostics/bynk.stub.at_system/) | A test `stub` is in a `system`-tier case, or at the scope of a `suite … as system` (or a suite whose cases run `as system`); each context runs as the Worker it deploys as, which builds its own capability providers, so there is no seam to override. | [`stub_clause`](/book/reference/grammar/#rule-stub_clause) | — |
| [`bynk.stub.bad_sequence`](/book/reference/diagnostics/bynk.stub.bad_sequence/) | A `stub … returns each […]` sequence is malformed (e.g. empty). |  | — |
| [`bynk.stub.generic_op`](/book/reference/diagnostics/bynk.stub.generic_op/) | A test `stub` targets a capability operation that declares its own type parameter — not supported at v1. |  | — |
| [`bynk.stub.not_a_seam`](/book/reference/diagnostics/bynk.stub.not_a_seam/) | A test `stub` overrides a capability the unit under test does not consume. |  | — |
| [`bynk.stub.rhs_type`](/book/reference/diagnostics/bynk.stub.rhs_type/) | A test `stub … returns <value>` right-hand side does not match the operation's return type. |  | — |
| [`bynk.stub.unknown_op`](/book/reference/diagnostics/bynk.stub.unknown_op/) | A test `stub` names an operation the capability does not declare. |  | — |

## Suites and cases

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.suite.const_name_clash`](/book/reference/diagnostics/bynk.suite.const_name_clash/) | A suite `const` has the name of a declaration or sum variant already in scope in the suite's cases. | [`const_decl`](/book/reference/grammar/#rule-const_decl) | — |
| [`bynk.suite.duplicate_case_name`](/book/reference/diagnostics/bynk.suite.duplicate_case_name/) | Two `case`s share a description. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.suite.duplicate_const`](/book/reference/diagnostics/bynk.suite.duplicate_const/) | A suite declares two `const`s of the same name, or a `const` with the name of one of its `type`s. | [`const_decl`](/book/reference/grammar/#rule-const_decl) | — |
| [`bynk.suite.duplicate_env`](/book/reference/diagnostics/bynk.suite.duplicate_env/) | An `env { … }` clause gives the same name twice at one scope (a suite's `env` clauses, or one case's); a case's `env` may override a suite's name, but not repeat its own. | [`env_clause`](/book/reference/grammar/#rule-env_clause) | — |
| [`bynk.suite.duplicate_fn`](/book/reference/diagnostics/bynk.suite.duplicate_fn/) | A suite declares two helper `fn`s of the same name. | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.suite.duplicate_type`](/book/reference/diagnostics/bynk.suite.duplicate_type/) | A suite declares two `type`s of the same name. | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| [`bynk.suite.env_binding_clash`](/book/reference/diagnostics/bynk.suite.env_binding_clash/) | A `system`-tier suite's or case's `env` names a binding a participant's Worker already declares (a Service Binding, KV namespace, or Durable Object namespace), or a key the test harness sets on it (a call-edge key, a `Foreign` caller's secret), so the entry would clash with it on the Worker's `env`. | [`env_clause`](/book/reference/grammar/#rule-env_clause) | — |
| [`bynk.suite.env_not_literal`](/book/reference/diagnostics/bynk.suite.env_not_literal/) | An `env { … }` entry's value is neither a `String` literal nor a `fixture` the same suite declares; a Worker's variables and secrets are strings, given verbatim. | [`env_clause`](/book/reference/grammar/#rule-env_clause) | — |
| [`bynk.suite.fn_effectful`](/book/reference/diagnostics/bynk.suite.fn_effectful/) | A suite helper `fn` returns an `Effect`; suite helpers are pure. | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.suite.fn_method`](/book/reference/diagnostics/bynk.suite.fn_method/) | A suite helper `fn` is a method (`fn Type.name`); suite helpers are free functions. | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.suite.fn_name_clash`](/book/reference/diagnostics/bynk.suite.fn_name_clash/) | A suite helper `fn` has the name of a declaration already in scope in the suite's cases. | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| [`bynk.suite.type_generic`](/book/reference/diagnostics/bynk.suite.type_generic/) | A suite declares a generic `type`; a suite type is a plain, non-generic shape. | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| [`bynk.suite.type_name_clash`](/book/reference/diagnostics/bynk.suite.type_name_clash/) | A suite `type`, or a variant of one, has the name of a declaration or sum variant already in scope in the suite's cases, or of another suite item. | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| [`bynk.suite.unknown_target`](/book/reference/diagnostics/bynk.suite.unknown_target/) | A `suite` targets a unit that does not exist. | [`suite_decl`](/book/reference/grammar/#rule-suite_decl) | — |

## Test tiers

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.tier.cross_context_needs_system`](/book/reference/diagnostics/bynk.tier.cross_context_needs_system/) | A `unit` or `integration` case, or a `property`, reaches another context's service, directly or through a target service or agent handler that calls one; below `system` no other context is stood up to call, so the flow belongs in a `suite … as system`. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.tier.mixed_system_suite`](/book/reference/diagnostics/bynk.tier.mixed_system_suite/) | A suite mixes `system` cases with `unit` or `integration` ones; a `system` case runs against deployed Workers and addresses services by context path, so it needs a `suite … as system` of its own. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.tier.system_needs_wire`](/book/reference/diagnostics/bynk.tier.system_needs_wire/) | An `as system` suite whose target has no serialisation edge: it consumes no other context and exposes no `http`, `queue` or `email` service, so there is no wire for a `system` case to cross. |  | — |

## Tests

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.test.actor_identity_required`](/book/reference/diagnostics/bynk.test.actor_identity_required/) | A test omits the identity an identity-carrying actor requires: a bare call-site `by <Actor>`, or no `by` at all on a handler that binds its `Caller`. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.actor_no_identity`](/book/reference/diagnostics/bynk.test.actor_no_identity/) | A call-site `by <Actor>(x)` supplies an identity to an actor that takes none — a unit-identity actor (e.g. `Visitor`) or `Nobody`. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.advance_not_clock`](/book/reference/diagnostics/bynk.test.advance_not_clock/) | An `advance` statement names a capability other than `Clock`; the test harness owns one time source, the first-party `Clock`. | [`advance_stmt`](/book/reference/grammar/#rule-advance_stmt) | — |
| [`bynk.test.advance_not_duration`](/book/reference/diagnostics/bynk.test.advance_not_duration/) | An `advance Clock by` statement's amount is not a `Duration`. | [`advance_stmt`](/book/reference/grammar/#rule-advance_stmt) | — |
| [`bynk.test.advance_outside_case`](/book/reference/diagnostics/bynk.test.advance_outside_case/) | An `advance Clock by` statement appears outside a test `case` body; only the test harness has a clock to move. | [`advance_stmt`](/book/reference/grammar/#rule-advance_stmt) | — |
| [`bynk.test.asset_address_method`](/book/reference/diagnostics/bynk.test.asset_address_method/) | A suite fetches an asset with a method other than `GET`; the platform serves assets to `GET` and answers other methods with 405. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.call_needs_in_process`](/book/reference/diagnostics/bynk.test.call_needs_in_process/) | A `system`-tier case calls an `on call` service with `svc.call(…)`; an `on call` service is internal to a deployment and has no public route for a `system` case to drive, so the case drives an `http` route that reaches it, or calls it at `unit` or `integration`. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.credential_needs_system`](/book/reference/diagnostics/bynk.test.credential_needs_system/) | A case drives `by Nobody` (the no-credential principal, which tests the auth seam's 401) outside a `system`-tier case, where there is no real seam to reject it. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.event_needs_in_process`](/book/reference/diagnostics/bynk.test.event_needs_in_process/) | A `system`-tier case delivers an event with `svc.event(e)`; at `system` an event reaches a subscriber only through its publisher's fan-out, so the case drives the emitting handler instead, or delivers the event at `unit` or `integration`. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.event_not_subscribed`](/book/reference/diagnostics/bynk.test.event_not_subscribed/) | A test body's `svc.event(e)` delivers an event the `from Events(E)` service does not subscribe to. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.event_publisher_ambiguous`](/book/reference/diagnostics/bynk.test.event_publisher_ambiguous/) | A test body's `svc.event(e)` omits the envelope, but the event's publisher, which the default envelope names, is not one context the target declares or consumes. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.expired_needs_bearer_route`](/book/reference/diagnostics/bynk.test.expired_needs_bearer_route/) | A case drives `by Expired` at an address that is not an http route with a `Bearer` actor (alone, or as a member of an actor sum) or an `Oidc` actor; it sends a correctly signed token whose `exp` has passed, so only a `Bearer` or `Oidc` seam can refuse it. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.extra_headers_authorization`](/book/reference/diagnostics/bynk.test.extra_headers_authorization/) | A `system` case sends `Authorization` as an extra request header to a route that declares no `headers:` parameter; the case's `by <Actor>` owns the credential the driver sends. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.faults_needs_in_process`](/book/reference/diagnostics/bynk.test.faults_needs_in_process/) | A `system`-tier case claims a fault with `expect <call> faults`, or a fault claim at any tier is driven `by Nobody`, `by Forged`, `by Stale`, `by Expired`, `by WrongIssuer`, `by WrongAudience`, `by WrongAlgorithm` or `by NoSubject`; at `system` a fault reaches the case as an error response from the deployed Worker, never a throw, so the claim belongs at `unit` or `integration`. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.forged_needs_signature_route`](/book/reference/diagnostics/bynk.test.forged_needs_signature_route/) | A case drives `by Forged` at an address that is not an http route with a `Signature`, `Bearer` or `Oidc` actor, or `by Stale` at one with no `Signature` actor (alone, or as a member of an actor sum); they send a bad credential, so only a `Signature`, `Bearer` or `Oidc` seam can refuse them. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.forged_needs_system`](/book/reference/diagnostics/bynk.test.forged_needs_system/) | A case drives `by Forged`, `by Stale`, `by Expired`, `by WrongIssuer`, `by WrongAudience`, `by WrongAlgorithm` or `by NoSubject` outside a `system`-tier case; only the `system` driver signs a request or mints a token, so only there does the real `Signature`, `Bearer` or `Oidc` seam see the bad credential. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.no_subject_needs_oidc_route`](/book/reference/diagnostics/bynk.test.no_subject_needs_oidc_route/) | A case drives `by NoSubject` at an address that is not an http route with an `Oidc` actor; it sends correctly signed tokens whose `sub` is empty and whose `sub` is missing, so only an `Oidc` seam can refuse them. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.nobody_needs_secured_route`](/book/reference/diagnostics/bynk.test.nobody_needs_secured_route/) | A case drives `by Nobody` at a route with no Bearer, `Oidc` or Signature actor (alone, or as a member of an actor sum) (e.g. a public `Visitor` route) — there is no auth seam to reject the missing credential. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.peek_not_a_database`](/book/reference/diagnostics/bynk.test.peek_not_a_database/) | `D1.peek` names a capability that is not derived from `D1`; only a D1 binding has a test database to read. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.peek_not_bound`](/book/reference/diagnostics/bynk.test.peek_not_bound/) | `D1.peek` is used anywhere but as the value of a `let`; the handle is what a case reads through. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.peek_outside_case`](/book/reference/diagnostics/bynk.test.peek_outside_case/) | `D1.peek` is used outside an `integration` or `system` case; only those tiers stand up a participant's test database. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.peek_unknown_capability`](/book/reference/diagnostics/bynk.test.peek_unknown_capability/) | `D1.peek` names a participant or a capability that the suite does not have. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.peek_write`](/book/reference/diagnostics/bynk.test.peek_write/) | A `D1.peek` handle is asked to write, by a write operation (`run`, `batch`) or by a statement that would write; the handle is read-only. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.principal_claims_not_oidc`](/book/reference/diagnostics/bynk.test.principal_claims_not_oidc/) | A call-site principal passes token claims for an actor that isn't `Oidc`. |  | — |
| [`bynk.test.principal_identity_mismatch`](/book/reference/diagnostics/bynk.test.principal_identity_mismatch/) | A call-site `by <Actor>` acts as an actor whose identity is incompatible with the addressed handler's actor, or names no member of the handler's actor sum. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.principal_on_asset`](/book/reference/diagnostics/bynk.test.principal_on_asset/) | A suite's asset fetch carries a `by` clause; an asset is public and served before the Worker runs. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.principal_on_wrong_method`](/book/reference/diagnostics/bynk.test.principal_on_wrong_method/) | A wrong-method `405` test carries a `by <Actor>` clause; it reaches no handler, so a principal is meaningless. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.principal_required`](/book/reference/diagnostics/bynk.test.principal_required/) | A test drives an identity-carrying handler, or one guarded by an actor sum, with no call-site `by <Actor>`. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.remote_clock_dependent`](/book/reference/diagnostics/bynk.test.remote_clock_dependent/) | Under `bynk test --against`, a `system` case's `expect` compares an `Instant` that isn't derived from the run (a constant such as `Instant.fromEpochMillis(0)`); in process the clock starts at the epoch, but a real runtime's Workers read wall-clock time. | [`case`](/book/reference/grammar/#rule-case) | Warning |
| [`bynk.test.remote_unsupported_advance`](/book/reference/diagnostics/bynk.test.remote_unsupported_advance/) | A `system` case moves the harness clock with `advance Clock`, which a run against a real runtime (`bynk test --against`) can't honour: the deployed Workers read wall-clock time. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.remote_unsupported_asset`](/book/reference/diagnostics/bynk.test.remote_unsupported_asset/) | A `system` case fetches an asset with `svc.GET(asset)`, which a run against a real runtime (`bynk test --against`) can't honour: the fetch is answered in process. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.remote_unsupported_call`](/book/reference/diagnostics/bynk.test.remote_unsupported_call/) | A `system` case addresses an `on call` service, which a run against a real runtime (`bynk test --against`) can't honour: the service is internal, and each call is signed for one caller. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.remote_unsupported_env`](/book/reference/diagnostics/bynk.test.remote_unsupported_env/) | A `system` case's `env` disagrees with the named target it would run against (`bynk test --against`): it sets a secret, sets a name the target doesn't declare under `vars` or declares with another value, or leaves unset a name the target declares. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.remote_unsupported_peek`](/book/reference/diagnostics/bynk.test.remote_unsupported_peek/) | A `system` case reads a participant's test database with `D1.peek`, which a run against a real runtime (`bynk test --against`) can't honour: a deployment has no in-process database. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.remote_unsupported_queue`](/book/reference/diagnostics/bynk.test.remote_unsupported_queue/) | A `system` case delivers a queue message with `q.message(m)`, which a run against a real runtime (`bynk test --against`) can't honour: no public wire reaches a queue consumer. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.remote_unsupported_schedule`](/book/reference/diagnostics/bynk.test.remote_unsupported_schedule/) | A `system` case runs a cron handler with `svc.schedule(…)`, which a run against a real runtime (`bynk test --against`) can't honour: no public wire reaches a cron handler. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.service_bad_address`](/book/reference/diagnostics/bynk.test.service_bad_address/) | A test body addresses a service the wrong way for its protocol (e.g. an http route without a leading path string). | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.service_call_arity`](/book/reference/diagnostics/bynk.test.service_call_arity/) | A test body's `svc.call(...)` passes the wrong number of arguments for the service's `on call` handler. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.service_no_call_handler`](/book/reference/diagnostics/bynk.test.service_no_call_handler/) | A test body invokes `svc.call(...)` on a service with no `on call` handler (a `from http`/`cron`/`queue` service). | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.service_unknown_route`](/book/reference/diagnostics/bynk.test.service_unknown_route/) | A test body addresses an http route / cron schedule / queue message the service does not declare. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.smoke_undeclared_status`](/book/reference/diagnostics/bynk.test.smoke_undeclared_status/) | A smoke case (`bynk test --smoke`) gets a non-5xx answer whose status is not among those its route's handler declares; the case still passes (warning). | [`case`](/book/reference/grammar/#rule-case) | Warning |
| [`bynk.test.snapshot_duplicate_name`](/book/reference/diagnostics/bynk.test.snapshot_duplicate_name/) | A case takes two snapshots (`expect v matches snapshot "name"`) under one name; a snapshot is keyed by its case and name. | [`snapshot_expr`](/book/reference/grammar/#rule-snapshot_expr) | — |
| [`bynk.test.snapshot_in_property`](/book/reference/diagnostics/bynk.test.snapshot_in_property/) | A `property` takes a snapshot; its body runs once per generated input, so the snapshot belongs in a `case` over a fixed value. | [`snapshot_expr`](/book/reference/grammar/#rule-snapshot_expr) | — |
| [`bynk.test.snapshot_name_not_literal`](/book/reference/diagnostics/bynk.test.snapshot_name_not_literal/) | `matches snapshot` is not followed by a string literal; a snapshot is named by a literal so the compiler knows every snapshot a suite takes. | [`snapshot_expr`](/book/reference/grammar/#rule-snapshot_expr) | — |
| [`bynk.test.snapshot_needs_unit_suite`](/book/reference/diagnostics/bynk.test.snapshot_needs_unit_suite/) | A snapshot is taken in a `system`-tier case or in an integration or system suite; snapshots are stored per unit suite file and compared in the unit harness. | [`snapshot_expr`](/book/reference/grammar/#rule-snapshot_expr) | — |
| [`bynk.test.snapshot_not_codable`](/book/reference/diagnostics/bynk.test.snapshot_not_codable/) | The value of an `expect … matches snapshot` is not `Codable`: a snapshot stores the value's `Json.encode` text, so it must be in the domain `Json.encode` accepts. | [`snapshot_expr`](/book/reference/grammar/#rule-snapshot_expr) | — |
| [`bynk.test.stale_needs_timestamp`](/book/reference/diagnostics/bynk.test.stale_needs_timestamp/) | A case drives `by Stale` at a `Signature` route whose actor declares no `timestamp` header and `tolerance`, so there is no replay window for the seam to refuse a stale signature by. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.system_claims_need_literal`](/book/reference/diagnostics/bynk.test.system_claims_need_literal/) | A `system`-tier case passes token claims to a call-site principal other than as a literal `TokenClaims.of([…])`; the driver mints the `Oidc` token's claims from those lines, which a `TokenClaims` value does not expose. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.unknown_actor`](/book/reference/diagnostics/bynk.test.unknown_actor/) | A call-site `by <Actor>` names an actor the target context does not declare and that is not a prelude actor. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.wire_http_only`](/book/reference/diagnostics/bynk.test.wire_http_only/) | A `Wire(...)` raw argument is passed to a queue, cron, `on call` or event address; `Wire` drives an http body or path segment only, so a queue message, cron tick or event is passed as a typed value. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.wire_needs_system`](/book/reference/diagnostics/bynk.test.wire_needs_system/) | A `Wire(...)` raw argument is used outside a `system`-tier service address; `Wire` hands pre-validation input to the boundary and is meaningless at `unit` or in any other position. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.wrong_algorithm_needs_oidc_route`](/book/reference/diagnostics/bynk.test.wrong_algorithm_needs_oidc_route/) | A case drives `by WrongAlgorithm` at an address that is not an http route with an `Oidc` actor; it sends a token with `alg: none` and an `HS256` token keyed with the JWKS public key, so only an `Oidc` seam can refuse them. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.wrong_audience_needs_oidc_route`](/book/reference/diagnostics/bynk.test.wrong_audience_needs_oidc_route/) | A case drives `by WrongAudience` at an address that is not an http route with an `Oidc` actor; it sends a correctly signed token whose `aud` is not the actor's, so only an `Oidc` seam can refuse it. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.wrong_issuer_needs_oidc_route`](/book/reference/diagnostics/bynk.test.wrong_issuer_needs_oidc_route/) | A case drives `by WrongIssuer` at an address that is not an http route with an `Oidc` actor; it sends a correctly signed token whose `iss` is not the actor's, so only an `Oidc` seam can refuse it. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.ws_lifecycle_by`](/book/reference/diagnostics/bynk.test.ws_lifecycle_by/) | A case's `svc.message(tc, frame)` or `svc.close(tc)` on a `from websocket` service carries a `by` clause; the connection's principal is given once, at `open`, and carried from there. | [`case`](/book/reference/grammar/#rule-case) | — |
| [`bynk.test.ws_needs_in_process`](/book/reference/diagnostics/bynk.test.ws_needs_in_process/) | A `system`-tier case drives a `from websocket` service by address (`svc.open(tc, …)`, `svc.message(tc, frame)`, `svc.close(tc)`); a deployed WebSocket is a real upgrade through the Worker and a Durable Object, which a `system` case does not drive, so the case runs at `unit` or `integration`. | [`case`](/book/reference/grammar/#rule-case) | — |

## Transitions (step invariants)

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.transition.cross_agent_reference`](/book/reference/diagnostics/bynk.transition.cross_agent_reference/) | A transition predicate references another agent; step invariants are per-agent. |  | — |
| [`bynk.transition.duplicate_name`](/book/reference/diagnostics/bynk.transition.duplicate_name/) | An agent declares two transitions with the same name. |  | — |
| [`bynk.transition.impure_predicate`](/book/reference/diagnostics/bynk.transition.impure_predicate/) | A transition predicate uses an effectful or test-only construct; a step invariant must be pure. |  | — |
| [`bynk.transition.may_violate`](/book/reference/diagnostics/bynk.transition.may_violate/) | A handler can make a move that an agent's `transition` refuses, from a state the agent can be in after a commit; at run time that call faults with `InvariantViolation` (warning). Resolve it in code: refuse the move first, with a `match` arm or an `is` test on the state field that the path can't get past. |  | Warning |
| [`bynk.transition.no_step_reference`](/book/reference/diagnostics/bynk.transition.no_step_reference/) | A transition references neither `old` nor `new`; it constrains one state, so it is an `invariant`, not a step. |  | — |
| [`bynk.transition.not_bool`](/book/reference/diagnostics/bynk.transition.not_bool/) | A transition predicate does not have type `Bool`. |  | — |

## Type checking

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.types.argument_mismatch`](/book/reference/diagnostics/bynk.types.argument_mismatch/) | A call, method, capability, or constructor argument has the wrong type. | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.types.asset_at_boundary`](/book/reference/diagnostics/bynk.types.asset_at_boundary/) | An `Asset` type appears in a record field, boundary or store position; pass its `.url` instead. |  | — |
| [`bynk.types.bad_field_default`](/book/reference/diagnostics/bynk.types.bad_field_default/) | A record or event field's default expression (`field: T = expr`) is not a static, wire-representable value of the field's declared type — a literal (including one admitted to a refined type), a sum variant of a type this context may construct (not a `uses`-sourced commons sum, which is `bynk.context.rebrand_construction`), `Some`/`None`/`Ok`/`Err`, a record, a list literal, `Map.empty()`/`Set.empty()`, or `T.unsafe(lit)` for an opaque type whose literal also satisfies the refinement. A field whose type is a generic record's type parameter has no default, and a default may not hold for one instantiation only: for a record field that applies a user generic (`Page[String]`, or `Page[T]` inside a generic record; an event or old-state field may not) it is a closed-form value at the instantiated type, writing nothing of the enclosing record's own type parameters. (Earlier versions reported this as `bynk.event.bad_field_default`.) |  | — |
| [`bynk.types.call_arity`](/book/reference/diagnostics/bynk.types.call_arity/) | A function value, a built-in `Map`/`Set`/`Cell`/`Cache`/`Log` operation, or `unreachable`/`todo` was called with the wrong number of arguments (a declared `fn` is `bynk.resolve.arity_mismatch`). | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.types.cannot_infer_option_type_param`](/book/reference/diagnostics/bynk.types.cannot_infer_option_type_param/) | The value type of `None` could not be inferred. | [`none_expr`](/book/reference/grammar/#rule-none_expr) | — |
| [`bynk.types.cannot_infer_result_type_params`](/book/reference/diagnostics/bynk.types.cannot_infer_result_type_params/) | The type parameters of a `Result` could not be inferred. |  | — |
| [`bynk.types.catastrophic_regex`](/book/reference/diagnostics/bynk.types.catastrophic_regex/) | A `Matches` pattern, or a regex method's pattern, can match the same text in exponentially many ways (nested or ambiguous repetition, bounded or not), or cannot be analysed, risking catastrophic backtracking (ReDoS). | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.claims_at_boundary`](/book/reference/diagnostics/bynk.types.claims_at_boundary/) | `TokenClaims` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent. |  | — |
| [`bynk.types.combinator_return_mismatch`](/book/reference/diagnostics/bynk.types.combinator_return_mismatch/) | A callback passed to a combinator (`map`/`andThen`/`flatMap`/`traverseAll`/…) returns the wrong type. | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.types.constructor_arity`](/book/reference/diagnostics/bynk.types.constructor_arity/) | `T.of` (or `T.unsafe`) on a refined or opaque type got the wrong number of arguments. |  | — |
| [`bynk.types.constructor_base_mismatch`](/book/reference/diagnostics/bynk.types.constructor_base_mismatch/) | A `.of` constructor was given an argument of the wrong base type. |  | — |
| [`bynk.types.decimal_division_unscaled`](/book/reference/diagnostics/bynk.types.decimal_division_unscaled/) | `/` was used on `Decimal` operands. A quotient has no exact decimal answer, so division states its scale and rounding mode: `a.div(b, 2, RoundingMode.HalfEven)` (#2622). |  | — |
| [`bynk.types.decimal_negation_too_large`](/book/reference/diagnostics/bynk.types.decimal_negation_too_large/) | A negated decimal literal would have more than 1000 characters of canonical text once its sign is counted, the most any `Decimal` may have (#2622). |  | — |
| [`bynk.types.deprecated_json_error`](/book/reference/diagnostics/bynk.types.deprecated_json_error/) | `JsonError` is the deprecated spelling of `DecodeError` (warning; auto-fixable). |  | Warning |
| [`bynk.types.derives_automatic`](/book/reference/diagnostics/bynk.types.derives_automatic/) | A `derives` clause names `Eq`, `Key` or `Codable`, which hold for a type automatically; only `Ord` is opted into. The diagnostic carries a machine-applicable fix that removes the name (constrained-generics slice 1). | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| [`bynk.types.derives_field_not_orderable`](/book/reference/diagnostics/bynk.types.derives_field_not_orderable/) | A type declares `derives Ord`, but one of its fields or payloads is not `Ord` (for example a `Map`, a `Set`, a `Bool`, a function, or a record that does not itself derive `Ord`); every part must be orderable for the declaration order to be a total order (constrained-generics slice 1). | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| [`bynk.types.duplicate_example`](/book/reference/diagnostics/bynk.types.duplicate_example/) | A refinement carries more than one `@example`. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.duplicate_literal_arm`](/book/reference/diagnostics/bynk.types.duplicate_literal_arm/) | A `match` has two arms for the same literal value. | [`match_arm`](/book/reference/grammar/#rule-match_arm) | — |
| [`bynk.types.duplicate_pattern_field`](/book/reference/diagnostics/bynk.types.duplicate_pattern_field/) | A record pattern lists the same field twice. | [`record_pattern`](/book/reference/grammar/#rule-record_pattern) | — |
| [`bynk.types.duplicate_variant_arm`](/book/reference/diagnostics/bynk.types.duplicate_variant_arm/) | A `match` has two arms for the same variant. | [`match_arm`](/book/reference/grammar/#rule-match_arm) | — |
| [`bynk.types.embeds_ambiguous`](/book/reference/diagnostics/bynk.types.embeds_ambiguous/) | A type is embedded by more than one variant of a sum, so `?`'s conversion would be ambiguous. |  | — |
| [`bynk.types.embeds_unknown_variant`](/book/reference/diagnostics/bynk.types.embeds_unknown_variant/) | An `embeds … as V` clause names a variant the sum does not declare. |  | — |
| [`bynk.types.embeds_variant_shape`](/book/reference/diagnostics/bynk.types.embeds_variant_shape/) | An `embeds E as V` target variant must have exactly one payload field, of type `E`. |  | — |
| [`bynk.types.empty_range_pattern`](/book/reference/diagnostics/bynk.types.empty_range_pattern/) | A range pattern or `via schema(...)` range has its bounds inverted (`lo > hi`), so it can never match. | [`int_range`](/book/reference/grammar/#rule-int_range) | — |
| [`bynk.types.empty_refinement`](/book/reference/diagnostics/bynk.types.empty_refinement/) | A refinement admits no values (contradictory predicates). | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.err_value_mismatch`](/book/reference/diagnostics/bynk.types.err_value_mismatch/) | An `Err` payload has the wrong type. | [`err_expr`](/book/reference/grammar/#rule-err_expr) | — |
| [`bynk.types.example_not_literal`](/book/reference/diagnostics/bynk.types.example_not_literal/) | An `@example` is not exactly one literal of the base type. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.example_on_named_field`](/book/reference/diagnostics/bynk.types.example_on_named_field/) | An `@example` on a field whose type is a named type (it belongs on the named type). | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.example_unneeded`](/book/reference/diagnostics/bynk.types.example_unneeded/) | An `@example` on a refinement `Val` can already generate (only `Matches` and `IsoDate` take one). | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.example_violates`](/book/reference/diagnostics/bynk.types.example_violates/) | An `@example` does not satisfy its refinement's predicates. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.extensible_match_needs_wildcard`](/book/reference/diagnostics/bynk.types.extensible_match_needs_wildcard/) | A `match` covers an extensible standard-library enum (`FetchError`, `Method`) without a wildcard; it may gain variants in a minor release. |  | — |
| [`bynk.types.field_access_on_non_record`](/book/reference/diagnostics/bynk.types.field_access_on_non_record/) | Field access on a value that is not a record. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| [`bynk.types.field_annotation_unknown`](/book/reference/diagnostics/bynk.types.field_annotation_unknown/) | A record field carries an annotation other than `@name("…")`. | [`record_field`](/book/reference/grammar/#rule-record_field) | — |
| [`bynk.types.field_default_on_variant`](/book/reference/diagnostics/bynk.types.field_default_on_variant/) | A sum variant's payload field declares a default (`field: T = expr`). Only a record's fields may; a variant is built like a call, with every payload field given. |  | — |
| [`bynk.types.field_name_bad_arg`](/book/reference/diagnostics/bynk.types.field_name_bad_arg/) | A field's `@name` does not take exactly one string literal, or a `String` constant bound to one. | [`record_field`](/book/reference/grammar/#rule-record_field) | — |
| [`bynk.types.field_name_case_conflict`](/book/reference/diagnostics/bynk.types.field_name_case_conflict/) | Two fields of a `cookies` or `query` record have wire names that differ only in case. | [`record_field`](/book/reference/grammar/#rule-record_field) | — |
| [`bynk.types.field_name_duplicate`](/book/reference/diagnostics/bynk.types.field_name_duplicate/) | A field has more than one `@name`, or two fields of a request record read the same wire name. | [`record_field`](/book/reference/grammar/#rule-record_field) | — |
| [`bynk.types.field_name_invalid`](/book/reference/diagnostics/bynk.types.field_name_invalid/) | A field's `@name` is not a valid name for where the record is read (a cookie name that is not a token, or an empty name). | [`record_field`](/book/reference/grammar/#rule-record_field) | — |
| [`bynk.types.field_name_not_request_record`](/book/reference/diagnostics/bynk.types.field_name_not_request_record/) | `@name` is on a field of a record that is not the `cookies`, `query` or `@form`/`@multipart` `body` record of an HTTP handler. | [`record_field`](/book/reference/grammar/#rule-record_field) | — |
| [`bynk.types.field_name_record_not_boundary`](/book/reference/diagnostics/bynk.types.field_name_record_not_boundary/) | A record with `@name` fields is used as a JSON body, a result, a store field, an event or a call argument. | [`record_field`](/book/reference/grammar/#rule-record_field) | — |
| [`bynk.types.field_refinement_not_base`](/book/reference/diagnostics/bynk.types.field_refinement_not_base/) | An inline field refinement requires a base or refined type. | [`record_field`](/book/reference/grammar/#rule-record_field) | — |
| [`bynk.types.field_value_mismatch`](/book/reference/diagnostics/bynk.types.field_value_mismatch/) | A record field was given a value of the wrong type. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| [`bynk.types.file_at_boundary`](/book/reference/diagnostics/bynk.types.file_at_boundary/) | A `File` (a file part of a multipart request), or a type carrying one, is used where it would be persisted, sent across a boundary or returned. |  | — |
| [`bynk.types.form_uncodable`](/book/reference/diagnostics/bynk.types.form_uncodable/) | `Form.decode` or `Form.encode` names a type that is not a record of form fields (a `String`, `Int`, `Bool`, `Decimal`, an enum, or a refined or opaque type over one, optionally in an `Option` or a `List`). |  | — |
| [`bynk.types.function_at_boundary`](/book/reference/diagnostics/bynk.types.function_at_boundary/) | A function type appeared in a serialisable or boundary position (a record field, sum payload, service/agent handler signature, capability operation signature, agent state field, or agent key); functions cannot serialise or cross a boundary. | [`function_type_ref`](/book/reference/grammar/#rule-function_type_ref) | — |
| [`bynk.types.guard_not_bool`](/book/reference/diagnostics/bynk.types.guard_not_bool/) | A match-arm `if` guard is not a `Bool` expression. | [`match_arm`](/book/reference/grammar/#rule-match_arm) | — |
| [`bynk.types.headers_at_boundary`](/book/reference/diagnostics/bynk.types.headers_at_boundary/) | `RequestHeaders` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent. |  | — |
| [`bynk.types.held_at_boundary`](/book/reference/diagnostics/bynk.types.held_at_boundary/) | A held value (`Connection[F]`) appears in a serialisable or boundary position — a held resource is built and disposed in place, never persisted or sent across a boundary (§2.9, real-time track slice 2). |  | — |
| [`bynk.types.held_not_comparable`](/book/reference/diagnostics/bynk.types.held_not_comparable/) | A held value (`Connection[F]`) is compared with `==`/`!=` — held values have identity, not value-equality (§2.9.3, real-time track slice 2). |  | — |
| [`bynk.types.if_branch_mismatch`](/book/reference/diagnostics/bynk.types.if_branch_mismatch/) | The branches of an `if` have different types. | [`if_expr`](/book/reference/grammar/#rule-if_expr) | — |
| [`bynk.types.if_non_bool_cond`](/book/reference/diagnostics/bynk.types.if_non_bool_cond/) | An `if` condition is not a `Bool`. | [`if_expr`](/book/reference/grammar/#rule-if_expr) | — |
| [`bynk.types.if_without_else_requires_unit`](/book/reference/diagnostics/bynk.types.if_without_else_requires_unit/) | An `if` with no `else` branch has a non-unit then-branch; the missing else defaults to `()`, so the branch must be `()` or `Effect[()]`. | [`if_expr`](/book/reference/grammar/#rule-if_expr) | — |
| [`bynk.types.interpolation_non_scalar`](/book/reference/diagnostics/bynk.types.interpolation_non_scalar/) | An interpolation hole holds a value with no string form. |  | — |
| [`bynk.types.invalid_regex`](/book/reference/diagnostics/bynk.types.invalid_regex/) | A `Matches` predicate, or a regex method's pattern (`containsRegex`, …), contains an invalid regular expression or malformed inline flags. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.inverted_range`](/book/reference/diagnostics/bynk.types.inverted_range/) | An `InRange` predicate has its bounds inverted. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.is_base_mismatch`](/book/reference/diagnostics/bynk.types.is_base_mismatch/) | An `is` refinement check is applied to a value of the wrong base type. | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| [`bynk.types.is_literal_pattern`](/book/reference/diagnostics/bynk.types.is_literal_pattern/) | A literal was used on the right of `is`; `is` tests type/refinement, not value equality (use `==`). | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| [`bynk.types.is_non_sum`](/book/reference/diagnostics/bynk.types.is_non_sum/) | `is` was applied to a value that is not a sum type. | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| [`bynk.types.is_range_pattern`](/book/reference/diagnostics/bynk.types.is_range_pattern/) | A range pattern was used directly on the right of `is`; `is` tests type/refinement, not value ranges (use `match`, a guard or `InRange`). | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| [`bynk.types.is_refined_pattern`](/book/reference/diagnostics/bynk.types.is_refined_pattern/) | A refined (`where`) pattern was used on the right of `is`; refined patterns are `match`-only. | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| [`bynk.types.is_unknown_variant`](/book/reference/diagnostics/bynk.types.is_unknown_variant/) | `is` names a variant the type does not have. | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| [`bynk.types.json_uncodable`](/book/reference/diagnostics/bynk.types.json_uncodable/) | A `Json.encode`/`Json.decode` target type cannot pass through the typed JSON codec (functions, effects, streams or a type that carries one, `bynk.crypto` keys, error builtins). | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.types.key_at_boundary`](/book/reference/diagnostics/bynk.types.key_at_boundary/) | A `bynk.crypto` `SigningKey` or `VerifyKey` appears in a storable or boundary-crossing position — a key is held non-extractable and lives for the call that imported it, never persisted, logged or sent (standard-library K4). |  | — |
| [`bynk.types.key_not_orderable`](/book/reference/diagnostics/bynk.types.key_not_orderable/) | A `sortBy`/`sortByDescending`/`min`/`max` key function does not return an orderable type. The key must be an `Ord` type, on a `List` and on a storage `Query` alike. |  | — |
| [`bynk.types.lambda_mismatch`](/book/reference/diagnostics/bynk.types.lambda_mismatch/) | A lambda's parameter count, parameter annotations, or body type do not match the expected function type. | [`lambda_expr`](/book/reference/grammar/#rule-lambda_expr) | — |
| [`bynk.types.let_annotation_mismatch`](/book/reference/diagnostics/bynk.types.let_annotation_mismatch/) | A `let` value does not match its type annotation. | [`let_stmt`](/book/reference/grammar/#rule-let_stmt) | — |
| [`bynk.types.list_element_mismatch`](/book/reference/diagnostics/bynk.types.list_element_mismatch/) | A list-literal element has a different type from the list's element type. | [`list_literal`](/book/reference/grammar/#rule-list_literal) | — |
| [`bynk.types.list_pattern_on_non_list`](/book/reference/diagnostics/bynk.types.list_pattern_on_non_list/) | A list pattern is matched against a value that is not a `List` (a record, sum, base type, `Set` or `Map`). | [`list_pattern`](/book/reference/grammar/#rule-list_pattern) | — |
| [`bynk.types.match_arm_mismatch`](/book/reference/diagnostics/bynk.types.match_arm_mismatch/) | A `match` arm has a different type from the others. | [`match_arm`](/book/reference/grammar/#rule-match_arm) | — |
| [`bynk.types.match_non_sum_discriminant`](/book/reference/diagnostics/bynk.types.match_non_sum_discriminant/) | `match` was applied to a value that is not a sum type. | [`match_expr`](/book/reference/grammar/#rule-match_expr) | — |
| [`bynk.types.method_arity`](/book/reference/diagnostics/bynk.types.method_arity/) | A method was called with the wrong number of arguments. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.types.method_not_found`](/book/reference/diagnostics/bynk.types.method_not_found/) | Called a method the type does not have. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.types.method_on_non_named_type`](/book/reference/diagnostics/bynk.types.method_on_non_named_type/) | A method was called on a built-in type that has no methods. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.types.mixed_pattern_bindings`](/book/reference/diagnostics/bynk.types.mixed_pattern_bindings/) | A pattern mixes named and positional bindings. | [`variant_pattern`](/book/reference/grammar/#rule-variant_pattern) | — |
| [`bynk.types.negative_length`](/book/reference/diagnostics/bynk.types.negative_length/) | A length predicate was given a negative value. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.never_inferred`](/book/reference/diagnostics/bynk.types.never_inferred/) | An unannotated `let` or `<-` binding has a type that is, or contains, the bottom type (`never returns`) because its value only faults (a faulting lambda bound this way included); the type cannot be written in source, so an annotation is needed. | [`let_stmt`](/book/reference/grammar/#rule-let_stmt) | — |
| [`bynk.types.no_numeric_coercion`](/book/reference/diagnostics/bynk.types.no_numeric_coercion/) | `Int`, `Float` and `Decimal` were mixed without an explicit conversion — in an operation, a comparison or refinement bounds. | [`binary_expr`](/book/reference/grammar/#rule-binary_expr), [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.non_exhaustive_match`](/book/reference/diagnostics/bynk.types.non_exhaustive_match/) | A `match` does not cover every value. | [`match_expr`](/book/reference/grammar/#rule-match_expr) | — |
| [`bynk.types.not_codable`](/book/reference/diagnostics/bynk.types.not_codable/) | A type is instantiated at a `Codable`-bounded parameter (of a generic function, or of a capability operation) but is outside the domain `Json.decode` accepts: top-level `()`, a function, `Effect`, `Query`, a `Stream` or a type that carries one, a connection, the HTTP and queue result types, `Asset`, request headers, token claims, or an actor (constrained-generics slice 3). |  | — |
| [`bynk.types.not_comparable`](/book/reference/diagnostics/bynk.types.not_comparable/) | A value compared with `==`/`!=` contains a function, `Effect`, or `Query` somewhere inside it — those have no value equality, so the type is not equality-supporting (type-system §2.3.5, #1652). |  | — |
| [`bynk.types.not_keyable`](/book/reference/diagnostics/bynk.types.not_keyable/) | A `Map` key, `Set` element, or `distinct`/`distinctBy`/`groupBy`/join key, or an argument to a `Key`-bounded parameter, is not `Key`. `String`, `Int`, `Bool`, `Instant`, `Duration`, `Bytes`, `Option`/`List` of a key type, refined and opaque types over them, and records, sums and enums made only of key types are; a `Float` is never one, at any depth, because `NaN` is not equal to itself (constrained-generics slice 2). | [`generic_type_ref`](/book/reference/grammar/#rule-generic_type_ref) | — |
| [`bynk.types.not_orderable`](/book/reference/diagnostics/bynk.types.not_orderable/) | A value is ordered (`<`, `<=`, `>`, `>=`), or passed to an `Ord`-bounded parameter, but its type is not `Ord`. `Int`, `Float`, `String`, `Duration`, `Instant`, `Bytes`, `Option`/`List` of an orderable type, refined types over those, and types that declare `derives Ord` are; adding `derives Ord` is offered as a fix that is never applied automatically, because it makes declaration order significant (constrained-generics slice 1). |  | — |
| [`bynk.types.ok_value_mismatch`](/book/reference/diagnostics/bynk.types.ok_value_mismatch/) | An `Ok` payload has the wrong type. | [`ok_expr`](/book/reference/grammar/#rule-ok_expr) | — |
| [`bynk.types.opaque_raw_outside`](/book/reference/diagnostics/bynk.types.opaque_raw_outside/) | `.raw` on an opaque type was used outside its defining commons. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| [`bynk.types.opaque_unsafe_outside`](/book/reference/diagnostics/bynk.types.opaque_unsafe_outside/) | `.unsafe` on an opaque type was used outside its defining context. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| [`bynk.types.or_pattern_binding_mismatch`](/book/reference/diagnostics/bynk.types.or_pattern_binding_mismatch/) | An or-pattern's alternatives don't all bind the same set of names. | [`match_arm`](/book/reference/grammar/#rule-match_arm), [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| [`bynk.types.or_pattern_type_mismatch`](/book/reference/diagnostics/bynk.types.or_pattern_type_mismatch/) | An or-pattern's alternatives give a shared binding different types (or refinements). | [`match_arm`](/book/reference/grammar/#rule-match_arm), [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| [`bynk.types.pattern_arity`](/book/reference/diagnostics/bynk.types.pattern_arity/) | A pattern binds the wrong number of payload fields. | [`variant_pattern`](/book/reference/grammar/#rule-variant_pattern) | — |
| [`bynk.types.pattern_type_mismatch`](/book/reference/diagnostics/bynk.types.pattern_type_mismatch/) | A pattern's type does not match the matched value. | [`variant_pattern`](/book/reference/grammar/#rule-variant_pattern) | — |
| [`bynk.types.polynomial_regex`](/book/reference/diagnostics/bynk.types.polynomial_regex/) | A `Matches` pattern can split the same text between repetitions, so matching time grows polynomially with input length, and no length predicate bounds the input small enough for its degree; a regex method's pattern has no bound to excuse it. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.polynomial_regex_capped`](/book/reference/diagnostics/bynk.types.polynomial_regex_capped/) | A `Matches` pattern's matching time grows polynomially with input length, but a `MaxLength` or `Length` predicate bounds the input small enough for its degree (warning). | [`refinement`](/book/reference/grammar/#rule-refinement) | Warning |
| [`bynk.types.predicate_base_mismatch`](/book/reference/diagnostics/bynk.types.predicate_base_mismatch/) | A predicate does not apply to the type's base (e.g. a string predicate on an `Int`). | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.query_at_boundary`](/book/reference/diagnostics/bynk.types.query_at_boundary/) | A `Query` type appears in a storable or boundary-crossing position — a query is built and executed in place, never persisted or sent (ADR 0115). |  | — |
| [`bynk.types.question_error_mismatch`](/book/reference/diagnostics/bynk.types.question_error_mismatch/) | `?` propagates an error type incompatible with the function's. | [`question_expr`](/book/reference/grammar/#rule-question_expr) | — |
| [`bynk.types.question_on_non_result`](/book/reference/diagnostics/bynk.types.question_on_non_result/) | `?` was applied to a non-`Result` value. | [`question_expr`](/book/reference/grammar/#rule-question_expr) | — |
| [`bynk.types.question_option_outside_http`](/book/reference/diagnostics/bynk.types.question_option_outside_http/) | `?` lifts an `Option` only inside a handler returning `HttpResult` (`None` becomes `NotFound`); elsewhere use `.okOr(err)`. | [`question_expr`](/book/reference/grammar/#rule-question_expr) | — |
| [`bynk.types.question_outside_result`](/book/reference/diagnostics/bynk.types.question_outside_result/) | `?` used in a function that does not return a `Result`. | [`question_expr`](/book/reference/grammar/#rule-question_expr) | — |
| [`bynk.types.range_pattern_on_non_int`](/book/reference/diagnostics/bynk.types.range_pattern_on_non_int/) | A range pattern (`lo..hi`) was matched against a scrutinee that is not an `Int`. | [`int_range`](/book/reference/grammar/#rule-int_range) | — |
| [`bynk.types.record_pattern_on_non_record`](/book/reference/diagnostics/bynk.types.record_pattern_on_non_record/) | A record pattern names a type that is not a record (a sum, enum, refined or opaque type, or a built-in). | [`record_pattern`](/book/reference/grammar/#rule-record_pattern) | — |
| [`bynk.types.regex_not_literal`](/book/reference/diagnostics/bynk.types.regex_not_literal/) | A regex method's pattern (`matchesRegex`, `containsRegex`, `findRegex`, `findAllRegex`, `replaceRegex`, `splitRegex`) is not a string literal, so its ReDoS check cannot run at compile time. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.types.rejection_at_boundary`](/book/reference/diagnostics/bynk.types.rejection_at_boundary/) | `BoundaryRejection` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent. |  | — |
| [`bynk.types.request_cookies_at_boundary`](/book/reference/diagnostics/bynk.types.request_cookies_at_boundary/) | `RequestCookies` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent. |  | — |
| [`bynk.types.request_query_at_boundary`](/book/reference/diagnostics/bynk.types.request_query_at_boundary/) | `RequestQuery` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent. |  | — |
| [`bynk.types.return_mismatch`](/book/reference/diagnostics/bynk.types.return_mismatch/) | A returned value does not match the declared return type. |  | — |
| [`bynk.types.rounding_mode_position`](/book/reference/diagnostics/bynk.types.rounding_mode_position/) | `RoundingMode.<mode>` was used outside the mode argument of a `Decimal`'s `round` or `div`. A mode is read where the rounding happens and is not a first-class value (#2622). |  | — |
| [`bynk.types.select_tail_has_select`](/book/reference/diagnostics/bynk.types.select_tail_has_select/) | A `D1`, `Postgres` or `MySql` `select[T]` or `firstAs[T]` statement does not start at `FROM` (for example it begins with `SELECT`, `WITH` or a parenthesis); the compiler writes the `SELECT` and column list from `T`. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.types.sequence_needs_result`](/book/reference/diagnostics/bynk.types.sequence_needs_result/) | `sequenceAll`/`sequenceTry` was called on a list whose elements are not `Result`s. They collect a `List[Result[T, E]]`. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.types.some_value_mismatch`](/book/reference/diagnostics/bynk.types.some_value_mismatch/) | A `Some` payload has the wrong type. | [`some_expr`](/book/reference/grammar/#rule-some_expr) | — |
| [`bynk.types.sql_not_literal`](/book/reference/diagnostics/bynk.types.sql_not_literal/) | A `D1` or `Postgres` statement's SQL (the first argument of `all`, `first`, `run`, `select` or `firstAs`, or the `sql` of a `SqlStatement` or a `PgStatement`) is neither a string literal nor a bare reference to a `String` constant bound to one, so data could be spliced into it instead of bound as a `SqlValue` or `PgValue` parameter (standard-library P4c, Hyperdrive H1 and H3). | [`method_call`](/book/reference/grammar/#rule-method_call), [`record_construction`](/book/reference/grammar/#rule-record_construction), [`record_spread`](/book/reference/grammar/#rule-record_spread) | — |
| [`bynk.types.sql_row_shape`](/book/reference/diagnostics/bynk.types.sql_row_shape/) | `Json.decodeSql[T]`'s (or `select[T]`'s, `firstAs[T]`'s) `T` is not a SQL row: a record whose fields are `Int`, `Float`, `String`, `Bool` or `Bytes` (or a refined or opaque type over one, or an `Option` of either), or a `List` of such records (standard-library P4c). A `Postgres` row may also hold a `Decimal` or an `Instant` (Hyperdrive H3). | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| [`bynk.types.sql_statement_at_boundary`](/book/reference/diagnostics/bynk.types.sql_statement_at_boundary/) | A `bynk.cloudflare` `SqlStatement`, `PgStatement` or `MySqlStatement` appears in a storable or boundary-crossing position, or as a `Json` codec target — a statement from outside would carry SQL that was never a literal (standard-library P4c). |  | — |
| [`bynk.types.stream_at_boundary`](/book/reference/diagnostics/bynk.types.stream_at_boundary/) | A `Stream` type, or a record or sum carrying one in a field, appears in a storable or boundary-crossing position — a stream is a live value-over-time source, never persisted or sent across a boundary (real-time track slice 0; #2623 admits it in capability signatures and record fields). |  | — |
| [`bynk.types.stream_not_comparable`](/book/reference/diagnostics/bynk.types.stream_not_comparable/) | A `Stream` value is compared with `==`/`!=` — a stream is a live value-over-time source, not a comparable value (real-time track slice 0). |  | — |
| [`bynk.types.type_mismatch`](/book/reference/diagnostics/bynk.types.type_mismatch/) | An operator or built-in method got an operand of a type it does not accept, or two operands that had to match did not. |  | — |
| [`bynk.types.uninferable_element_type`](/book/reference/diagnostics/bynk.types.uninferable_element_type/) | An empty `[]` (or `List.empty()` / `Map.empty()`) has no expected type to infer its element type from. | [`list_literal`](/book/reference/grammar/#rule-list_literal) | — |
| [`bynk.types.unknown_annotation`](/book/reference/diagnostics/bynk.types.unknown_annotation/) | An annotation after a refinement is not `@example`. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.unknown_field`](/book/reference/diagnostics/bynk.types.unknown_field/) | Referenced a field the record type does not declare. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| [`bynk.types.unknown_pattern_field`](/book/reference/diagnostics/bynk.types.unknown_pattern_field/) | A pattern names a field the variant does not have. | [`variant_pattern`](/book/reference/grammar/#rule-variant_pattern) | — |
| [`bynk.types.unknown_rounding_mode`](/book/reference/diagnostics/bynk.types.unknown_rounding_mode/) | A `Decimal` `round` or `div` was not given a rounding mode written `RoundingMode.<mode>`; the modes are `Down`, `Up`, `Floor`, `Ceiling`, `HalfUp`, `HalfDown` and `HalfEven` (#2622). |  | — |
| [`bynk.types.unknown_static_member`](/book/reference/diagnostics/bynk.types.unknown_static_member/) | A name after `HttpResult.` that is not one of its variants. An unknown member of a type you declared is `bynk.resolve.unknown_static_member`. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| [`bynk.types.unknown_variant_in_pattern`](/book/reference/diagnostics/bynk.types.unknown_variant_in_pattern/) | A pattern names a variant the sum type does not have. | [`variant_pattern`](/book/reference/grammar/#rule-variant_pattern) | — |
| [`bynk.types.unreachable_arm`](/book/reference/diagnostics/bynk.types.unreachable_arm/) | A `match` arm is unreachable. | [`match_arm`](/book/reference/grammar/#rule-match_arm) | — |
| [`bynk.types.variant_arity`](/book/reference/diagnostics/bynk.types.variant_arity/) | A variant constructor got the wrong number of arguments for its payload fields (a pattern with the wrong number of bindings is `bynk.types.pattern_arity`). |  | — |
| [`bynk.types.variant_missing_payload`](/book/reference/diagnostics/bynk.types.variant_missing_payload/) | A variant requiring a payload was used without one. |  | — |
| [`bynk.types.variant_payload_mismatch`](/book/reference/diagnostics/bynk.types.variant_payload_mismatch/) | A variant payload has the wrong type. |  | — |

## Uses

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.uses.name_conflict`](/book/reference/diagnostics/bynk.uses.name_conflict/) | Two commons a unit `uses` declare the same type, function, constant, asset or embed; or a value crosses between a unit's code and an imported declaration's position whose type the unit shadows; or a reference names an import the unit's own declaration shadows. | [`uses_decl`](/book/reference/grammar/#rule-uses_decl) | — |
| [`bynk.uses.self_reference`](/book/reference/diagnostics/bynk.uses.self_reference/) | A commons `uses` itself. | [`uses_decl`](/book/reference/grammar/#rule-uses_decl) | — |
| [`bynk.uses.target_is_context`](/book/reference/diagnostics/bynk.uses.target_is_context/) | `uses` targets a context instead of a commons. | [`uses_decl`](/book/reference/grammar/#rule-uses_decl) | — |
| [`bynk.uses.unknown_commons`](/book/reference/diagnostics/bynk.uses.unknown_commons/) | `uses` names a commons that does not exist. | [`uses_decl`](/book/reference/grammar/#rule-uses_decl) | — |

## Value fabrication

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.val.agent_not_generable`](/book/reference/diagnostics/bynk.val.agent_not_generable/) | A `for all`/`Val` cannot generate an agent — fabricated agent states need not be reachable. | [`for_all`](/book/reference/grammar/#rule-for_all) | — |
| [`bynk.val.arity`](/book/reference/diagnostics/bynk.val.arity/) | `Val[T]` was given the wrong number of pin arguments. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| [`bynk.val.generic_type`](/book/reference/diagnostics/bynk.val.generic_type/) | A `Val[…]` fabricates a value of a generic type; per-instantiation value fabrication is not yet wired (ADR 0197). The type itself may cross a boundary through its monomorphised codec (v0.174). Named `bynk.generics.generic_record_at_boundary` before #3379. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| [`bynk.val.literal_violates`](/book/reference/diagnostics/bynk.val.literal_violates/) | A pinned `Val[T]` value violates the type's refinement. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| [`bynk.val.needs_pin`](/book/reference/diagnostics/bynk.val.needs_pin/) | A bare `Val[T]` cannot generate a value (e.g. a `Matches` string); pin one. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| [`bynk.val.outside_test`](/book/reference/diagnostics/bynk.val.outside_test/) | `Val[T]` was used outside a test case body. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| [`bynk.val.pin_not_literal`](/book/reference/diagnostics/bynk.val.pin_not_literal/) | A `Val[T]` pin argument is not a compile-time literal. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| [`bynk.val.pin_unsupported`](/book/reference/diagnostics/bynk.val.pin_unsupported/) | A pin was given for a type kind that does not support pinning. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| [`bynk.val.unknown_type`](/book/reference/diagnostics/bynk.val.unknown_type/) | `Val[T]` names a type that does not resolve. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| [`bynk.val.unsupported_kind`](/book/reference/diagnostics/bynk.val.unsupported_kind/) | `Val[T]` cannot fabricate a value for this kind of type. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |

## npm requirements

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.requires.conflicting_range`](/book/reference/diagnostics/bynk.requires.conflicting_range/) | Two adapter `binding … requires { … }` entries give different version ranges for one npm package. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| [`bynk.requires.unapproved`](/book/reference/diagnostics/bynk.requires.unapproved/) | A reached adapter of a git or registry dependency requires an npm package, at a range, that the project hasn't approved in `bynk.lock`; review it and run `bynk approve <package>`. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| [`bynk.requires.unpinned_dependency`](/book/reference/diagnostics/bynk.requires.unpinned_dependency/) | An adapter `binding … requires { … }` entry has an unpinned version range. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
