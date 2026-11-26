//! #4504: the shape of a `cors` origin.
//!
//! The router compares a request's `Origin` header with each entry of `origins` exactly, and a
//! browser sends an origin only in its serialised form: `scheme://host[:port]`, the scheme and the
//! host in lower case, no path, query, fragment or trailing slash, and no port that is the scheme's
//! default. An entry in any other shape matches nothing, so every cross-origin request from that
//! site is refused and nothing says why. This reads an entry and, when a browser could never send
//! it, says what is wrong with it and what to write instead.
//!
//! Two entries are not origins and are accepted as they are: `*`, where the policy allows it, and
//! `null`, the value a browser sends for an opaque origin (a sandboxed frame, a `file:` page).
//!
//! An entry that names an `env(…)` constant is checked too: that constant has been rewritten to the
//! literal `bynk.toml` gives it for the environment being built before the checker runs
//! (`env_consts`), so what is read here is the value that environment deploys.

/// Why an entry is not an origin a browser sends, and the entry that would be.
#[derive(Debug, PartialEq, Eq)]
pub struct Problem {
    /// What is wrong, as a phrase that completes "this origin …".
    pub reason: String,
    /// The origin to write instead, when one follows from the entry without guessing.
    pub suggestion: Option<String>,
    /// The suggestion supplied a scheme the entry did not have (`https`), which is a guess about
    /// how the site is served, so the author is told to use the real one.
    pub scheme_assumed: bool,
}

/// `None` when `entry` is an origin a browser can send (or `*` / `null`); otherwise what is wrong.
pub fn check(entry: &str) -> Option<Problem> {
    if entry == "*" || entry == "null" || is_serialised(entry) {
        return None;
    }
    // `Null` is `null` a browser does not send: say that, not "no scheme".
    if entry.eq_ignore_ascii_case("null") {
        return Some(Problem {
            reason: "is `null` in upper case: a browser sends it in lower case".into(),
            suggestion: Some("null".into()),
            scheme_assumed: false,
        });
    }
    Some(diagnose(entry))
}

/// The scheme's default port, which a browser leaves out of an origin.
fn default_port(scheme: &str) -> Option<&'static str> {
    match scheme {
        "http" => Some("80"),
        "https" => Some("443"),
        _ => None,
    }
}

fn is_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c))
}

/// A lower-case host: dot-separated labels of letters, digits, `-` and `_`, or a bracketed IPv6
/// address.
fn is_host(s: &str) -> bool {
    if let Some(inner) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return !inner.is_empty()
            && inner.contains(':')
            && inner
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c) || c == ':' || c == '.');
    }
    !s.is_empty()
        && s.split('.').all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        })
}

/// A port as a browser writes it: 1 to 65535, no leading zero.
fn is_port(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 5
        && !s.starts_with('0')
        && s.chars().all(|c| c.is_ascii_digit())
        && s.parse::<u32>().is_ok_and(|p| p <= 65535)
}

/// The `host` and optional `port` of an authority; the bracketed IPv6 form has colons of its own.
fn split_authority(authority: &str) -> (&str, Option<&str>) {
    if authority.starts_with('[') {
        return match authority.find("]:") {
            Some(i) => (&authority[..=i], Some(&authority[i + 2..])),
            None => (authority, None),
        };
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    }
}

/// Whether `s` is exactly the serialised origin of some URL a browser loads.
fn is_serialised(s: &str) -> bool {
    let Some((scheme, authority)) = s.split_once("://") else {
        return false;
    };
    let (host, port) = split_authority(authority);
    is_scheme(scheme)
        && is_host(host)
        && port.is_none_or(|p| is_port(p) && default_port(scheme) != Some(p))
}

