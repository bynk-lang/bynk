//! Predicate entailment over the closed refinement vocabulary (#2802).
//!
//! One engine answers "does the sender's conjunction of predicates guarantee
//! this receiver predicate?" for both consumers that ask it: the checker's
//! cross-context argument and return matching
//! ([`crate::checker`]'s `refinements_match`) and the deploy-gate classifier
//! ([`crate::contract_compat`]). The two used to hold separate notions
//! (set-equality here, entailment there), so the compiler rejected a program
//! the gate called compatible.
//!
//! Predicates arrive in their canonical text (`contract::canon_pred_set`),
//! so `NonEmpty` is already `MinLength(1)` and `InRange` bounds are values, not
//! lexemes. The engine is **sound but incomplete**: it models an `Int` as
//! unbounded, so it never concludes something only the safe-integer limit would
//! give. `Matches` is entailed only by an identical `Matches` (regex-language
//! inclusion is not attempted); a `Matches` on the sender is otherwise ignored,
//! so `Matches(p) && MaxLength(4)` still entails `MaxLength(8)`.

use std::cmp::Ordering;

#[derive(Clone, Copy)]
struct Interval<T> {
    lo: Option<(T, bool)>,
    hi: Option<(T, bool)>,
}

impl<T: PartialOrd + Copy> Interval<T> {
    const ALL: Self = Interval { lo: None, hi: None };

    fn intersect(self, o: Self) -> Self {
        Interval {
            lo: tighter(self.lo, o.lo, Ordering::Greater),
            hi: tighter(self.hi, o.hi, Ordering::Less),
        }
    }

    /// Every value of `self` lies in `outer`.
    fn within(self, outer: Self) -> bool {
        let lo_ok = match (self.lo, outer.lo) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some((s, so)), Some((o, oo))) => match s.partial_cmp(&o) {
                Some(Ordering::Greater) => true,
                Some(Ordering::Equal) => so || !oo,
                _ => false,
            },
        };
        let hi_ok = match (self.hi, outer.hi) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some((s, so)), Some((o, oo))) => match s.partial_cmp(&o) {
                Some(Ordering::Less) => true,
                Some(Ordering::Equal) => so || !oo,
                _ => false,
            },
        };
        lo_ok && hi_ok
    }
}

/// The tighter of two bounds; `want` is `Greater` for lower bounds.
fn tighter<T: PartialOrd + Copy>(
    a: Option<(T, bool)>,
    b: Option<(T, bool)>,
    want: Ordering,
) -> Option<(T, bool)> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some((av, ao)), Some((bv, bo))) => match av.partial_cmp(&bv) {
            Some(o) if o == want => Some((av, ao)),
            Some(Ordering::Equal) => Some((av, ao || bo)),
            _ => Some((bv, bo)),
        },
    }
}

/// `args` of `Name(a, b)`, split on the top-level comma.
fn pred_args<'a>(p: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let inner = p.strip_prefix(name)?.strip_prefix('(')?.strip_suffix(')')?;
    Some(inner.split(", ").collect())
}

fn closed<T>(lo: T, hi: T) -> Interval<T> {
    Interval {
        lo: Some((lo, false)),
        hi: Some((hi, false)),
    }
}

/// A predicate as a bound on the numeric domain (`Int`, `Float` or `Decimal`) or
/// the length domain.
enum Domain<'a> {
    Int(Interval<i128>),
    Float(Interval<f64>),
    Decimal(Interval<Dec<'a>>),
    Length(Interval<i128>),
}

/// A canonical decimal bound (`-1.5`, `0`), compared by value and independent of
/// scale (`1.2 == 1.20`).
#[derive(Clone, Copy)]
struct Dec<'a>(&'a str);

impl Dec<'_> {
    /// The coefficient (the digits with the point removed) and the scale.
    fn parts(&self) -> (String, u32) {
        match self.0.split_once('.') {
            Some((int, frac)) => (format!("{int}{frac}"), frac.len() as u32),
            None => (self.0.to_string(), 0),
        }
    }
}

impl PartialEq for Dec<'_> {
    fn eq(&self, o: &Self) -> bool {
        self.partial_cmp(o) == Some(Ordering::Equal)
    }
}

impl PartialOrd for Dec<'_> {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        let ((ac, asc), (bc, bsc)) = (self.parts(), o.parts());
        Some(bynk_syntax::decimal::cmp(&ac, asc, &bc, bsc))
    }
}

/// A canonical decimal bound is digits, an optional leading `-` and an optional
/// point, with at least one digit.
fn dec(text: &str) -> Option<Dec<'_>> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let ok = digits.bytes().any(|b| b.is_ascii_digit())
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && digits.bytes().filter(|&b| b == b'.').count() <= 1;
    ok.then_some(Dec(text))
}

