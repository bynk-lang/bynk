---
title: Diagnostic index
---
<!-- GENERATED FILE — do not edit by hand.
     Source: bynkc/src/diagnostics.rs (`render_markdown`).
     Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it diagnostics_registry:: -->

Every diagnostic code the compiler can emit, with a one-line summary of the cause, grouped by category. For step-by-step cause-and-fix guidance on the most common ones, see the [troubleshooting guides](/book/troubleshooting/). For a code with a curated explanation, `bynk explain <code>` prints it in a terminal, and in VS Code the diagnostic's **Explain** quick fix (or **Bynk: Explain Diagnostic Code…**) shows the same text in a panel, offline.

There are **640** codes in total.

## Agents

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.agent.class_conflict` | An agent class has more than one fate: it is renamed twice, both renamed and removed, or renamed onto a class that already has its own recorded history in `bynk.schema.lock`. |  | — |
| `bynk.agent.class_removed` | An agent class recorded in `bynk.schema.lock` is no longer declared, so a deployment's stored instances of it would be orphaned. If the class was never deployed, delete its `[agents]` entry from the lock; renaming or removing a deployed class gets its own declaration in a later state-migrations slice. |  | — |
| `bynk.agent.construction_arity` | An agent was constructed with the wrong number of key arguments. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| `bynk.agent.handler_arity` | An agent handler was called with the wrong number of arguments. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| `bynk.agent.handler_not_found` | Called a handler the agent does not declare. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| `bynk.agent.key_mismatch` | An agent key argument has the wrong type. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| `bynk.agent.migrate_below_floor` | A `migrate` step reads a schema version below the agent's floor. A complete `bynk migrate sweep` raised the floor past it, so no stored instance can hold that version: the step emits nothing and can be deleted (warning). |  | Warning |
| `bynk.agent.migrate_duplicate` | Two `migrate` steps in one agent read the same schema version. |  | — |
| `bynk.agent.migrate_duplicate_assignment` | A `migrate` step assigns the same `store` field more than once; each field takes at most one value per step (record-literal semantics). |  | — |
| `bynk.agent.migrate_impure` | A `migrate` step uses an effect, a capability, `?`, `expect`, or the agent's `key`; steps are pure and key-free so a migration sweep can run them by object ID. |  | — |
| `bynk.agent.migrate_incomplete` | A `migrate` step does not account for every change between its version and the current shape: a removed field must be read (`old.f`) or dropped, and a retyped or re-kinded field must be assigned (or dropped, for a `Set`, `Log` or `Cache` field). |  | — |
| `bynk.agent.migrate_shape_mismatch` | A `migrate` step's declared old shape (or a `drop`) names a field the lock does not record at that version, or records with a different structure. |  | — |
| `bynk.agent.migrate_unknown_version` | A `migrate` step reads a schema version the lock does not record below the agent's current version. |  | — |
| `bynk.agent.non_additive_state_change` | An agent's stored state changed in a way deployed data cannot be read through — a `store` field was removed, retyped (including inside a stored record or sum), or changed storage kind. A renamed field is a removed field plus an added one. If the class was never deployed, delete its `[agents]` entry from `bynk.schema.lock`; otherwise widen rather than narrow, or stage the change through a new field. |  | — |
| `bynk.agent.outside_context` | An `agent` was declared outside a context. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| `bynk.agent.refinement_changed` | A stored `store` field's refinement changed while its structure did not. A widening is safe; a narrowing faults on load for any stored value that no longer satisfies it (`RehydrationViolation`), so migrate the data first (warning). |  | Warning |
| `bynk.agent.removed_live` | A `removed agent` declaration names a class that is still declared; removing a class deletes its stored data. |  | — |
| `bynk.agent.replaces_live` | `agent New replaces Old` names an `Old` that is still declared; a rename moves the old class's data to the new one. |  | — |
| `bynk.agent.replaces_unknown` | `agent New replaces Old` names an `Old` that `bynk.schema.lock` does not record for this context, so there is no recorded class to rename. |  | — |
| `bynk.agent.return_not_effect` | An agent handler's return type is not an `Effect`. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| `bynk.agent.state_name_conflict` | A type is named `<Agent>State` beside agent `<Agent>`; that name is the agent's state record (its `Cell` fields), which its handlers and invariants see in place of the type. |  | — |
| `bynk.agent.version_without_path` | A schema version between the agent's floor and its current version has no way forward: no `migrate` step covers it and its path to the current shape is not entirely additive, so a stored record at that version faults on load with `NoMigrationPath` (warning). |  | Warning |
| `bynk.agents.bad_state_initialiser` | An agent `store` field initialiser is not a static value of the field's type. | [`store_field`](/book/reference/grammar/#rule-store_field) | — |
| `bynk.agents.non_zeroable_state_field` | An agent `store` field has no initialiser and no implicit zero value. | [`store_field`](/book/reference/grammar/#rule-store_field) | — |

## Boundaries

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.boundary.ambiguous_type` | A value's type is declared, with different shapes, by more than one consumed context, so its fields cannot be read. |  | — |
| `bynk.boundary.structural_mismatch` | Data crossing a context boundary did not match the expected shape. |  | — |

## Capabilities

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.capability.base_not_instantiable` | A derived capability's base is not a platform resource that can be instantiated. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| `bynk.capability.derived_outside_context` | A derived capability was declared outside a context. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| `bynk.capability.instance_only` | A platform capability that is used only through named instances (`Service`, `R2`, `RateLimit`, `EdgeCache`) was used bare, in a `given` clause. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| `bynk.capability.invalid_option` | A derived capability's option is not one string literal, is given twice, or (`@worker`) is not a Worker name outside the project. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| `bynk.capability.missing_option` | A derived capability lacks an option its base requires, such as a `Service` instance's `@worker`. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| `bynk.capability.not_provided` | On the Workers target, a capability a context requires has no provider in it. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| `bynk.capability.op_arity` | A capability operation was called with the wrong number of arguments. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| `bynk.capability.outside_context` | A `capability` was declared outside a context. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| `bynk.capability.provider_agent_cross_context` | On the Workers target, a context requires a capability whose provider, in another context, constructs that context's agents; the consumer's Worker would build the provider without a binding for those agents' Durable Objects. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| `bynk.capability.unknown_base` | A derived capability's base is not a capability the context consumes. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| `bynk.capability.unknown_operation` | Referenced an operation the capability does not declare. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| `bynk.capability.unknown_option` | A derived capability carries an option its base does not take. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |

## Cloudflare

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.cloudflare.edge_cache_duplicate` | A context declares a second `EdgeCache` instance; the edge cache is one setting for the whole Worker. |  | — |
| `bynk.cloudflare.edge_cache_exported` | An exported capability reaches an `EdgeCache` instance through its provider's `given`; a consuming context would build it in its own Worker, whose purge misses this one's cache. |  | — |
| `bynk.cloudflare.edge_cache_in_agent` | An agent handler reaches an `EdgeCache` instance, directly or through a provider's `given`; an agent's purge would not reach the Worker's cache. |  | — |
| `bynk.cloudflare.edge_cache_in_queue` | A queue consumer reaches an `EdgeCache` instance; purge from a service handler instead. |  | — |
| `bynk.cloudflare.edge_cache_in_schedule` | An `on schedule` handler reaches an `EdgeCache` instance; a cron run's purge is accepted but clears nothing from the fetch cache, so purge from a service handler. |  | — |
| `bynk.cloudflare.edge_cache_public_authenticated` | In a context with `EdgeCache`, a `@cache(scope: public)` route is served by an actor that authenticates; its response would be shared at the edge with every caller. |  | — |

## Consumes

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.consumes.alias_conflict` | Two `consumes` aliases collide. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.consumes.capability_name_clash` | Two flattened `consumes U { Cap }` capabilities collide, or one clashes with a local capability. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.consumes.name_conflict` | A `consumes` name collides with another name in scope. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.consumes.self_reference` | A context `consumes` itself. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.consumes.service_arity` | A consumed service was called with the wrong number of arguments. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.consumes.signature_type_unconsumed` | A cross-context call reaches a service whose signature names a type the consumed context itself consumes from a third context, which the caller does not consume. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.consumes.target_is_commons` | `consumes` targets a `commons` instead of a context. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.consumes.unknown_context` | `consumes` names a context that does not exist. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.consumes.unknown_service` | Called a service the consumed context does not declare. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |

## Contexts

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.context.consumes_cycle` | Contexts form a `consumes` dependency cycle. |  | — |
| `bynk.context.duplicate_binding` | A Worker would declare two bindings with the same name, such as an agent `Kv` and the `bynk.cloudflare` KV namespace (both `KV`). |  | — |
| `bynk.context.external_construction` | A context-owned type was constructed from outside that context. |  | — |
| `bynk.context.external_provider` | A bodiless (external) provider was declared outside an `adapter`. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.context.opaque_inspection` | An opaquely-exported type was inspected from outside its context. |  | — |
| `bynk.context.rebrand_construction` | A variant of a `uses`-sourced commons sum type was constructed directly inside a context, where the emitter's per-context rebrand leaves its constructors out of scope. |  | — |

## Contracts

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.contract.duplicate_name` | A function declares two contract clauses (`requires`/`ensures`) with the same name. |  | — |
| `bynk.contract.impure_predicate` | A contract predicate uses an effectful or test-only construct; a contract clause must be pure. |  | — |
| `bynk.contract.not_bool` | A contract predicate does not have type `Bool`. |  | — |
| `bynk.contract.restated_by_test` | A `case`/`property` merely restates a contract clause already declared at the function; the test is redundant. |  | — |
| `bynk.contract.result_in_requires` | A precondition (`requires`) references `result`; the return value is only in scope inside an `ensures`. |  | — |

## Cron

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.cron.bad_params` | A cron handler declares more than one parameter, or a non-`Int` one. | [`cron_handler`](/book/reference/grammar/#rule-cron_handler) | — |
| `bynk.cron.duplicate_schedule` | Two cron handlers declare the same schedule. | [`cron_handler`](/book/reference/grammar/#rule-cron_handler) | — |
| `bynk.cron.invalid_schedule` | A cron expression is not five whitespace-separated fields. | [`cron_handler`](/book/reference/grammar/#rule-cron_handler) | — |
| `bynk.cron.return_not_effect_result` | A cron handler does not return `Effect[Result[(), E]]`. | [`cron_handler`](/book/reference/grammar/#rule-cron_handler) | — |

## Doc comments

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.doc.unresolved_link` | A `[Name]` doc link resolves to nothing in the unit's scope (editor hint only). |  | Warning |

## Effects

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.effect.bind_in_pure_context` | An `<-` bind was used in a pure (non-effectful) context. | [`effect_let_stmt`](/book/reference/grammar/#rule-effect_let_stmt) | — |
| `bynk.effect.bind_on_non_effect` | An `<-` bind was applied to a non-`Effect` value. | [`effect_let_stmt`](/book/reference/grammar/#rule-effect_let_stmt) | — |
| `bynk.effect.capability_in_pure_context` | A capability was used in a pure context. |  | — |
| `bynk.effect.cross_context_in_pure_context` | A cross-context call was made in a pure context. |  | — |
| `bynk.effect.do_in_pure_context` | A `do` statement was used in a pure (non-effectful) context. | [`do_stmt`](/book/reference/grammar/#rule-do_stmt) | — |
| `bynk.effect.do_on_non_effect` | A `do` statement was applied to a non-`Effect` value. | [`do_stmt`](/book/reference/grammar/#rule-do_stmt) | — |
| `bynk.effect.do_requires_unit` | A `do` statement was applied to a valued `Effect[T]`; `do` performs a unit effect, so a real result would be dropped — use `let _ <- e` instead. | [`do_stmt`](/book/reference/grammar/#rule-do_stmt) | — |
| `bynk.effect.fn_value_in_pure_context` | An effectful function value was called in a pure context; like a capability call, it is legal only where the enclosing body is effectful. | [`call`](/book/reference/grammar/#rule-call) | — |
| `bynk.effect.unbound_effect` | An `Effect` value in an effectful body was built but not bound with `<-`, sequenced with `do` or returned — in a plain `let`, a list element or a constructor payload — so it runs eagerly and unawaited (#1658). |  | — |

## Embedded text

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.embed.binary_content` | An `embed` names a file with a NUL byte, so it is binary, not text. |  | — |
| `bynk.embed.duplicate` | An `embed` name is declared more than once in the same unit. |  | — |
| `bynk.embed.invalid_encoding` | An `embed` names a file that is not valid UTF-8. |  | — |
| `bynk.embed.missing_file` | An `embed` names a file that is not in the project's `embeds/` folder. |  | — |
| `bynk.embed.needs_project` | A single-file build met an `embed` declaration, which needs a project's `embeds/` folder. |  | — |
| `bynk.embed.outside_context_or_commons` | An `embed` declaration appears outside a context or commons. |  | — |
| `bynk.embed.outside_embeds_dir` | An `embed` names a file outside the project's `embeds/` folder. |  | — |
| `bynk.embed.too_large` | An `embed` names a file over 256 KiB, or the project's embedded files total over 1 MiB. |  | — |
| `bynk.embed.unreadable_file` | An `embed` names a file in the project's `embeds/` folder that could not be read. |  | — |
| `bynk.embed.unsafe_path` | An `embed` path is empty, absolute, or contains a `..` segment. |  | — |

