---
title: Complete grammar (appendix)
---
<!-- GENERATED FILE — do not edit by hand.
     Source: tree-sitter-bynk/src/grammar.json, via bynkc/tests/grammar_reference.rs.
     Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it grammar_reference:: -->

The complete Bynk grammar, generated from the `tree-sitter-bynk` grammar. For the annotated, per-construct reference see [Syntax & grammar](/book/reference/grammar/).

**Notation.** `"x"` a literal token · `/x/` a regular expression · `( … )?` optional · `( … )*` zero or more · `( … )+` one or more · `a | b` choice · `ε` empty. Rule names are the readable display names (a leading `_` denotes an internal helper rule; trivial wrappers are collapsed). `doc_block` is an external token — a `--- … ---` documentation block.

```ebnf
source_file ::= (commons_decl | context_decl | adapter_decl | suite_decl)+ | item_fragment+ | expr_fragment
item_fragment ::= context_body_item | handler | store_field | key_decl
expr_fragment ::= statement+ expression? | expression
commons_decl ::= "commons" qualified_name ("{" commons_body_item* "}" | commons_body_item*)
context_decl ::= "context" qualified_name ("{" context_body_item* "}" | context_body_item*)
adapter_decl ::= "adapter" qualified_name ("{" adapter_body_item* "}" | adapter_body_item*)
suite_decl ::= "suite" qualified_name ("as" ("unit" | "integration" | "system"))? ("{" test_body_item* "}" | test_body_item*)
qualified_name ::= identifier ("." identifier)*
commons_body_item ::= uses_decl | type_decl | fn_decl | messages_decl | capability_decl | provider_decl | service_decl | agent_decl | actor_decl | event_decl | asset_decl | embed_decl
context_body_item ::= uses_decl | consumes_decl | exports_decl | type_decl | fn_decl | capability_decl | provider_decl | service_decl | agent_decl | actor_decl | messages_decl | event_decl | asset_decl | embed_decl | removed_agent_decl
adapter_body_item ::= binding_decl | uses_decl | consumes_decl | exports_decl | type_decl | fn_decl | capability_decl | provider_decl | service_decl | agent_decl | actor_decl | messages_decl | event_decl | asset_decl | embed_decl
test_body_item ::= uses_decl | consumes_decl | env_clause | stub_clause | case | property_decl
uses_decl ::= "uses" qualified_name
consumes_decl ::= "consumes" qualified_name ("as" identifier | "{" (identifier ("," identifier)*)? ","? "}")?
binding_decl ::= "binding" string_literal ("includes" "[" (string_literal ("," string_literal)*)? ","? "]")? ("requires" "{" (binding_requirement ("," binding_requirement)*)? ","? "}")?
binding_requirement ::= string_literal ":" string_literal
exports_decl ::= "exports" ("opaque" | "transparent" | "capability") "{" (identifier ("," identifier)*)? ","? "}"
visibility_modifier ::= "private"
type_decl ::= visibility_modifier? "type" identifier ("[" identifier ("," identifier)* "]")? "=" type_body
type_body ::= opaque_type | refined_type | record_type | sum_type | enum_type
opaque_type ::= "opaque" base_type ("where" refinement)?
refined_type ::= base_type ("where" refinement)?
record_type ::= "{" (record_field ("," record_field)*)? ","? "}"
record_field ::= identifier ":" type_ref ("where" refinement)? ("=" expression)?
event_decl ::= "event" identifier store_annotation* "=" record_type
asset_decl ::= visibility_modifier? "asset" identifier "=" string_literal ("at" string_literal)?
embed_decl ::= visibility_modifier? "embed" identifier "=" string_literal
sum_type ::= sum_variant+ ("embeds" type_ref "as" constant_name ("," type_ref "as" constant_name)*)?
sum_variant ::= "|" constant_name ("(" (variant_payload_field ("," variant_payload_field)*)? ","? ")")?
variant_payload_field ::= identifier ":" type_ref
enum_type ::= "enum" "{" (constant_name ("," constant_name)*)? ","? "}"
refinement ::= refinement_pred ("&&" refinement_pred)*
refinement_pred ::= pred_call | predicate_name
pred_call ::= predicate_name "(" (pred_arg ("," pred_arg)*)? ")"
predicate_name ::= "Matches" | "InRange" | "MinLength" | "MaxLength" | "Length" | "NonNegative" | "Positive" | "NonEmpty"
pred_arg ::= "-"? number_literal | "-"? float_literal | string_literal
base_type ::= "Int" | "String" | "Bool" | "Float" | "Duration" | "Instant" | "Bytes"
type_ref ::= function_type_ref | base_type | unit_type | validation_error_type | generic_type_ref | applied_type_ref | identifier
applied_type_ref ::= identifier "[" type_ref ("," type_ref)* "]"
function_type_ref ::= (base_type | unit_type | validation_error_type | generic_type_ref | applied_type_ref | identifier | "(" type_ref ("," type_ref)* ","? ")") "->" type_ref
unit_type ::= "(" ")"
validation_error_type ::= "ValidationError"
generic_type_ref ::= ("Option" | "Effect" | "HttpResult" | "List" | "Set" | "Stream" | "Query" | "Connection" | "History") "[" type_ref "]" | ("Result" | "Map") "[" type_ref "," type_ref "]"
fn_decl ::= visibility_modifier? "fn" (method_name | identifier) ("[" identifier ("," identifier)* "]")? "(" params? ")" "->" type_ref requires_clause* ensures_clause* block
method_name ::= identifier "." identifier
requires_clause ::= "requires" identifier ":" expression
ensures_clause ::= "ensures" identifier ":" expression
params ::= (self_param | param) ("," param)* ","?
self_param ::= "self"
param ::= identifier ":" type_ref
capability_decl ::= "capability" identifier ("{" capability_op* "}" | "=" identifier store_annotation*)
capability_op ::= "fn" identifier ("[" identifier ("," identifier)* "]")? "(" (param ("," param)*)? ","? ")" "->" type_ref
messages_decl ::= visibility_modifier? "messages" string_literal store_annotation* "{" message_entry* "}"
message_entry ::= string_literal "=>" string_literal ","?
provider_decl ::= "provides" identifier "=" identifier given_clause? ("{" provider_op* "}")?
provider_op ::= "fn" identifier "(" (param ("," param)*)? ","? ")" "->" type_ref block
service_decl ::= "service" identifier service_protocol? by_clause? given_clause? "{" cors_policy? security_policy? limits_policy? (store_annotation* handler)* "}"
cors_policy ::= "cors" "{" (cors_field ","?)* "}"
cors_field ::= identifier ":" expression
security_policy ::= "security" "{" (security_field ","?)* "}"
security_field ::= identifier ":" expression
limits_policy ::= "limits" "{" (limits_field ","?)* "}"
limits_field ::= identifier ":" expression
service_protocol ::= "from" ("http" | "cron" | "queue" "(" string_literal ")" | "websocket" "(" "in" ":" type_ref "," "out" ":" type_ref ","? ")" | "Events" "(" type_ref event_pattern? ")" schema_dispatch_clause?)
event_pattern ::= "{" (event_pattern_field ",")* ".." "}"
schema_dispatch_clause ::= "via" "schema" "(" "-"? number_literal ")"
event_pattern_field ::= identifier ":" event_pattern_value
event_pattern_value ::= "-"? number_literal | string_literal | boolean_literal | (identifier ".")? identifier
agent_decl ::= "agent" identifier ("replaces" identifier)? "{" key_decl store_field* (invariant_decl | transition_decl | migrate_decl)* (store_annotation* call_handler)* "}"
invariant_decl ::= "invariant" identifier ":" expression
transition_decl ::= "transition" identifier ":" expression
migrate_decl ::= "migrate" "from" number_literal ("(" identifier ":" type_ref ")")? "{" (let_stmt | assign_stmt | migrate_drop)* "}"
migrate_drop ::= "drop" identifier
removed_agent_decl ::= "removed" "agent" identifier
key_decl ::= "key" identifier ":" type_ref
store_field ::= "store" identifier ":" store_kind store_annotation* ("=" expression)?
store_kind ::= identifier ("[" type_ref ("," type_ref)* "]")?
store_annotation ::= "@" identifier ("(" (annotation_arg ("," annotation_arg)*)? ","? ")")?
annotation_arg ::= (identifier ":")? expression
handler ::= call_handler | http_handler | cron_handler | queue_handler | ws_open_handler | ws_close_handler | event_handler | rejected_handler
rejected_handler ::= "on" "rejected" "(" (param ("," param)*)? ","? ")" "->" type_ref by_clause? given_clause? block
event_handler ::= "on" "event" "(" (param ("," param)*)? ","? ")" "->" type_ref by_clause? given_clause? block
call_handler ::= "on" "call" identifier? "(" (param ("," param)*)? ","? ")" "->" type_ref by_clause? given_clause? block
http_handler ::= "on" http_method "(" string_literal ")" "(" (param ("," param)*)? ","? ")" "->" type_ref by_clause? given_clause? block
http_method ::= "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
cron_handler ::= "on" "schedule" "(" string_literal ")" "(" (param ("," param)*)? ","? ")" "->" type_ref by_clause? given_clause? block
queue_handler ::= "on" "message" "(" (param ("," param)*)? ","? ")" "->" type_ref by_clause? given_clause? block
ws_open_handler ::= "on" "open" "(" (param ("," param)*)? ","? ")" "->" type_ref by_clause? given_clause? block
ws_close_handler ::= "on" "close" "(" (param ("," param)*)? ","? ")" "->" type_ref by_clause? given_clause? block
given_clause ::= "given" qualified_name ("," qualified_name)*
actor_decl ::= "actor" identifier ("{" "auth" "=" scheme scheme_config? ("," "identity" "=" type_ref)? "}" | "=" identifier "where" expression)
scheme ::= "None" | "Internal" | "Bearer" | "Signature" | "Oidc" | "Foreign"
scheme_config ::= "(" scheme_arg ("," scheme_arg)* ")"
scheme_arg ::= identifier "=" (string_literal | number_literal)
by_clause ::= "by" (identifier ":")? identifier ("|" identifier)*
stub_clause ::= "stub" identifier "." identifier "(" (("_" | expression) ("," ("_" | expression))* ","?)? ")" ("returns" "each" "[" (("fails" | expression) ("," ("fails" | expression))* ","?)? "]" | "returns" expression | "fails")
env_clause ::= "env" "{" (env_entry ","?)* "}"
env_entry ::= identifier "=" expression
case ::= "case" string_literal ("as" ("unit" | "integration" | "system"))? "{" (stub_clause | env_clause)* statement* expression? "}"
property_decl ::= "property" string_literal "{" for_all "}"
for_all ::= "for" "all" for_all_binding ("," for_all_binding)* ("where" expression)? block
for_all_binding ::= identifier ":" type_ref
block ::= "{" statement* expression? "}"
statement ::= let_stmt | effect_let_stmt | let_else_stmt | effect_send_stmt | do_stmt | assign_stmt | advance_stmt | par_stmt | expect_expr
let_stmt ::= "let" binding_name (":" type_ref)? "=" expression
effect_let_stmt ::= "let" binding_name (":" type_ref)? "<-" expression call_site_actor?
let_else_stmt ::= "let" (let_pattern | let_or_pattern) ("=" | "<-") expression call_site_actor? "else" (block | let_else_match)
let_pattern ::= literal_pattern | paren_pattern | let_variant_pattern
let_or_pattern ::= let_pattern "|" pattern
let_variant_pattern ::= identifier "." identifier ("(" (pattern_binding ("," pattern_binding)*)? ","? ")")? | identifier "(" (pattern_binding ("," pattern_binding)*)? ","? ")"
let_else_match ::= "match" "{" match_arm* "}"
call_site_actor ::= "by" identifier ("(" expression ("," expression)? ")")?
effect_send_stmt ::= "~>" expression
do_stmt ::= "do" expression
assign_stmt ::= (identifier | contextual_identifier) ":=" expression
advance_stmt ::= "advance" identifier "by" expression
par_stmt ::= "par" "{" par_statement* "}"
par_statement ::= let_stmt | effect_let_stmt | effect_send_stmt | do_stmt | assign_stmt | advance_stmt | par_stmt | expect_expr
binding_name ::= identifier | "_"
expression ::= if_expr | match_expr | is_expr | expect_expr | binary_expr | unary_expr | primary
expect_expr ::= "expect" (observation_expr | faults_expr | expression)
faults_expr ::= expression call_site_actor? "faults"
observation_expr ::= identifier "." identifier ("called" ("once" | number_literal "times")? ("with" expression)? | "never" "called" | "before" identifier "." identifier)
trace_expr ::= "trace" "(" identifier "." identifier ")"
if_expr ::= "if" expression block ("else" (if_expr | block))?
match_expr ::= "match" expression "{" match_arm* "}"
match_arm ::= (pattern | refined_pattern) ("if" expression)? "=>" expression ","?
refined_pattern ::= wildcard_pattern "where" refinement
pattern ::= wildcard_pattern | literal_pattern | variant_pattern | record_pattern | or_pattern | paren_pattern
record_pattern ::= identifier "{" (record_pattern_field ",")* ".." "}"
record_pattern_field ::= identifier ":" pattern | identifier
is_base_pattern ::= wildcard_pattern | literal_pattern | variant_pattern | paren_pattern
is_pattern ::= is_base_pattern | is_or_pattern
is_or_pattern ::= is_pattern "|" is_base_pattern
or_pattern ::= pattern "|" pattern
paren_pattern ::= "(" pattern ")"
wildcard_pattern ::= "_"
literal_pattern ::= "-"? number_literal | string_literal | boolean_literal
variant_pattern ::= (identifier ".")? identifier ("(" (pattern_binding ("," pattern_binding)*)? ","? ")")?
pattern_binding ::= named_binding | pattern
named_binding ::= identifier ":" pattern
is_expr ::= expression "is" is_pattern
binary_expr ::= expression "implies" expression | expression "||" expression | expression "&&" expression | expression ("==" | "!=") expression | expression ("<" | "<=" | ">" | ">=") expression | expression ("+" | "-") expression | expression ("*" | "/") expression
unary_expr ::= ("!" | "-") expression
primary ::= lambda_expr | paren_expr | method_call | field_access | call | record_construction | record_spread | question_expr | ok_expr | err_expr | some_expr | none_expr | effect_pure_expr | val_expr | wire_expr | trace_expr | list_literal | block | number_literal | float_literal | string_literal | boolean_literal | unit_literal | self_expr | identifier | contextual_identifier
contextual_identifier ::= "trace" | "advance" | "par"
lambda_expr ::= "(" (lambda_param ("," lambda_param)*)? ")" "=>" (expression | block)
lambda_param ::= (identifier | contextual_identifier) (":" type_ref)?
paren_expr ::= "(" expression ")"
method_call ::= primary "." identifier ("[" type_ref ("," type_ref)* "]")? "(" (expression ("," expression)*)? ","? ")"
field_access ::= primary "." identifier
call ::= (identifier | contextual_identifier) ("[" type_ref ("," type_ref)* "]")? "(" (expression ("," expression)*)? ","? ")"
list_literal ::= "[" (expression ("," expression)* ","?)? "]"
record_construction ::= identifier "{" (field_init ("," field_init)*)? ","? "}"
field_init ::= identifier ":" expression | identifier
record_spread ::= identifier "{" "..." expression ("," field_init)* ","? "}" | "{" "..." expression ("," field_init)* ","? "}"
question_expr ::= expression "?"
ok_expr ::= "Ok" "(" expression ")"
err_expr ::= "Err" "(" expression ")"
some_expr ::= "Some" "(" expression ")"
none_expr ::= "None"
effect_pure_expr ::= "Effect" "." "pure" "(" expression ")"
val_expr ::= "Val" "[" type_ref "]" val_arg?
val_arg ::= "(" expression ("," expression)* ","? ")" | "{" (field_init ("," field_init)*)? ","? "}"
wire_expr ::= "Wire" "(" expression ")"
self_expr ::= "self"
identifier ::= /[A-Za-z][A-Za-z0-9_]*/
constant_name ::= /[A-Z][A-Za-z0-9_]*/
number_literal ::= /[0-9]+(_[0-9]+)*/
float_literal ::= /[0-9]+(_[0-9]+)*\.[0-9]+(_[0-9]+)*([eE][+-]?[0-9]+(_[0-9]+)*)?|[0-9]+(_[0-9]+)*[eE][+-]?[0-9]+(_[0-9]+)*/
string_literal ::= """ (/[^"\\\n]+/ | /\\[nt"\\]/ | string_interpolation)* """
string_interpolation ::= "\(" expression ")"
boolean_literal ::= "true" | "false"
unit_literal ::= "(" ")"
line_comment ::= "//" /[^\n]*/
```

## Tokens & trivia

- **Word token:** `identifier`
- **Ignored between tokens:** `/\s+/`, `doc_line`, `line_comment`, `doc_block`
- **External tokens:** `doc_block`, `doc_line`
