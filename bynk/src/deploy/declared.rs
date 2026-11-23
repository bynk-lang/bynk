//! 1C.3 (#4040): a Worker's binding declarations, filled from the build's
//! `WorkerBindings`, the model its `wrangler.toml` was rendered from.
//!
//! [`declared_bindings`] is the one place that decides what each kind of
//! binding declares to the deploy: an exhaustive `match` on [`BindingSource`]
//! with no catch-all arm, so a new kind does not compile until it says.

use super::*;
use bynk_check::worker_bindings::{BindingSource, WorkerBinding};
use bynk_project::worker_layout::WorkerLayout;

/// The sixteen binding fields of [`Resources`], as one Worker's bindings
/// declare them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DeclaredBindings {
    pub(crate) binds_to: Vec<String>,
    pub(crate) foreign_services: Vec<ForeignService>,
    pub(crate) producer_queues: Vec<String>,
    pub(crate) needs_kv: bool,
    pub(crate) kv_instances: Vec<String>,
    pub(crate) r2_buckets: Vec<R2Decl>,
    pub(crate) vectorize_indexes: Vec<VectorizeDecl>,
    pub(crate) containers: Vec<ContainerDecl>,
    pub(crate) d1_databases: Vec<D1Decl>,
    pub(crate) hyperdrive: Vec<HyperdriveDecl>,
    pub(crate) ratelimits: Vec<String>,
    pub(crate) analytics_datasets: Vec<AnalyticsDecl>,
    pub(crate) stored_secrets: Vec<StoredSecretDecl>,
    pub(crate) ai_search: Vec<AiSearchDecl>,
    pub(crate) mtls_certificates: Vec<MtlsCertificateDecl>,
    pub(crate) email_senders: Vec<EmailSendersDecl>,
}

