//! #3302: the sender domains an `Email` instance may send as.
//!
//! `capability Mail = Email @allowedSenders(["orders@shop.example"])` binds a
//! `[[send_email]]` entry whose `allowed_sender_addresses` Cloudflare enforces.
//! A domain is onboarded to Email Sending on the account (DNS records, a
//! dashboard step), which is not something a build can or should do, so
//! `bynk deploy` only *checks* it: for each distinct domain among the allowed
//! senders it finds the zone, then asks whether Email Sending is enabled on
//! the domain (or on a wildcard covering it), and fails, before anything is
//! pushed, naming every domain that is not. It never configures one.
//!
//! The Email Sending API has no field called "verified": the signal is the
//! sending subdomain's `enabled` flag, which is what it can honestly check. A
//! domain whose DNS records are still propagating may pass here and be
//! refused by the platform as `SenderNotVerified` at run time, which a handler
//! already answers.
//!
//! An instance with no `@allowedSenders` asks nothing, and `--dry-run` stays
//! offline, as for a linked Secrets Store secret.

use super::*;
use crate::migrate::live::{self, Credentials, Rest};

/// The account's zones and their Email Sending subdomains, as far as the check
/// needs them. A trait so the check is tested against a fake and the REST
/// client against a stub.
pub(crate) trait SendingDomains {
    /// The id of the zone named exactly `name` on the account, if it has one.
    fn zone_id(&self, name: &str) -> Result<Option<String>, String>;
    /// Whether Email Sending is enabled on `domain` in zone `zone_id`: by an
    /// enabled subdomain of exactly that name, or an enabled wildcard
    /// (`*.base`) covering it.
    fn sending_enabled(&self, zone_id: &str, domain: &str) -> Result<bool, String>;
}

/// Cloudflare's REST API, for the zone and Email Sending listings.
pub(crate) struct RestSendingDomains {
    pub(crate) rest: Rest,
}

impl RestSendingDomains {
    /// A client for the account in `CLOUDFLARE_ACCOUNT_ID`, or a message
    /// naming the missing variable and what the token needs.
    pub(crate) fn from_env() -> Result<Self, String> {
        let credentials = Credentials::from_env_for(
            "check an email sender domain",
            "an API token that can read the zone and its Email Sending settings (a `wrangler login` session is not enough: the lookup calls Cloudflare's API directly)",
        )?;
        Ok(Self {
            rest: Rest {
                agent: live::agent()?,
                credentials,
                base: live::API.to_string(),
            },
        })
    }
}

impl SendingDomains for RestSendingDomains {
    fn zone_id(&self, name: &str) -> Result<Option<String>, String> {
        let body = self.rest.get_global(&format!(
            "/zones?name={name}&account.id={}&per_page=50",
            self.rest.credentials.account
        ))?;
        Ok(body["result"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|z| {
                (z["name"].as_str() == Some(name)).then(|| z["id"].as_str().map(str::to_string))?
            }))
    }

    fn sending_enabled(&self, zone_id: &str, domain: &str) -> Result<bool, String> {
        for page in 1.. {
            let body = self.rest.get_global(&format!(
                "/zones/{zone_id}/email/sending/subdomains?per_page=100&page={page}"
            ))?;
            let entries = body["result"].as_array().cloned().unwrap_or_default();
            if entries.iter().any(|e| {
                e["enabled"].as_bool() == Some(true)
                    && e["name"].as_str().is_some_and(|n| covers(n, domain))
            }) {
                return Ok(true);
            }
            let pages = body["result_info"]["total_pages"].as_u64().unwrap_or(1);
            if entries.is_empty() || page >= pages {
                break;
            }
        }
        Ok(false)
    }
}

/// Whether a sending subdomain named `entry` (an exact name, or a leftmost
/// wildcard `*.base`) is `domain`, or covers it.
fn covers(entry: &str, domain: &str) -> bool {
    match entry.strip_prefix("*.") {
        Some(base) => {
            let base = base.to_ascii_lowercase();
            let domain = domain.to_ascii_lowercase();
            domain.len() > base.len() && domain.ends_with(&format!(".{base}"))
        }
        None => entry.eq_ignore_ascii_case(domain),
    }
}

