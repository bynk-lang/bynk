//! #3355: the destination addresses an inbound email handler forwards to.
//!
//! A `from email` handler can return `Forward(to: "finance@shop.example")`, but
//! Cloudflare forwards only to a **verified destination address** of the
//! account. A forward to any other address is refused at run time (the handler
//! falls back to `Reject("forward refused")` and the sender's mail bounces), and
//! the cause shows only in the Worker's logs. So `bynk deploy` resolves each
//! *literal* destination against the account's Email Routing destination
//! addresses and **warns**, never fails: verifying is a human step (the owner
//! clicks a link Cloudflare emails to that address), and a failing deploy would
//! block unrelated changes until someone had clicked it.
//!
//! - The literals are found on the checker's typed tree (`ProjectCheck::
//!   email_forwards`), so a comment or an unrelated string never counts.
//! - A destination computed at run time cannot be known at deploy: the plan
//!   says so once per handler.
//! - The lookup is `GET /accounts/{account_id}/email/routing/addresses`
//!   (account-scoped: destination addresses are shared by the account's zones),
//!   page-based, every page followed. Each entry carries `email` and `verified`,
//!   a nullable date-time that is `null` until the owner has clicked the link.
//! - The token needs the account-level `Email Routing Addresses Read`
//!   permission (Write also works). Without it, or without credentials, the
//!   check is skipped with one warning and the deploy continues.
//! - `--dry-run` shows the same warnings.

use super::*;
use crate::cloudflare::{CloudflareError, Rest};
use bynk_driver::colour;
use bynk_driver::project::EmailForward;

/// The permission a token needs to list destination addresses.
pub(crate) const TOKEN_SCOPE: &str = "Email Routing Addresses: Read";

/// One entry of the account's destination addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Destination {
    /// The address as it was entered.
    pub(crate) email: String,
    /// Whether the owner has clicked the link Cloudflare emailed (the API's
    /// `verified` date-time is set).
    pub(crate) verified: bool,
}

/// One page of the listing, with how many pages there are in all.
pub(crate) struct AddressPage {
    pub(crate) addresses: Vec<Destination>,
    pub(crate) total_pages: u64,
}

/// The account's destination addresses, a page at a time. A trait so the check
/// is tested against a fake and the REST client against a stub.
pub(crate) trait DestinationAddresses {
    /// Page `page` (from 1) of the listing.
    fn page(&self, page: u64) -> Result<AddressPage, CloudflareError>;
}

/// Cloudflare's REST API, for the destination-address listing.
pub(crate) struct RestDestinations {
    pub(crate) rest: Rest,
}

impl RestDestinations {
    /// A client for the account in `CLOUDFLARE_ACCOUNT_ID`, or why there is
    /// none: a missing credential skips the check, it does not fail the deploy.
    pub(crate) fn from_env() -> Result<Self, CloudflareError> {
        Ok(Self {
            rest: crate::cloudflare::rest::for_purpose(
                "check an email forward destination",
                &format!(
                    "an API token with \"{TOKEN_SCOPE}\" (a `wrangler login` session is not enough: the lookup calls Cloudflare's API directly)"
                ),
            )?,
        })
    }
}

impl DestinationAddresses for RestDestinations {
    fn page(&self, page: u64) -> Result<AddressPage, CloudflareError> {
        let body = self
            .rest
            .get(&format!("/email/routing/addresses?per_page=50&page={page}"))?;
        let addresses = body["result"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                Some(Destination {
                    email: e["email"].as_str()?.to_string(),
                    // `null` (or absent) until the link has been clicked.
                    verified: e["verified"].as_str().is_some(),
                })
            })
            .collect();
        Ok(AddressPage {
            addresses,
            total_pages: body["result_info"]["total_pages"].as_u64().unwrap_or(1),
        })
    }
}