## Expectations

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.expect.faults_not_effect` | A fault claim's subject (`expect <call> faults`) is not an effectful call: a value, or a call whose type is not `Effect[_]`. | [`faults_expr`](/book/reference/grammar/#rule-faults_expr) | — |
| `bynk.expect.not_bool` | `expect` was given a non-`Bool` predicate. | [`expect_expr`](/book/reference/grammar/#rule-expect_expr) | — |
| `bynk.expect.outside_case` | `expect` was used outside a `case` body. | [`expect_expr`](/book/reference/grammar/#rule-expect_expr) | — |

## Exports

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.exports.capability_not_provided` | An exported capability has no provider in its context. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| `bynk.exports.conflicting_visibility` | A type is exported with conflicting visibilities. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| `bynk.exports.derived_capability` | `exports capability` names a derived capability, whose instance stays with its context. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| `bynk.exports.duplicate_export` | The same name is exported more than once. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| `bynk.exports.duplicate_in_clause` | A name appears twice in one `exports` clause. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| `bynk.exports.undeclared_capability` | `exports capability` names a capability that is not declared. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| `bynk.exports.undeclared_type` | `exports` names a type that is not declared. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |

## Given capabilities

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.given.cross_context_unknown_capability` | `given B.Cap` names a capability the consumed context does not export. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.given.undeclared_capability`](/book/guides/effects-and-capabilities/understand-the-capability-model/) | A handler uses a capability it did not declare with `given`. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| [`bynk.given.unknown_capability`](/book/reference/capabilities/#declaring-a-capability) | `given` names a capability that does not exist. | [`given_clause`](/book/reference/grammar/#rule-given_clause) | — |
| `bynk.given.unused_capability` | A `given` capability is never used (warning). | [`given_clause`](/book/reference/grammar/#rule-given_clause) | Warning |

## HTTP

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.http.body_on_get_or_delete` | A GET or DELETE handler declares a `body` parameter. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| `bynk.http.cache_bad_immutable` | A `@cache` annotation's `immutable` is not a `Bool` literal. |  | — |
| `bynk.http.cache_bad_max_age` | A `@cache` annotation's `maxAge` is missing or not a positive `Duration` literal. |  | — |
| `bynk.http.cache_bad_scope` | A `@cache` annotation's `scope` is not `public` or `private`. |  | — |
| `bynk.http.cache_duplicate` | A handler carries more than one `@cache` annotation. |  | — |
| `bynk.http.cache_max_age_fractional_seconds` | A `@cache` annotation's `maxAge` is positive but does not resolve to a whole number of seconds. |  | — |
| `bynk.http.cache_on_non_get` | A `@cache` annotation is placed on a handler that is not `on http GET`. |  | — |
| `bynk.http.cache_unknown_arg` | A `@cache` annotation has an argument outside the closed set (`maxAge`/`scope`/`immutable`). |  | — |
| `bynk.http.cors_invalid_field` | A `cors` policy field (`headers`/`credentials`/`maxAge`) has the wrong value shape. |  | — |
| `bynk.http.cors_invalid_origins` | A `cors` policy's `origins` is missing, empty, or not a list of string literals. |  | — |
| `bynk.http.cors_not_http` | A `cors { }` policy appears on a service that is not `from http`. |  | — |
| `bynk.http.cors_unknown_field` | A `cors { }` policy declares a field outside the closed set. |  | — |
| `bynk.http.cors_wildcard_credentials` | A `cors` policy combines `credentials: true` with the wildcard origin `["*"]`. |  | — |
| `bynk.http.duplicate_rejected` | A `from http` service declares `on rejected` more than once. | [`rejected_handler`](/book/reference/grammar/#rule-rejected_handler) | — |
| `bynk.http.duplicate_route` | Two handlers share the same method and route. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| [`bynk.http.error_status_position`](/book/reference/http/#error-bodies) | An `ErrorStatus` name is used anywhere but as `RawError`'s first argument. |  | — |
| `bynk.http.extra_param` | A handler parameter is not a path parameter, `body` or `headers`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| `bynk.http.headers_wrong_type` | An HTTP handler's `headers` parameter is not typed `RequestHeaders`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| `bynk.http.invalid_path` | An HTTP route path is malformed. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| `bynk.http.limit_bad_max_body` | A `@limit` annotation's `maxBody` is missing or not a positive `Int` literal. |  | — |
| `bynk.http.limit_duplicate` | A handler carries more than one `@limit` annotation. |  | — |
| `bynk.http.limit_on_bodyless` | A `@limit` annotation is placed on a handler that takes no body (a GET or DELETE). |  | — |
| `bynk.http.limit_unknown_arg` | A `@limit` annotation has an argument outside the closed set (`maxBody`). |  | — |
| `bynk.http.limits_invalid_field` | A `limits` policy field (`maxBody`) has the wrong value shape. |  | — |
| `bynk.http.limits_not_http` | A `limits { }` policy appears on a service that is not `from http`. |  | — |
| `bynk.http.limits_unknown_field` | A `limits { }` policy declares a field outside the closed set. |  | — |
| `bynk.http.path_param_not_stringy` | A path parameter's type is not constructible from a string. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| `bynk.http.raw_bad_content_type` | A `@raw` annotation's `contentType` is missing or not a `type/subtype` (or `"*/*"`) string literal. |  | — |
| `bynk.http.raw_body_not_bytes` | The `body` of a `@raw` handler is not typed `Bytes`. |  | — |
| [`bynk.http.raw_body_unbounded`](/book/reference/http/#raw-request-bodies) | A `@raw` route has no request-body cap (no `@limit` and no service `limits`). |  | — |
| `bynk.http.raw_duplicate` | A handler carries more than one `@raw` annotation. |  | — |
| `bynk.http.raw_error_status` | `RawError`'s first argument is not an `ErrorStatus` name (a 4xx or 5xx `HttpResult` variant name). |  | — |
| `bynk.http.raw_on_bodyless` | A `@raw` annotation is placed on a handler that takes no body. |  | — |
| `bynk.http.raw_unknown_arg` | A `@raw` annotation has an argument outside the closed set (`contentType`). |  | — |
| `bynk.http.rejected_signature` | An `on rejected` handler is not `on rejected(why: BoundaryRejection) -> HttpResult[()]` with no `by` and no `given`. | [`rejected_handler`](/book/reference/grammar/#rule-rejected_handler) | — |
| `bynk.http.reserved_prefix` | A route uses the reserved `/_bynk/` prefix. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| `bynk.http.return_not_effect_http_result` | An HTTP handler does not return `Effect[HttpResult[T]]`. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| `bynk.http.security_invalid_field` | A `security` policy field (`hsts`/`nosniff`) has the wrong value shape. |  | — |
| `bynk.http.security_not_http` | A `security { }` policy appears on a service that is not `from http`. |  | — |
| `bynk.http.security_unknown_field` | A `security { }` policy declares a field outside the closed set. |  | — |
| `bynk.http.unbound_path_param` | A `:name` route segment has no matching handler parameter. | [`http_handler`](/book/reference/grammar/#rule-http_handler) | — |
| `bynk.http.unknown_handler_annotation` | A handler carries an annotation outside the closed set (`@cache`/`@limit`). |  | — |

## Lexer

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.lex.bad_escape` | An invalid escape sequence in a string literal. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| `bynk.lex.float_literal_overflow` | A float literal does not fit a finite 64-bit float. | [`float_literal`](/book/reference/grammar/#rule-float_literal) | — |
| `bynk.lex.integer_overflow` | An integer literal is outside the `Int` range, the JS safe integers ±(2^53 − 1). | [`number_literal`](/book/reference/grammar/#rule-number_literal) | — |
| `bynk.lex.interpolation_too_deep` | A string interpolation `\(…)` nests deeper than the lexer's fixed limit. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| `bynk.lex.legacy_comment` | A `--` line comment, the spelling before #2203; a comment is `//` (auto-fixable; `bynk fmt` rewrites it). |  | — |
| `bynk.lex.legacy_doc_line` | A `--|` one-line doc, the spelling before #2203; a one-line doc is `--- text` (auto-fixable; `bynk fmt` rewrites it). |  | — |
| `bynk.lex.slash_doc` | A `///` doc comment first on its line; a Bynk doc is `--- text` or a `---` block (auto-fixable). |  | — |
| `bynk.lex.unclosed_doc_block` | A documentation block is not closed. |  | — |
| `bynk.lex.unexpected_character` | An unexpected character in the source. |  | — |
| `bynk.lex.unterminated_interpolation` | An interpolation hole `\(…)` is not closed on its line. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |
| `bynk.lex.unterminated_string` | A string literal is not terminated. | [`string_literal`](/book/reference/grammar/#rule-string_literal) | — |

## Message bundles

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.messages.format_mismatch` | A code's placeholder is formatted as a different ICU kind (plain/plural/select/number/date) across declared locales. |  | — |
| `bynk.messages.incomplete` | A locale is missing a code the reference locale declares. |  | — |
| `bynk.messages.invalid_locale_tag` | A `messages` block's locale tag is not a valid `LocaleTag` (e.g. `messages "xx"` where `xx` doesn't match the tag pattern). |  | — |
| `bynk.messages.malformed_icu_syntax` | A message template's ICU placeholder syntax is invalid — unbalanced arm braces, an unknown format keyword, `#` outside a plural arm, a missing mandatory `other` arm, or an explicitly out-of-scope construct (`selectordinal`, `offset:`/`=N`, a CLDR skeleton). |  | — |
| `bynk.messages.missing_locale_dependency` | A commons declaring `messages` doesn't `uses bynk.locale` and/or `uses bynk.locale.types`, which its generated `render` and the types its signature names need. |  | — |
| `bynk.messages.missing_reference` | A message bundle has no `@reference` block. |  | — |
| `bynk.messages.multiple_reference` | A message bundle has more than one `@reference` block. |  | — |
| `bynk.messages.outside_commons` | A `messages` declaration appears outside a commons. |  | — |
| `bynk.messages.placeholder_mismatch` | A locale's template for a code uses a different set of `{name}` placeholders than the reference locale's. |  | — |

## Observation

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.observe.impure_with` | A `with` predicate uses an effectful or test-only construct; it must be pure. |  | — |
| `bynk.observe.not_a_seam` | An observation targets a capability the unit under test does not consume. |  | — |
| `bynk.observe.trace_outside_test` | `trace(Cap.op)` appears outside a `case` body. |  | — |
| `bynk.observe.unknown_op` | An observation names an operation the capability does not declare. |  | — |
| `bynk.observe.with_not_bool` | A `with` predicate does not have type `Bool`. |  | — |

## Other

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.actor.bearer_identity_not_string_constructible` | A `Bearer` actor's identity is not a string-constructible type. |  | — |
| `bynk.actor.bearer_missing_secret` | A `Bearer` actor does not name its signing secret. |  | — |
| `bynk.actor.binder_shadows_param` | A `by` actor binder collides with a handler parameter of the same name. |  | — |
| `bynk.actor.by_on_agent` | A `by` actor clause was placed on an agent `on call` handler, which has no actor. |  | — |
| `bynk.actor.duplicate_sum_scheme` | Two peers in a multi-actor sum share an authentication scheme. |  | — |
| `bynk.actor.foreign_context_name_unkeyable` | A context declaring a `Foreign` actor has a name no call key can be named from. |  | — |
| `bynk.actor.foreign_duplicate_secret` | A `Foreign` actor's secret is also another actor's secret in the same context. |  | — |
| `bynk.actor.foreign_identity_unsupported` | A `Foreign` actor declares an `identity`. |  | — |
| `bynk.actor.foreign_missing_secret` | A `Foreign` actor does not name the secret its calls are signed with. |  | — |
| `bynk.actor.foreign_name_is_context_name` | A `Foreign` actor's name could be read as a context's name. |  | — |
| `bynk.actor.foreign_reserved_secret` | A `Foreign` actor's secret uses a `BYNK_CALL_` edge key's name. |  | — |
| `bynk.actor.foreign_secret_not_string` | A `Foreign` actor's secret is not a string. |  | — |
| `bynk.actor.identity_not_sealed` | An actor identity type is not a context-ownable (sealed) value type. |  | — |
| `bynk.actor.missing_by_on_http` | An HTTP handler lacks the required `by` actor clause. |  | — |
| `bynk.actor.oidc_identity_not_string_constructible` | An `Oidc` actor's identity is not a string-constructible type. |  | — |
| `bynk.actor.oidc_missing_audience` | An `Oidc` actor does not name its `audience`. |  | — |
| `bynk.actor.oidc_missing_issuer` | An `Oidc` actor does not name its `issuer`. |  | — |
| `bynk.actor.oidc_missing_jwks` | An `Oidc` actor does not name its `jwks` endpoint. |  | — |
| `bynk.actor.oidc_not_in_sum` | An `Oidc` actor appears as a member of a multi-actor sum. |  | — |
| `bynk.actor.oidc_trust_from_is_secret` | An `Oidc` actor's `trustFrom` names an environment variable that an actor uses as its `secret`. |  | — |
| `bynk.actor.oidc_trust_from_not_string` | An `Oidc` actor's `trustFrom` isn't a string naming an environment variable. |  | — |
| `bynk.actor.outside_context` | An `actor` was declared outside a context (e.g. in a commons). |  | — |
| `bynk.actor.refinement_base_unsupported` | A refinement actor's base is not a `Bearer` actor (no claims to authorise against). |  | — |
| `bynk.actor.refinement_in_sum` | A refinement actor appears as a member of a multi-actor sum. |  | — |
| `bynk.actor.refinement_predicate_unsupported` | A refinement actor's `where` predicate is outside the closed claim-predicate set. |  | — |
| `bynk.actor.scheme_not_admissible` | An actor's scheme is not admissible on this handler's protocol. |  | — |
| `bynk.actor.signature_identity_unsupported` | A `Signature` actor declared an `identity`, which is not yet supported. |  | — |
| `bynk.actor.signature_missing_header` | A `Signature` actor does not name its signature header. |  | — |
| `bynk.actor.signature_missing_secret` | A `Signature` actor does not name its signing secret. |  | — |
| `bynk.actor.signature_requires_body` | A `Signature` handler does not take a `body` parameter. |  | — |
| `bynk.actor.signature_tolerance_without_timestamp` | A `Signature` actor set `tolerance` without a `timestamp` header. |  | — |
| `bynk.actor.sum_requires_binder` | A multi-actor sum `by` clause has no binder to match the resolved actor. |  | — |
| `bynk.actor.unknown_actor` | A handler's `by` clause names an actor that is not declared. |  | — |
| `bynk.actor.unknown_scheme` | An actor declares an authentication scheme that is not compiler-known. |  | — |
| `bynk.actor.unreachable_sum_arm` | A multi-actor sum has an arm unreachable after a catch-all (`None`) peer. |  | — |
| `bynk.adapter.consumes_context` | An `adapter` consumed a context; adapter dependencies are adapter-to-adapter. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.adapter.consumes_requires_selection` | An `adapter` used a whole-unit or aliased `consumes`; adapters must select capabilities with `consumes U { Cap, … }`. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.adapter.disallowed_item` | An `adapter` declared a `service`, `agent`, or other item it may not contain. | [`adapter_decl`](/book/reference/grammar/#rule-adapter_decl) | — |
| `bynk.adapter.duplicate_binding` | An `adapter` declared more than one `binding` clause. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| `bynk.adapter.include_duplicate` | An adapter `binding … includes [ … ]` list names one file more than once. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| `bynk.adapter.include_kind` | An adapter `binding … includes [ … ]` entry is not a `.wasm` file; only WebAssembly modules can be included. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| `bynk.adapter.include_missing` | An adapter `binding … includes [ … ]` entry names a file that could not be read. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| `bynk.adapter.include_outside` | An adapter `binding … includes [ … ]` path is absolute or climbs out of the adapter's directory with `..`. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| `bynk.adapter.no_binding` | An `adapter` declares an external provider but no `binding` module to supply it. | [`adapter_decl`](/book/reference/grammar/#rule-adapter_decl) | — |
| `bynk.adapter.provider_has_body` | A provider inside an `adapter` has a Bynk body; adapter providers must be external. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.cell.invalid_target` | A `:=` write targets something that is not a `store Cell` field. |  | — |
| `bynk.cell.self_reference` | A `:=` right-hand side reads the cell being written (a read-modify-write); use `.update`. |  | — |
| `bynk.dependency.path_outside_root` | A path dependency declared by a dependency resolves outside the workspace; a dependency's own path dependencies are honoured only between members of one workspace. |  | — |
| `bynk.deploy.contract_skew` | A worker was compiled against a contract its live dependencies no longer provide, or no longer provides the contract its live callers were compiled against, so `bynk deploy --context` refuses to ship it. |  | — |
| `bynk.deploy.foreign_contract_skew` | A build breaks the contract a declared foreign caller's client was generated at, as pinned in `bynk.foreign.toml`, so `bynk deploy` refuses to ship it. |  | — |
| `bynk.duration.literal_overflow` | A `Duration` literal (`<int>.<unit>`) exceeds the representable millisecond range. |  | — |
| `bynk.event.bad_field_default` | An event field's default expression (`field: T = expr`) is not a static, wire-representable value of the field's declared type — a literal (including one admitted to a refined type), a sum variant, `Some`/`None`/`Ok`/`Err`, a record, or `T.unsafe(lit)` for an opaque type whose literal also satisfies the refinement. |  | — |
| `bynk.event.bad_params` | An `on event` handler declared the wrong number of parameters, or a second parameter whose type is not `EventEnvelope` — it takes the event payload and, optionally, the runtime envelope. |  | — |
| `bynk.event.bad_schema_dispatch` | A `via schema(...)` dispatch clause's argument is malformed — it must be a single, positive, positional `Int` literal. |  | — |
| `bynk.event.bad_schema_version` | An event's `@schema(N)` annotation is malformed — `N` must be a single, positive, positional `Int` literal, and `@schema` may appear at most once on an event. |  | — |
| `bynk.event.default_outside_event` | A field default (`field: T = expr`) was written on a record field outside an `event` declaration — a default is only meaningful on an event's own field, since it exists to let an older wire event missing this key still deserialise. |  | — |
| `bynk.event.duplicate_handler` | A `from Events` service declares more than one `on event` handler; it has exactly one. | [`event_handler`](/book/reference/grammar/#rule-event_handler) | — |
| `bynk.event.emit_not_an_event` | `Events.emit[E]` named a type `E` that is declared in this context, but is not itself an `event` — only an `event` type may be emitted. |  | — |
| `bynk.event.emit_outside_owner` | `Events.emit[E]` named an event `E` not declared in the emitting context — only an event's declaring context may emit it. |  | — |
| `bynk.event.handler_param_type_mismatch` | An `on event(e: T)` handler's declared parameter type does not match its `from Events(E)` header's event type. |  | — |
| `bynk.event.non_additive_schema_change` | An event's field shape changed in a way the schema registry cannot evolve additively — a field was removed, retyped, added without a default, or lost a default it previously had. Give the new shape a new event type name, or make the change additive. |  | — |
| `bynk.event.outside_context` | An `event` was declared outside a context. |  | — |
| `bynk.event.pattern_duplicate_field` | A `from Events(E { ... })` subscription pattern listed the same field more than once. |  | — |
| `bynk.event.pattern_type_mismatch` | A `from Events(E { ... })` subscription pattern field's matched value is not compatible with that field's declared type. |  | — |
| `bynk.event.pattern_unknown_field` | A `from Events(E { ... })` subscription pattern named a field that `E` does not declare. |  | — |
| `bynk.event.pattern_unknown_variant` | A `from Events(E { ... })` subscription pattern's variant value names a variant that does not exist on the field's declared sum type. |  | — |
| `bynk.event.pattern_variant_payload` | A `from Events(E { ... })` subscription pattern's variant value names a variant that carries a payload — only nullary variants are admitted, since testing the tag alone would silently ignore the payload. |  | — |
| `bynk.event.return_not_effect_unit` | An `on event` handler returns an `Effect` of something other than `()`; emission is fire-and-forget, so it returns `Effect[()]`. | [`event_handler`](/book/reference/grammar/#rule-event_handler) | — |
| `bynk.event.schema_version_mismatch` | An event's `@schema(N)` annotation disagrees with the version the schema registry computes from the event's build history. |  | — |
| `bynk.event.unknown_annotation` | An `event` declaration carried an `@`-annotation other than `@schema` — event annotations are a closed set. |  | — |
| `bynk.event.unknown_subscription` | A `from Events(E)` subscription named `E`, which is not a declared event in this context or any consumed context. |  | — |
| `bynk.fmt.comment_loss` | The formatter would drop a comment, so it leaves the file unchanged. |  | — |
| `bynk.fmt.mixed_comment_syntax` | A file mixes the retired `--` comment or `--|` doc spellings with a line that reads differently under the old and new comment rules (#2203), so the formatter leaves it unchanged. |  | — |
| `bynk.fmt.roundtrip` | The formatter's output does not re-parse to the same program, so it leaves the file unchanged. |  | — |
| `bynk.generics.duplicate_type_param` | A `type` or `fn` declares the same type-parameter name more than once (v0.157, ADR 0183). |  | — |
| `bynk.generics.generic_non_record` | A `type` declaration carries type parameters on a refined or opaque body; only a record (`type Name[T] = { … }`) or sum (`type Name[T] = | … | …`) body may be generic (v0.157/#593, ADRs 0183/0197). | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| `bynk.generics.generic_record_at_boundary` | A `Val[…]` fabricates a value of a generic type; per-instantiation value fabrication is not yet wired (ADR 0197). Since v0.174 a generic-record instantiation may otherwise cross a boundary through its monomorphised codec. |  | — |
| `bynk.generics.generic_sum_embeds` | A generic sum type carries an `embeds` clause; embedding into a generic sum is not supported (#593). | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| `bynk.generics.method_on_generic_type` | A *static* method is attached to a generic type; static methods on generic types are deferred (they have no receiver to supply the type's parameters). Instance methods on generic types are supported (#594). | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| `bynk.generics.no_bounds` | A type parameter carries a bound (`[A: …]`); bounded generics are not in v0.20a. | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| `bynk.generics.recursive_generic_at_boundary` | A recursive generic record (one that transitively contains itself, through any wrapper or generic argument) appears at a boundary; it has no finite set of monomorphised codecs, so it is not yet boundary-serialisable (ADR 0197). |  | — |
| `bynk.generics.type_arg_count` | A user-declared generic type is applied to the wrong number of type arguments, or a generic type is named without its `[…]` arguments (v0.157, ADR 0183). | [`applied_type_ref`](/book/reference/grammar/#rule-applied_type_ref) | — |
| `bynk.generics.type_arg_mismatch` | Inferred or explicit type arguments conflict, have the wrong arity, target a non-generic function, or a type parameter shadows a declared type. | [`call`](/book/reference/grammar/#rule-call) | — |
| `bynk.generics.uninferable_type_arg` | A generic function's type parameter could not be inferred from the arguments and was not given explicitly (`name[T](…)`); a bare generic function also cannot be passed as a value in v0.20a. | [`call`](/book/reference/grammar/#rule-call) | — |
| `bynk.held.branch_divergence` | Branches of a conditional leave a held value (e.g. `Connection[F]`) in inconsistent ownership states — one consumes or stores it, another leaves it owned (§2.9.5, real-time track slice 2). |  | — |
| `bynk.held.consume_on_borrow` | A consuming operation (`close`/`put`/`take`) is called on a *borrowed* held reference — borrows admit only non-consuming operations like `send` (§2.9.3, real-time track slice 2). |  | — |
| `bynk.held.leak` | A held value (`Connection[F]`) is still owned at scope exit — it must be disposed (stored, closed, or transferred) before the handler or function returns (§2.9.1, real-time track slice 2). |  | — |
| `bynk.held.query_accessor_on_held_map` | A key-aware query accessor (`.entries`/`.keys`/`.values`) is used on a held `Map[K, Connection]` — a held resource is iterated with the broadcast ops (`forEach`/`parTraverse`), not a key query. |  | — |
| `bynk.held.unsupported_map_op` | A held `Map[K, Connection]` is given an `update`/`upsert` — a held resource cannot be transformed by a `(Connection) -> Connection` function; use `put`/`get`/`remove` (real-time track slice 3b-ii). |  | — |
| `bynk.held.unsupported_storage` | A held value (`Connection[F]`) is stored in a `Set`/`Log`/`Cache` — held values may only live in `Cell[Option[Connection]]` or `Map[K, Connection]` (§2.9.3, real-time track slice 2). |  | — |
| `bynk.held.use_after_consume` | A held value (`Connection[F]`) is used after a consuming operation (`close`/`put`/`take`) ended its lifetime (§2.9.2, real-time track slice 2). |  | — |
| `bynk.history.not_an_agent` | A `for all run: History[T]` names a `T` that is not an agent — only an agent has handlers to sequence and reachable states to observe (testing track slice 7, ADR 0155). |  | — |
| `bynk.history.not_generable` | A `for all run: History[Agent]` targets an agent with a handler parameter whose type cannot be generated (e.g. a `Matches` refinement), so its call-history cannot be driven (testing track slice 7, ADR 0155). |  | — |
| `bynk.history.outside_property` | `History[Agent]` appears outside a `property`'s `for all` binding — it is a test-only generator, not a value type (testing track slice 7, ADR 0155). |  | — |
| `bynk.history.restates_invariant` | A history property merely re-checks a guarantee a declared `invariant`/`transition` already enforces on every reached state (testing track slice 7, ADR 0155). |  | — |
| `bynk.index.bad_argument` | An `@indexed` argument is not a `by: <field>` label. |  | — |
| `bynk.index.missing` | A query filters a map by equality on a field that is not `@indexed` (a perf-hint warning). |  | Warning |
| `bynk.index.unkeyable_key` | An `@indexed(by: k)` field is not value-keyable. |  | — |
| `bynk.index.unknown_key` | An `@indexed(by: k)` field is not a field of the map's value type. |  | — |
| `bynk.index.unused` | A declared `@indexed(by: k)` is never used by an equality filter (a hygiene warning). |  | Warning |
| `bynk.invariant.cross_agent_reference` | An invariant predicate references another agent; invariants are per-agent. |  | — |
| `bynk.invariant.duplicate_name` | An agent declares two invariants with the same name. |  | — |
| `bynk.invariant.impure_predicate` | An invariant predicate uses an effectful or test-only construct. |  | — |
| `bynk.invariant.not_bool` | An invariant predicate does not have type `Bool`. |  | — |
| `bynk.lambda.unannotated_param` | A lambda parameter has no type annotation in a position where no function type is expected to infer it from. | [`lambda_expr`](/book/reference/grammar/#rule-lambda_expr) | — |
| `bynk.list.deprecated_function` | A `bynk.list` free function (`map`/`filter`/`find`/`any`/`all`) is deprecated in favour of the `List` method form (warning; auto-fixable). |  | Warning |
| `bynk.locale.multiple_message_bundles` | A context consumes `Locale` but its direct `uses` reaches two or more message-bundle commons — there is no single bundle to negotiate against. |  | — |
| `bynk.namespace.reserved` | A user unit is named `bynk` or `bynk.*`; the `bynk` root is reserved for the toolchain. |  | — |
| `bynk.package.alias_shadows_unit` | A dependency alias in `[dependencies]` is also the first segment of one of the project's own units, so `<alias>.…` would be ambiguous. |  | — |
| `bynk.package.checksum_mismatch` | A git dependency's store entry, or a freshly fetched commit, doesn't hash to the checksum `bynk.lock` records; `bynk` refuses to build it. |  | — |
| `bynk.package.dependency_cycle` | The package graph has a cycle: a dependency depends, directly or not, on itself or on the project. |  | — |
| `bynk.package.dependency_not_found` | A path dependency's directory has no readable `bynk.toml`, its `bynk.toml` is invalid, or it has no `[package]`. |  | — |
| `bynk.package.dev_dependency_in_source` | A commons, context or adapter reaches a `[dev-dependencies]` alias, which only a suite may use. |  | — |
| `bynk.package.duplicate_source` | One package is reached from two different directories; a build holds one copy of each package. |  | — |
| `bynk.package.git_failed` | `git` couldn't clone, fetch or check out a git dependency, or the revision it names doesn't exist. |  | — |
| `bynk.package.lock_invalid` | `bynk.lock` can't be read: it isn't valid TOML, has no `version`, is a version this toolchain doesn't read, or locks a package to something that isn't a commit. |  | — |
| `bynk.package.lock_out_of_date` | `--locked` was given, and resolving the manifests would change `bynk.lock`. |  | — |
| `bynk.package.name_mismatch` | A path dependency's `package` names a different package than the directory's `[package] name`. |  | — |
| `bynk.package.not_fetched` | A git dependency has no `bynk.lock` entry, or its store entry is missing; `bynk fetch` fetches it (refused under `--offline`). |  | — |
| `bynk.package.unit_not_exported` | A `uses` or `consumes` reaches a unit its dependency declares but doesn't list in `[package] exports`. |  | — |
| `bynk.package.unknown_unit` | A `uses` or `consumes` names, through a dependency alias, a unit the dependency doesn't declare. |  | — |
| `bynk.package.unsafe_source` | A git dependency's URL or revision looks like a command-line option, or uses a transport other than `https`, `ssh` (or `file`, from the project's own manifests). |  | — |
| `bynk.package.unsupported_in_dependency` | A dependency declares an `asset` or an `embed`, which would resolve against the consuming project's `assets/` or `embeds/`; a per-package namespace for either isn't designed yet. |  | — |
| `bynk.package.version_conflict` | One package is reached at two different git revisions (or from git and a path); a build, or a workspace, holds one copy of each package. |  | — |
| `bynk.query.join_key_mismatch` | A `joinOn`/`leftJoin` left and right key function return different types. |  | — |
| `bynk.query.sum_needs_numeric` | A `sum`/`average` key function does not return a numeric type (`Int`, `Float`, or `Duration`). |  | — |
| `bynk.registry.account_refused` | `bynk login`, `bynk logout` or a `bynk org` command didn't go through: you aren't signed in or your session expired, the registry has no account API (or is read through a mirror), the sign-in was declined or its code expired, or the registry refused the request (a taken or reserved name, not the owner, an unknown member or token). |  | — |
| `bynk.registry.bad_archive` | A registry archive is malformed or hostile: not a `bynk-tree-v1` stream, a path that is absolute or climbs out of the package, an out-of-order or repeated file, or past the size limits. |  | — |
| `bynk.registry.bad_signature` | An index line isn't signed by a key the trusted key's rotation chain reaches, or its signed registry `id` or package `name` isn't the one asked for (a forged or replayed line). |  | — |
| `bynk.registry.http_failed` | A registry couldn't be read: unreachable, an error status, a refused or missing token, a redirect from https to http, too many redirects, or a response past the size limit. |  | — |
| `bynk.registry.key_mismatch` | A registry's keys don't lead from the key trusted for it (pinned in `[registries]`, or recorded in `bynk.lock`), or it now declares a different `id`. |  | — |
| `bynk.registry.nonconformant` | `bynk registry conformance` found a registry breaking the protocol: a check failed (a wrong status, an unsigned or mis-signed line, a checksum that isn't the tree sent, a refusal without its JSON body, …); each failure names the check and the protocol section it holds the registry to. |  | — |
| `bynk.registry.publish_refused` | `bynk publish` or `bynk yank` didn't go through: the project isn't a versioned package, it has a path or git dependency, its registry has no publish API (or is read through a mirror), or the registry refused the request (no or wrong token, an existing version, an invalid upload). |  | — |
| `bynk.registry.source_mismatch` | `bynk.lock` locks a package to one registry, but its organisation now routes to another; a misrouted organisation fails rather than resolving a same-named package elsewhere. |  | — |
| `bynk.registry.unavailable` | No registry serves a package: its organisation isn't listed by any `[registries]` entry (the first-party registry isn't live yet), the registry has no `config.json`, or it has no such package or archive. |  | — |
| [`bynk.registry.withdrawn`](/book/guides/projects-build-and-deployment/run-a-private-registry/#withdraw-a-version) | `bynk.lock` locks a registry version that its registry withdrew (a takedown: yanked, and its archive deleted), and its tree isn't in the store. |  | — |
| `bynk.requires.conflicting_range` | Two adapter `binding … requires { … }` entries give different version ranges for one npm package. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| [`bynk.requires.unapproved`](/book/guides/projects-build-and-deployment/share-code-between-projects/#approve-a-dependencys-npm-packages) | A reached adapter of a git or registry dependency requires an npm package, at a range, that the project hasn't approved in `bynk.lock`; review it and run `bynk approve <package>`. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| `bynk.requires.unpinned_dependency` | An adapter `binding … requires { … }` entry has an unpinned version range. | [`binding_decl`](/book/reference/grammar/#rule-binding_decl) | — |
| `bynk.secrets.computed_name` | A `bynk.Secrets` read names its secret with a computed expression rather than a literal, so `bynk deploy` cannot plan it (warning). |  | Warning |
| `bynk.send.in_agent_handler` | A `~>` send was used in an agent handler, which has no execution context to keep a fire-and-forget send alive. | [`effect_send_stmt`](/book/reference/grammar/#rule-effect_send_stmt) | — |
| `bynk.send.in_pure_context` | A `~>` send was used in a pure (non-effectful) context. | [`effect_send_stmt`](/book/reference/grammar/#rule-effect_send_stmt) | — |
| `bynk.send.non_effect` | A `~>` send was applied to a non-`Effect` value. | [`effect_send_stmt`](/book/reference/grammar/#rule-effect_send_stmt) | — |
| `bynk.send.requires_unit` | A `~>` send targets an operation whose reply is not `Effect[()]`. | [`effect_send_stmt`](/book/reference/grammar/#rule-effect_send_stmt) | — |
| `bynk.send.store_op` | A `~>` send targets an agent store operation, which is part of the handler's atomic commit and cannot be sent without waiting. | [`effect_send_stmt`](/book/reference/grammar/#rule-effect_send_stmt) | — |
| `bynk.store.annotation_kind_mismatch` | A storage annotation is used on a kind it does not apply to (e.g. `@ttl` on a `Map`). |  | — |
| `bynk.store.annotation_unsupported` | A known storage annotation (`@ttl`/`@retain`/`@indexed`/`@bounded`) is used before the slice that supports it. |  | — |
| `bynk.store.cache_needs_clock` | A handler performs a `Cache` operation (TTL expiry reads the clock) without declaring `given Clock`. |  | — |
| `bynk.store.cache_ttl_required` | A `Cache` field is missing its required `@ttl(<duration>)` annotation (a keyed store with no expiry is a `Map`). |  | — |
| `bynk.store.kind_arity` | A storage kind was applied to the wrong number of type arguments (e.g. `Cell[A, B]`). |  | — |
| `bynk.store.kind_unsupported` | A known storage kind (`Queue`) is used before the slice that supports it. |  | — |
| `bynk.store.log_needs_clock` | A handler calls `Log.append` (which stamps the current time) without declaring `given Clock`. |  | — |
| `bynk.store.unkeyable_key` | A store `Set` element or `Map`/`Cache` key is not value-keyable — it must be `String`, `Int`, or a refined/opaque type over them (#1680). |  | — |
| `bynk.store.unknown_annotation` | A `store` field carries an annotation outside the closed `@indexed`/`@ttl`/`@retain`/`@bounded` set. |  | — |
| `bynk.store.unknown_kind` | A `store` field's type is not a known storage kind. |  | — |
| `bynk.store.unknown_map_accessor` | A `store Map` field access is not one of its query accessors (`entries`/`keys`/`values`). |  | — |
| `bynk.store.unknown_op` | A storage-`Map`/`Set` operation is not a recognised entry/membership method. |  | — |
| `bynk.stub.at_system` | A test `stub` is in a `system`-tier case, or at the scope of a `suite … as system` (or a suite whose cases run `as system`); each context runs as the Worker it deploys as, which builds its own capability providers, so there is no seam to override. | [`stub_clause`](/book/reference/grammar/#rule-stub_clause) | — |
| `bynk.stub.bad_sequence` | A `stub … returns each […]` sequence is malformed (e.g. empty). |  | — |
| `bynk.stub.generic_op` | A test `stub` targets a capability operation that declares its own type parameter — not supported at v1. |  | — |
| `bynk.stub.not_a_seam` | A test `stub` overrides a capability the unit under test does not consume. |  | — |
| `bynk.stub.rhs_type` | A test `stub … returns <value>` right-hand side does not match the operation's return type. |  | — |
| `bynk.stub.unknown_op` | A test `stub` names an operation the capability does not declare. |  | — |
| `bynk.target.browser_bundle_only` | The `browser` platform builds only the in-process `Bundle` topology; `--target workers` is not a browser build. |  | — |
| `bynk.target.vendor_conflict` | One deployment unit's in-process closure uses platform-native capabilities from two mutually-exclusive platforms. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.target.vendor_required` | A deployment unit uses a platform-native capability but the build selects another `--platform`. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.test.actor_identity_required` | A test omits the identity an identity-carrying actor requires: a bare call-site `by <Actor>`, or no `by` at all on a handler that binds its `Caller`. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.actor_no_identity` | A call-site `by <Actor>(x)` supplies an identity to an actor that takes none — a unit-identity actor (e.g. `Visitor`) or `Nobody`. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.advance_not_clock` | An `advance` statement names a capability other than `Clock`; the test harness owns one time source, the first-party `Clock`. | [`advance_stmt`](/book/reference/grammar/#rule-advance_stmt) | — |
| `bynk.test.advance_not_duration` | An `advance Clock by` statement's amount is not a `Duration`. | [`advance_stmt`](/book/reference/grammar/#rule-advance_stmt) | — |
| `bynk.test.advance_outside_case` | An `advance Clock by` statement appears outside a test `case` body; only the test harness has a clock to move. | [`advance_stmt`](/book/reference/grammar/#rule-advance_stmt) | — |
| `bynk.test.asset_address_method` | A suite fetches an asset with a method other than `GET`; the platform serves assets to `GET` and answers other methods with 405. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.credential_needs_system` | A case drives `by Nobody` (the no-credential principal, which tests the auth seam's 401) outside a `system`-tier case, where there is no real seam to reject it. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.event_needs_in_process` | A `system`-tier case delivers an event with `svc.event(e)`; at `system` an event reaches a subscriber only through its publisher's fan-out, so the case drives the emitting handler instead, or delivers the event at `unit` or `integration`. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.event_not_subscribed` | A test body's `svc.event(e)` delivers an event the `from Events(E)` service does not subscribe to. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.event_publisher_ambiguous` | A test body's `svc.event(e)` omits the envelope, but the event's publisher, which the default envelope names, is not one context the target declares or consumes. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.faults_needs_in_process` | A `system`-tier case claims a fault with `expect <call> faults`, or a fault claim at any tier is driven `by Nobody`, `by Forged` or `by Stale`; at `system` a fault reaches the case as an error response from the deployed Worker, never a throw, so the claim belongs at `unit` or `integration`. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.forged_needs_signature_route` | A case drives `by Forged` or `by Stale` at an address that is not an http route with a `Signature` actor (alone, or as a member of an actor sum); they send a bad signature, so only a `Signature` seam can refuse them. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.forged_needs_system` | A case drives `by Forged` or `by Stale` outside a `system`-tier case; only the `system` driver signs a request, so only there does the real `Signature` seam see the bad signature. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.nobody_needs_secured_route` | A case drives `by Nobody` at a route with no Bearer or Signature actor (alone, or as a member of an actor sum) (e.g. a public `Visitor` route) — there is no auth seam to reject the missing credential. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.oidc_needs_in_process` | A `system`-tier case calls an http route whose actor is `Oidc`-secured; the `system` driver has no token issuer yet, so the request without a token could only be refused with a `401`, and the call belongs at `unit` or `integration`. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.principal_claims_not_oidc` | A call-site principal passes token claims for an actor that isn't `Oidc`. |  | — |
| `bynk.test.principal_identity_mismatch` | A call-site `by <Actor>` acts as an actor whose identity is incompatible with the addressed handler's actor, or names no member of the handler's actor sum. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.principal_on_asset` | A suite's asset fetch carries a `by` clause; an asset is public and served before the Worker runs. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.principal_on_wrong_method` | A wrong-method `405` test carries a `by <Actor>` clause; it reaches no handler, so a principal is meaningless. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.principal_required` | A test drives an identity-carrying handler, or one guarded by an actor sum, with no call-site `by <Actor>`. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.remote_clock_dependent` | Under `bynk test --against`, a `system` case's `expect` compares an `Instant` that isn't derived from the run (a constant such as `Instant.fromEpochMillis(0)`); in process the clock starts at the epoch, but a real runtime's Workers read wall-clock time. | [`case`](/book/reference/grammar/#rule-case) | Warning |
| `bynk.test.remote_unsupported_advance` | A `system` case moves the harness clock with `advance Clock`, which a run against a real runtime (`bynk test --against`) can't honour: the deployed Workers read wall-clock time. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.remote_unsupported_asset` | A `system` case fetches an asset with `svc.GET(asset)`, which a run against a real runtime (`bynk test --against`) can't honour: the fetch is answered in process. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.remote_unsupported_call` | A `system` case addresses an `on call` service, which a run against a real runtime (`bynk test --against`) can't honour: the service is internal, and each call is signed for one caller. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.remote_unsupported_env` | A `system` case's `env` disagrees with the named target it would run against (`bynk test --against`): it sets a secret, sets a name the target doesn't declare under `vars` or declares with another value, or leaves unset a name the target declares. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.remote_unsupported_queue` | A `system` case delivers a queue message with `q.message(m)`, which a run against a real runtime (`bynk test --against`) can't honour: no public wire reaches a queue consumer. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.remote_unsupported_schedule` | A `system` case runs a cron handler with `svc.schedule(…)`, which a run against a real runtime (`bynk test --against`) can't honour: no public wire reaches a cron handler. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.service_bad_address` | A test body addresses a service the wrong way for its protocol (e.g. an http route without a leading path string). | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.service_call_arity` | A test body's `svc.call(...)` passes the wrong number of arguments for the service's `on call` handler. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.service_no_call_handler` | A test body invokes `svc.call(...)` on a service with no `on call` handler (a `from http`/`cron`/`queue` service). | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.service_unknown_route` | A test body addresses an http route / cron schedule / queue message the service does not declare. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.stale_needs_timestamp` | A case drives `by Stale` at a `Signature` route whose actor declares no `timestamp` header and `tolerance`, so there is no replay window for the seam to refuse a stale signature by. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.unknown_actor` | A call-site `by <Actor>` names an actor the target context does not declare and that is not a prelude actor. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.test.wire_needs_system` | A `Wire(...)` raw argument is used outside a `system`-tier service address; `Wire` hands pre-validation input to the boundary and is meaningless at `unit` or in any other position. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.tier.cross_context_needs_system` | A `unit` or `integration` case, or a `property`, reaches another context's service, directly or through a target service or agent handler that calls one; below `system` no other context is stood up to call, so the flow belongs in a `suite … as system`. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.tier.mixed_system_suite` | A suite mixes `system` cases with `unit` or `integration` ones; a `system` case runs against deployed Workers and addresses services by context path, so it needs a `suite … as system` of its own. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.tier.system_needs_wire` | An `as system` test stands up fewer than two contexts; the system tier wires across contexts. |  | — |
| `bynk.wasm.panic` | The in-browser compiler panicked internally (a compiler bug, reported as a diagnostic instead of crashing the page). |  | — |
| `bynk.wasm.strip_failed` | The in-browser compiler could not strip the emitted TypeScript to JavaScript. |  | — |
| `bynk.ws.message_frame_param` | A WebSocket `on message` handler does not have exactly one parameter of the service's inbound (`in:`) frame type — the decoded frame (real-time track slice 3b-iii). |  | — |
| `bynk.ws.open_given_unsupported` | A WebSocket `on open` handler declares `given` capabilities — unsupported at v1, since on Workers the handler runs inside the connection-hosting Durable Object, which has no composition root to supply them (real-time track slice 3b). |  | — |
| `bynk.ws.open_transfer_cross_file` | On the Workers target, a WebSocket `on open` handler transfers its `connection` to an agent declared in a different file of the same context — the open is hosted in the agent's Durable Object, which is emitted from the agent's own file, so the upgrade would never be handled; the service and the agent must share a file. |  | — |
| `bynk.ws.open_transfer_shape` | A WebSocket `on open` handler does not transfer its `connection` into exactly one agent, so the Workers upgrade has no single Durable Object to route to (real-time track slice 3b). |  | — |
| `bynk.ws.route_param_mismatch` | A WebSocket `on message`/`on close` route parameter does not match the `on open` parameter at the same position — route values are recovered positionally from the connection, so they must be a type-compatible prefix of the `on open` parameters (real-time track slice 3b-iii). |  | — |

## Parser

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.parse.consumes_after_decls` | `consumes` appears after other declarations. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| `bynk.parse.dangling_handler_annotation` | A handler-position annotation (e.g. `@cache`) is not followed by an `on` handler. |  | — |
| `bynk.parse.doc_block_contains_code` | A documentation block's content (outside Markdown code fences) parses as declarations, so a pair of dash dividers has turned code into documentation. |  | — |
| `bynk.parse.duplicate_cors` | A service declares more than one `cors { }` policy. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.parse.duplicate_limits` | A service declares more than one `limits { }` policy. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.parse.duplicate_security` | A service declares more than one `security { }` policy. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.parse.empty_agent` | An `agent` body is empty. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| `bynk.parse.empty_capability` | A `capability` body is empty. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| `bynk.parse.empty_interpolation` | An interpolation hole `\(…)` contains no expression. |  | — |
| `bynk.parse.empty_match` | A `match` has no arms. | [`match_expr`](/book/reference/grammar/#rule-match_expr) | — |
| `bynk.parse.empty_service` | A `service` body is empty. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.parse.event_pattern_empty` | A `from Events(E { ... })` subscription pattern listed no fields — use `from Events(E)` (no braces) for an unfiltered subscription. |  | — |
| `bynk.parse.expected_agent_key` | Expected a `key` declaration in an agent. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| `bynk.parse.expected_agent_storage` | An agent declares no storage — it has no `store` fields. |  | — |
| `bynk.parse.expected_base_type` | Expected a base type. | [`base_type`](/book/reference/grammar/#rule-base_type) | — |
| `bynk.parse.expected_capability_op` | Expected a capability operation. | [`capability_op`](/book/reference/grammar/#rule-capability_op) | — |
| `bynk.parse.expected_expression` | Expected an expression. |  | — |
| `bynk.parse.expected_handler` | Expected a handler. | [`handler`](/book/reference/grammar/#rule-handler) | — |
| `bynk.parse.expected_item` | Expected a declaration. |  | — |
| `bynk.parse.expected_predicate` | Expected a refinement predicate. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| `bynk.parse.expected_provider_op` | Expected a provider operation. | [`provider_op`](/book/reference/grammar/#rule-provider_op) | — |
| `bynk.parse.expected_token` | Expected a specific token. |  | — |
| `bynk.parse.expected_type` | Expected a type. |  | — |
| `bynk.parse.expected_unit_header` | Expected a `commons` or `context` header. |  | — |
| `bynk.parse.expected_visibility` | Expected a visibility keyword. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| `bynk.parse.exports_after_decls` | `exports` appears after other declarations. | [`exports_decl`](/book/reference/grammar/#rule-exports_decl) | — |
| `bynk.parse.extra_tokens` | Unexpected tokens after an otherwise complete construct. |  | — |
| `bynk.parse.generic_arg_count` | Wrong number of generic type arguments. | [`generic_type_ref`](/book/reference/grammar/#rule-generic_type_ref) | — |
| `bynk.parse.handler_in_agent` | A protocol handler (`on GET`/`schedule`/`message`) was declared in an agent. | [`handler`](/book/reference/grammar/#rule-handler) | — |
| `bynk.parse.invariant_after_handler` | An `invariant` was declared after a handler; invariants precede handlers. |  | — |
| `bynk.parse.let_else_on_binding` | An `else` follows a plain `let name = …` binding, which matches every value, so the `else` could never run. | [`let_stmt`](/book/reference/grammar/#rule-let_stmt), [`effect_let_stmt`](/book/reference/grammar/#rule-effect_let_stmt) | — |
| `bynk.parse.let_pattern_annotation` | A `let` with a pattern carries a type annotation; the pattern's names take their types from the value. | [`let_else_stmt`](/book/reference/grammar/#rule-let_else_stmt) | — |
| `bynk.parse.let_pattern_without_else` | A `let` binds a pattern but has no `else` saying what the block produces when the value does not match. | [`let_else_stmt`](/book/reference/grammar/#rule-let_else_stmt) | — |
| `bynk.parse.malformed_float_literal` | A float literal is missing a digit on one side of the `.` (`1.`, `.5`). | [`float_literal`](/book/reference/grammar/#rule-float_literal) | — |
| `bynk.parse.migrate_after_handler` | A `migrate` step is declared after an agent handler; migration steps precede the handlers. |  | — |
| `bynk.parse.migrate_item` | A `migrate` step's body holds something other than `field := value`, `let` or `drop field`. |  | — |
| `bynk.parse.multiple_docs` | A declaration or member has two docs directly above it; a target has one doc, so a doc of more than one line is a `---` block. |  | — |
| `bynk.parse.nesting_too_deep` | An expression or type nests deeper than the parser's fixed limit. |  | — |
| `bynk.parse.non_associative` | A non-associative operator was chained (e.g. `a == b == c`). | [`binary_expr`](/book/reference/grammar/#rule-binary_expr) | — |
| `bynk.parse.orphan_doc_block` | A documentation block is not attached to a declaration (warning). |  | Warning |
| `bynk.parse.policy_order` | A service's `cors`/`security`/`limits` policy is out of order or follows a handler; they open the body in the order `cors`, `security`, `limits`. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.parse.refined_pattern_inner` | A refined pattern's inner form is something other than `_`. | [`refined_pattern`](/book/reference/grammar/#rule-refined_pattern) | — |
| `bynk.parse.reserved_keyword` | A reserved keyword was used as an identifier. | [`identifier`](/book/reference/grammar/#rule-identifier) | — |
| `bynk.parse.self_outside_method` | `self` used outside a method or handler. | [`self_expr`](/book/reference/grammar/#rule-self_expr) | — |
| `bynk.parse.storage_after_phase` | Agent storage (`state` / `store`) is declared after the invariants or handlers. |  | — |
| `bynk.parse.transition_after_handler` | A `transition` is declared after an agent handler; step invariants precede the handlers. |  | — |
| `bynk.parse.unexpected_adapter` | An `adapter` appeared where it is not allowed. |  | — |
| `bynk.parse.unexpected_context` | A `context` appeared where it is not allowed. | [`context_decl`](/book/reference/grammar/#rule-context_decl) | — |
| `bynk.parse.unexpected_eof` | Unexpected end of input. |  | — |
| `bynk.parse.unexpected_suite` | A `suite` appeared where it is not allowed. | [`suite_decl`](/book/reference/grammar/#rule-suite_decl) | — |
| `bynk.parse.unknown_effect_method` | An unknown method on `Effect`. |  | — |
| `bynk.parse.unknown_handler_kind` | An unknown handler form (expected `call`, an HTTP method, `schedule`, or `message`). | [`handler`](/book/reference/grammar/#rule-handler) | — |
| `bynk.parse.unknown_predicate` | An unknown refinement predicate. | [`predicate_name`](/book/reference/grammar/#rule-predicate_name) | — |
| `bynk.parse.unknown_tier` | A `case`/`suite` `as <tier>` clause names something other than `unit`, `integration`, or `system`. |  | — |
| `bynk.parse.uses_after_decls` | `uses` appears after other declarations. | [`uses_decl`](/book/reference/grammar/#rule-uses_decl) | — |
| `bynk.parse.variant_name_case` | A sum-type or enum variant name is not capitalised. | [`sum_variant`](/book/reference/grammar/#rule-sum_variant), [`enum_type`](/book/reference/grammar/#rule-enum_type) | — |

## Project

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.project.contracts_lock_corrupt` | `bynk.contracts.lock` (the older service contracts a callee still accepts) is missing its version field, empty, truncated, unparseable, or of an unsupported version — restore it from version control rather than deleting it, since a callee that forgets the contracts it accepts refuses its live callers. |  | — |
| `bynk.project.contracts_lock_oversized` | `bynk.contracts.lock` has one service accepting more older contracts than the build keeps comfortably (warning). |  | Warning |
| `bynk.project.contracts_lock_stale` | `bynk.contracts.lock` lists an older contract the service no longer accepts, because it is unreadable or no longer compatible with the contract the service has now; the entry is ignored (warning). |  | Warning |
| `bynk.project.duplicate_compatibility_flag` | `bynk.toml`'s `[workers] compatibility_flags` lists a flag twice, or one every Bynk Worker already has; the duplicate is dropped (warning). |  | Warning |
| `bynk.project.duplicate_module_doc` | More than one file of a multi-file unit carries a module doc. |  | — |
| `bynk.project.export_context` | `bynk.toml`'s `[package] exports` lists a context: a package shares commons and adapters, never a context, which belongs to the project that deploys it. |  | — |
| `bynk.project.export_unknown` | `bynk.toml`'s `[package] exports` lists a name that isn't a commons or adapter the project declares. |  | — |
| `bynk.project.file_and_directory` | A unit exists as both a file and a directory. |  | — |
| `bynk.project.inconsistent_commons_name` | A source file's path does not match its declared name. |  | — |
| `bynk.project.kind_conflict` | A name is declared as both a commons and a context. |  | — |
| `bynk.project.no_root` | No project root could be determined. |  | — |
| `bynk.project.no_sources` | The project contains no source files. |  | — |
| `bynk.project.read_failed` | A source file could not be read. |  | — |
| `bynk.project.schema_registry_corrupt` | `bynk.schema.lock` (the schema registry for events and agent state) is missing its version field, empty, truncated, or otherwise unparseable — restore it from version control rather than deleting it, since deleting it would silently re-baseline every event's and agent's history. |  | — |
| `bynk.project.unstable_not_experimental` | `bynk.toml`'s `[unstable] allow` lists an item that nothing experimental lies in or under: it is stable now, so the entry can go (warning). |  | Warning |
| `bynk.project.unstable_unknown_item` | `bynk.toml`'s `[unstable] allow` lists a path that names no standard-library unit, exported item or operation. |  | — |

## Properties (generative tests)

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.property.restates_refinement` | A `property` merely re-checks a refinement its type already guarantees. | [`for_all`](/book/reference/grammar/#rule-for_all) | — |
| `bynk.property.where_not_bool` | A `for all ... where` filter does not type to `Bool`. | [`for_all`](/book/reference/grammar/#rule-for_all) | — |

## Providers

| Code | Summary | Construct | Severity |
|---|---|---|---|
| [`bynk.provider.agent_call_undeclared_capability`](/book/guides/effects-and-capabilities/compose-a-provider/#the-rules) | A provider calls an agent handler that needs a capability the provider's `given` does not list. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.provider.dependency_cycle` | Providers form a capability dependency cycle through `given`. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.provider.extra_operation` | A `provides` block implements an operation not in the capability. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.provider.generic_op_requires_external` | A Bynk-bodied `provides` implements a capability operation that declares its own type parameter — only an external (bodiless) provider can. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.provider.missing_operation` | A `provides` block is missing a capability operation. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.provider.outside_context` | `provides` was declared outside a context. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.provider.signature_mismatch` | A `provides` operation's signature does not match the capability. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.provider.unknown_capability` | `provides` names a capability that does not exist. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |

## Queue

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.queue.bad_params` | An `on message` handler does not take exactly one `message` parameter. | [`queue_handler`](/book/reference/grammar/#rule-queue_handler) | — |
| `bynk.queue.duplicate_consumer` | Two `on message` handlers consume the same queue. | [`queue_handler`](/book/reference/grammar/#rule-queue_handler) | — |
| `bynk.queue.invalid_name` | A `from queue("…")` binding has an empty queue name, or one Cloudflare refuses. | [`queue_handler`](/book/reference/grammar/#rule-queue_handler) | — |
| `bynk.queue.return_not_queue_result` | An `on message` handler does not return `Effect[QueueResult]`. | [`handler`](/book/reference/grammar/#rule-handler) | — |

## Record spread

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.record_spread.field_type_mismatch` | A record-spread override has the wrong type for the field. | [`record_spread`](/book/reference/grammar/#rule-record_spread) | — |
| `bynk.record_spread.non_record_base` | The base of a record spread is not a record. | [`record_spread`](/book/reference/grammar/#rule-record_spread) | — |
| `bynk.record_spread.type_mismatch` | A record spread's base is a different record type. | [`record_spread`](/book/reference/grammar/#rule-record_spread) | — |
| `bynk.record_spread.unknown_field` | A record spread overrides a field the record does not have. | [`record_spread`](/book/reference/grammar/#rule-record_spread) | — |

## Refinement

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.refine.literal_violates` | A literal does not satisfy the refined type's predicate. | [`refined_type`](/book/reference/grammar/#rule-refined_type) | — |

## Resolution

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.resolve.ambiguous_variant` | A variant name is ambiguous across several sum types. |  | — |
| `bynk.resolve.arity_mismatch` | A function was called with the wrong number of arguments. | [`call`](/book/reference/grammar/#rule-call) | — |
| `bynk.resolve.duplicate_actor` | Two actors share a name. |  | — |
| `bynk.resolve.duplicate_agent` | Two agents share a name. | [`agent_decl`](/book/reference/grammar/#rule-agent_decl) | — |
| `bynk.resolve.duplicate_capability` | Two capabilities share a name. | [`capability_decl`](/book/reference/grammar/#rule-capability_decl) | — |
| `bynk.resolve.duplicate_field` | A record declares a field twice. | [`record_type`](/book/reference/grammar/#rule-record_type) | — |
| `bynk.resolve.duplicate_field_init` | A record construction initialises a field twice. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| `bynk.resolve.duplicate_fn` | Two functions share a name. | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| `bynk.resolve.duplicate_message_code` | A message bundle declares the same code twice in one block. |  | — |
| `bynk.resolve.duplicate_message_locale` | Two `messages` blocks in one bundle declare the same locale tag. |  | — |
| `bynk.resolve.duplicate_method` | Two methods share a name. | [`fn_decl`](/book/reference/grammar/#rule-fn_decl) | — |
| `bynk.resolve.duplicate_param` | A parameter name is repeated. | [`param`](/book/reference/grammar/#rule-param) | — |
| `bynk.resolve.duplicate_provider` | A capability is provided more than once. | [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.resolve.duplicate_service` | Two services share a name. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.resolve.duplicate_type` | Two types share a name. | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| `bynk.resolve.duplicate_variant` | A sum type declares a variant twice. | [`sum_type`](/book/reference/grammar/#rule-sum_type) | — |
| `bynk.resolve.fn_without_call` | A function was referenced without being called. |  | — |
| `bynk.resolve.let_shadows_fn` | A `let` binding shadows a function. | [`let_stmt`](/book/reference/grammar/#rule-let_stmt) | — |
| `bynk.resolve.let_shadows_type` | A `let` binding shadows a type. | [`let_stmt`](/book/reference/grammar/#rule-let_stmt) | — |
| `bynk.resolve.method_unknown_type` | A method is defined on an unknown type. |  | — |
| [`bynk.resolve.missing_field`](/book/reference/types/#record-types) | A record construction omits a required field. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| `bynk.resolve.name_conflict` | Two declarations share a name. |  | — |
| `bynk.resolve.not_a_record_type` | Record syntax was used on a non-record type. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| `bynk.resolve.opaque_record_construction` | An opaque type was constructed with record syntax. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| `bynk.resolve.param_as_function` | A value (such as a parameter) was called as a function. | [`call`](/book/reference/grammar/#rule-call) | — |
| `bynk.resolve.recursive_record_field` | A record directly contains a field of its own type. | [`record_type`](/book/reference/grammar/#rule-record_type) | — |
| `bynk.resolve.reserved_builtin_type` | A type declaration reuses a compiler-known built-in type name. | [`type_decl`](/book/reference/grammar/#rule-type_decl) | — |
| `bynk.resolve.reserved_host_name` | A declaration is named `globalThis`, which the generated TypeScript uses to reach host globals. | [`type_decl`](/book/reference/grammar/#rule-type_decl), [`fn_decl`](/book/reference/grammar/#rule-fn_decl), [`agent_decl`](/book/reference/grammar/#rule-agent_decl), [`provider_decl`](/book/reference/grammar/#rule-provider_decl) | — |
| `bynk.resolve.reserved_payload_field` | A sum variant declares a payload field named `kind`, the variant's wire discriminant. | [`sum_type`](/book/reference/grammar/#rule-sum_type) | — |
| `bynk.resolve.self_outside_method` | `self` referenced outside a method or handler. | [`self_expr`](/book/reference/grammar/#rule-self_expr) | — |
| `bynk.resolve.type_as_function` | A type name was called as if it were a function. | [`call`](/book/reference/grammar/#rule-call) | — |
| `bynk.resolve.type_in_expr` | A type name was used where a value is expected. |  | — |
| `bynk.resolve.unconsumed_context` | A context's service was called without a `consumes` declaration. | [`consumes_decl`](/book/reference/grammar/#rule-consumes_decl) | — |
| [`bynk.resolve.unknown_field`](/book/reference/types/#record-types) | Accessed a field the record does not have. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| `bynk.resolve.unknown_function` | Called a function that does not exist. | [`call`](/book/reference/grammar/#rule-call) | — |
| [`bynk.resolve.unknown_name`](/book/guides/program-structure/how-a-program-is-shaped/) | Referenced a name that is not in scope. |  | — |
| `bynk.resolve.unknown_static_member` | Referenced an unknown static member (e.g. `T.x`). | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| [`bynk.resolve.unknown_type`](/book/reference/types/) | Referenced a type that does not exist. |  | — |

## Services

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.service.missing_from` | A `from`-less service has a handler other than `on call`. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.service.mixed_protocols` | A service mixes handler forms that do not match its `from <protocol>`. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.service.outside_context` | A `service` was declared outside a context. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.service.return_not_effect` | A service handler's return type is not an `Effect`. | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.service.unknown_protocol` | A `from <protocol>` names an unknown protocol (e.g. a transport like Kafka). | [`service_decl`](/book/reference/grammar/#rule-service_decl) | — |
| `bynk.service.unknown_via_clause` | A `via <name>(...)` clause on a `from Events(...)` header named something other than `schema` — `via` clauses are a closed set, and only `via schema(...)` exists today. |  | — |
| `bynk.service.websocket_header` | The `from websocket` header is malformed — it binds frame types as `websocket(in: <type>, out: <type>)` (real-time track slice 3). |  | — |
| `bynk.service.websocket_multiple` | A context holds more than one `from websocket` service — at v1 the Workers upgrade routes by the `Upgrade: websocket` header alone, so one WebSocket service per context (real-time track slice 3b). |  | — |
| `bynk.service.websocket_open_arity` | A `from websocket` service must hold exactly one `on open` handler (the edge upgrade), and at most one `on message` (inbound) and one `on close` (real-time track slice 3/3b-iii). |  | — |
| `bynk.service.websocket_param_not_stringy` | A `from websocket` service's `on open` parameter has a type not constructible from `String`; it arrives as a query-string value (#1657). |  | — |

## Stability

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.stability.experimental` | An experimental standard-library item is used without an `[unstable] allow` entry in `bynk.toml` covering it. |  | — |

## Static assets

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.asset.duplicate` | An `asset` name is declared more than once in the same unit. |  | — |
| `bynk.asset.invalid_at_path` | An `asset`'s `at` path is not a URL path naming one file, or lies under `/_assets/` or `/_bynk/`. |  | — |
| `bynk.asset.may_shadow_route` | A parameterised HTTP route matches a fixed asset path, so it never sees requests for that path (a warning). |  | Warning |
| `bynk.asset.missing_file` | An `asset` names a file that is not in the project's `assets/` folder. |  | — |
| `bynk.asset.needs_project` | A single-file build met an `asset` declaration; assets need a project build. |  | — |
| `bynk.asset.not_served` | A deployable Bundle build met an `asset`; a Bundle ships no files, so its `.url` would not be served (a warning). |  | Warning |
| `bynk.asset.outside_assets_dir` | An `asset` path does not name a file in the project's `assets/` folder. |  | — |
| `bynk.asset.outside_context_or_commons` | An `asset` declaration appears outside a context or commons. |  | — |
| `bynk.asset.path_conflict` | Two different asset files would be served at one fixed path by the same Worker. |  | — |
| `bynk.asset.shadows_route` | A fixed asset path equals a literal HTTP route, which the platform-served asset makes unreachable. |  | — |
| `bynk.asset.unreadable_file` | An `asset` names a file in the project's `assets/` folder that could not be read. |  | — |
| `bynk.asset.unsafe_path` | An `asset` path is empty, absolute, or contains a `..` segment. |  | — |

## Suites and cases

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.suite.duplicate_case_name` | Two `case`s share a description. | [`case`](/book/reference/grammar/#rule-case) | — |
| `bynk.suite.duplicate_env` | An `env { … }` clause gives the same name twice at one scope (a suite's `env` clauses, or one case's); a case's `env` may override a suite's name, but not repeat its own. | [`env_clause`](/book/reference/grammar/#rule-env_clause) | — |
| `bynk.suite.env_binding_clash` | A `system`-tier suite's or case's `env` names a binding a participant's Worker already declares (a Service Binding, KV namespace, or Durable Object namespace), or a key the test harness sets on it (a call-edge key, a `Foreign` caller's secret), so the entry would clash with it on the Worker's `env`. | [`env_clause`](/book/reference/grammar/#rule-env_clause) | — |
| `bynk.suite.env_not_literal` | An `env { … }` entry's value is not a `String` literal; a Worker's variables and secrets are strings, given verbatim. | [`env_clause`](/book/reference/grammar/#rule-env_clause) | — |
| `bynk.suite.unknown_target` | A `suite` targets a unit that does not exist. | [`suite_decl`](/book/reference/grammar/#rule-suite_decl) | — |

## Transitions (step invariants)

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.transition.cross_agent_reference` | A transition predicate references another agent; step invariants are per-agent. |  | — |
| `bynk.transition.duplicate_name` | An agent declares two transitions with the same name. |  | — |
| `bynk.transition.impure_predicate` | A transition predicate uses an effectful or test-only construct; a step invariant must be pure. |  | — |
| `bynk.transition.may_violate` | A handler can make a move that an agent's `transition` refuses, from a state the agent can be in after a commit; at run time that call faults with `InvariantViolation` (warning). Resolve it in code: refuse the move first, with a `match` arm or an `is` test on the state field that the path can't get past. |  | Warning |
| `bynk.transition.no_step_reference` | A transition references neither `old` nor `new`; it constrains one state, so it is an `invariant`, not a step. |  | — |
| `bynk.transition.not_bool` | A transition predicate does not have type `Bool`. |  | — |

## Type checking

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.types.argument_mismatch` | A call, method, capability, or constructor argument has the wrong type. | [`call`](/book/reference/grammar/#rule-call) | — |
| `bynk.types.asset_at_boundary` | An `Asset` type appears in a record field, boundary or store position; pass its `.url` instead. |  | — |
| `bynk.types.call_arity` | A function value was applied with the wrong number of arguments. | [`call`](/book/reference/grammar/#rule-call) | — |
| `bynk.types.cannot_infer_option_type_param` | The value type of `None` could not be inferred. | [`none_expr`](/book/reference/grammar/#rule-none_expr) | — |
| `bynk.types.cannot_infer_result_type_params` | The type parameters of a `Result` could not be inferred. |  | — |
| `bynk.types.catastrophic_regex` | A `Matches` pattern can match the same text in exponentially many ways (nested or ambiguous repetition, bounded or not), or cannot be analysed, risking catastrophic backtracking (ReDoS). | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| [`bynk.types.claims_at_boundary`](/book/reference/actors/#reading-claims) | `TokenClaims` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent. |  | — |
| `bynk.types.combinator_return_mismatch` | A callback passed to a combinator (`map`/`andThen`/`flatMap`/`traverseAll`/…) returns the wrong type. | [`call`](/book/reference/grammar/#rule-call) | — |
| `bynk.types.constructor_arity` | A variant constructor got the wrong number of arguments. |  | — |
| `bynk.types.constructor_base_mismatch` | A `.of` constructor was given an argument of the wrong base type. |  | — |
| `bynk.types.duplicate_literal_arm` | A `match` has two arms for the same literal value. | [`match_arm`](/book/reference/grammar/#rule-match_arm) | — |
| `bynk.types.duplicate_variant_arm` | A `match` has two arms for the same variant. | [`match_arm`](/book/reference/grammar/#rule-match_arm) | — |
| `bynk.types.embeds_ambiguous` | A type is embedded by more than one variant of a sum, so `?`'s conversion would be ambiguous. |  | — |
| `bynk.types.embeds_unknown_variant` | An `embeds … as V` clause names a variant the sum does not declare. |  | — |
| `bynk.types.embeds_variant_shape` | An `embeds E as V` target variant must have exactly one payload field, of type `E`. |  | — |
| `bynk.types.empty_refinement` | A refinement admits no values (contradictory predicates). | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| `bynk.types.err_value_mismatch` | An `Err` payload has the wrong type. | [`err_expr`](/book/reference/grammar/#rule-err_expr) | — |
| `bynk.types.extensible_match_needs_wildcard` | A `match` covers an extensible standard-library enum (`FetchError`, `Method`) without a wildcard; it may gain variants in a minor release. |  | — |
| `bynk.types.field_access_on_non_record` | Field access on a value that is not a record. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| `bynk.types.field_refinement_not_base` | An inline field refinement requires a base or refined type. | [`record_field`](/book/reference/grammar/#rule-record_field) | — |
| `bynk.types.field_value_mismatch` | A record field was given a value of the wrong type. | [`record_construction`](/book/reference/grammar/#rule-record_construction) | — |
| `bynk.types.function_at_boundary` | A function type appeared in a serialisable or boundary position (a record field, sum payload, service/agent handler signature, capability operation signature, agent state field, or agent key); functions cannot serialise or cross a boundary. | [`function_type_ref`](/book/reference/grammar/#rule-function_type_ref) | — |
| `bynk.types.guard_not_bool` | A match-arm `if` guard is not a `Bool` expression. | [`match_arm`](/book/reference/grammar/#rule-match_arm) | — |
| `bynk.types.headers_at_boundary` | `RequestHeaders` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent. |  | — |
| `bynk.types.held_at_boundary` | A held value (`Connection[F]`) appears in a serialisable or boundary position — a held resource is built and disposed in place, never persisted or sent across a boundary (§2.9, real-time track slice 2). |  | — |
| `bynk.types.held_not_comparable` | A held value (`Connection[F]`) is compared with `==`/`!=` — held values have identity, not value-equality (§2.9.3, real-time track slice 2). |  | — |
| `bynk.types.if_branch_mismatch` | The branches of an `if` have different types. | [`if_expr`](/book/reference/grammar/#rule-if_expr) | — |
| `bynk.types.if_non_bool_cond` | An `if` condition is not a `Bool`. | [`if_expr`](/book/reference/grammar/#rule-if_expr) | — |
| `bynk.types.if_without_else_requires_unit` | An `if` with no `else` branch has a non-unit then-branch; the missing else defaults to `()`, so the branch must be `()` or `Effect[()]`. | [`if_expr`](/book/reference/grammar/#rule-if_expr) | — |
| `bynk.types.interpolation_non_scalar` | An interpolation hole holds a value with no string form. |  | — |
| `bynk.types.invalid_regex` | A `Matches` predicate contains an invalid regular expression. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| `bynk.types.inverted_range` | An `InRange` predicate has its bounds inverted. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| `bynk.types.is_base_mismatch` | An `is` refinement check is applied to a value of the wrong base type. | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| `bynk.types.is_literal_pattern` | A literal was used on the right of `is`; `is` tests type/refinement, not value equality (use `==`). | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| `bynk.types.is_non_sum` | `is` was applied to a value that is not a sum type. | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| `bynk.types.is_refined_pattern` | A refined (`where`) pattern was used on the right of `is`; refined patterns are `match`-only. | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| `bynk.types.is_unknown_variant` | `is` names a variant the type does not have. | [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| `bynk.types.json_uncodable` | A `Json.encode`/`Json.decode` target type cannot pass through the typed JSON codec (functions, effects, error builtins). | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| `bynk.types.key_at_boundary` | A `bynk.crypto` `SigningKey` or `VerifyKey` appears in a storable or boundary-crossing position — a key is held non-extractable and lives for the call that imported it, never persisted, logged or sent (standard-library K4). |  | — |
| `bynk.types.key_not_orderable` | A `sortBy`/`min`/`max` key function does not return an orderable type (`Int`, `Float`, `String`, `Duration`, or `Instant`). |  | — |
| `bynk.types.lambda_mismatch` | A lambda's parameter count, parameter annotations, or body type do not match the expected function type. | [`lambda_expr`](/book/reference/grammar/#rule-lambda_expr) | — |
| `bynk.types.let_annotation_mismatch` | A `let` value does not match its type annotation. | [`let_stmt`](/book/reference/grammar/#rule-let_stmt) | — |
| `bynk.types.list_element_mismatch` | A list-literal element has a different type from the list's element type. | [`list_literal`](/book/reference/grammar/#rule-list_literal) | — |
| `bynk.types.match_arm_mismatch` | A `match` arm has a different type from the others. | [`match_arm`](/book/reference/grammar/#rule-match_arm) | — |
| `bynk.types.match_non_sum_discriminant` | `match` was applied to a value that is not a sum type. | [`match_expr`](/book/reference/grammar/#rule-match_expr) | — |
| `bynk.types.method_arity` | A method was called with the wrong number of arguments. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| `bynk.types.method_not_found` | Called a method the type does not have. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| `bynk.types.method_on_non_named_type` | A method was called on a built-in type that has no methods. | [`method_call`](/book/reference/grammar/#rule-method_call) | — |
| `bynk.types.mixed_pattern_bindings` | A pattern mixes named and positional bindings. | [`variant_pattern`](/book/reference/grammar/#rule-variant_pattern) | — |
| `bynk.types.negative_length` | A length predicate was given a negative value. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| `bynk.types.no_numeric_coercion` | `Int` and `Float` were mixed without an explicit conversion — in an operation or in refinement bounds. | [`binary_expr`](/book/reference/grammar/#rule-binary_expr), [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| `bynk.types.non_exhaustive_match` | A `match` does not cover every variant. | [`match_expr`](/book/reference/grammar/#rule-match_expr) | — |
| `bynk.types.not_comparable` | A value compared with `==`/`!=` contains a function, `Effect`, or `Query` somewhere inside it — those have no value equality, so the type is not equality-supporting (type-system §2.3.5, #1652). |  | — |
| `bynk.types.ok_value_mismatch` | An `Ok` payload has the wrong type. | [`ok_expr`](/book/reference/grammar/#rule-ok_expr) | — |
| `bynk.types.opaque_raw_outside` | `.raw` on an opaque type was used outside its defining commons. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| `bynk.types.opaque_unsafe_outside` | `.unsafe` on an opaque type was used outside its defining context. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| `bynk.types.or_pattern_binding_mismatch` | An or-pattern's alternatives don't all bind the same set of names. | [`match_arm`](/book/reference/grammar/#rule-match_arm), [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| `bynk.types.or_pattern_type_mismatch` | An or-pattern's alternatives give a shared binding different types (or refinements). | [`match_arm`](/book/reference/grammar/#rule-match_arm), [`is_expr`](/book/reference/grammar/#rule-is_expr) | — |
| `bynk.types.pattern_arity` | A pattern binds the wrong number of payload fields. | [`variant_pattern`](/book/reference/grammar/#rule-variant_pattern) | — |
| `bynk.types.pattern_type_mismatch` | A pattern's type does not match the matched value. | [`variant_pattern`](/book/reference/grammar/#rule-variant_pattern) | — |
| `bynk.types.polynomial_regex` | A `Matches` pattern can split the same text between repetitions, so matching time grows polynomially with input length, and no length predicate bounds the input small enough for its degree. | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| `bynk.types.polynomial_regex_capped` | A `Matches` pattern's matching time grows polynomially with input length, but a `MaxLength` or `Length` predicate bounds the input small enough for its degree (warning). | [`refinement`](/book/reference/grammar/#rule-refinement) | Warning |
| `bynk.types.predicate_base_mismatch` | A predicate does not apply to the type's base (e.g. a string predicate on an `Int`). | [`refinement`](/book/reference/grammar/#rule-refinement) | — |
| `bynk.types.query_at_boundary` | A `Query` type appears in a storable or boundary-crossing position — a query is built and executed in place, never persisted or sent (ADR 0115). |  | — |
| `bynk.types.question_error_mismatch` | `?` propagates an error type incompatible with the function's. | [`question_expr`](/book/reference/grammar/#rule-question_expr) | — |
| `bynk.types.question_on_non_result` | `?` was applied to a non-`Result` value. | [`question_expr`](/book/reference/grammar/#rule-question_expr) | — |
| `bynk.types.question_option_outside_http` | `?` lifts an `Option` only inside a handler returning `HttpResult` (`None` becomes `NotFound`); elsewhere use `.okOr(err)`. | [`question_expr`](/book/reference/grammar/#rule-question_expr) | — |
| `bynk.types.question_outside_result` | `?` used in a function that does not return a `Result`. | [`question_expr`](/book/reference/grammar/#rule-question_expr) | — |
| `bynk.types.rejection_at_boundary` | `BoundaryRejection` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent. |  | — |
| `bynk.types.return_mismatch` | A returned value does not match the declared return type. |  | — |
| `bynk.types.some_value_mismatch` | A `Some` payload has the wrong type. | [`some_expr`](/book/reference/grammar/#rule-some_expr) | — |
| `bynk.types.stream_at_boundary` | A `Stream` type appears in a storable or boundary-crossing position — a stream is a live value-over-time source, never persisted or sent across a boundary (real-time track slice 0). |  | — |
| `bynk.types.stream_not_comparable` | A `Stream` value is compared with `==`/`!=` — a stream is a live value-over-time source, not a comparable value (real-time track slice 0). |  | — |
| `bynk.types.type_mismatch` | Two types that were required to match did not. |  | — |
| `bynk.types.uninferable_element_type` | An empty `[]` (or `List.empty()` / `Map.empty()`) has no expected type to infer its element type from. | [`list_literal`](/book/reference/grammar/#rule-list_literal) | — |
| `bynk.types.unkeyable_distinct` | A `distinct`/`distinctBy` element or key is not value-keyable (`String`, `Int`, or a refined/opaque type over them). |  | — |
| `bynk.types.unkeyable_map_key` | A `Map` key type is not value-keyable (`String`, `Int`, or a refined/opaque type over them). | [`generic_type_ref`](/book/reference/grammar/#rule-generic_type_ref) | — |
| `bynk.types.unknown_field` | Referenced a field the record type does not declare. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| `bynk.types.unknown_pattern_field` | A pattern names a field the variant does not have. | [`variant_pattern`](/book/reference/grammar/#rule-variant_pattern) | — |
| `bynk.types.unknown_static_member` | Referenced an unknown static member on a type. | [`field_access`](/book/reference/grammar/#rule-field_access) | — |
| `bynk.types.unknown_variant_in_pattern` | A pattern names a variant the sum type does not have. | [`variant_pattern`](/book/reference/grammar/#rule-variant_pattern) | — |
| `bynk.types.unreachable_arm` | A `match` arm is unreachable. | [`match_arm`](/book/reference/grammar/#rule-match_arm) | — |
| `bynk.types.variant_arity` | A variant constructor got the wrong number of payload values. |  | — |
| `bynk.types.variant_missing_payload` | A variant requiring a payload was used without one. |  | — |
| `bynk.types.variant_payload_mismatch` | A variant payload has the wrong type. |  | — |

## Uses

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.uses.name_conflict` | A `uses` name collides with another name, a value crosses between a unit's code and an imported declaration's position whose type the unit shadows, or a reference names an import the unit's own declaration shadows. | [`uses_decl`](/book/reference/grammar/#rule-uses_decl) | — |
| `bynk.uses.self_reference` | A commons `uses` itself. | [`uses_decl`](/book/reference/grammar/#rule-uses_decl) | — |
| `bynk.uses.target_is_context` | `uses` targets a context instead of a commons. | [`uses_decl`](/book/reference/grammar/#rule-uses_decl) | — |
| `bynk.uses.unknown_commons` | `uses` names a commons that does not exist. | [`uses_decl`](/book/reference/grammar/#rule-uses_decl) | — |

## Value fabrication

| Code | Summary | Construct | Severity |
|---|---|---|---|
| `bynk.val.agent_not_generable` | A `for all`/`Val` cannot generate an agent — fabricated agent states need not be reachable. | [`for_all`](/book/reference/grammar/#rule-for_all) | — |
| `bynk.val.arity` | `Val[T]` was given the wrong number of pin arguments. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| `bynk.val.literal_violates` | A pinned `Val[T]` value violates the type's refinement. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| `bynk.val.needs_pin` | A bare `Val[T]` cannot generate a value (e.g. a `Matches` string); pin one. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| `bynk.val.outside_test` | `Val[T]` was used outside a test case body. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| `bynk.val.pin_not_literal` | A `Val[T]` pin argument is not a compile-time literal. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| `bynk.val.pin_unsupported` | A pin was given for a type kind that does not support pinning. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| `bynk.val.unknown_type` | `Val[T]` names a type that does not resolve. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
| `bynk.val.unsupported_kind` | `Val[T]` cannot fabricate a value for this kind of type. | [`val_expr`](/book/reference/grammar/#rule-val_expr) | — |