/// What the Worker in directory `dir` declares, from its `bindings`
/// (`WorkerBindings::of_worker`). `scripts` maps each Worker's script name to
/// its directory, and `layout` is the one the build used; `workers_dir` is
/// where the build staged each database's migrations.
///
/// A Service Binding to a Worker the build does not have is a foreign one: a
/// `ForeignService` whose `worker` is the script name of one of the build's
/// own Workers is still a project edge, as when the config was parsed (a
/// target is foreign when its script is not the build's, not when its source
/// said `Service`).
pub(crate) fn declared_bindings(
    workers_dir: &Path,
    dir: &str,
    bindings: &[WorkerBinding],
    layout: &WorkerLayout,
    scripts: &BTreeMap<String, String>,
) -> Result<DeclaredBindings, String> {
    use bynk_emit::emitter::wrangler as names;
    let mut out = DeclaredBindings::default();
    for binding in bindings {
        let name = binding.name.clone();
        match &binding.source {
            BindingSource::Service { context } => out.binds_to.push(layout.dir(context)),
            BindingSource::ForeignService { worker, .. } => match scripts.get(worker) {
                Some(target) => out.binds_to.push(target.clone()),
                None => out.foreign_services.push(ForeignService {
                    binding: name,
                    worker: worker.clone(),
                }),
            },
            BindingSource::Kv => out.needs_kv = true,
            BindingSource::KvInstance { .. } => out.kv_instances.push(name),
            BindingSource::R2Instance { jurisdiction, .. } => out.r2_buckets.push(R2Decl {
                name: names::r2_bucket_name(dir, &name),
                binding: name,
                jurisdiction: jurisdiction.clone(),
            }),
            BindingSource::RateLimitInstance { .. } => out.ratelimits.push(name),
            BindingSource::QueueProducer { queue, .. } => out.producer_queues.push(queue.clone()),
            BindingSource::AnalyticsDataset { .. } => out.analytics_datasets.push(AnalyticsDecl {
                name: names::analytics_dataset_name(dir, &name),
                binding: name,
            }),
            BindingSource::D1Database { .. } => {
                let database = bynk_check::d1_migrations::d1_database_name(dir, &name);
                let staged = workers_dir
                    .join(dir)
                    .join(names::D1_MIGRATIONS_DIR)
                    .join(&database);
                out.d1_databases.push(D1Decl {
                    migrations: if staged.is_dir() {
                        migration_files(&staged)?
                    } else {
                        Vec::new()
                    },
                    name: database,
                    binding: name,
                });
            }
            BindingSource::StoredSecret { store, secret, .. } => {
                out.stored_secrets.push(StoredSecretDecl {
                    binding: name,
                    store: store.clone(),
                    secret: secret.clone(),
                });
            }
            BindingSource::Email {
                allowed_senders, ..
            } => {
                if !allowed_senders.is_empty() {
                    out.email_senders.push(EmailSendersDecl {
                        binding: name,
                        senders: allowed_senders.clone(),
                    });
                }
            }
            BindingSource::Postgres { .. } => out.hyperdrive.push(HyperdriveDecl {
                binding: name,
                kind: HyperdriveKind::Postgres,
                tls: None,
            }),
            // The sidecar writes the default where the declaration left it out.
            BindingSource::MySql { tls, .. } => out.hyperdrive.push(HyperdriveDecl {
                binding: name,
                kind: HyperdriveKind::MySql,
                tls: Some(tls.clone().unwrap_or_else(|| "required".to_string())),
            }),
            BindingSource::VectorizeIndex {
                dimensions,
                metric,
                indexed,
                ..
            } => out.vectorize_indexes.push(VectorizeDecl {
                name: names::vectorize_index_name(dir, &name),
                binding: name,
                dimensions: *dimensions,
                metric: metric.clone(),
                indexed: indexed.clone(),
                adopt: None,
            }),
            BindingSource::ContainerInstance {
                capability,
                class_name,
                build,
                instance_type,
                ..
            } => out.containers.push(ContainerDecl {
                capability: capability.clone(),
                class: class_name.clone(),
                build: build.clone(),
                instance_type: instance_type.clone(),
                adopt: None,
            }),
            BindingSource::AiSearchInstance { instance, .. } => out.ai_search.push(AiSearchDecl {
                binding: name,
                instance: instance.clone(),
            }),
            BindingSource::MtlsCertificate { certificate, .. } => {
                out.mtls_certificates.push(MtlsCertificateDecl {
                    binding: name,
                    certificate: certificate.clone(),
                });
            }
            // A Durable Object is declared by the config's `exports`, which the
            // deploy reads as a setting, and a singleton is a plain table with
            // nothing to create or link.
            BindingSource::Singleton { .. }
            | BindingSource::Agent { .. }
            | BindingSource::EventsFanout { .. } => {}
        }
    }
    // A Worker calling itself is no edge (ADR 0874 D7).
    out.binds_to.retain(|target| target != dir);
    out.foreign_services
        .sort_by(|a, b| a.binding.cmp(&b.binding));
    out.producer_queues.sort();
    out.producer_queues.dedup();
    out.kv_instances.sort();
    out.r2_buckets.sort_by(|a, b| a.binding.cmp(&b.binding));
    out.vectorize_indexes
        .sort_by(|a, b| a.binding.cmp(&b.binding));
    out.containers.sort_by(|a, b| a.class.cmp(&b.class));
    out.d1_databases.sort_by(|a, b| a.binding.cmp(&b.binding));
    out.hyperdrive.sort_by(|a, b| a.binding.cmp(&b.binding));
    out.ratelimits.sort();
    out.analytics_datasets
        .sort_by(|a, b| a.binding.cmp(&b.binding));
    out.stored_secrets.sort_by(|a, b| a.binding.cmp(&b.binding));
    out.ai_search.sort_by(|a, b| a.binding.cmp(&b.binding));
    out.mtls_certificates
        .sort_by(|a, b| a.binding.cmp(&b.binding));
    out.email_senders.sort_by(|a, b| a.binding.cmp(&b.binding));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_project::worker_groups::{WorkerGroup, WorkerGroups};

    fn binding(name: &str, source: BindingSource) -> WorkerBinding {
        WorkerBinding {
            name: name.to_string(),
            source,
        }
    }

    fn declared(dir: &str, bindings: &[WorkerBinding]) -> DeclaredBindings {
        declared_in(Path::new("/nowhere"), dir, bindings, &BTreeMap::new())
    }

    fn declared_in(
        workers_dir: &Path,
        dir: &str,
        bindings: &[WorkerBinding],
        scripts: &BTreeMap<String, String>,
    ) -> DeclaredBindings {
        declared_bindings(
            workers_dir,
            dir,
            bindings,
            &WorkerLayout::identity(),
            scripts,
        )
        .unwrap()
    }

    /// A Service Binding to a context is a project edge to that context's
    /// Worker, and a Worker's own context is no edge to itself.
    #[test]
    fn a_service_binding_is_an_edge_to_the_contexts_worker() {
        let got = declared(
            "shop-orders",
            &[
                binding(
                    "SHOP_PAYMENT",
                    BindingSource::Service {
                        context: "shop.payment".into(),
                    },
                ),
                binding(
                    "SHOP_ORDERS",
                    BindingSource::Service {
                        context: "shop.orders".into(),
                    },
                ),
            ],
        );
        assert_eq!(got.binds_to, ["shop-payment"]);
        assert!(got.foreign_services.is_empty());
    }

    /// A group's members share one Worker, so a binding to any member is an edge
    /// to the group's directory, and a member calling another is no edge.
    #[test]
    fn a_service_binding_to_a_group_member_is_an_edge_to_the_group() {
        let layout = WorkerLayout::from_groups(&WorkerGroups::from([(
            "core".to_string(),
            WorkerGroup {
                contexts: vec!["shop.orders".to_string(), "shop.pricing".to_string()],
                ..WorkerGroup::default()
            },
        )]));
        let service = |context: &str| {
            binding(
                "SVC",
                BindingSource::Service {
                    context: context.into(),
                },
            )
        };
        let target = |dir, context| {
            declared_bindings(
                Path::new("/nowhere"),
                dir,
                &[service(context)],
                &layout,
                &BTreeMap::new(),
            )
            .unwrap()
            .binds_to
        };
        assert_eq!(target("shop-payment", "shop.pricing"), ["core"]);
        assert!(target("core", "shop.pricing").is_empty());
    }

    /// The foreign rule is the build's script names, not the variant (ADR 0193
    /// D3): a `Service` capability naming one of the build's own Workers is a
    /// project edge, and one naming none is foreign.
    #[test]
    fn a_foreign_service_is_foreign_unless_its_script_is_the_builds() {
        let scripts = BTreeMap::from([("core-script".to_string(), "core".to_string())]);
        let foreign = |name: &str, worker: &str| {
            binding(
                name,
                BindingSource::ForeignService {
                    capability: "Cap".into(),
                    worker: worker.into(),
                },
            )
        };
        let got = declared_in(
            Path::new("/nowhere"),
            "shop",
            &[
                foreign("SERVICE_LEGACY", "legacy-billing"),
                foreign("SERVICE_CORE", "core-script"),
                foreign("SERVICE_ARCHIVE", "archive"),
            ],
            &scripts,
        );
        assert_eq!(got.binds_to, ["core"]);
        assert_eq!(
            got.foreign_services,
            [
                ForeignService {
                    binding: "SERVICE_ARCHIVE".into(),
                    worker: "archive".into()
                },
                ForeignService {
                    binding: "SERVICE_LEGACY".into(),
                    worker: "legacy-billing".into()
                },
            ]
        );
    }

    /// The sidecar writes `required` where a `MySql` instance says no `tls`.
    #[test]
    fn a_mysql_binding_defaults_its_tls_and_a_postgres_one_has_none() {
        let mysql = |name: &str, tls: Option<&str>| {
            binding(
                name,
                BindingSource::MySql {
                    capability: "Cap".into(),
                    tls: tls.map(str::to_string),
                },
            )
        };
        let got = declared(
            "demo",
            &[
                mysql("HYPERDRIVE_B", None),
                mysql("HYPERDRIVE_A", Some("verify-full")),
                binding(
                    "HYPERDRIVE_C",
                    BindingSource::Postgres {
                        capability: "Cap".into(),
                    },
                ),
            ],
        );
        let shown: Vec<_> = got
            .hyperdrive
            .iter()
            .map(|h| (h.binding.as_str(), h.kind, h.tls.as_deref()))
            .collect();
        assert_eq!(
            shown,
            [
                ("HYPERDRIVE_A", HyperdriveKind::MySql, Some("verify-full")),
                ("HYPERDRIVE_B", HyperdriveKind::MySql, Some("required")),
                ("HYPERDRIVE_C", HyperdriveKind::Postgres, None),
            ]
        );
    }

    /// A database's migrations are the `.sql` files in its staged directory when
    /// it has one, sorted, and none when it has not.
    #[test]
    fn a_d1_databases_migrations_are_its_staged_files() {
        let dir = std::env::temp_dir().join(format!("bynk-declared-d1-{}", std::process::id()));
        let staged = dir.join("demo/migrations/demo-orders");
        std::fs::create_dir_all(&staged).unwrap();
        for file in ["0002_add.sql", "0001_init.sql", "notes.txt"] {
            std::fs::write(staged.join(file), "").unwrap();
        }
        let d1 = |name: &str| {
            binding(
                name,
                BindingSource::D1Database {
                    capability: "Cap".into(),
                },
            )
        };
        let got = declared_in(
            &dir,
            "demo",
            &[d1("D1_ORDERS"), d1("D1_AUDIT")],
            &BTreeMap::new(),
        );
        let _ = std::fs::remove_dir_all(&dir);
        let shown: Vec<_> = got
            .d1_databases
            .iter()
            .map(|d| (d.binding.as_str(), d.name.as_str(), d.migrations.clone()))
            .collect();
        assert_eq!(
            shown,
            [
                ("D1_AUDIT", "demo-audit", Vec::<String>::new()),
                (
                    "D1_ORDERS",
                    "demo-orders",
                    vec!["0001_init.sql".to_string(), "0002_add.sql".to_string()]
                ),
            ]
        );
    }

    /// One binding of each remaining kind projects to its field, with the
    /// account-facing names the renderer derives and each list in the order the
    /// parse sorted it.
    #[test]
    fn each_kind_declares_what_the_parse_read_for_it() {
        let cap = || "Cap".to_string();
        let got = declared(
            "demo",
            &[
                binding("KV", BindingSource::Kv),
                binding(
                    "KV_SESSIONS",
                    BindingSource::KvInstance { capability: cap() },
                ),
                binding("KV_CACHE", BindingSource::KvInstance { capability: cap() }),
                binding(
                    "R2_ARCHIVES",
                    BindingSource::R2Instance {
                        capability: cap(),
                        jurisdiction: Some("eu".into()),
                    },
                ),
                binding(
                    "RATELIMIT_LOGIN",
                    BindingSource::RateLimitInstance {
                        capability: cap(),
                        limit: 100,
                        period: 60,
                    },
                ),
                binding(
                    "QUEUE_ORDERS",
                    BindingSource::QueueProducer {
                        capability: cap(),
                        queue: "orders".into(),
                    },
                ),
                binding(
                    "QUEUE_AGAIN",
                    BindingSource::QueueProducer {
                        capability: cap(),
                        queue: "orders".into(),
                    },
                ),
                binding(
                    "ANALYTICS_SIGNUPS",
                    BindingSource::AnalyticsDataset { capability: cap() },
                ),
                binding(
                    "STORED_KEY",
                    BindingSource::StoredSecret {
                        capability: cap(),
                        store: "payments".into(),
                        secret: "stripe".into(),
                    },
                ),
                binding(
                    "EMAIL_OPEN",
                    BindingSource::Email {
                        capability: cap(),
                        to: None,
                        allowed_recipients: vec!["a@x.dev".into()],
                        allowed_senders: Vec::new(),
                    },
                ),
                binding(
                    "EMAIL_PINNED",
                    BindingSource::Email {
                        capability: cap(),
                        to: None,
                        allowed_recipients: Vec::new(),
                        allowed_senders: vec!["noreply@x.dev".into()],
                    },
                ),
                binding(
                    "VECTORIZE_DOCS",
                    BindingSource::VectorizeIndex {
                        capability: cap(),
                        dimensions: 768,
                        metric: "cosine".into(),
                        indexed: vec!["kind".into()],
                    },
                ),
                binding(
                    "CONTAINER_RENDERER",
                    BindingSource::ContainerInstance {
                        capability: "Renderer".into(),
                        class_name: "RendererContainer".into(),
                        build: "docker/render".into(),
                        root_from_worker: "../../..".into(),
                        port: 8080,
                        sleep_after: Some(30),
                        instance_type: Some("basic".into()),
                        forward: Vec::new(),
                    },
                ),
                binding(
                    "AI_SEARCH_HELP",
                    BindingSource::AiSearchInstance {
                        capability: cap(),
                        instance: "help-docs".into(),
                    },
                ),
                binding(
                    "MTLS_PARTNER",
                    BindingSource::MtlsCertificate {
                        capability: cap(),
                        certificate: "partner-cert".into(),
                    },
                ),
            ],
        );
        assert!(got.needs_kv);
        assert_eq!(got.kv_instances, ["KV_CACHE", "KV_SESSIONS"]);
        assert_eq!(
            got.r2_buckets,
            [R2Decl {
                binding: "R2_ARCHIVES".into(),
                name: "demo-archives".into(),
                jurisdiction: Some("eu".into()),
            }]
        );
        assert_eq!(got.ratelimits, ["RATELIMIT_LOGIN"]);
        assert_eq!(got.producer_queues, ["orders"]);
        assert_eq!(
            got.analytics_datasets,
            [AnalyticsDecl {
                binding: "ANALYTICS_SIGNUPS".into(),
                name: "demo_signups".into(),
            }]
        );
        assert_eq!(
            got.stored_secrets,
            [StoredSecretDecl {
                binding: "STORED_KEY".into(),
                store: "payments".into(),
                secret: "stripe".into(),
            }]
        );
        assert_eq!(
            got.email_senders,
            [EmailSendersDecl {
                binding: "EMAIL_PINNED".into(),
                senders: vec!["noreply@x.dev".into()],
            }]
        );
        assert_eq!(
            got.vectorize_indexes,
            [VectorizeDecl {
                binding: "VECTORIZE_DOCS".into(),
                name: "demo-docs".into(),
                dimensions: 768,
                metric: "cosine".into(),
                indexed: vec!["kind".into()],
                adopt: None,
            }]
        );
        assert_eq!(
            got.containers,
            [ContainerDecl {
                capability: "Renderer".into(),
                class: "RendererContainer".into(),
                build: "docker/render".into(),
                instance_type: Some("basic".into()),
                adopt: None,
            }]
        );
        assert_eq!(
            got.ai_search,
            [AiSearchDecl {
                binding: "AI_SEARCH_HELP".into(),
                instance: "help-docs".into(),
            }]
        );
        assert_eq!(
            got.mtls_certificates,
            [MtlsCertificateDecl {
                binding: "MTLS_PARTNER".into(),
                certificate: "partner-cert".into(),
            }]
        );
    }

    /// A Durable Object binding and a singleton declare nothing to the deploy:
    /// the first is a class in `exports`, the second a plain table.
    #[test]
    fn agents_fan_outs_and_singletons_declare_no_binding_resource() {
        let got = declared(
            "demo",
            &[
                binding(
                    "AGENT_COUNTER",
                    BindingSource::Agent {
                        class_name: "Counter".into(),
                    },
                ),
                binding(
                    "EVENTS_FANOUT",
                    BindingSource::EventsFanout {
                        class_name: "Fanout".into(),
                    },
                ),
                binding(
                    "VERSION",
                    BindingSource::Singleton {
                        capability: "VersionMetadata".into(),
                    },
                ),
            ],
        );
        assert_eq!(got, DeclaredBindings::default());
    }
}
