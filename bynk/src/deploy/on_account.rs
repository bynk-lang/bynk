//! #3494: what the account already holds of the resources a plan would create.
//!
//! The plan reads `bynk.deploy.lock`, so a resource the ledger does not record
//! is `create` even when one of its name is already on the account, which the
//! deploy then adopts (R2, D1, Vectorize) or reuses (a queue), and which stops
//! the deploy for KV (Cloudflare refuses a second namespace of one title). With
//! `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` set, the plan asks the
//! account, `--dry-run` included, for the kinds it would otherwise call
//! `create`, and says `reuse` for a name it finds. Without them the plan stays
//! ledger-only, as before, and says so once. A listing that fails is named in
//! one note and leaves that kind ledger-only: the check never fails a deploy.
//!
//! Every listing is `GET /accounts/{account_id}/…`, every page followed:
//! - KV: `/storage/kv/namespaces` (`title`), page-based;
//! - R2: `/r2/buckets` (`name`), cursor-based, one listing per jurisdiction
//!   (`cf-r2-jurisdiction`), since a bucket is named within its jurisdiction;
//! - D1: `/d1/database` (`name`), page-based;
//! - Queues: `/queues` (`queue_name`), page-based;
//! - Vectorize: `/vectorize/v2/indexes` (`name`), one page.

use std::collections::{BTreeMap, BTreeSet};

use super::kind::KindId;
use crate::cloudflare::{CloudflareError, Rest};

/// One listing: a kind, and for R2 the jurisdiction (`None` is the default).
pub(crate) type Listing = (KindId, Option<String>);

/// The account's names of one kind.
pub(crate) trait Listings {
    /// Every name of `kind` on the account; an R2 bucket's within `jurisdiction`.
    fn names(
        &self,
        kind: KindId,
        jurisdiction: Option<&str>,
    ) -> Result<BTreeSet<String>, CloudflareError>;
}

/// The names the account holds, for the listings a plan asked for.
#[derive(Debug, Default)]
pub(crate) struct OnAccount {
    names: BTreeMap<Listing, BTreeSet<String>>,
    /// One note per listing that could not be read.
    pub(crate) notes: Vec<String>,
}

impl OnAccount {
    /// Read each of `wanted` from `listings`.
    pub(crate) fn read(wanted: &BTreeSet<Listing>, listings: &dyn Listings) -> Self {
        let mut on = OnAccount::default();
        for (kind, jurisdiction) in wanted {
            match listings.names(*kind, jurisdiction.as_deref()) {
                Ok(names) => {
                    on.names.insert((*kind, jurisdiction.clone()), names);
                }
                Err(e) => on.notes.push(format!(
                    "bynk: warning: could not list the account's {}{} ({e}); the plan says what \
                     `bynk.deploy.lock` records for them",
                    kind.account_noun(),
                    jurisdiction
                        .as_deref()
                        .map(|j| format!(" in jurisdiction {j}"))
                        .unwrap_or_default(),
                )),
            }
        }
        on
    }

    /// Whether the account holds `name` of `kind` (within `jurisdiction`), or
    /// `None` for a listing not read: unknown, never read as absent.
    pub(crate) fn holds(
        &self,
        kind: KindId,
        jurisdiction: Option<&str>,
        name: &str,
    ) -> Option<bool> {
        self.names
            .get(&(kind, jurisdiction.map(str::to_string)))
            .map(|names| names.contains(name))
    }
}

/// The listings over Cloudflare's REST API.
pub(crate) struct RestListings(pub(crate) Rest);

impl RestListings {
    /// From `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`, or why not.
    pub(crate) fn from_env() -> Result<Self, CloudflareError> {
        Ok(RestListings(crate::cloudflare::rest::for_purpose(
            "check the plan against the account",
            "an API token that can read the account's KV, R2, D1, Queues and Vectorize",
        )?))
    }

    /// Every page of a page-based listing at `path` (which takes `page=`), the
    /// `field` of each entry of `result`.
    fn paged(&self, path: &str, field: &str) -> Result<BTreeSet<String>, CloudflareError> {
        let mut names = BTreeSet::new();
        let rule = crate::cloudflare::paging::Numbered {
            cap: None,
            missing_total: crate::cloudflare::paging::MissingTotal::WhileFull,
            stop_on_empty: false,
        };
        rule.follow(|page| -> Result<_, CloudflareError> {
            let sep = if path.contains('?') { '&' } else { '?' };
            let body = self.0.get(&format!("{path}{sep}page={page}"))?;
            let entries = body["result"]
                .as_array()
                .ok_or_else(|| format!("GET {path}: no `result` list"))?;
            names.extend(
                entries
                    .iter()
                    .filter_map(|e| e[field].as_str().map(str::to_string)),
            );
            Ok(crate::cloudflare::paging::Page::of(&body, entries.len()))
        })?;
        Ok(names)
    }

    /// Every bucket in `jurisdiction`, following the listing's cursor.
    fn buckets(&self, jurisdiction: Option<&str>) -> Result<BTreeSet<String>, CloudflareError> {
        let mut names = BTreeSet::new();
        crate::cloudflare::paging::cursor(true, |cursor| -> Result<_, CloudflareError> {
            let path = match cursor {
                // The cursor is opaque: encoded, so a `+` or `=` in it survives.
                Some(c) => format!(
                    "/r2/buckets?per_page=1000&cursor={}",
                    crate::dev_inspect::percent_encode(c)
                ),
                None => "/r2/buckets?per_page=1000".to_string(),
            };
            let body = match jurisdiction {
                Some(j) => self.0.get_with_header(&path, ("cf-r2-jurisdiction", j))?,
                None => self.0.get(&path)?,
            };
            names.extend(
                body["result"]["buckets"]
                    .as_array()
                    .ok_or("GET /r2/buckets: no `buckets` list")?
                    .iter()
                    .filter_map(|b| b["name"].as_str().map(str::to_string)),
            );
            Ok(body["result_info"]["cursor"].as_str().map(str::to_string))
        })?;
        Ok(names)
    }
}

