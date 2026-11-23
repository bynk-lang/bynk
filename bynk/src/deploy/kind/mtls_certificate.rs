//! 1D.1e (#3929): mTLS certificates, each `MtlsCertificate` instance, as one [`ResourceKind`].
//!
//! An `MtlsCertificate` instance (#3513 A) presents an account's client certificate. The record is the certificate's name and id, never any key material: the deploy never asks for it. It is what `bynk dev` binds by.
//!
//! Linked, never created: the deploy looks it up before the run confirms, records what it was linked
//! to before the push (one write for the Worker), and never orphans or prunes it. Every deploy
//! refreshes the record.

use super::*;

/// The mTLS certificate kind.
pub(crate) struct MtlsCertificate;

/// [`MtlsCertificate`]'s records: `Environment::mtls_certificate_links`, by Worker and binding.
pub(crate) struct MtlsCertificateTable;

impl LedgerTable for MtlsCertificateTable {
    type Record = MtlsCertificateLink;

    fn get<'l>(
        env: &'l Environment,
        worker: &str,
        binding: &str,
    ) -> Option<&'l MtlsCertificateLink> {
        env.mtls_certificate_links
            .get(worker)
            .and_then(|links| links.get(binding))
    }

    fn insert(env: &mut Environment, worker: &str, binding: &str, record: MtlsCertificateLink) {
        env.mtls_certificate_links
            .entry(worker.to_string())
            .or_default()
            .insert(binding.to_string(), record);
    }

    fn remove(env: &mut Environment, worker: &str, binding: &str) {
        if let Some(links) = env.mtls_certificate_links.get_mut(worker) {
            links.remove(binding);
            if links.is_empty() {
                env.mtls_certificate_links.remove(worker);
            }
        }
    }

    fn entries(env: &Environment) -> Vec<(&str, &str, &MtlsCertificateLink)> {
        env.mtls_certificate_links
            .iter()
            .flat_map(|(worker, links)| {
                links
                    .iter()
                    .map(move |(binding, link)| (worker.as_str(), binding.as_str(), link))
            })
            .collect()
    }
}

impl ResourceKind for MtlsCertificate {
    const ID: KindId = KindId::MtlsCertificate;

    type Decl<'r> = &'r MtlsCertificateDecl;
    type Record = MtlsCertificateLink;
    type Table = MtlsCertificateTable;
    type Line<'r> = PlanMtlsCertificate<'r>;
    type Orphan = NoOrphan;
    /// The certificates the run linked each `(worker, binding)` to, by id ([`link_for_run`]).
    type Live<'a> = Option<&'a BTreeMap<(String, String), String>>;
    /// The certificate's id, which the config binds it by.
    type Bound = String;
    type Change = TableChange<MtlsCertificateTable>;
    type Step<'r> = MtlsCertificateStep<'r>;

