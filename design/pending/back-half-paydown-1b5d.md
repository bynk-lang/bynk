---
level: patch
changelog: Internal restructuring: the test harness builds the system suites' http, cron, queue and email drivers, their request-init objects, credentials and tokens as typed nodes instead of text, so a request's options object prints without a trailing comma and a driver's credential statements each print on their own line; no behaviour changes
probes: ts_raw_sites_testgen, testgen_print_calls, ts_writes
---
refactor: the system drivers are nodes (1B.5d)

Part of #4054. Part of #3676.

The cron, queue and email drivers (`system_jobs.rs`) and the http drivers (`system_support.rs`) are `TsDecl::Function` nodes. The request each http driver builds is nodes from `system_request.rs`: its header entries, URL template, request-init object (`request_init`), the secret reads its credentials sign with (`secret_read`), and the Bearer and OIDC tokens (`bearer_token`, `oidc_token`). The signing statements of a `Signature` route and of an actor sum (`__auth`), the `__token` and `__cookie` constants, the streamed `@raw` body and the two-token principals are statements, not text. `driver_param_ty` returns a `TsType`, `sysdrive_driver` takes the body statements and payload arguments as nodes, and `wrong_method_driver` is built in `system_request.rs`.

The harness no longer prints a node to compare or scan it: the two deps factories are compared with `==`, and a generic parameter's use is found with `unbound_names_in`. The observation helpers splice like the other runtime modules. `body_field`, `renamed_specs_arg` and the `template` builder are deleted.

`bynk_ts` prints a left-nested `??` chain flat (`a ?? b ?? c`), as it prints `+`, so a secret read keeps its text.

Goldens: see the PR.