/// Every destination address of the account: every page of the listing, as
/// `linked.rs` follows a paginated one.
pub(crate) fn list_all(
    account: &dyn DestinationAddresses,
) -> Result<Vec<Destination>, CloudflareError> {
    let mut out = Vec::new();
    crate::cloudflare::paging::Numbered::TO_TOTAL.follow(|page| -> Result<_, CloudflareError> {
        let got = account.page(page)?;
        let read = crate::cloudflare::paging::Page {
            entries: got.addresses.len(),
            total_pages: Some(got.total_pages),
            ..Default::default()
        };
        out.extend(got.addresses);
        Ok(read)
    })?;
    Ok(out)
}

/// What the account holds for an address.
#[derive(Debug, PartialEq, Eq)]
enum Standing {
    Verified,
    Unverified,
    /// Not a destination address of the account. Carries the stored address
    /// that differs from it only by letter case in the local part, if any.
    Missing(Option<String>),
}

/// Whether `a` and `b` are the same address. Cloudflare stores a destination as
/// it was entered. The domain is case-insensitive by definition; the local part
/// is case-sensitive by RFC 5321, though in practice no mail system treats it
/// so. This compares the domain without regard to case and the local part
/// exactly, which is the conservative reading: a mismatch only in the local
/// part's case is reported as missing, with the stored spelling named, rather
/// than guessed to be the same.
fn same_address(a: &str, b: &str) -> bool {
    match (a.rsplit_once('@'), b.rsplit_once('@')) {
        (Some((la, da)), Some((lb, db))) => la == lb && da.eq_ignore_ascii_case(db),
        _ => false,
    }
}

fn standing(address: &str, known: &[Destination]) -> Standing {
    if let Some(found) = known.iter().find(|d| same_address(&d.email, address)) {
        return if found.verified {
            Standing::Verified
        } else {
            Standing::Unverified
        };
    }
    Standing::Missing(
        known
            .iter()
            .find(|d| d.email.eq_ignore_ascii_case(address))
            .map(|d| d.email.clone()),
    )
}

/// `` `service` (context `x`) `` for a handler, as a warning names it.
fn handler_label(f: &EmailForward) -> String {
    if f.fallback {
        return format!("the fallback of `{}` (context `{}`)", f.service, f.context);
    }
    format!("`{}` (context `{}`)", f.service, f.context)
}

/// The notes and warnings for the `Forward` verdicts of the handlers of
/// `forwards`, asking `account` only when a literal destination needs it.
pub(crate) fn forward_warnings(
    forwards: &[EmailForward],
    account: Result<&dyn DestinationAddresses, CloudflareError>,
) -> Vec<String> {
    let mut out = computed_notes(forwards);
    let mut literals: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for f in forwards {
        if let Some(to) = &f.to {
            literals.entry(to).or_default().push(handler_label(f));
        }
    }
    if literals.is_empty() {
        return out;
    }
    let known = match account.and_then(list_all) {
        Ok(known) => known,
        Err(e) => {
            out.push(lookup_warning(&literals, e));
            return out;
        }
    };
    for (to, by) in &literals {
        if let Some(w) = destination_warning(to, by, &standing(to, &known)) {
            out.push(w);
        }
    }
    out
}

/// One note per handler that forwards to an address computed at run time.
fn computed_notes(forwards: &[EmailForward]) -> Vec<String> {
    let handlers: BTreeSet<String> = forwards
        .iter()
        .filter(|f| f.to.is_none())
        .map(handler_label)
        .collect();
    handlers
        .into_iter()
        .map(|h| {
            format!(
                "bynk: note: {h} forwards to an address computed at run time, which cannot be checked at deploy: make sure it is a verified destination address (Email Routing → Destination addresses), or Cloudflare refuses the forward and the sender's mail bounces."
            )
        })
        .collect()
}

/// The one warning for a lookup that did not answer: which check was skipped,
/// and (for a refused token) the permission to grant.
fn lookup_warning(literals: &BTreeMap<&str, Vec<String>>, e: CloudflareError) -> String {
    let addresses = literals
        .keys()
        .map(|a| format!("`{a}`"))
        .collect::<Vec<_>>()
        .join(", ");
    // A refused token (`CloudflareError::is_denied`) names the permission to grant.
    if e.is_denied() {
        format!(
            "bynk: warning: could not check that the email forward destination(s) {addresses} are verified: the token needs the account-level \"{TOKEN_SCOPE}\" permission (or Write) ({e}). The check is skipped; the deploy continues."
        )
    } else {
        format!(
            "bynk: warning: could not check that the email forward destination(s) {addresses} are verified: {e}. The check is skipped; the deploy continues."
        )
    }
}

