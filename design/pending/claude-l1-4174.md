---
level: patch
changelog: The workerd smoke tests take their ports from the OS, so two never collide and a wrangler that cannot start fails at once
---

## ADR: workerd-smokes-take-ports-from-the-os
title: The workerd smoke harness takes its ports from the OS and fails fast when wrangler exits
summary: Fixes the flaky migrate smoke (#4174), whose pid-derived ports collided and whose boot wait ignored a dead wrangler

**Context.** `workers_runtime_smoke::the_migrate_route_wakes_a_stored_instance_on_workerd` failed once in CI after
about 184 s, then passed in 5 s on retry (#4174). Under contention (ten copies of the test with CPU spinners)
the failure reproduced in three of ten runs, at the 180 s boot window each time. The cause was not workerd's
boot time: `serve_smoke_with_args` chose `base + pid % 10000` for wrangler's port (and `port + 1` for its inspector), so
concurrent tests could pick the same port. Wrangler then exited at once with `Address already in use`, and the
boot wait kept polling the dead process until its window ran out.

**Decision.** The harness asks the OS for two free loopback ports and passes them to wrangler. Its boot wait
checks whether wrangler has exited: if so it stops waiting and reports wrangler's own output at once, as a
failure when that output says a port was in use, and as the existing skip otherwise (no network to provision
wrangler). The assertions are unchanged and no retry is added.

**Consequences.** The same contention gives twenty passes in twenty. A deliberate break (the migrate wake
not committing) still fails the test on its second-wake assertion. The hello-world smoke in the same file keeps
its own pid-derived port and boot loop; it is outside this issue.