    fn declared(r: &Resources) -> Vec<&MtlsCertificateDecl> {
        r.mtls_certificates.iter().collect()
    }

    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        &decl.binding
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanMtlsCertificate<'r>] {
        &context.mtls_certificates
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanMtlsCertificate<'r>> {
        &mut context.mtls_certificates
    }

    fn live_in<'a>(lives: Lives<'a>) -> Self::Live<'a> {
        lives.linked
    }

    fn plan_line<'r>(
        _cx: &PlanCx<'_>,
        decl: Self::Decl<'r>,
        _live: Self::Live<'_>,
    ) -> PlanMtlsCertificate<'r> {
        PlanMtlsCertificate {
            binding: &decl.binding,
            certificate: &decl.certificate,
        }
    }

    /// Linked the same way as an AI Search instance, and worded for the same reason: the plan
    /// has not looked the certificate up.
    fn short_line(cert: &PlanMtlsCertificate<'_>, out: &mut String) {
        out.push_str(&format!(
            "will link mtls-certificate {} ({}; not created; not checked in a dry run)\n",
            cert.certificate, cert.binding
        ));
    }

    fn shown_orphan(orphan: &NoOrphan, _environment: &str) -> NoOrphan {
        match *orphan {}
    }

    fn short_orphan(orphan: &NoOrphan, _out: &mut String) {
        match *orphan {}
    }

    fn orphan_worker(orphan: &NoOrphan) -> &str {
        match *orphan {}
    }

    /// One step for all the Worker's links, written once when any changed.
    fn steps<'r>(
        _cx: &PlanCx<'_>,
        declared: &'r Resources,
        _live: Self::Live<'_>,
    ) -> Vec<MtlsCertificateStep<'r>> {
        if declared.mtls_certificates.is_empty() {
            return Vec::new();
        }
        vec![MtlsCertificateStep {
            decls: declared.mtls_certificates.iter().collect(),
        }]
    }

    fn steps_of<'p, 'r>(steps: &'p WorkerSteps<'r>) -> &'p [MtlsCertificateStep<'r>] {
        &steps.mtls_certificates
    }

    fn steps_mut<'p, 'r>(steps: &'p mut WorkerSteps<'r>) -> &'p mut Vec<MtlsCertificateStep<'r>> {
        &mut steps.mtls_certificates
    }

    /// Each link recorded, the ledger written once when any changed, before the push.
    fn apply(
        cx: &StepCx<'_, '_, MtlsCertificate>,
        step: &MtlsCertificateStep<'_>,
    ) -> Result<Applied<MtlsCertificate>, DeployFailure> {
        apply_links(cx, &step.decls)
    }

    fn materialise(
        config: &Path,
        worker: &str,
        bound: &BTreeMap<String, String>,
    ) -> Result<(), DeployFailure> {
        if !bound.is_empty() && !materialise_mtls_certificates(config, bound) {
            return Err(DeployFailure::driver(format!(
                "could not materialise the mTLS certificate ids into `{worker}`'s generated configuration"
            )));
        }
        Ok(())
    }
}

/// [`MtlsCertificate`]'s step: each of the Worker's links, written once when any changed.
fn apply_links(
    cx: &StepCx<'_, '_, MtlsCertificate>,
    decls: &[&MtlsCertificateDecl],
) -> Result<Applied<MtlsCertificate>, DeployFailure> {
    let worker = cx.worker;
    let mut applied = Applied::nothing();
    let mut links = Links::new();
    for &decl in decls {
        let (link, id) = link(cx, decl)?;
        links.link(cx, &decl.binding, link);
        applied.bound.push((decl.binding.clone(), id));
    }
    if let Some(commit) = links.commit(
        worker,
        format!("linked `{worker}`'s mTLS certificates but could not record them"),
    ) {
        applied = applied.with(commit);
    }
    Ok(applied)
}

/// What `decl` was linked to, as the ledger records it, and the certificate id. Linked before the
/// run confirmed ([`link_for_run`]), so a binding with no id here is a bug, not a user error. Only
/// a certificate's name and id: no key material crossed the wire.
fn link(
    cx: &StepCx<'_, '_, MtlsCertificate>,
    decl: &MtlsCertificateDecl,
) -> Result<(MtlsCertificateLink, String), DeployFailure> {
    let worker = cx.worker;
    let Some(id) = cx
        .live
        .and_then(|linked| linked.get(&(worker.to_string(), decl.binding.clone())))
    else {
        return Err(DeployFailure::driver(format!(
            "`{}` of `{worker}` was not linked to an mTLS certificate before the push",
            decl.binding
        )));
    };
    Ok((
        MtlsCertificateLink {
            certificate: decl.certificate.clone(),
            certificate_id: id.clone(),
        },
        id.clone(),
    ))
}

/// #3513 A: one `MtlsCertificate` instance's link. No action: `bynk deploy` neither uploads nor
/// changes the certificate.
#[derive(Debug, Serialize)]
pub(crate) struct PlanMtlsCertificate<'a> {
    pub(crate) binding: &'a str,
    pub(crate) certificate: &'a str,
}

