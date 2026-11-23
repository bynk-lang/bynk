use super::serving::Serving;
use super::stand_ins::StandIn;
use super::stand_ins::stand_in_name;
use super::watch::persist_to_args;
use crate::cloudflare::CloudflareApi;
use crate::probe::Provenance;
use bynk_driver::colour;
use bynk_driver::emsg;
use std::path::Path;

/// The config every context is served from: its generated `wrangler.toml`
/// with the development bindings applied and each binding to a foreign Worker
/// pointed at its stand-in, beside the original so `main` resolves the same.
pub const DEV_CONFIG: &str = "wrangler.dev.toml";

/// Write each served context's [`DEV_CONFIG`] from its generated
/// `wrangler.toml`, the last step after every build. A copy, not an edit: a
/// rebuild rewrites `wrangler.toml` with the deploy's names (its real Workers,
/// its derived Vectorize indexes), and `wrangler dev` watching it would reload
/// with those bound (review of #2008; #3505) before the development bindings
/// replaced them. The copy is replaced by a rename once they are all applied,
/// so wrangler only ever reads it whole, and only when it changes.
pub(super) fn publish_dev_configs(
    workers_dir: &Path,
    serving: &[Serving],
    plans: &[(String, Vec<StandIn>)],
) -> Result<(), String> {
    for context in serving.iter().map(|s| &s.worker) {
        let targets: std::collections::BTreeMap<String, String> = plans
            .iter()
            .filter(|(c, _)| c == context)
            .flat_map(|(_, stand_ins)| stand_ins)
            .flat_map(|s| {
                let name = stand_in_name(context, &s.worker);
                s.bindings.iter().map(move |b| (b.clone(), name.clone()))
            })
            .collect();
        let dir = workers_dir.join(context);
        let text = std::fs::read_to_string(dir.join("wrangler.toml")).map_err(|e| e.to_string())?;
        let out = bynk_emit::emitter::wrangler::retarget_services(&text, &targets)?;
        let path = dir.join(DEV_CONFIG);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(out.as_str()) {
            let staged = dir.join(format!("{DEV_CONFIG}.tmp"));
            std::fs::write(&staged, out).map_err(|e| e.to_string())?;
            std::fs::rename(&staged, &path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Fill each served Worker's generated config from the deploy ledger, for
/// `-- --remote`. A never-deployed project gets an actionable error instead of
/// sending a generated placeholder to Cloudflare; `false` after printing it.
pub(super) fn materialise_remote_state(
    project_root: &Path,
    workers_dir: &Path,
    serving: &[Serving],
    environment: &str,
) -> bool {
    for s in serving {
        let config = workers_dir.join(&s.worker).join("wrangler.toml");
        if let Err(e) =
            crate::deploy::materialise_deploy_state(project_root, &s.worker, &config, environment)
        {
            emsg!("bynk: error: {e}");
            return false;
        }
    }
    true
}

/// What binding the served Workers' `Vectorize` instances for development did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DevVectorize {
    /// Bound as `[dev.vectorize-index]` says (or nothing to bind).
    Bound,
    /// Not bound, but every instance is left out, so the config names no
    /// deploy index: safe to serve.
    Unbound,
    /// Neither bound nor unbound: the config may still name the deploy's
    /// indexes, so it must not be served (#3505 review).
    Unsafe,
}

impl DevVectorize {
    /// Whether the configs may then be served: never with the deploy's indexes
    /// possibly still bound, and on the first build only once bound as asked.
    fn may_publish(self, announce: bool) -> bool {
        match self {
            DevVectorize::Bound => true,
            DevVectorize::Unbound => !announce,
            DevVectorize::Unsafe => false,
        }
    }
}

/// #3479: bind the served Workers' `Vectorize` instances for development
/// (see [`crate::dev_vectorize`]), printing why not on a failure and, when
/// `announce`, the one note of what is bound. `--remote` keeps the account's
/// deployed names, so it binds nothing.
fn bind_dev_vectorize(
    project_root: &Path,
    workers_dir: &Path,
    available: &[String],
    serving: &[Serving],
    remote: bool,
    announce: bool,
    model: &bynk_emit::emit_model::WorkersBuild,
) -> DevVectorize {
    if remote {
        return DevVectorize::Bound;
    }
    let names: Vec<String> = serving.iter().map(|s| s.worker.clone()).collect();
    match crate::dev_vectorize::bind_for_dev(project_root, workers_dir, available, &names, model) {
        Ok(bound) => {
            if announce && let Some(note) = crate::dev_vectorize::report_note(&bound) {
                colour::eprintln(&note);
            }
            DevVectorize::Bound
        }
        // The first build stops here, before anything is served.
        Err(e) if announce => {
            emsg!("bynk: error: {e}");
            DevVectorize::Unsafe
        }
        Err(e) => {
            // A rebuild keeps serving.
            emsg!("bynk: warning: {e}");
            // A rebuild's configs name the deploy's indexes again: never serve
            // them. Unbound, a call to an instance faults until `bynk.toml` reads.
            match crate::dev_vectorize::unbind_all(workers_dir, &names) {
                Ok(()) => {
                    emsg!(
                        "bynk dev: warning: Vectorize instances are unbound until `bynk.toml` is fixed; calls to them fail"
                    );
                    DevVectorize::Unbound
                }
                Err(e) => {
                    emsg!("bynk: warning: could not unbind the Vectorize instances: {e}");
                    DevVectorize::Unsafe
                }
            }
        }
    }
}

/// #3319: the local connection strings of the served Workers' `Postgres`
/// instances, or `None` after printing why not. `--remote` runs against the
/// account's Hyperdrive config, so no local string applies.
pub(super) fn hyperdrive_local_env(
    project_root: &Path,
    workers_dir: &Path,
    serving: &[Serving],
    remote: bool,
) -> Option<crate::dev_hyperdrive::LocalConnections> {
    if remote {
        return Some(Default::default());
    }
    let workers: Vec<String> = serving.iter().map(|s| s.worker.clone()).collect();
    let lookup = |name: &str| std::env::var(name).ok();
    match crate::dev_hyperdrive::local_connections(project_root, workers_dir, &workers, &lookup) {
        Ok((strings, warnings)) => {
            for warning in warnings {
                emsg!("bynk: warning: {warning}");
            }
            Some(strings)
        }
        Err(e) => {
            emsg!("bynk: error: {e}");
            None
        }
    }
}

/// #3301: whether the table `wrangler secrets-store secret list` printed has
/// a row for secret `name`: the name is a whole cell of the first column, so
/// a longer name that merely starts with it does not count. Only names are
/// ever in that table, never a value.
fn secret_listed(stdout: &str, name: &str) -> bool {
    stdout
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix('│'))
        .filter_map(|row| row.split('│').next())
        .any(|cell| cell.trim() == name)
}

/// #3301: what `bynk dev` says of a stored secret the local Secrets Store
/// does not hold. Under `bynk dev` the binding reads the local store, never
/// the account's (a production secret cannot be read locally), whose id is the
/// store's name as written.
fn missing_local_secret_hint(worker: &str, binding: &str, store: &str, secret: &str) -> String {
    format!(
        "bynk dev: warning: `{worker}` reads secret `{secret}` of Secrets Store `{store}` through `{binding}`, from the local store, which does not have it, so reading it will fault; create it with `wrangler secrets-store secret create {store} --name {secret} --scopes workers` (no `--remote`; it asks for the value)"
    )
}

/// #3301: warn about each served Worker's stored secret that the local
/// Secrets Store lacks (`wrangler secrets-store secret list <store>`, local,
/// from the Worker's directory where `wrangler dev` keeps its state). A list
/// that cannot be read warns too: the secret may be there, but it was not
/// confirmed. The session keeps serving.
pub(super) fn warn_missing_local_secrets(
    provenance: &Provenance,
    workers_dir: &Path,
    serving: &[Serving],
    wrangler_args: &[String],
) {
    for s in serving {
        let worker_dir = workers_dir.join(&s.worker);
        let Ok(config) = std::fs::read_to_string(worker_dir.join("wrangler.toml")) else {
            continue;
        };
        let Ok(entries) = bynk_emit::emitter::wrangler::stored_secret_entries(&config) else {
            continue;
        };
        for (binding, store, secret) in entries {
            let listed = crate::cloudflare::Wrangler::new(provenance, &worker_dir)
                .secrets_store_list_local(&worker_dir, &store, &persist_to_args(wrangler_args));
            if listed.as_ref().is_err_and(|e| e.is_missing()) {
                return;
            }
            let present = listed.is_ok_and(|stdout| secret_listed(&stdout, &secret));
            if !present {
                eprintln!(
                    "{}",
                    missing_local_secret_hint(&s.worker, &binding, &store, &secret)
                );
            }
        }
    }
}

/// What a build's development configs are made from.
pub(super) struct DevConfigs<'a> {
    pub(super) project_root: &'a Path,
    pub(super) workers_dir: &'a Path,
    /// Every Worker of the build.
    pub(super) available: &'a [String],
    pub(super) serving: &'a [Serving],
    pub(super) stand_ins: &'a [(String, Vec<StandIn>)],
    pub(super) remote: bool,
    pub(super) environment: &'a str,
}

impl DevConfigs<'_> {
    /// Make each served Worker's development config from the build's generated
    /// one, after every build. `announce` on the first build, which fails on what a
    /// rebuild only reports. `false` after printing why.
    ///
    /// #3505: every change is made to the generated config, which nothing serves,
    /// and the served [`DEV_CONFIG`] is replaced whole only after the last of
    /// them, so `wrangler dev` never reloads with the deploy's names bound.
    pub(super) fn write(
        &self,
        model: &bynk_emit::emit_model::WorkersBuild,
        announce: bool,
    ) -> bool {
        dev_config_patches(
            self.project_root,
            self.workers_dir,
            self.serving,
            self.environment,
        );
        // #3479: each `Vectorize` instance is bound to the index
        // `[dev.vectorize-index]` names, or left unbound.
        let bound = bind_dev_vectorize(
            self.project_root,
            self.workers_dir,
            self.available,
            self.serving,
            self.remote,
            announce,
            model,
        );
        if !bound.may_publish(announce) {
            if bound == DevVectorize::Unsafe && !announce {
                emsg!("bynk dev: still serving the last good build's configs");
            }
            return false;
        }
        match publish_dev_configs(self.workers_dir, self.serving, self.stand_ins) {
            Ok(()) => bound == DevVectorize::Bound,
            Err(e) => {
                // At start the session stops; on a rebuild it keeps serving.
                let label = if announce { "error" } else { "warning" };
                emsg!("bynk: {label}: could not write the development configs: {e}");
                false
            }
        }
    }
}

