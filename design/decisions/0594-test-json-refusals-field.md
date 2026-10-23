# 0594 — A failed history reports the declaration that refused each of its steps

- **Status:** Accepted (v0.395.2)

**Context.** A driven history records an agent's refusal as data on the step
(`accepted: false`, ADR 0155 D4), not as a failure, so a history that checks
`accepted` fails with a message that names the call and not the `invariant` or
`transition` that refused it. The `violation` field of a failed case (extending
ADR 0572) cannot carry it: the refusal is not what failed the case, and one run
can refuse several steps.

**Decision.** Each step the driver records as refused carries the refusal's record
as `refusal`. A failed history re-drives its shrunk sequence and reports the
refused steps of that run as `refusals`, after `violation`: a list of
`{ index, call, kind, agent, name, location }`, where `index` is the step's
position in the reported sequence (from 0) and `call` the handler it called. The
rest is the declaration, as `violation` gives it. The field is absent when no step
was refused, and a malformed entry is dropped. It is carried by the JSON document
and the `ndjson` `case` event, ahead of `durationMs`, and read by `dev_run`.

**Consequences.** A tool can show, for each refused step of a failing history, which
declaration refused it and open it. `refusals` says what was refused, not that the
refusal caused the failure: a history that expected a refusal fails for another
reason and still lists it. Only the shrunk sequence is described; a refusal in a
run that passed is not reported. A `system` suite, a remote run and a property are
unchanged, as none of them drives a history through this driver.