/// A Worker's links' step (1D.2b): each declaration, linked to the id found before the run
/// confirmed.
#[derive(Debug, Clone)]
pub(crate) struct MtlsCertificateStep<'r> {
    pub(crate) decls: Vec<&'r MtlsCertificateDecl>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> MtlsCertificateLink {
        MtlsCertificateLink {
            certificate: "acme-client".into(),
            certificate_id: "c-123".into(),
        }
    }

    /// Provision `declared`'s links of `shop` into `lock`, recording in `lock_path`.
    fn provision(
        lock: &mut DeployLock,
        lock_path: &Path,
        declared: &Resources,
        linked: &BTreeMap<(String, String), String>,
    ) -> Result<BTreeMap<String, String>, DeployFailure> {
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        provision_kind::<MtlsCertificate>(
            Site {
                tools: Tools {
                    api: &Wrangler::new(&provenance, Path::new("/nonexistent")),
                    project_root: Path::new("/nonexistent"),
                },
                environment: "production",
                worker: "shop",
                project: &project,
            },
            declared,
            &mut LedgerWriter::new(lock, lock_path),
            Lives {
                linked: Some(linked),
                ..Lives::default()
            },
        )
    }

    fn linked() -> BTreeMap<(String, String), String> {
        BTreeMap::from([(
            ("shop".to_string(), "MTLS_BACKEND".to_string()),
            "c-123".to_string(),
        )])
    }

    #[test]
    fn the_table_keeps_the_ledgers_shape() {
        let mut env = Environment::default();
        MtlsCertificateTable::insert(&mut env, "shop", "MTLS_BACKEND", link());
        assert_eq!(
            MtlsCertificateTable::get(&env, "shop", "MTLS_BACKEND"),
            Some(&link())
        );
        assert_eq!(
            MtlsCertificateTable::entries(&env),
            vec![("shop", "MTLS_BACKEND", &link())]
        );
        MtlsCertificateTable::remove(&mut env, "shop", "MTLS_BACKEND");
        assert!(env.mtls_certificate_links.is_empty());
    }

    #[test]
    fn a_links_record_is_written_once_and_a_repeat_writes_nothing() {
        let dir =
            std::env::temp_dir().join(format!("bynk-mtls-certificate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        let bound = provision(
            &mut lock,
            &path,
            &Resources::default().mtls_certificates(&[("MTLS_BACKEND", "acme-client")]),
            &linked(),
        )
        .unwrap();
        assert_eq!(bound.len(), 1);
        assert_eq!(
            lock.recorded::<MtlsCertificate>("production", "shop", "MTLS_BACKEND"),
            Some(&link())
        );
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();
        provision(
            &mut lock,
            &path,
            &Resources::default().mtls_certificates(&[("MTLS_BACKEND", "acme-client")]),
            &linked(),
        )
        .unwrap();
        assert!(!path.exists(), "an unchanged link is not written again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_says_what_was_linked() {
        let e = provision(
            &mut DeployLock::default(),
            Path::new("/nonexistent/bynk.deploy.lock"),
            &Resources::default().mtls_certificates(&[("MTLS_BACKEND", "acme-client")]),
            &linked(),
        )
        .unwrap_err();
        assert!(
            e.message
                .starts_with("linked `shop`'s mTLS certificates but could not record them in "),
            "{}",
            e.message
        );
    }

    #[test]
    fn the_short_line_says_it_is_linked_not_created() {
        let decls = Resources::default().mtls_certificates(&[("MTLS_BACKEND", "acme-client")]);
        let lock = DeployLock::default();
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "production"),
            worker: "shop",
        };
        let line = MtlsCertificate::plan_line(
            &cx,
            MtlsCertificate::declared(&decls)[0],
            MtlsCertificate::live_in(Lives::default()),
        );
        let mut out = String::new();
        MtlsCertificate::short_line(&line, &mut out);
        assert_eq!(
            out,
            "will link mtls-certificate acme-client (MTLS_BACKEND; not created; not checked in a dry run)\n"
        );
    }

    #[test]
    fn a_binding_the_run_did_not_link_is_a_bug_named() {
        let e = provision(
            &mut DeployLock::default(),
            Path::new("/nonexistent/bynk.deploy.lock"),
            &Resources::default().mtls_certificates(&[("MTLS_BACKEND", "acme-client")]),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert_eq!(
            e.message,
            "`MTLS_BACKEND` of `shop` was not linked to an mTLS certificate before the push"
        );
    }
}
