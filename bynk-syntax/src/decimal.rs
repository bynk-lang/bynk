//! Compile-time `Decimal` arithmetic helpers (#2622): the comparison and sign
//! tests the checker needs to evaluate a decimal literal against a refinement
//! (`Positive`, `NonNegative`, `InRange`). A decimal here is an optionally
//! `-`-prefixed coefficient string and a scale; the runtime owns the real
//! arithmetic (`bynk-emit/runtime/src/decimal.ts`).

use std::cmp::Ordering;

/// The sign of a coefficient: `-1`, `0` or `1`.
pub fn sign(coef: &str) -> i32 {
    let (neg, mag) = split_sign(coef);
    if mag.bytes().all(|b| b == b'0') {
        0
    } else if neg {
        -1
    } else {
        1
    }
}

fn split_sign(coef: &str) -> (bool, &str) {
    match coef.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, coef),
    }
}

/// A decimal in canonical text form: `render("120", 2) == "1.20"`,
/// `render("-5", 3) == "-0.005"`. A scale of 0 prints no point.
pub fn render(coef: &str, scale: u32) -> String {
    let (neg, mag) = split_sign(coef);
    let scale = scale as usize;
    let mag = if mag.len() <= scale {
        format!("{}{mag}", "0".repeat(scale + 1 - mag.len()))
    } else {
        mag.to_string()
    };
    let (int, frac) = mag.split_at(mag.len() - scale);
    let sign = if neg { "-" } else { "" };
    if scale == 0 {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac}")
    }
}

/// The value-canonical text of a decimal: trailing fraction zeros dropped, so
/// `1.20` and `1.2` agree (a contract is about values, not scales).
pub fn canonical(coef: &str, scale: u32) -> String {
    let (neg, mag) = split_sign(coef);
    let (mut mag, mut scale) = (mag.trim_start_matches('0').to_string(), scale);
    while scale > 0 && mag.ends_with('0') {
        mag.pop();
        scale -= 1;
    }
    if mag.is_empty() {
        return "0".to_string();
    }
    render(&format!("{}{mag}", if neg { "-" } else { "" }), scale)
}

/// The numeric order of two decimals, ignoring how each is scaled
/// (`1.20 == 1.2`).
pub fn cmp(a_coef: &str, a_scale: u32, b_coef: &str, b_scale: u32) -> Ordering {
    let (sa, sb) = (sign(a_coef), sign(b_coef));
    if sa != sb {
        return sa.cmp(&sb);
    }
    if sa == 0 {
        return Ordering::Equal;
    }
    let (_, ma) = split_sign(a_coef);
    let (_, mb) = split_sign(b_coef);
    let scale = a_scale.max(b_scale);
    let pad = |m: &str, s: u32| {
        let mut out = m.trim_start_matches('0').to_string();
        out.push_str(&"0".repeat((scale - s) as usize));
        out
    };
    let (pa, pb) = (pad(ma, a_scale), pad(mb, b_scale));
    let mag = pa.len().cmp(&pb.len()).then_with(|| pa.cmp(&pb));
    if sa < 0 { mag.reverse() } else { mag }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_by_value_not_scale() {
        assert_eq!(cmp("120", 2, "12", 1), Ordering::Equal);
        assert_eq!(cmp("1", 0, "99", 2), Ordering::Greater);
        assert_eq!(cmp("-5", 0, "-50", 1), Ordering::Equal);
        assert_eq!(cmp("-5", 0, "-6", 0), Ordering::Greater);
        assert_eq!(cmp("-1", 0, "1", 0), Ordering::Less);
        assert_eq!(cmp("0", 0, "-0", 3), Ordering::Equal);
    }

    #[test]
    fn renders() {
        assert_eq!(render("120", 2), "1.20");
        assert_eq!(render("-5", 3), "-0.005");
        assert_eq!(render("7", 0), "7");
        assert_eq!(render("0", 2), "0.00");
    }

    #[test]
    fn canonicalises() {
        assert_eq!(canonical("120", 2), "1.2");
        assert_eq!(canonical("100", 2), "1");
        assert_eq!(canonical("-000", 1), "0");
        assert_eq!(canonical("5", 3), "0.005");
    }

    #[test]
    fn signs() {
        assert_eq!(sign("000"), 0);
        assert_eq!(sign("-0"), 0);
        assert_eq!(sign("-12"), -1);
        assert_eq!(sign("12"), 1);
    }
}
