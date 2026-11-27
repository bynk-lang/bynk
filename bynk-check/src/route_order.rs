//! The order in which an HTTP router tries its routes (#3909).
//!
//! Routes of the same method are compared segment by segment, left to right. At
//! the first segment where two routes differ, a literal segment beats a `:name`
//! segment, and a `:name` segment beats a `*name` rest (#3752, D5), so the
//! outcome never depends on how a route is spelt. Segment names are not part of
//! a route's shape, and empty segments are insignificant (`/items`, `/items/`
//! and `//items` are one path).
//!
//! A rest is a route's last segment and matches **zero or more** remaining
//! segments. Where it matches zero, it still loses at the position it starts:
//! `/blog` beats `/blog/*path` for `/blog`.
//!
//! Two functions share that rule: [`order`], the order the emitted router tries
//! its routes in, and [`shadowed`], the routes no request can reach.

use std::cmp::Ordering;

/// One segment of a route: literal text, a `:name` parameter, or (last only) a
/// `*name` rest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Seg<'a> {
    Lit(&'a str),
    Param,
    Rest,
}

impl Seg<'_> {
    /// How strongly a segment binds where two routes differ: lower wins. The end
    /// of a route ranks with a literal: it only ever meets a rest, and beats it.
    fn rank(seg: Option<&Self>) -> u8 {
        match seg {
            None | Some(Seg::Lit(_)) => 0,
            Some(Seg::Param) => 1,
            Some(Seg::Rest) => 2,
        }
    }
}

fn segments(path: &str) -> Vec<Seg<'_>> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let last = segs.len().wrapping_sub(1);
    segs.iter()
        .enumerate()
        .map(|(i, s)| {
            if s.starts_with(':') && s.len() > 1 {
                Seg::Param
            } else if i == last && is_rest_segment(s) {
                Seg::Rest
            } else {
                Seg::Lit(s)
            }
        })
        .collect()
}