/// What `bynk dev` changes in each Worker's development copy of its config: the
/// remote-only bindings (AI Search, #3480; mTLS certificates, #3513), and the
/// agent-inspection registry (#3581).
fn dev_config_patches(
    project_root: &Path,
    workers_dir: &Path,
    serving: &[Serving],
    environment: &str,
) {
    proxy_remote_bindings(project_root, workers_dir, serving, environment);
    bind_inspect_registry(workers_dir, serving);
}

/// #3581: bind the agent-inspection registry in each Worker's development copy of its
/// config, for a Worker whose entry exports its class (one that hosts an agent). The
/// config `bynk deploy` reads is a different build and never carries it.
fn bind_inspect_registry(workers_dir: &Path, serving: &[Serving]) {
    for s in serving {
        let dir = workers_dir.join(&s.worker);
        let (Ok(text), Ok(entry)) = (
            std::fs::read_to_string(dir.join("wrangler.toml")),
            std::fs::read_to_string(dir.join("index.ts")),
        ) else {
            continue;
        };
        match bynk_emit::emitter::wrangler::enable_agent_inspection(&text, &entry) {
            Ok(out) if out != text => {
                if let Err(e) = std::fs::write(dir.join("wrangler.toml"), out) {
                    emsg!(
                        "bynk: warning: could not bind `{}`'s inspect registry: {e}",
                        s.worker
                    );
                }
            }
            Ok(_) => {}
            Err(e) => emsg!(
                "bynk: warning: could not bind `{}`'s inspect registry: {e}",
                s.worker
            ),
        }
    }
}

