//! Cloudflare's paginated listings, followed in one place (1D.3c, #4043 Decision K).
//!
//! Before 1D.3c, ten loops in seven files each wrote their own page counter and stop rule.
//! They follow three rules, and each keeps its own exactly, so every request sequence is
//! unchanged:
//!
//! - numbered pages that stop at `total_pages`, read as the last page when it is absent
//!   ([`MissingTotal::AssumeLast`]), some capped and some stopping early;
//! - numbered pages that, without `total_pages`, go on while a page is full
//!   ([`MissingTotal::WhileFull`], D1's listing);
//! - a cursor ([`cursor`]), with or without a stop on a cursor that repeats.
//!
//! A site reads each page itself and reports what it counted ([`Page`]): what counts as an
//! empty page differs between sites (an entry's raw count, or the entries it could read), and
//! keeping that with the site is what keeps its rule exact. The R2 jurisdiction header is the
//! site's own fetch, as before.
//!
//! Two inconsistencies are kept, not fixed, because fixing either changes the requests made:
//! a missing `total_pages` read as the last page stops after one page of a listing that, like
//! D1's, omits it; and the Durable Object objects listing has no stop on a repeated cursor.

/// What a site read from one numbered page.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Page {
    /// How many entries the page held, as the site counts them.
    pub(crate) entries: usize,
    /// `result_info.total_pages`, when Cloudflare sent it.
    pub(crate) total_pages: Option<u64>,
    /// `result_info.per_page`, when Cloudflare sent it (read by [`MissingTotal::WhileFull`]).
    pub(crate) per_page: Option<u64>,
    /// The site has what it came for: no further page is asked for.
    pub(crate) done: bool,
}

impl Page {
    /// A page of `entries`, with the `total_pages` and `per_page` of `body`'s `result_info`.
    pub(crate) fn of(body: &serde_json::Value, entries: usize) -> Page {
        let info = &body["result_info"];
        Page {
            entries,
            total_pages: info["total_pages"].as_u64(),
            per_page: info["per_page"].as_u64(),
            done: false,
        }
    }

    /// This page, with the site's early stop.
    pub(crate) fn done_if(self, done: bool) -> Page {
        Page { done, ..self }
    }
}

/// How a numbered listing without `total_pages` is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MissingTotal {
    /// As the last page.
    AssumeLast,
    /// As going on while a page holds `per_page` entries.
    WhileFull,
}

/// A numbered listing's stop rule.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Numbered {
    /// The most pages read, when there is a limit.
    pub(crate) cap: Option<u64>,
    pub(crate) missing_total: MissingTotal,
    /// Whether a page with no entries ends the listing even when `total_pages` says more.
    pub(crate) stop_on_empty: bool,
}

impl Numbered {
    /// The common rule: stop at `total_pages` (the last page when absent) or on an empty page.
    pub(crate) const TO_TOTAL: Numbered = Numbered {
        cap: None,
        missing_total: MissingTotal::AssumeLast,
        stop_on_empty: true,
    };

    /// [`Numbered::TO_TOTAL`], reading at most `cap` pages.
    pub(crate) const fn capped(cap: u64) -> Numbered {
        Numbered {
            cap: Some(cap),
            ..Numbered::TO_TOTAL
        }
    }

    /// Follow the listing from page 1: `each` asks for and reads one page.
    pub(crate) fn follow<E>(self, mut each: impl FnMut(u64) -> Result<Page, E>) -> Result<(), E> {
        for page in 1..=self.cap.unwrap_or(u64::MAX) {
            let read = each(page)?;
            let more = match (read.total_pages, self.missing_total) {
                (Some(total), _) => page < total,
                (None, MissingTotal::AssumeLast) => false,
                (None, MissingTotal::WhileFull) => read
                    .per_page
                    .is_some_and(|per| read.entries as u64 >= per && read.entries > 0),
            };
            if read.done || (self.stop_on_empty && read.entries == 0) || !more {
                break;
            }
        }
        Ok(())
    }
}