/// #3752 (D2): whether a route segment is a `*name` rest: `*` and then an
/// identifier, the whole segment (`*key.txt` is not one). Only a route's last
/// segment is read as one; the checker refuses a `*` anywhere else.
pub fn is_rest_segment(seg: &str) -> bool {
    seg.strip_prefix('*').is_some_and(|name| {
        name.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// The segment `route` presents at `at`: its own, or, past a final rest, the
/// rest again (it takes any number of further segments).
fn seg_at<'r, 'a>(route: &'r [Seg<'a>], at: usize) -> Option<&'r Seg<'a>> {
    route
        .get(at)
        .or_else(|| route.last().filter(|s| **s == Seg::Rest))
}

/// Whether two routes of one method can match a common path: no position where
/// both are literals that differ, and the same number of segments unless a rest
/// takes up the difference.
fn overlaps(a: &[Seg], b: &[Seg]) -> bool {
    for i in 0.. {
        match (a.get(i), b.get(i)) {
            (Some(Seg::Rest), _) | (_, Some(Seg::Rest)) => return true,
            (None, None) => return true,
            (None, Some(_)) | (Some(_), None) => return false,
            (Some(Seg::Lit(p)), Some(Seg::Lit(q))) if p != q => return false,
            _ => {}
        }
    }
    unreachable!("the loop returns at the shorter route's end")
}

/// Whether, of two routes that [`overlaps`], `a` is tried first: at the first
/// segment where they differ, the more specific one wins (a literal, then a
/// parameter, then a rest). `None` when the two have the same shape (they differ
/// at most in parameter names).
fn precedes(a: &[Seg], b: &[Seg]) -> Option<bool> {
    for i in 0.. {
        let (x, y) = (a.get(i), b.get(i));
        let (rx, ry) = (Seg::rank(x), Seg::rank(y));
        if rx != ry {
            return Some(rx < ry);
        }
        if x.is_none() || y.is_none() || x == Some(&Seg::Rest) {
            return None;
        }
    }
    unreachable!("the loop returns at the shorter route's end")
}

/// The indices of `routes` in the order a router tries them. `routes` is each
/// route's `(method, path)`, already in the order to keep wherever the rule leaves
/// two routes unordered, so a router whose routes do not overlap keeps its order.
/// Only routes of one method that match a common path are ordered by the rule.
pub fn order(routes: &[(&str, &str)]) -> Vec<usize> {
    let segs: Vec<Vec<Seg>> = routes.iter().map(|(_, p)| segments(p)).collect();
    let n = routes.len();
    // `before[b]` lists the routes that must come before `b`.
    let mut before: Vec<Vec<usize>> = vec![Vec::new(); n];
    for a in 0..n {
        for b in 0..n {
            if a != b
                && routes[a].0 == routes[b].0
                && overlaps(&segs[a], &segs[b])
                && precedes(&segs[a], &segs[b]) == Some(true)
            {
                before[b].push(a);
            }
        }
    }
    // Kahn's algorithm, always taking the lowest remaining index: stable.
    let mut done = vec![false; n];
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let next = (0..n)
            .find(|&i| !done[i] && before[i].iter().all(|&a| done[a]))
            .expect("the specificity rule has no cycle");
        done[next] = true;
        out.push(next);
    }
    out
}

/// The routes no request can reach. `routes` is each route's `(method, path)`;
/// the result is, for each unreachable route, its index and the indices of the
/// routes that cover it. A route is unreachable when every path it matches is
/// matched first by other routes of its method. A route spelt identically to
/// another is `bynk.http.duplicate_route`'s concern and covers nothing here.
pub fn shadowed(routes: &[(&str, &str)]) -> Vec<(usize, Vec<usize>)> {
    let segs: Vec<Vec<Seg>> = routes.iter().map(|(_, p)| segments(p)).collect();
    let mut out = Vec::new();
    for r in 0..routes.len() {
        let covering: Vec<usize> = (0..routes.len())
            .filter(|&a| {
                a != r
                    && routes[a].0 == routes[r].0
                    && routes[a].1 != routes[r].1
                    && overlaps(&segs[a], &segs[r])
                    && match precedes(&segs[a], &segs[r]) {
                        Some(first) => first,
                        // The same shape: the one spelt first (alphabetically) is tried first.
                        None => routes[a].1 < routes[r].1,
                    }
            })
            .collect();
        if !covering.is_empty()
            && covered(&segs[r], 0, covering.iter().map(|&a| &segs[a]).collect())
        {
            out.push((r, covering));
        }
    }
    out
}

/// Whether the paths `route` matches from segment `at` on are all matched by one
/// of `earlier`: routes already known to match the segments before `at`. A rest
/// in `earlier` at `at` matches everything from there on; a rest in `route`
/// stands for the path ending here or going on with any segment, each a case.
/// The recursion ends: `at` only grows, and once it passes every finite route in
/// `earlier`, none is left.
fn covered(route: &[Seg], at: usize, earlier: Vec<&Vec<Seg>>) -> bool {
    if earlier.iter().any(|e| e.get(at) == Some(&Seg::Rest)) {
        return true;
    }
    // The routes that end exactly here.
    let ends = || earlier.iter().any(|e| e.len() == at);
    // The routes that match a segment holding `value`: literal `value`, or any parameter.
    let matching = |value: &str| -> Vec<&Vec<Seg>> {
        earlier
            .iter()
            .copied()
            .filter(|e| {
                matches!(e.get(at), Some(Seg::Param)) || e.get(at) == Some(&Seg::Lit(value))
            })
            .collect()
    };
    match seg_at(route, at) {
        None => ends(),
        Some(Seg::Lit(v)) => covered(route, at + 1, matching(v)),
        Some(seg @ (Seg::Param | Seg::Rest)) => {
            if *seg == Seg::Rest && !ends() {
                return false;
            }
            // Every literal the earlier routes name here is a case of its own, and
            // every other value is the case the parameters alone must cover.
            let mut literals: Vec<&str> = earlier
                .iter()
                .filter_map(|e| match e.get(at) {
                    Some(Seg::Lit(l)) => Some(*l),
                    _ => None,
                })
                .collect();
            literals.sort_unstable();
            literals.dedup();
            let others: Vec<&Vec<Seg>> = earlier
                .iter()
                .copied()
                .filter(|e| matches!(e.get(at), Some(Seg::Param)))
                .collect();
            literals
                .into_iter()
                .all(|l| covered(route, at + 1, matching(l)))
                && covered(route, at + 1, others)
        }
    }
}

/// Whether two route patterns match a common path (no position where both are
/// literals that differ, and the same number of segments unless a rest takes up
/// the difference).
pub fn overlap(a: &str, b: &str) -> bool {
    overlaps(&segments(a), &segments(b))
}

/// Whether `general` matches every path `specific` matches.
pub fn covers(general: &str, specific: &str) -> bool {
    let (g, s) = (segments(general), segments(specific));
    covered(&s, 0, vec![&g])
}

/// The comparison [`order`] applies to two routes, for callers that sort
/// patterns of one method (an `Ordering::Equal` means the rule does not decide).
pub fn compare(a: &str, b: &str) -> Ordering {
    let (sa, sb) = (segments(a), segments(b));
    if !overlaps(&sa, &sb) {
        return Ordering::Equal;
    }
    match precedes(&sa, &sb) {
        Some(true) => Ordering::Less,
        Some(false) => Ordering::Greater,
        None => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_beats_a_parameter_at_the_first_difference() {
        let routes = [("GET", "/a/:x/c"), ("GET", "/a/b/:y")];
        assert_eq!(order(&routes), vec![1, 0]);
        let swapped = [("GET", "/a/b/:y"), ("GET", "/a/:x/c")];
        assert_eq!(order(&swapped), vec![0, 1]);
    }

    #[test]
    fn non_overlapping_routes_keep_their_order() {
        let routes = [("GET", "/b"), ("GET", "/a/:x"), ("POST", "/a/:x")];
        assert_eq!(order(&routes), vec![0, 1, 2]);
    }

    #[test]
    fn me_beats_id_whatever_the_declaration_order() {
        let routes = [("GET", "/users/:id"), ("GET", "/users/me")];
        assert_eq!(order(&routes), vec![1, 0]);
        // A different method is not compared.
        let other = [("POST", "/users/me"), ("GET", "/users/:id")];
        assert_eq!(order(&other), vec![0, 1]);
    }

    #[test]
    fn overlap_alone_is_not_unreachable() {
        assert!(shadowed(&[("GET", "/users/me"), ("GET", "/users/:id")]).is_empty());
        assert!(shadowed(&[("GET", "/a/:x/c"), ("GET", "/a/b/:y")]).is_empty());
    }

    #[test]
    fn a_route_wholly_covered_is_unreachable() {
        // `/a/:x` is covered by `/a/b` and the same-shaped `/a/:y`.
        let routes = [("GET", "/a/b"), ("GET", "/a/:x"), ("GET", "/a/:y")];
        let found = shadowed(&routes);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 2);
    }

    #[test]
    fn a_union_of_routes_can_cover_a_route() {
        // `/p/q` (neither `x` nor `y`) is left to `/:a/:b`: reachable.
        let reachable = [("GET", "/x/:b"), ("GET", "/:a/y"), ("GET", "/:a/:b")];
        assert!(shadowed(&reachable).is_empty());
        // With a same-shaped route tried first, `/:p/:q2` is left nothing.
        let covered = [
            ("GET", "/x/y"),
            ("GET", "/x/:q"),
            ("GET", "/:p/y"),
            ("GET", "/:p/:q"),
            ("GET", "/:p/:q2"),
        ];
        let found = shadowed(&covered);
        assert_eq!(found.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![4]);
    }

    #[test]
    fn methods_do_not_cover_each_other() {
        assert!(shadowed(&[("GET", "/a/b"), ("POST", "/a/:x")]).is_empty());
    }

    #[test]
    fn identical_spellings_are_the_duplicate_rules_concern() {
        assert!(shadowed(&[("GET", "/a/:x"), ("GET", "/a/:x")]).is_empty());
    }

    #[test]
    fn empty_segments_are_insignificant() {
        assert_eq!(compare("/a/b/", "/a/:x"), Ordering::Less);
        assert_eq!(shadowed(&[("GET", "/a/b"), ("GET", "/a//:x/")]).len(), 0);
        assert_eq!(shadowed(&[("GET", "/a/:y"), ("GET", "//a//:x/")]).len(), 1);
    }

    // #3752 (D5): every line of the track's §5.4, in both declaration orders.
    #[test]
    fn a_literal_beats_a_rest() {
        // `GET /files/index.json` beats `GET /files/*key` for `/files/index.json`.
        assert_eq!(compare("/files/index.json", "/files/*key"), Ordering::Less);
        assert_eq!(
            order(&[("GET", "/files/*key"), ("GET", "/files/index.json")]),
            vec![1, 0]
        );
    }

    #[test]
    fn a_longer_literal_prefix_beats_a_shorter_rest() {
        // `GET /a/b/*y` beats `GET /a/*x` for `/a/b/c`: at position 2, `b` beats `*x`.
        assert_eq!(order(&[("GET", "/a/*x"), ("GET", "/a/b/*y")]), vec![1, 0]);
        assert!(shadowed(&[("GET", "/a/b/*y"), ("GET", "/a/*x")]).is_empty());
    }

    #[test]
    fn a_parameter_beats_a_rest_which_takes_longer_paths() {
        // `GET /a/:id` beats `GET /a/*x` for `/a/1`; `/a/1/2` goes to `/a/*x`.
        assert_eq!(order(&[("GET", "/a/*x"), ("GET", "/a/:id")]), vec![1, 0]);
        assert!(overlap("/a/*x", "/a/1/2"));
        assert!(!overlap("/a/:id", "/a/1/2"));
        // Neither shadows the other: `/a/*x` still answers `/a` and `/a/1/2`.
        assert!(shadowed(&[("GET", "/a/:p"), ("GET", "/a/*x")]).is_empty());
    }

    #[test]
    fn a_rest_matching_zero_segments_still_loses_where_it_starts() {
        // `GET /blog` beats `/blog/*path` for `/blog`, and so for `/blog/` too.
        assert_eq!(compare("/blog", "/blog/*path"), Ordering::Less);
        assert_eq!(compare("/blog/", "/blog/*path"), Ordering::Less);
        assert!(overlap("/blog", "/blog/*path"));
        assert!(shadowed(&[("GET", "/blog"), ("GET", "/blog/*path")]).is_empty());
    }

    #[test]
    fn two_rests_at_one_position_shadow() {
        // `GET /a/*x` beside `GET /a/*y` is `bynk.http.unreachable_route`, as for `:name`.
        let found = shadowed(&[("GET", "/a/*x"), ("GET", "/a/*y")]);
        assert_eq!(found, vec![(1, vec![0])]);
    }

    #[test]
    fn a_rest_is_covered_only_by_a_rest_or_every_length() {
        // A root catch-all covers a narrower rest.
        assert_eq!(shadowed(&[("GET", "/*all"), ("GET", "/a/*x")]), vec![]);
        assert_eq!(shadowed(&[("GET", "/a/*x"), ("GET", "/*all")]), vec![]);
        // `/:a` (one segment) and `/` don't cover `/*p` (two or more).
        assert!(shadowed(&[("GET", "/"), ("GET", "/:a"), ("GET", "/*p")]).is_empty());
        // A rest covers a fixed route under it only if it is tried first: it isn't.
        assert!(shadowed(&[("GET", "/*p"), ("GET", "/a/b")]).is_empty());
        assert!(covers("/*p", "/a/b"));
        assert!(covers("/a/*p", "/a/*q"));
        assert!(!covers("/a/:x", "/a/*q"));
        assert!(!covers("/a/*q", "/b"));
    }

    #[test]
    fn a_rest_is_read_only_as_a_last_named_segment() {
        assert!(is_rest_segment("*key"));
        assert!(!is_rest_segment("*"));
        assert!(!is_rest_segment("x*key"));
        assert!(!is_rest_segment("*key*"));
        assert!(!is_rest_segment("*key.txt"));
        assert!(!is_rest_segment("*1x"));
        // Not last: a literal shape (the checker refuses it).
        assert_eq!(segments("/*a/b"), vec![Seg::Lit("*a"), Seg::Lit("b")]);
        assert_eq!(segments("/a/*b/"), vec![Seg::Lit("a"), Seg::Rest]);
    }
}