/// #3480: each served Worker's development copy of its config proxies an
/// `[[ai_search]]` binding to the deployed instance (`remote = true`), since
/// AI Search has no local simulation. The generated config `bynk deploy`
/// reads is written elsewhere and never carries it. A failure is reported and
/// the Worker serves without, so a call faults naming the binding.
fn proxy_ai_search(workers_dir: &Path, serving: &[Serving]) {
    for s in serving {
        let path = workers_dir.join(&s.worker).join("wrangler.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match bynk_emit::emitter::wrangler::enable_remote_ai_search(&text) {
            Ok(out) if out != text => {
                if let Err(e) = std::fs::write(&path, out) {
                    emsg!(
                        "bynk: warning: could not proxy `{}`'s AI Search bindings to the account: {e}",
                        s.worker
                    );
                }
            }
            Ok(_) => {}
            Err(e) => emsg!(
                "bynk: warning: could not proxy `{}`'s AI Search bindings: {e}",
                s.worker
            ),
        }
    }
}

/// #3513 A: each served Worker's development copy of its config runs an `[[mtls_certificates]]`
/// binding on Cloudflare (`remote = true`), bound by the certificate id the last deploy of
/// `environment` recorded, since mTLS has no local simulation. A certificate no deploy has linked
/// is reported, and the Worker serves with it unbound, so a call through it fails.
fn proxy_mtls(project_root: &Path, workers_dir: &Path, serving: &[Serving], environment: &str) {
    for s in serving {
        let path = workers_dir.join(&s.worker).join("wrangler.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !text.contains("mtls_certificates") {
            continue;
        }
        if let Err(e) =
            crate::deploy::materialise_mtls_for_dev(project_root, &s.worker, &path, environment)
        {
            emsg!("bynk: warning: {e}");
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match bynk_emit::emitter::wrangler::enable_remote_mtls(&text) {
            Ok(out) if out != text => {
                if let Err(e) = std::fs::write(&path, out) {
                    emsg!(
                        "bynk: warning: could not run `{}`'s mTLS bindings on the account: {e}",
                        s.worker
                    );
                }
            }
            Ok(_) => {}
            Err(e) => emsg!(
                "bynk: warning: could not run `{}`'s mTLS bindings: {e}",
                s.worker
            ),
        }
    }
}

/// #3513 B: each served Worker's development copy runs its `[analytics]` (Analytics SQL)
/// binding on Cloudflare (`remote = true`), since it has no local implementation.
fn proxy_analytics_sql(workers_dir: &Path, serving: &[Serving]) {
    for s in serving {
        let path = workers_dir.join(&s.worker).join("wrangler.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !text.contains("[analytics]") {
            continue;
        }
        match bynk_emit::emitter::wrangler::enable_remote_analytics_sql(&text) {
            Ok(out) if out != text => {
                if let Err(e) = std::fs::write(&path, out) {
                    emsg!(
                        "bynk: warning: could not run `{}`'s Analytics SQL binding on the account: {e}",
                        s.worker
                    );
                }
            }
            Ok(_) => {}
            Err(e) => emsg!(
                "bynk: warning: could not run `{}`'s Analytics SQL binding: {e}",
                s.worker
            ),
        }
    }
}

/// The remote-only bindings of the development copy: AI Search (#3480), mTLS certificates
/// (#3513 A) and Analytics SQL (#3513 B).
fn proxy_remote_bindings(
    project_root: &Path,
    workers_dir: &Path,
    serving: &[Serving],
    environment: &str,
) {
    proxy_ai_search(workers_dir, serving);
    proxy_mtls(project_root, workers_dir, serving, environment);
    proxy_analytics_sql(workers_dir, serving);
}

/// #3478: the `--var`s of [`dev_var_args`] for `project_root`'s `[dev]`
/// acknowledgements, or `None` after printing why not.
pub(super) fn dev_vars_for(project_root: &Path) -> Option<Vec<String>> {
    match bynk_project::dev_ack::try_read_dev_acks(project_root) {
        Ok(acks) => {
            let mut args = dev_var_args(&acks);
            // #3581: the local key `bynk dev inspect` signs under, as `bynk.dev`.
            args.extend([
                "--var".to_string(),
                format!(
                    "{}:{}",
                    bynk_emit::INSPECT_CALLER_KEY_VAR,
                    bynk_emit::LOCAL_CALL_KEY
                ),
            ]);
            Some(args)
        }
        Err(e) => {
            emsg!("bynk: error: {e}");
            None
        }
    }
}

/// #3478: the `--var`s every served Worker gets: `BYNK_DEV` marks a
/// development session, and each acknowledged remote-only kind (`[dev] ai =
/// "allow"`) sets its variable to `allow`, which its provider reads to stay
/// quiet. A deployed Worker has none of them.
fn dev_var_args(acks: &std::collections::BTreeSet<&'static str>) -> Vec<String> {
    use bynk_project::dev_ack::{ALLOW, DEV_VAR, REMOTE_KINDS};
    let mut args = vec!["--var".to_string(), format!("{DEV_VAR}:1")];
    for kind in REMOTE_KINDS.iter().filter(|k| acks.contains(k.key)) {
        args.push("--var".to_string());
        args.push(format!("{}:{ALLOW}", kind.var));
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev::serving::Serving;
    use crate::dev::stand_ins::StandIn;

    use crate::dev::test_support::generated_shop_config;
    use crate::dev::test_support::names;
    use crate::dev::test_support::serving_shop;
    use crate::dev::test_support::shop_model;
    use crate::dev::test_support::vectorize_project;
    use std::collections::BTreeMap;

    /// #3478: every served Worker is marked as a development session, and an
    /// acknowledged remote-only kind sets its own variable to `allow`.
    #[test]
    fn dev_vars_mark_the_session_and_carry_acknowledgements() {
        use std::collections::BTreeSet;
        assert_eq!(dev_var_args(&BTreeSet::new()), ["--var", "BYNK_DEV:1"]);
        assert_eq!(
            dev_var_args(&BTreeSet::from(["ai"])),
            ["--var", "BYNK_DEV:1", "--var", "BYNK_DEV_ACK_AI:allow"]
        );
        // #3479: Vectorize shares the mechanism, under its own key and variable.
        assert_eq!(
            dev_var_args(&BTreeSet::from(["vectorize"])),
            [
                "--var",
                "BYNK_DEV:1",
                "--var",
                "BYNK_DEV_ACK_VECTORIZE:allow"
            ]
        );
        // #3480: AI Search has its own acknowledgement.
        assert_eq!(
            dev_var_args(&BTreeSet::from(["ai-search"])),
            [
                "--var",
                "BYNK_DEV:1",
                "--var",
                "BYNK_DEV_ACK_AI_SEARCH:allow"
            ]
        );
        // #3513 A: and so does an mTLS certificate binding.
        assert_eq!(
            dev_var_args(&BTreeSet::from(["mtls"])),
            ["--var", "BYNK_DEV:1", "--var", "BYNK_DEV_ACK_MTLS:allow"]
        );
        // #3513 B: and Analytics SQL.
        assert_eq!(
            dev_var_args(&BTreeSet::from(["analytics-sql"])),
            [
                "--var",
                "BYNK_DEV:1",
                "--var",
                "BYNK_DEV_ACK_ANALYTICS_SQL:allow"
            ]
        );
    }

    #[test]
    fn a_secret_is_listed_only_by_a_whole_name() {
        let table = "🔐 Listing secrets...\n┌─────────────┬────┐\n│ Name        │ ID │\n├─────────────┼────┤\n│ stripe-live │ 1  │\n└─────────────┴────┘\n";
        assert!(secret_listed(table, "stripe-live"));
        assert!(!secret_listed(table, "stripe"));
        assert!(!secret_listed(table, "stripe-live-2"));
        assert!(!secret_listed("", "stripe-live"));
    }

    #[test]
    fn a_missing_local_secret_is_told_how_to_create_it_locally() {
        let hint =
            missing_local_secret_hint("shop", "SECRET_STRIPE_KEY", "payments", "stripe-live");
        assert!(
            hint.contains(
                "wrangler secrets-store secret create payments --name stripe-live --scopes workers"
            ),
            "{hint}"
        );
        assert!(hint.contains("no `--remote`"), "{hint}");
    }

    /// The served copy points the binding at the stand-in; the generated
    /// config keeps the real name (review of #2008).
    #[test]
    fn the_stand_in_config_is_a_retargeted_copy() {
        let dir = std::env::temp_dir().join(format!("bynk-stand-in-copy-{}", std::process::id()));
        let ctx = dir.join("shop-billing");
        std::fs::create_dir_all(&ctx).unwrap();
        let generated = "name = \"shop-billing\"\n\n[[services]]\nbinding = \"SERVICE_BILLING\"\nservice = \"legacy-billing\"\n";
        std::fs::write(ctx.join("wrangler.toml"), generated).unwrap();
        let plans = vec![(
            "shop-billing".to_string(),
            vec![StandIn {
                worker: "legacy-billing".into(),
                bindings: names(&["SERVICE_BILLING"]),
            }],
        )];
        let serving = vec![Serving {
            worker: "shop-billing".to_string(),
            port: None,
            inspector_port: None,
            inspect: false,
        }];
        publish_dev_configs(&dir, &serving, &plans).unwrap();
        assert_eq!(
            std::fs::read_to_string(ctx.join("wrangler.toml")).unwrap(),
            generated
        );
        let served = std::fs::read_to_string(ctx.join(DEV_CONFIG)).unwrap();
        assert!(
            served.contains("service = \"bynk-stand-in-shop-billing-legacy-billing\""),
            "{served}"
        );
        assert!(!ctx.join(format!("{DEV_CONFIG}.tmp")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #3505: a rebuild writes the generated config with the deploy's derived
    /// index names. What `wrangler dev` serves from must never carry them, not
    /// even between the compile and the development rebind.
    #[test]
    fn a_rebuild_never_exposes_the_deploys_index_to_wrangler() {
        let (root, workers) = vectorize_project("rebuild", "");
        let available = names(&["shop"]);
        let serving = serving_shop();
        let configs = DevConfigs {
            project_root: &root,
            workers_dir: &workers,
            available: &available,
            serving: &serving,
            stand_ins: &[],
            remote: false,
            environment: "default",
        };
        assert!(configs.write(&shop_model(), true));
        let served = workers.join("shop").join(DEV_CONFIG);
        let first = std::fs::read_to_string(&served).unwrap();
        assert!(first.contains("index_name = \"docs-dev\""), "{first}");
        assert!(!first.contains("shop-docs"), "{first}");

        // The rebuild's compile rewrites the generated config.
        std::fs::write(
            workers.join("shop").join("wrangler.toml"),
            generated_shop_config(""),
        )
        .unwrap();
        let between = std::fs::read_to_string(&served).unwrap();
        assert!(
            !between.contains("shop-docs"),
            "served before the rebind: {between}"
        );

        assert!(configs.write(&shop_model(), false));
        let after = std::fs::read_to_string(&served).unwrap();
        assert!(after.contains("index_name = \"docs-dev\""), "{after}");
        assert!(!after.contains("shop-docs"), "{after}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #3505 review: a rebuild that cannot bind serves its Workers unbound, never
    /// with the deploy's index; one whose config may still name it serves nothing
    /// new.
    #[test]
    fn a_rebuild_that_cannot_bind_never_publishes_the_deploys_index() {
        let (root, workers) = vectorize_project("unbound", "");
        let available = names(&["shop"]);
        let serving = serving_shop();
        let configs = DevConfigs {
            project_root: &root,
            workers_dir: &workers,
            available: &available,
            serving: &serving,
            stand_ins: &[],
            remote: false,
            environment: "default",
        };
        assert!(configs.write(&shop_model(), true));
        // `vars` that is not a table: the binding cannot edit the config.
        std::fs::write(
            workers.join("shop").join("wrangler.toml"),
            generated_shop_config("").replace(
                "name = \"shop\"\n",
                "name = \"shop\"\nvars = \"not a table\"\n",
            ),
        )
        .unwrap();
        assert!(!configs.write(&shop_model(), false));
        let served = std::fs::read_to_string(workers.join("shop").join(DEV_CONFIG)).unwrap();
        assert!(!served.contains("shop-docs"), "{served}");
        assert!(!served.contains("VECTORIZE_DOCS"), "{served}");
        let _ = std::fs::remove_dir_all(&root);

        // A config the unbinding could not edit (a failed write) is never
        // published; the last good one keeps serving.
        assert!(!DevVectorize::Unsafe.may_publish(false));
        assert!(!DevVectorize::Unsafe.may_publish(true));
        assert!(DevVectorize::Unbound.may_publish(false));
        assert!(!DevVectorize::Unbound.may_publish(true));
        assert!(DevVectorize::Bound.may_publish(true));
    }

    /// #3505: a Worker served with stand-ins is served from its copy, and the
    /// copy carries the development bindings too.
    #[test]
    fn a_worker_with_stand_ins_serves_the_development_index() {
        let (root, workers) = vectorize_project(
            "stand-ins",
            "\n[[services]]\nbinding = \"SERVICE_BILLING\"\nservice = \"legacy-billing\"\n",
        );
        let available = names(&["shop"]);
        let serving = serving_shop();
        let plans = vec![(
            "shop".to_string(),
            vec![StandIn {
                worker: "legacy-billing".into(),
                bindings: names(&["SERVICE_BILLING"]),
            }],
        )];
        let configs = DevConfigs {
            project_root: &root,
            workers_dir: &workers,
            available: &available,
            serving: &serving,
            stand_ins: &plans,
            remote: false,
            environment: "default",
        };
        assert!(configs.write(&shop_model(), true));
        let served = std::fs::read_to_string(workers.join("shop").join(DEV_CONFIG)).unwrap();
        assert!(
            served.contains("service = \"bynk-stand-in-shop-legacy-billing\""),
            "{served}"
        );
        assert!(served.contains("index_name = \"docs-dev\""), "{served}");
        assert!(!served.contains("shop-docs"), "{served}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An mTLS certificate's name is replaced in the served copy by the ledger's
    /// id (or the entry dropped), which the model, still naming the certificate,
    /// must not be read against: `bynk dev` reads Worker bindings from the model,
    /// never from the files it patches.
    #[test]
    fn the_model_is_checked_against_the_build_before_the_dev_patches_edit_it() {
        use bynk_check::worker_bindings::{BindingSource, WorkerBinding};
        let (root, workers) = vectorize_project(
            "mtls",
            "\n[[mtls_certificates]]\nbinding = \"MTLS_BACKEND\"\ncertificate_id = \"acme-client\"\n",
        );
        let binding = |name: &str, source| WorkerBinding {
            name: name.to_string(),
            source,
        };
        let model = bynk_emit::emit_model::WorkersBuild {
            bindings: bynk_emit::emitter::WorkerBindings::of_workers(BTreeMap::from([(
                "shop".to_string(),
                vec![
                    binding(
                        "VECTORIZE_DOCS",
                        BindingSource::VectorizeIndex {
                            capability: "Docs".to_string(),
                            dimensions: 768,
                            metric: "cosine".to_string(),
                            indexed: Vec::new(),
                        },
                    ),
                    binding(
                        "MTLS_BACKEND",
                        BindingSource::MtlsCertificate {
                            capability: "Backend".to_string(),
                            certificate: "acme-client".to_string(),
                        },
                    ),
                ],
            )])),
            layout: bynk_project::worker_layout::WorkerLayout::identity(),
        };
        let available = names(&["shop"]);
        let serving = serving_shop();
        let configs = DevConfigs {
            project_root: &root,
            workers_dir: &workers,
            available: &available,
            serving: &serving,
            stand_ins: &[],
            remote: false,
            environment: "default",
        };
        assert!(configs.write(&model, true));
        // The patch ran: the unlinked certificate is not in the served copy.
        let served = std::fs::read_to_string(workers.join("shop").join(DEV_CONFIG)).unwrap();
        assert!(!served.contains("acme-client"), "{served}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
