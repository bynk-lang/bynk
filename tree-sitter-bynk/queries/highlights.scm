; Bynk highlighting queries.
;
; Maps grammar nodes to standard tree-sitter highlighting groups. See
; design/bynk-tree-sitter-spec.md §2 for the group inventory.

; -- Keywords --

[
  "if"
  "else"
  "let"
  "match"
  "is"
  "expect"
  "do"
  "on"
  "given"
  "call"
  "from"
  "http"
  "cron"
  "queue"
  "websocket"
  "schedule"
  "message"
  "Events"
] @keyword

[
  "uses"
  "consumes"
  "as"
] @keyword.import

[
  "commons"
  "context"
  "type"
  "fn"
  "capability"
  "provides"
  "stub"
  "service"
  "agent"
  "store"
  "cors"
  "security"
  "limits"
  "exports"
  "key"
  "suite"
  "case"
  "property"
  "invariant"
  "transition"
  "migrate"
  "drop"
  "replaces"
  "removed"
  "requires"
  "ensures"
  "messages"
  "event"
  "rejected"
  "asset"
  "embed"
] @keyword.declaration

[
  "opaque"
  "transparent"
] @keyword.modifier

[
  "where"
  "implies"
  "enum"
  "for"
  "all"
  "embeds"
] @keyword.operator

; v0.117: the observation sugar words — contextual (these tokens exist only
; inside an `observation_expr`, so matching them globally never repaints an
; ordinary identifier of the same spelling).
[
  "called"
  "never"
  "once"
  "times"
  "with"
  "before"
] @keyword.operator

; v0.118: the contextual test-tier words on a `suite`/`case` header (after `as`)
; and the `stub` clause right-hand-side words. Like the observation words,
; these tokens exist only in their clauses, so a global match never repaints an
; ordinary identifier of the same spelling.
[
  "unit"
  "integration"
  "system"
  "returns"
  "each"
  "fails"
] @keyword.operator

; #2134: the contextual `env` word heading a suite/case `env { … }` clause.
"env" @keyword

; #1706: the fault-claim word — contextual, like the observation words: the
; token exists only inside a `faults_expr`.
"faults" @keyword.operator

; #2640: `par` is contextual (an identifier elsewhere), so it is a keyword only
; as the head of a `par_stmt`.
(par_stmt "par" @keyword)

; v0.117: the `trace(Cap.op)` escape hatch — a test-only builtin.
"trace" @keyword

; HTTP method on `on http METHOD "path"` handlers.
(http_method) @keyword

; -- Types --

(base_type) @type.builtin
(validation_error_type) @type.builtin

(builtin_type) @type.builtin

; User-defined type names appear in type positions.
(type_decl name: (identifier) @type)
; Events track slice 0/1 (spine #936): an `event` declaration names a type,
; like `type_decl`; the header's `event_type` and a pattern field's
; qualifier both reference one.
(event_decl name: (identifier) @type)
(service_protocol event_type: (identifier) @type)
(event_pattern_value type: (identifier) @type)
(opaque_type) @type
(generic_type_ref arg: (identifier) @type)
; v0.157 (ADR 0183): a user generic-type application — the head name and each
; argument are types.
(applied_type_ref name: (identifier) @type)
(applied_type_ref arg: (identifier) @type)
(record_field type: (identifier) @type)
(variant_payload_field type: (identifier) @type)
(param type: (identifier) @type)
(method_name type: (identifier) @type)
(record_construction type: (identifier) @type)
(record_spread type: (identifier) @type)
(variant_pattern type: (identifier) @type)
(record_pattern type: (identifier) @type)
(key_decl type: (identifier) @type)

; -- Functions --

(fn_decl name: (identifier) @function)
(fn_decl name: (method_name method: (identifier) @function.method))
(call name: (identifier) @function)
(method_call method: (identifier) @function.method)
(capability_op name: (identifier) @function.method)
; S4a (#1966): the base of a derived capability (`capability Sessions = Kv`).
(capability_decl base: (identifier) @type)
(provider_op name: (identifier) @function.method)
(call_handler method: (identifier) @function.method)

["Ok" "Err" "Some"] @function.builtin
(none_expr) @constant.builtin
(effect_pure_expr "Effect" @type.builtin
                  "pure" @function.builtin)
(val_expr "Val" @function.builtin)

; -- Variables / parameters --

(param name: (identifier) @variable.parameter)
(let_stmt name: (identifier) @variable)
(effect_let_stmt name: (identifier) @variable)
(self_expr) @variable.builtin
(self_param) @variable.builtin
(key_decl name: (identifier) @variable.parameter)

; -- Fields --

(record_field name: (identifier) @field)
(variant_payload_field name: (identifier) @field)
(field_access field: (identifier) @field)
(field_init name: (identifier) @field)
(field_init shorthand: (identifier) @field)
(named_binding field: (identifier) @field)
; #2641: a record pattern's field name (the shorthand form also binds it).
(record_pattern_field name: (identifier) @field)
; Events track slice 1 (spine #936): a subscription pattern's field name.
(event_pattern_field name: (identifier) @field)

; -- Variants & constants --

; Sum/enum variant declarations keep the dedicated `constant_name` node.
(constant_name) @constant
; Variant names in patterns are plain identifiers; treat a capitalised
; pattern variant as a constant.
((variant_pattern variant: (identifier) @constant)
 (#match? @constant "^[A-Z]"))
; v0.145 (ADR 0169): a lowercase-led pattern variant is a payload *binding* (the
; universal capitalisation convention) — a bare `n` or the leaf of a nested
; pattern like `Some(n)`. Highlight it as a variable, mirroring the retired
; `positional_binding` capture now that the payload position is a full pattern.
((variant_pattern variant: (identifier) @variable)
 (#match? @variable "^[a-z_]"))
; Events track slice 1 (spine #936): a subscription pattern's variant value
; (`region: Region.Domestic` / bare `Domestic`) — always a nullary reference
; (the checker rejects a payload-carrying variant), so unlike
; `variant_pattern` there is no binding-vs-tag ambiguity; still keyed on
; capitalisation for consistency with every other variant reference.
((event_pattern_value variant: (identifier) @constant)
 (#match? @constant "^[A-Z]"))
(boolean_literal) @constant.builtin

; -- Literals --

(string_literal) @string
(number_literal) @number
(float_literal) @number
(unit_literal) @constant.builtin

; -- Refinement predicates --

(predicate_name) @attribute

; -- Module references --

(qualified_name (identifier) @module)
(uses_decl target: (qualified_name) @module)
(consumes_decl target: (qualified_name) @module)
(suite_decl target: (qualified_name) @module)

; -- Operators & punctuation --

[
  "+" "-" "*" "/"
  "==" "!=" "<" "<=" ">" ">="
  "&&" "||" "!"
  "<-" "~>" ":=" "->" "=" "?"
] @operator

[
  "(" ")" "{" "}" "[" "]"
] @punctuation.bracket

[
  "," ":" "|"
] @punctuation.delimiter

[
  "=>" "..."
] @punctuation.special

; -- Comments --

(line_comment) @comment
(doc_block) @comment.documentation
(doc_line) @comment.documentation

; -- Errors (tree-sitter recovery) --

(ERROR) @error

; #2321: `includes` is contextual (an identifier elsewhere), so it is its own node.
(includes_keyword) @keyword.declaration