/// What is wrong with an entry `is_serialised` refused. The reason is the first fault in reading
/// order; the suggestion applies every repair that needs no guess.
fn diagnose(entry: &str) -> Problem {
    if entry.is_empty() {
        return Problem {
            reason: "is empty".into(),
            suggestion: None,
            scheme_assumed: false,
        };
    }
    if entry.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Problem {
            reason: "contains whitespace, which an origin never does".into(),
            suggestion: None,
            scheme_assumed: false,
        };
    }
    let (scheme, rest, scheme_written) = match entry.split_once("://") {
        Some((scheme, rest)) => (scheme, rest, true),
        None => ("", entry.strip_prefix("//").unwrap_or(entry), false),
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.contains('@') {
        return Problem {
            reason: "has user information (`user@`), which an origin never carries".into(),
            suggestion: None,
            scheme_assumed: false,
        };
    }
    let (host, port) = split_authority(authority);
    if host.contains('*') {
        return Problem {
            reason: "has a wildcard in the host: origins are compared exactly, so there is no \
                     subdomain wildcard, and a browser never sends a `*`"
                .into(),
            suggestion: None,
            scheme_assumed: false,
        };
    }
    if !host.is_ascii() {
        return Problem {
            reason: "has a host that is not ASCII: a browser sends an internationalised host in \
                     its punycode (`xn--`) form"
                .into(),
            suggestion: None,
            scheme_assumed: false,
        };
    }
    let host_lower = host.to_ascii_lowercase();
    let scheme_lower = scheme.to_ascii_lowercase();
    if !is_host(&host_lower) || port.is_some_and(|p| !is_port(p)) {
        return Problem {
            reason: "is not `scheme://host[:port]`".into(),
            suggestion: None,
            scheme_assumed: false,
        };
    }

    // Every fault past here has a repair, so the suggestion is the entry with all of them made.
    let repaired_scheme = if scheme_written {
        scheme_lower.clone()
    } else {
        "https".to_string()
    };
    // The default port is the repaired scheme's: `app.example.com:443` is `https://app.example.com`.
    let keep_port = port.filter(|p| default_port(&repaired_scheme) != Some(*p));
    let suggestion = format!(
        "{repaired_scheme}://{host_lower}{}",
        keep_port.map(|p| format!(":{p}")).unwrap_or_default()
    );
    let reason = if !scheme_written {
        "has no scheme: a browser sends `scheme://host`".to_string()
    } else if !is_scheme(&scheme_lower) {
        return Problem {
            reason: "is not `scheme://host[:port]`".into(),
            suggestion: None,
            scheme_assumed: false,
        };
    } else if scheme != scheme_lower {
        "has an upper-case scheme: a browser sends the scheme in lower case".to_string()
    } else if host != host_lower {
        "has an upper-case host: a browser sends the host in lower case".to_string()
    } else if tail == "/" {
        "ends in a slash: a browser sends no trailing slash".to_string()
    } else if tail.starts_with('/') {
        "has a path: an origin is only `scheme://host[:port]`".to_string()
    } else if tail.starts_with('?') {
        "has a query: an origin is only `scheme://host[:port]`".to_string()
    } else if tail.starts_with('#') {
        "has a fragment: an origin is only `scheme://host[:port]`".to_string()
    } else {
        format!(
            "writes the scheme's default port: a browser leaves `:{}` out",
            port.unwrap_or_default()
        )
    };
    Problem {
        reason,
        suggestion: Some(suggestion),
        scheme_assumed: !scheme_written,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (entry, the corrected form, or `None` where no honest correction exists).
    const REFUSED: &[(&str, Option<&str>)] = &[
        ("https://app.example.com/", Some("https://app.example.com")),
        (
            "https://app.example.com/app",
            Some("https://app.example.com"),
        ),
        (
            "https://app.example.com?x=1",
            Some("https://app.example.com"),
        ),
        (
            "https://app.example.com#top",
            Some("https://app.example.com"),
        ),
        ("https://App.example.com", Some("https://app.example.com")),
        ("HTTPS://app.example.com", Some("https://app.example.com")),
        ("https://App.example.com/", Some("https://app.example.com")),
        ("app.example.com", Some("https://app.example.com")),
        ("//app.example.com", Some("https://app.example.com")),
        ("localhost:8787", Some("https://localhost:8787")),
        // The default port is the repaired scheme's, so it goes with the scheme that is added.
        ("app.example.com:443", Some("https://app.example.com")),
        ("Null", Some("null")),
        ("NULL", Some("null")),
        (
            "https://app.example.com:443",
            Some("https://app.example.com"),
        ),
        ("http://localhost:80", Some("http://localhost")),
        ("http://localhost:8787/", Some("http://localhost:8787")),
        ("https://*.example.com", None),
        ("https://bücher.example", None),
        ("https://user@app.example.com", None),
        ("https://app.example.com:0", None),
        ("https://app.example.com:99999", None),
        ("https://app.example.com:08787", None),
        ("", None),
        ("https://app example.com", None),
        ("https://", None),
    ];

    #[test]
    fn a_shape_a_browser_never_sends_is_refused_with_its_repair() {
        for (entry, want) in REFUSED {
            let problem = check(entry).unwrap_or_else(|| panic!("`{entry}` should be refused"));
            assert_eq!(
                problem.suggestion.as_deref(),
                *want,
                "`{entry}`: {problem:?}"
            );
        }
    }

    /// Advice must be followed by the checker that gave it: every suggestion is itself accepted.
    #[test]
    fn every_suggestion_is_an_origin_the_check_accepts() {
        for (entry, _) in REFUSED {
            if let Some(fixed) = check(entry).and_then(|p| p.suggestion) {
                assert_eq!(
                    check(&fixed),
                    None,
                    "`{entry}` suggests `{fixed}`, which is refused"
                );
            }
        }
    }

    /// Only a repair that invented the scheme says so.
    #[test]
    fn a_supplied_scheme_is_flagged_and_a_kept_one_is_not() {
        assert!(check("app.example.com").unwrap().scheme_assumed);
        assert!(check("//app.example.com:443").unwrap().scheme_assumed);
        assert!(!check("https://app.example.com/").unwrap().scheme_assumed);
        assert!(!check("https://*.example.com").unwrap().scheme_assumed);
    }

    #[test]
    fn an_origin_a_browser_sends_is_accepted() {
        for ok in [
            "*",
            "null",
            "https://app.example.com",
            "http://localhost:8787",
            "http://localhost",
            "https://a-b.c_d.example.co.uk",
            "https://203.0.113.7",
            "https://203.0.113.7:8443",
            "http://[::1]:8787",
            "http://[2001:db8::1]",
            "capacitor://localhost",
            "chrome-extension://abcdefghijklmnop",
            "ionic://localhost",
            "app+v1://main",
            // Only the default port of `http` and `https` is left out of an origin.
            "wss://sock.example.com:443",
        ] {
            assert_eq!(check(ok), None, "`{ok}` is an origin a browser sends");
        }
    }

    #[test]
    fn the_reason_names_the_first_fault_in_reading_order() {
        let reason = |e: &str| check(e).expect("refused").reason;
        assert!(reason("app.example.com").contains("no scheme"));
        assert!(reason("HTTPS://app.example.com").contains("upper-case scheme"));
        assert!(reason("https://App.example.com").contains("upper-case host"));
        assert!(reason("https://app.example.com/").contains("slash"));
        assert!(reason("https://app.example.com/app").contains("path"));
        assert!(reason("https://app.example.com?x").contains("query"));
        assert!(reason("https://app.example.com#x").contains("fragment"));
        assert!(reason("https://app.example.com:443").contains("default port"));
        assert!(reason("https://*.example.com").contains("subdomain wildcard"));
        assert!(reason("https://bücher.example").contains("punycode"));
        assert!(reason("https://u@app.example.com").contains("user information"));
        assert!(reason("").contains("empty"));
    }
}