fn domain<'a>(base: &str, p: &'a str) -> Option<Domain<'a>> {
    let int_pred = || -> Option<Interval<i128>> {
        Some(match p {
            "Positive" => Interval {
                lo: Some((1, false)),
                hi: None,
            },
            "NonNegative" => Interval {
                lo: Some((0, false)),
                hi: None,
            },
            _ => {
                let a = pred_args(p, "InRange")?;
                closed(a.first()?.parse().ok()?, a.get(1)?.parse().ok()?)
            }
        })
    };
    let float_pred = || -> Option<Interval<f64>> {
        Some(match p {
            "Positive" => Interval {
                lo: Some((0.0, true)),
                hi: None,
            },
            "NonNegative" => Interval {
                lo: Some((0.0, false)),
                hi: None,
            },
            _ => {
                let a = pred_args(p, "InRangeF")?;
                closed(a.first()?.parse().ok()?, a.get(1)?.parse().ok()?)
            }
        })
    };
    let dec_pred = || -> Option<Interval<Dec<'a>>> {
        Some(match p {
            "Positive" => Interval {
                lo: Some((dec("0")?, true)),
                hi: None,
            },
            "NonNegative" => Interval {
                lo: Some((dec("0")?, false)),
                hi: None,
            },
            _ => {
                let a = pred_args(p, "InRangeD")?;
                closed(dec(a.first()?)?, dec(a.get(1)?)?)
            }
        })
    };
    if let Some(a) = pred_args(p, "MinLength") {
        return Some(Domain::Length(Interval {
            lo: Some((a.first()?.parse().ok()?, false)),
            hi: None,
        }));
    }
    if let Some(a) = pred_args(p, "MaxLength") {
        return Some(Domain::Length(Interval {
            lo: None,
            hi: Some((a.first()?.parse().ok()?, false)),
        }));
    }
    if let Some(a) = pred_args(p, "Length") {
        let n = a.first()?.parse().ok()?;
        return Some(Domain::Length(closed(n, n)));
    }
    match base {
        "Int" => int_pred().map(Domain::Int),
        "Float" => float_pred().map(Domain::Float),
        "Decimal" => dec_pred().map(Domain::Decimal),
        _ => None,
    }
}

/// Does the conjunction `sender` guarantee the predicate `q`?
pub fn entailed(base: &str, sender: &[String], q: &str) -> bool {
    if sender.iter().any(|p| p == q) {
        return true;
    }
    let Some(want) = domain(base, q) else {
        // `Matches`, or a predicate this version cannot reason about: only an
        // identical predicate satisfies it.
        return false;
    };
    let given = sender.iter().filter_map(|p| domain(base, p));
    match want {
        Domain::Int(w) => {
            let mut have = Interval::ALL;
            for d in given {
                if let Domain::Int(i) = d {
                    have = have.intersect(i);
                }
            }
            have.within(w)
        }
        Domain::Float(w) => {
            let mut have = Interval::ALL;
            for d in given {
                if let Domain::Float(i) = d {
                    have = have.intersect(i);
                }
            }
            have.within(w)
        }
        Domain::Decimal(w) => {
            let mut have = Interval::ALL;
            for d in given {
                if let Domain::Decimal(i) = d {
                    have = have.intersect(i);
                }
            }
            have.within(w)
        }
        Domain::Length(w) => {
            let mut have = Interval::ALL;
            for d in given {
                if let Domain::Length(i) = d {
                    have = have.intersect(i);
                }
            }
            have.within(w)
        }
    }
}

/// The receiver predicates the sender's conjunction does not guarantee; empty
/// when the sender is admissible where the receiver is expected.
pub fn unmet<'a>(base: &str, sender: &[String], receiver: &'a [String]) -> Vec<&'a String> {
    receiver
        .iter()
        .filter(|q| !entailed(base, sender, q))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn decimal_bounds_compare_by_value_not_scale() {
        // `canon_predicate` drops trailing zeros, so scales differ between sides.
        assert!(entailed(
            "Decimal",
            &s(&["InRangeD(0.5, 1)"]),
            "InRangeD(0, 2)"
        ));
        assert!(entailed(
            "Decimal",
            &s(&["InRangeD(1.2, 1.2)"]),
            "InRangeD(1.20, 1.3)"
        ));
        assert!(!entailed(
            "Decimal",
            &s(&["InRangeD(0, 2)"]),
            "InRangeD(0.5, 1)"
        ));
        assert!(!entailed(
            "Decimal",
            &s(&["InRangeD(-1.5, 2)"]),
            "InRangeD(-1, 2)"
        ));
    }

    #[test]
    fn decimal_sign_predicates() {
        assert!(entailed("Decimal", &s(&["Positive"]), "NonNegative"));
        assert!(!entailed("Decimal", &s(&["NonNegative"]), "Positive"));
        assert!(entailed("Decimal", &s(&["InRangeD(0.01, 5)"]), "Positive"));
        assert!(!entailed("Decimal", &s(&["InRangeD(0, 5)"]), "Positive"));
        assert!(!entailed("Decimal", &s(&["Positive"]), "InRangeD(0, 5)"));
    }

    #[test]
    fn matches_is_entailed_only_by_the_identical_pattern() {
        let m = "Matches(\"[a-z]+\")";
        assert!(entailed("String", &s(&[m]), m));
        assert!(!entailed("String", &s(&["Matches(\"[a-z0-9]+\")"]), m));
        // A sender's `Matches` is ignored for other receiver predicates.
        assert!(entailed("String", &s(&[m, "MaxLength(4)"]), "MaxLength(8)"));
        assert!(!entailed("String", &s(&[m]), "MaxLength(8)"));
    }

    #[test]
    fn float_positive_is_open_at_zero() {
        assert!(entailed("Float", &s(&["Positive"]), "NonNegative"));
        assert!(!entailed("Float", &s(&["NonNegative"]), "Positive"));
        assert!(entailed(
            "Float",
            &s(&["InRangeF(0.5, 1)"]),
            "InRangeF(0, 2)"
        ));
    }
}