/// The domain of a sender address, lowercased: what follows its `@`.
fn domain_of(address: &str) -> Option<String> {
    address
        .rsplit_once('@')
        .map(|(_, d)| d.to_ascii_lowercase())
        .filter(|d| {
            // Only what a host name holds: the domain goes into a query
            // string, so a stray `&` or `#` must never reach it.
            !d.is_empty()
                && d.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
}

/// The zones a domain could belong to, nearest first: the domain itself, then
/// each parent with at least two labels (`mail.shop.example`, `shop.example`).
fn zone_candidates(domain: &str) -> Vec<&str> {
    let mut out = vec![domain];
    let mut rest = domain;
    while let Some((_, parent)) = rest.split_once('.') {
        if !parent.contains('.') {
            break;
        }
        out.push(parent);
        rest = parent;
    }
    out
}

/// What the account answered, for a message: a 401 or 403 is the token lacking
/// the permission, which is the one thing worth naming rather than echoing.
fn explain(said: String) -> String {
    if said.contains("answered 401") || said.contains("answered 403") {
        format!(
            "{said} (the token needs permission, on this account, to read the zone and its Email Sending settings)"
        )
    } else {
        said
    }
}

/// Check every `@allowedSenders` domain of `order`, asking `account` once per
/// distinct domain. Every problem is collected, not just the first: onboarding
/// several domains is one dashboard visit, and is better found together.
pub(crate) fn check_sender_domains(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    account: &dyn SendingDomains,
) -> Result<(), Vec<String>> {
    // Each domain, with the first place that names it, for the message.
    let mut domains: BTreeMap<String, String> = BTreeMap::new();
    let mut problems = Vec::new();
    for worker in order {
        for decl in &resources[worker].email_senders {
            for sender in &decl.senders {
                let who = format!("`{}` of `{worker}` may send as `{sender}`", decl.binding);
                match domain_of(sender) {
                    Some(domain) => {
                        domains.entry(domain).or_insert(who);
                    }
                    None => problems.push(format!("{who}, which has no domain")),
                }
            }
        }
    }
    for (domain, who) in &domains {
        if let Some(problem) = check_domain(domain, account) {
            problems.push(format!("{who}: {problem}"));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Why `domain` cannot be sent from, or `None` when Email Sending is enabled
/// on it.
fn check_domain(domain: &str, account: &dyn SendingDomains) -> Option<String> {
    for candidate in zone_candidates(domain) {
        match account.zone_id(candidate) {
            Err(said) => {
                return Some(format!(
                    "could not look up the zone of `{domain}`: {}",
                    explain(said)
                ));
            }
            Ok(None) => continue,
            Ok(Some(zone)) => {
                return match account.sending_enabled(&zone, domain) {
                    Ok(true) => None,
                    Ok(false) => Some(format!(
                        "`{domain}` does not have Email Sending enabled on this account; onboard it under Email Service > Email Sending in the dashboard (`bynk deploy` checks a sender domain and never configures one)"
                    )),
                    Err(said) => Some(format!(
                        "could not check Email Sending on `{domain}`: {}",
                        explain(said)
                    )),
                };
            }
        }
    }
    Some(format!(
        "`{domain}` is not a zone on this account, nor in one, so it cannot send mail; add the domain to the account and onboard it to Email Sending"
    ))
}

/// The check a run does before it confirms: every allowed sender's domain,
/// each problem printed, `false` when any was found (the run stops there, with
/// nothing pushed).
pub(crate) fn check_senders_for_run(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> bool {
    if !any_email_senders(order, resources) {
        return true;
    }
    let account = match RestSendingDomains::from_env() {
        Ok(account) => account,
        Err(e) => {
            eprintln!("bynk: {e}");
            return false;
        }
    };
    match check_sender_domains(order, resources, &account) {
        Ok(()) => true,
        Err(problems) => {
            for problem in problems {
                eprintln!("bynk: {problem}");
            }
            false
        }
    }
}

/// Whether any Worker of `order` has a sender to check.
pub(crate) fn any_email_senders(order: &[String], resources: &BTreeMap<String, Resources>) -> bool {
    order.iter().any(|w| !resources[w].email_senders.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::live::tests::{rest, stub};
    use std::cell::RefCell;

    /// An account with the zones `zones` (name, id, enabled sending
    /// subdomains), counting what it was asked.
    struct Fake {
        zones: Vec<(&'static str, &'static str, Vec<&'static str>)>,
        asked: RefCell<Vec<String>>,
        refuse: Option<&'static str>,
    }

    impl Fake {
        fn new(zones: Vec<(&'static str, &'static str, Vec<&'static str>)>) -> Self {
            Self {
                zones,
                asked: RefCell::new(Vec::new()),
                refuse: None,
            }
        }
    }

    impl SendingDomains for Fake {
        fn zone_id(&self, name: &str) -> Result<Option<String>, String> {
            self.asked.borrow_mut().push(format!("zone {name}"));
            if let Some(said) = self.refuse {
                return Err(said.to_string());
            }
            Ok(self
                .zones
                .iter()
                .find(|(n, ..)| *n == name)
                .map(|(_, id, _)| id.to_string()))
        }
        fn sending_enabled(&self, zone_id: &str, domain: &str) -> Result<bool, String> {
            self.asked
                .borrow_mut()
                .push(format!("sending {zone_id}/{domain}"));
            Ok(self.zones.iter().any(|(_, id, enabled)| {
                *id == zone_id && enabled.iter().any(|e| covers(e, domain))
            }))
        }
    }

    /// A Worker's declared `(binding, senders)` pairs.
    type Declared<'a> = &'a [(&'a str, &'a [&'a str])];

    fn project(workers: &[(&str, Declared)]) -> (Vec<String>, BTreeMap<String, Resources>) {
        let order = workers.iter().map(|(w, _)| w.to_string()).collect();
        let resources = workers
            .iter()
            .map(|(w, s)| (w.to_string(), Resources::default().email_senders(s)))
            .collect();
        (order, resources)
    }

    #[test]
    fn an_enabled_sender_domain_passes() {
        let (order, resources) = project(&[(
            "shop",
            &[(
                "EMAIL_MAIL",
                &["orders@shop.example", "billing@Shop.Example"],
            )],
        )]);
        let account = Fake::new(vec![("shop.example", "z1", vec!["shop.example"])]);
        check_sender_domains(&order, &resources, &account).unwrap();
        // One question per distinct domain, however it is written or often.
        assert_eq!(
            *account.asked.borrow(),
            ["zone shop.example", "sending z1/shop.example"]
        );
    }

    #[test]
    fn a_subdomain_is_checked_in_the_zone_that_holds_it() {
        let (order, resources) =
            project(&[("shop", &[("EMAIL_MAIL", &["orders@mail.shop.example"])])]);
        let account = Fake::new(vec![("shop.example", "z1", vec!["mail.shop.example"])]);
        check_sender_domains(&order, &resources, &account).unwrap();
        assert_eq!(
            *account.asked.borrow(),
            [
                "zone mail.shop.example",
                "zone shop.example",
                "sending z1/mail.shop.example"
            ]
        );
    }

    #[test]
    fn a_wildcard_covers_its_subdomains_and_not_its_base() {
        assert!(covers("*.shop.example", "mail.shop.example"));
        assert!(covers("*.shop.example", "a.b.shop.example"));
        assert!(!covers("*.shop.example", "shop.example"));
        assert!(!covers("*.shop.example", "xshop.example"));
        assert!(covers("mail.shop.example", "MAIL.shop.example"));
        // Case never matters, in an exact name or a wildcard's base.
        assert!(covers("*.Shop.Example", "mail.shop.example"));
        assert!(covers("*.shop.example", "Mail.SHOP.example"));
        assert!(!covers("mail.shop.example", "shop.example"));
    }

    #[test]
    fn a_domain_without_sending_fails_naming_the_domain_and_what_fixes_it() {
        let (order, resources) = project(&[("shop", &[("EMAIL_MAIL", &["orders@shop.example"])])]);
        let account = Fake::new(vec![("shop.example", "z1", vec![])]);
        let problems = check_sender_domains(&order, &resources, &account).unwrap_err();
        assert_eq!(problems.len(), 1);
        let p = &problems[0];
        assert!(p.contains("`EMAIL_MAIL` of `shop`"), "{p}");
        assert!(p.contains("`orders@shop.example`"), "{p}");
        assert!(
            p.contains("`shop.example` does not have Email Sending enabled"),
            "{p}"
        );
        assert!(p.contains("never configures one"), "{p}");
    }

    #[test]
    fn a_domain_outside_the_account_fails_naming_it() {
        let (order, resources) =
            project(&[("shop", &[("EMAIL_MAIL", &["orders@elsewhere.example"])])]);
        let problems =
            check_sender_domains(&order, &resources, &Fake::new(Vec::new())).unwrap_err();
        assert!(
            problems[0].contains("`elsewhere.example` is not a zone on this account"),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn every_problem_is_reported_together() {
        let (order, resources) = project(&[
            ("a", &[("EMAIL_MAIL", &["x@one.example", "x@good.example"])]),
            ("b", &[("EMAIL_OPS", &["x@two.example"])]),
        ]);
        let account = Fake::new(vec![
            ("one.example", "z1", vec![]),
            ("good.example", "z2", vec!["good.example"]),
        ]);
        let problems = check_sender_domains(&order, &resources, &account).unwrap_err();
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn a_refused_lookup_names_the_permission_the_token_needs() {
        let (order, resources) = project(&[("shop", &[("EMAIL_MAIL", &["a@shop.example"])])]);
        let mut account = Fake::new(Vec::new());
        account.refuse = Some("GET /zones: Cloudflare answered 403 [{\"code\":10000}]");
        let problems = check_sender_domains(&order, &resources, &account).unwrap_err();
        assert!(
            problems[0].contains("needs permission") && problems[0].contains("Email Sending"),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn the_rest_client_finds_the_zone_and_follows_every_page_of_subdomains() {
        let (base, seen) = stub(vec![
            (
                "/zones?name=shop.example&account.id=acct&per_page=50",
                r#"{"success":true,"result":[{"id":"z1","name":"shop.example"}]}"#.into(),
            ),
            (
                "/zones?name=absent.example&account.id=acct&per_page=50",
                r#"{"success":true,"result":[]}"#.into(),
            ),
            (
                "/zones/z1/email/sending/subdomains?per_page=100&page=1",
                r#"{"success":true,"result":[{"name":"off.shop.example","enabled":false}],"result_info":{"total_pages":2}}"#.into(),
            ),
            (
                "/zones/z1/email/sending/subdomains?per_page=100&page=2",
                r#"{"success":true,"result":[{"name":"mail.shop.example","enabled":true},{"name":"*.wild.shop.example","enabled":true}],"result_info":{"total_pages":2}}"#.into(),
            ),
        ]);
        let api = RestSendingDomains { rest: rest(base) };
        assert_eq!(api.zone_id("shop.example").unwrap().as_deref(), Some("z1"));
        assert_eq!(api.zone_id("absent.example").unwrap(), None);
        assert!(api.sending_enabled("z1", "mail.shop.example").unwrap());
        assert!(api.sending_enabled("z1", "x.wild.shop.example").unwrap());
        // Present but disabled, and absent, are both "not enabled".
        assert!(!api.sending_enabled("z1", "off.shop.example").unwrap());
        assert!(!api.sending_enabled("z1", "none.shop.example").unwrap());
        let seen = seen.lock().unwrap();
        // Zones are not under the account's path; the token is sent on each.
        assert!(seen[0].starts_with("/zones?name=shop.example"), "{seen:?}");
        assert!(seen.iter().all(|s| s.ends_with("Bearer tok")), "{seen:?}");
    }

    #[test]
    fn a_project_without_senders_asks_the_account_nothing() {
        let (order, resources) = project(&[("shop", &[])]);
        assert!(!any_email_senders(&order, &resources));
        assert!(check_senders_for_run(&order, &resources));
    }

    #[test]
    fn a_domain_with_query_characters_is_refused_not_sent_to_the_api() {
        assert_eq!(domain_of("a@Shop.Example").as_deref(), Some("shop.example"));
        for bad in [
            "a@x.example&account.id=other",
            "a@x.example#frag",
            "a@x.example?q=1",
            "a@x=y.example",
        ] {
            assert_eq!(domain_of(bad), None, "{bad}");
        }
        let (order, resources) =
            project(&[("shop", &[("EMAIL_MAIL", &["a@x.example&account.id=other"])])]);
        let account = Fake::new(Vec::new());
        let problems = check_sender_domains(&order, &resources, &account).unwrap_err();
        assert!(
            problems[0].contains("which has no domain"),
            "{}",
            problems[0]
        );
        assert!(account.asked.borrow().is_empty());
    }

    #[test]
    fn candidates_stop_before_a_bare_top_level_label() {
        assert_eq!(
            zone_candidates("a.b.shop.example"),
            ["a.b.shop.example", "b.shop.example", "shop.example"]
        );
        assert_eq!(zone_candidates("shop.example"), ["shop.example"]);
    }
}