/// The warning for one literal destination, or `None` when it is verified.
fn destination_warning(to: &str, by: &[String], standing: &Standing) -> Option<String> {
    let by = by.join(", ");
    let how = "Cloudflare refuses a forward to an address that is not verified, so the sender's mail bounces (\"forward refused\")";
    match standing {
        Standing::Verified => None,
        Standing::Unverified => Some(format!(
            "bynk: warning: {by} forwards to `{to}`, which is not verified yet. {how}. The owner of `{to}` verifies it by clicking the link Cloudflare emailed to that address; its status is under Email Routing → Destination addresses."
        )),
        Standing::Missing(near) => {
            let near = near
                .as_ref()
                .map(|n| format!(" (the account holds `{n}`, which differs only in letter case)"))
                .unwrap_or_default();
            Some(format!(
                "bynk: warning: {by} forwards to `{to}`, which is not a destination address of this account{near}. {how}. Add it under Email Routing → Destination addresses; Cloudflare then emails a link to it, which its owner clicks to verify it."
            ))
        }
    }
}

/// What a run prints before it confirms (and a `--dry-run` prints in place of
/// it): the notes and warnings for the forwards of `order`'s Workers, with the
/// account asked only when a literal destination needs it. Never refuses.
pub(crate) fn forward_warnings_for_run(
    forwards: &[EmailForward],
    order: &[String],
    dir_of: &dyn Fn(&str) -> String,
) -> Vec<String> {
    let forwards: Vec<EmailForward> = forwards
        .iter()
        .filter(|f| order.iter().any(|w| *w == dir_of(&f.context)))
        .cloned()
        .collect();
    match RestDestinations::from_env() {
        Ok(rest) => forward_warnings(&forwards, Ok(&rest)),
        Err(e) => forward_warnings(&forwards, Err(e)),
    }
}

/// The `Forward` verdicts of the project at `root`, from its checked tree.
/// Empty when the project cannot be read or checked: the build has already
/// reported that, and this is only a warning.
pub(crate) fn project_forwards(root: &Path) -> Vec<EmailForward> {
    let Ok(options) = bynk_driver::try_project_options(root) else {
        return Vec::new();
    };
    let check = bynk_driver::project::check_project(&options.tests(false));
    check.email_forwards
}

/// Print the plan, then (#3355) the warnings for a literal `Forward`
/// destination Cloudflare would refuse, never failing on one. Both `--dry-run`
/// and a real run print them, before the confirmation.
pub(crate) fn print_plan_then_forward_warnings(
    plan: &Plan<'_>,
    opts: &DeployOptions,
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    project_root: &Path,
) {
    print_plan(plan, opts.format);
    // #3494: what checking the plan against the account found.
    for note in plan.account_notes() {
        colour::eprintln(note);
    }
    if any_email_handlers(order, resources) {
        let groups = super::plan::GroupNames::of(project_root);
        let dir_of = |context: &str| groups.dir(context);
        for warning in forward_warnings_for_run(&project_forwards(project_root), order, &dir_of) {
            colour::eprintln(&warning);
        }
    }
}

