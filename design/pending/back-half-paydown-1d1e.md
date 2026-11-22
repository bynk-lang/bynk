---
level: patch
changelog: Internal restructuring of `bynk deploy`; no change to plans or `bynk.deploy.lock`
probes: deploy_kind_accessors
---
refactor(deploy): the linked kinds and container images as `ResourceKind`s (1D.1e)

Closes #3929. Part of #3676.

No output changes. Stored secrets (`deploy/kind/stored_secret.rs`), AI Search instances (`deploy/kind/ai_search.rs`), mTLS certificates (`deploy/kind/mtls_certificate.rs`) and container images (`deploy/kind/container_image.rs`) implement `ResourceKind`, which completes the kind list. A linked kind is looked up before the run confirms, as before (the ids reach it through `Lives::linked`); its `provision_all` records only the links that changed, writes the ledger once with its own message, and the stored secrets and certificates then bind their ids into the configuration. A container's adopted image is named in the configuration and then recorded (or, when it is built again, forgotten) through the kind, with the ledger written for every Worker that has a container. None of the four reports orphans. The plan's datasets, foreign services, email addresses and Durable Object classes keep their places in the short format as `Slot`s in the list. `record_stored_secret`, `record_ai_search`, `record_mtls_certificate`, `record_container_image`, `recorded_stored_secret`, `recorded_ai_search`, `recorded_mtls_certificate`, `recorded_container_image`, `record_links`, `record_mtls_links`, `record_ai_search_links`, `record_linked_resources`, `adopt_containers_for_push` and `record_container_images` are deleted. From the 1D.1d review, the queue loop ends its borrow of the run's attempts before making any queue. The byte-identity fixtures are unchanged. `deploy_kind_accessors` falls from 11 to 0.