impl Listings for RestListings {
    fn names(
        &self,
        kind: KindId,
        jurisdiction: Option<&str>,
    ) -> Result<BTreeSet<String>, CloudflareError> {
        match kind {
            KindId::Kv => self.paged("/storage/kv/namespaces?per_page=100", "title"),
            KindId::R2 => self.buckets(jurisdiction),
            KindId::D1 => self.paged("/d1/database?per_page=100", "name"),
            KindId::Queue => self.paged("/queues", "queue_name"),
            KindId::Vectorize => self.paged("/vectorize/v2/indexes", "name"),
            // Neither a Hyperdrive config's name nor a rate limiter's id is checked against the
            // account, nor anything linked or adopted: the plan never asks.
            KindId::Hyperdrive
            | KindId::RateLimit
            | KindId::StoredSecret
            | KindId::AiSearch
            | KindId::MtlsCertificate
            | KindId::ContainerImage => {
                Err(format!("the account's {} are not listed", kind.account_noun()).into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::live::tests::{rest, stub};

    #[test]
    fn a_listing_follows_every_page() {
        let (base, seen) = stub(vec![
            (
                "/storage/kv/namespaces?per_page=100&page=1",
                r#"{"success":true,"result":[{"id":"a","title":"shop"}],"result_info":{"total_pages":2}}"#.into(),
            ),
            (
                "/storage/kv/namespaces?per_page=100&page=2",
                r#"{"success":true,"result":[{"id":"b","title":"shop-staging"}],"result_info":{"total_pages":2}}"#.into(),
            ),
        ]);
        let listings = RestListings(rest(base));
        assert_eq!(
            listings.names(KindId::Kv, None).unwrap(),
            BTreeSet::from(["shop".to_string(), "shop-staging".to_string()])
        );
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// A listing whose `result_info` has no `total_pages` (D1's) pages on while a
    /// page comes back full, and stops at the first short one.
    #[test]
    fn a_listing_without_a_page_count_pages_while_pages_are_full() {
        let (base, seen) = stub(vec![
            (
                "/d1/database?per_page=100&page=1",
                format!(
                    r#"{{"success":true,"result":[{}],"result_info":{{"page":1,"per_page":100,"count":100}}}}"#,
                    (0..100)
                        .map(|i| format!(r#"{{"name":"db-{i}"}}"#))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            ),
            (
                "/d1/database?per_page=100&page=2",
                r#"{"success":true,"result":[{"name":"shop-orders"}],"result_info":{"page":2,"per_page":100,"count":1}}"#.into(),
            ),
        ]);
        let listings = RestListings(rest(base));
        let names = listings.names(KindId::D1, None).unwrap();
        assert_eq!(names.len(), 101);
        assert!(names.contains("shop-orders"));
        assert_eq!(seen.lock().unwrap().len(), 2, "no third page asked for");
    }

    #[test]
    fn buckets_follow_the_cursor() {
        // An opaque cursor goes back percent-encoded: a `+` would read as a space.
        let (base, seen) = stub(vec![
            (
                "/r2/buckets?per_page=1000&cursor=a%2Bb%2Fc%3D",
                r#"{"success":true,"result":{"buckets":[{"name":"shop-files-staging"}]},"result_info":{"cursor":""}}"#.into(),
            ),
            (
                "/r2/buckets?per_page=1000",
                r#"{"success":true,"result":{"buckets":[{"name":"shop-files"}]},"result_info":{"cursor":"a+b/c="}}"#.into(),
            ),
        ]);
        let listings = RestListings(rest(base));
        assert_eq!(
            listings.names(KindId::R2, None).unwrap(),
            BTreeSet::from(["shop-files".to_string(), "shop-files-staging".to_string()]),
            "{:?}",
            seen.lock().unwrap()
        );
    }

    struct Fake;

    impl Listings for Fake {
        fn names(
            &self,
            kind: KindId,
            _: Option<&str>,
        ) -> Result<BTreeSet<String>, CloudflareError> {
            match kind {
                KindId::D1 => Err("Cloudflare answered 403".into()),
                _ => Ok(BTreeSet::from(["shop-docs".to_string()])),
            }
        }
    }

    #[test]
    fn a_failed_listing_is_one_note_and_reads_as_unknown() {
        let wanted = BTreeSet::from([(KindId::Vectorize, None), (KindId::D1, None)]);
        let on = OnAccount::read(&wanted, &Fake);
        assert_eq!(on.holds(KindId::Vectorize, None, "shop-docs"), Some(true));
        assert_eq!(on.holds(KindId::Vectorize, None, "shop-notes"), Some(false));
        assert_eq!(
            on.holds(KindId::Kv, None, "shop-docs"),
            None,
            "not asked for"
        );
        assert_eq!(on.holds(KindId::D1, None, "shop-docs"), None, "not read");
        assert_eq!(on.notes.len(), 1, "{:?}", on.notes);
        assert!(on.notes[0].contains("D1 databases"), "{}", on.notes[0]);
        assert!(on.notes[0].contains("403"), "{}", on.notes[0]);
    }
}