/// Whether any Worker of `order` has an inbound email address, so a project
/// with none is not checked again.
pub(crate) fn any_email_handlers(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> bool {
    order
        .iter()
        .any(|w| !resources[w].email_addresses.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::live::tests::{rest, stub};
    use std::cell::RefCell;

    /// An account whose destination addresses are listed in `pages`, counting
    /// the pages it was asked for.
    struct Fake {
        pages: Vec<Vec<(&'static str, bool)>>,
        asked: RefCell<Vec<u64>>,
        refuse: Option<crate::cloudflare::RestError>,
    }

    impl Fake {
        fn new(pages: Vec<Vec<(&'static str, bool)>>) -> Self {
            Self {
                pages,
                asked: RefCell::new(Vec::new()),
                refuse: None,
            }
        }
    }

    impl DestinationAddresses for Fake {
        fn page(&self, page: u64) -> Result<AddressPage, CloudflareError> {
            self.asked.borrow_mut().push(page);
            if let Some(e) = &self.refuse {
                return Err(e.clone().into());
            }
            let addresses = self
                .pages
                .get(page as usize - 1)
                .into_iter()
                .flatten()
                .map(|(email, verified)| Destination {
                    email: email.to_string(),
                    verified: *verified,
                })
                .collect();
            Ok(AddressPage {
                addresses,
                total_pages: self.pages.len() as u64,
            })
        }
    }

    fn forward(service: &str, to: Option<&str>) -> EmailForward {
        EmailForward {
            context: "shop.support".into(),
            service: service.into(),
            address: format!("{service}@shop.example"),
            to: to.map(str::to_string),
            fallback: false,
        }
    }

    fn run(forwards: &[EmailForward], account: &Fake) -> Vec<String> {
        forward_warnings(forwards, Ok(account))
    }

    #[test]
    fn a_verified_destination_is_not_warned_about() {
        let account = Fake::new(vec![vec![("finance@shop.example", true)]]);
        let w = run(
            &[forward("billing", Some("finance@shop.example"))],
            &account,
        );
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn an_unverified_destination_warns_and_says_how_to_verify_it() {
        let account = Fake::new(vec![vec![("finance@shop.example", false)]]);
        let w = run(
            &[forward("billing", Some("finance@shop.example"))],
            &account,
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("`billing` (context `shop.support`)"),
            "{}",
            w[0]
        );
        assert!(
            w[0].contains("`finance@shop.example`, which is not verified yet"),
            "{}",
            w[0]
        );
        assert!(
            w[0].contains("clicking the link Cloudflare emailed")
                && w[0].contains("Email Routing → Destination addresses"),
            "{}",
            w[0]
        );
    }

    #[test]
    fn an_unverified_fallback_warns_as_a_forward_does_and_says_it_is_the_fallback() {
        // #3885: a fallback that cannot be forwarded to would bounce the very mail it exists to
        // keep, so it is checked like a `Forward` destination.
        let account = Fake::new(vec![vec![("ops@shop.example", false)]]);
        let fallback = EmailForward {
            fallback: true,
            ..forward("support", Some("ops@shop.example"))
        };
        let w = run(&[fallback], &account);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("the fallback of `support` (context `shop.support`) forwards to `ops@shop.example`, which is not verified yet"),
            "{}",
            w[0]
        );
    }

    #[test]
    fn a_missing_destination_warns() {
        let account = Fake::new(vec![vec![("other@shop.example", true)]]);
        let w = run(
            &[forward("billing", Some("finance@shop.example"))],
            &account,
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("not a destination address of this account"),
            "{}",
            w[0]
        );
        assert!(w[0].contains("Add it under Email Routing"), "{}", w[0]);
    }

    #[test]
    fn handlers_forwarding_to_one_address_share_its_warning() {
        let account = Fake::new(vec![vec![]]);
        let w = run(
            &[
                forward("billing", Some("finance@shop.example")),
                forward("ops", Some("finance@shop.example")),
            ],
            &account,
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("`billing`") && w[0].contains("`ops`"),
            "{}",
            w[0]
        );
    }

    #[test]
    fn a_computed_destination_is_one_note_per_handler_and_asks_nothing() {
        let account = Fake::new(vec![vec![]]);
        let w = run(
            &[
                forward("billing", None),
                forward("billing", None),
                forward("ops", None),
            ],
            &account,
        );
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w.iter().all(|n| n.starts_with("bynk: note:")), "{w:?}");
        assert!(
            w[0].contains("`billing`") && w[1].contains("`ops`"),
            "{w:?}"
        );
        // Nothing literal to resolve, so the account is not asked.
        assert!(account.asked.borrow().is_empty());
    }

    #[test]
    fn no_forward_at_all_says_nothing_and_asks_nothing() {
        // What a comment that mentions an address leaves: no entry.
        let account = Fake::new(vec![vec![]]);
        assert!(run(&[], &account).is_empty());
        assert!(account.asked.borrow().is_empty());
    }

    #[test]
    fn every_page_of_the_listing_is_followed() {
        let account = Fake::new(vec![
            vec![("a@shop.example", true)],
            vec![("b@shop.example", true)],
            vec![("finance@shop.example", true)],
        ]);
        let w = run(
            &[forward("billing", Some("finance@shop.example"))],
            &account,
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(*account.asked.borrow(), [1, 2, 3]);
    }

    #[test]
    fn the_domain_matches_without_case_and_the_local_part_exactly() {
        let account = Fake::new(vec![vec![
            ("finance@Shop.Example", true),
            ("Ops@shop.example", true),
        ]]);
        let w = run(
            &[
                forward("billing", Some("finance@shop.example")),
                forward("ops", Some("ops@shop.example")),
            ],
            &account,
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("`ops@shop.example`")
                && w[0].contains("the account holds `Ops@shop.example`"),
            "{}",
            w[0]
        );
    }

    #[test]
    fn a_token_without_the_permission_is_one_warning_and_the_deploy_goes_on() {
        let mut account = Fake::new(vec![]);
        account.refuse = Some(crate::cloudflare::RestError::answered(
            "GET",
            "/email/routing/addresses",
            403,
            "[{\"code\":10000}]",
        ));
        let w = run(
            &[
                forward("billing", Some("finance@shop.example")),
                forward("ops", Some("ops@shop.example")),
            ],
            &account,
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("Email Routing Addresses: Read"), "{}", w[0]);
        assert!(
            w[0].contains("The check is skipped; the deploy continues."),
            "{}",
            w[0]
        );
    }

    #[test]
    fn missing_credentials_skip_the_check_with_one_warning() {
        let w = forward_warnings(
            &[forward("billing", Some("finance@shop.example"))],
            Err(CloudflareError::Credentials(
                "set CLOUDFLARE_API_TOKEN to check".into(),
            )),
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("set CLOUDFLARE_API_TOKEN"), "{}", w[0]);
    }

    #[test]
    fn the_rest_client_follows_pages_and_reads_the_verified_date() {
        let (base, seen) = stub(vec![
            (
                "/accounts/acct/email/routing/addresses?per_page=50&page=1",
                r#"{"success":true,"result":[{"id":"1","email":"a@shop.example","verified":"2025-01-02T03:04:05Z"}],"result_info":{"count":1,"page":1,"per_page":50,"total_count":2,"total_pages":2}}"#.into(),
            ),
            (
                "/accounts/acct/email/routing/addresses?per_page=50&page=2",
                r#"{"success":true,"result":[{"id":"2","email":"b@shop.example","verified":null}],"result_info":{"count":1,"page":2,"per_page":50,"total_count":2,"total_pages":2}}"#.into(),
            ),
        ]);
        let api = RestDestinations { rest: rest(base) };
        let all = list_all(&api).unwrap();
        assert_eq!(
            all,
            [
                Destination {
                    email: "a@shop.example".into(),
                    verified: true
                },
                Destination {
                    email: "b@shop.example".into(),
                    verified: false
                },
            ]
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen.iter().all(|s| s.ends_with("Bearer tok")), "{seen:?}");
    }

    #[test]
    fn authentication_failures_are_told_from_other_ones() {
        let at = |status, errors: &str| {
            CloudflareError::from(crate::cloudflare::RestError::answered(
                "GET", "/x", status, errors,
            ))
        };
        assert!(at(403, "[]").is_denied());
        assert!(at(400, "[{\"code\":9109}]").is_denied());
        assert!(!at(500, "[]").is_denied());
        assert!(!CloudflareError::Credentials("set X".into()).is_denied());
    }
}
