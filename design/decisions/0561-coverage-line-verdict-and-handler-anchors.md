# 0561 — Coverage line verdicts do not depend on V8 block splitting, and agent handler headers anchor to their own line

- **Status:** Accepted (v0.382.1)

**Context.** An agent handler's generated header and prologue inherited the previous statement's source-map anchor, so an uncalled handler's ranges landed on a neighbour's last body line. The coverage verdict then took the tightest V8 range among all generated lines mapping to a `.bynk` line, whatever its count, so a not-taken block (whose existence depends on which code ran) could flip an executed line to uncovered between a whole run and a single case (#2270).

**Decision.** The emitter anchors each agent handler's header and prologue to its own `on call` line, and the code after the last handler to the agent declaration. A `.bynk` line is covered if an executed V8 range maps to it, except that an uncalled function vetoes an executed range that strictly contains it (the hoisted-export case); a not-taken block never vetoes.

**Consequences.** The verdict is the same for a whole run and any subset of cases. An uncalled handler's own line is now reported uncovered. Service handler headers still map to the service declaration; that is tracked separately.
