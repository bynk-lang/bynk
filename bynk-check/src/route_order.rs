//! The order in which an HTTP router tries its routes (#3909).
//!
//! Routes of the same method are compared segment by segment, left to right. At
//! the first segment where two routes differ, a literal segment beats a `:name`
//! segment, so the outcome never depends on how a route is spelt. Segment names
//! are not part of a route's shape, and empty segments are insignificant
//! (`/items`, `/items/` and `//items` are one path).
//!
//! Two functions share that rule: [`order`], the order the emitted router tries
//! its routes in, and [`shadowed`], the routes no request can reach.

use std::cmp::Ordering;

/// One segment of a route: literal text, or a `:name` parameter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Seg<'a> {
    Lit(&'a str),
    Param,
}

fn segments(path: &str) -> Vec<Seg<'_>> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.starts_with(':') && s.len() > 1 {
                Seg::Param
            } else {
                Seg::Lit(s)
            }
        })
        .collect()
}

/// Whether two routes of one method can match a common path: the same number of
/// segments, and no position where both are literals that differ.
fn overlaps(a: &[Seg], b: &[Seg]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| match (x, y) {
            (Seg::Lit(p), Seg::Lit(q)) => p == q,
            _ => true,
        })
}

/// Whether, of two routes that [`overlaps`], `a` is tried first: at the first
/// segment where they differ, the literal one wins. `None` when the two have
/// the same shape (they differ at most in parameter names).
fn precedes(a: &[Seg], b: &[Seg]) -> Option<bool> {
    a.iter().zip(b).find_map(|(x, y)| match (x, y) {
        (Seg::Lit(_), Seg::Param) => Some(true),
        (Seg::Param, Seg::Lit(_)) => Some(false),
        _ => None,
    })
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
            .expect("the literal-before-parameter rule has no cycle");
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
/// of `earlier`: routes already known to match the segments before `at`.
fn covered(route: &[Seg], at: usize, earlier: Vec<&Vec<Seg>>) -> bool {
    if at == route.len() {
        return !earlier.is_empty();
    }
    // The routes that match a segment holding `value`: literal `value`, or any parameter.
    let matching = |value: &str| -> Vec<&Vec<Seg>> {
        earlier
            .iter()
            .copied()
            .filter(|e| matches!(e[at], Seg::Param) || e[at] == Seg::Lit(value))
            .collect()
    };
    match route[at] {
        Seg::Lit(v) => covered(route, at + 1, matching(v)),
        Seg::Param => {
            // Every literal the earlier routes name here is a case of its own, and
            // every other value is the case the parameters alone must cover.
            let mut literals: Vec<&str> = earlier
                .iter()
                .filter_map(|e| match e[at] {
                    Seg::Lit(l) => Some(l),
                    Seg::Param => None,
                })
                .collect();
            literals.sort_unstable();
            literals.dedup();
            let others: Vec<&Vec<Seg>> = earlier
                .iter()
                .copied()
                .filter(|e| matches!(e[at], Seg::Param))
                .collect();
            literals
                .into_iter()
                .all(|l| covered(route, at + 1, matching(l)))
                && covered(route, at + 1, others)
        }
    }
}

/// Whether two route patterns match a common path (the same number of segments,
/// and no position where both are literals that differ).
pub fn overlap(a: &str, b: &str) -> bool {
    overlaps(&segments(a), &segments(b))
}

/// Whether `general` matches every path `specific` matches: the same number of
/// segments, and at each a parameter, or the same literal.
pub fn covers(general: &str, specific: &str) -> bool {
    let (g, s) = (segments(general), segments(specific));
    g.len() == s.len()
        && g.iter().zip(&s).all(|(g, s)| match (g, s) {
            (Seg::Param, _) => true,
            (Seg::Lit(a), Seg::Lit(b)) => a == b,
            (Seg::Lit(_), Seg::Param) => false,
        })
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
}