/// Follow a cursor listing: `each` asks for the page at a cursor (`None` for the first) and
/// returns the cursor Cloudflare sent next, if any. An absent or empty cursor ends it, and so
/// does one that repeats when `stop_on_repeat` is set.
pub(crate) fn cursor<E>(
    stop_on_repeat: bool,
    mut each: impl FnMut(Option<&str>) -> Result<Option<String>, E>,
) -> Result<(), E> {
    let mut at: Option<String> = None;
    loop {
        match each(at.as_deref())? {
            Some(next) if !next.is_empty() && !(stop_on_repeat && at.as_deref() == Some(&next)) => {
                at = Some(next);
            }
            _ => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pages `rule` asks for over `pages`: (entries, total_pages, per_page) of each.
    fn asked(rule: Numbered, pages: &[(usize, Option<u64>, Option<u64>)]) -> Vec<u64> {
        let mut asked = Vec::new();
        rule.follow(|page| -> Result<Page, ()> {
            asked.push(page);
            let (entries, total_pages, per_page) = pages[page as usize - 1];
            Ok(Page {
                entries,
                total_pages,
                per_page,
                done: false,
            })
        })
        .unwrap();
        asked
    }

    #[test]
    fn a_numbered_listing_stops_at_its_total_or_an_empty_page() {
        let rule = Numbered::TO_TOTAL;
        assert_eq!(asked(rule, &[(2, Some(3), None); 3]), [1, 2, 3]);
        assert_eq!(
            asked(rule, &[(2, Some(3), None), (0, Some(3), None)]),
            [1, 2]
        );
        // No total reads as the last page, even when the page is full.
        assert_eq!(asked(rule, &[(100, None, Some(100))]), [1]);
        assert_eq!(asked(Numbered::capped(2), &[(1, Some(9), None); 2]), [1, 2]);
    }

    #[test]
    fn a_listing_without_a_total_goes_on_while_full_when_its_rule_says() {
        let rule = Numbered {
            cap: None,
            missing_total: MissingTotal::WhileFull,
            stop_on_empty: false,
        };
        assert_eq!(
            asked(
                rule,
                &[(2, None, Some(2)), (2, None, Some(2)), (1, None, Some(2))]
            ),
            [1, 2, 3]
        );
        assert_eq!(asked(rule, &[(0, None, Some(0))]), [1]);
        // With a total, an empty page does not end it under this rule.
        assert_eq!(
            asked(rule, &[(0, Some(2), None), (1, Some(2), None)]),
            [1, 2]
        );
    }

    #[test]
    fn a_site_that_has_what_it_came_for_stops_early() {
        let mut asked = 0;
        Numbered::TO_TOTAL
            .follow(|page| -> Result<Page, ()> {
                asked += 1;
                Ok(Page {
                    entries: 1,
                    total_pages: Some(5),
                    done: page == 2,
                    ..Page::default()
                })
            })
            .unwrap();
        assert_eq!(asked, 2);
    }

    #[test]
    fn a_cursor_is_followed_until_it_ends_or_repeats() {
        let run = |stop_on_repeat: bool, next: &[&str]| {
            let mut seen = Vec::new();
            let mut answers = next.iter();
            cursor(stop_on_repeat, |at| -> Result<Option<String>, ()> {
                seen.push(at.map(str::to_string));
                Ok(answers.next().map(|n| n.to_string()))
            })
            .unwrap();
            seen
        };
        assert_eq!(
            run(true, &["a", "b", ""]),
            [None, Some("a".into()), Some("b".into())]
        );
        assert_eq!(run(true, &["a", "a"]), [None, Some("a".into())]);
        assert_eq!(
            run(false, &["a", "a", ""]),
            [None, Some("a".into()), Some("a".into())]
        );
    }
}
