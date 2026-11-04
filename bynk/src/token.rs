//! `bynk token` (#2869): mint a credential for the project's own `Bearer`
//! actor, for calling a guarded route by hand — `curl` under `bynk dev`, or a
//! smoke test against a deployment.
//!
//! The actor declares everything a minter needs: `auth = Bearer(secret =
//! "<ENV>")`, HS256, and `sub` as the identity. This signs the token exactly as
//! the emitted verifier (`bynk-emit/src/emitter/runtime/auth.ts`) and the test
//! runtime's signer (`test_runtime/jwt_signer.ts`) do — the same header, the
//! same `sub`/`exp` claims, the same unpadded base64url — so the actor's seam
//! accepts it. It is a different command from `bynk org token mint`, which
//! mints a registry publish token.
//!
//! The secret is read from the environment, under the name the actor declares,
//! and nowhere else: it is never an argument (it would show in a process
//! listing), and it is never printed or put in an error. Only the token goes to
//! stdout, so `$(bynk token Reader --sub me)` works.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use bynk_check::actors::Scheme;
use bynk_syntax::ast::{ActorDecl, CommonsItem, SourceUnit, TypeDecl, TypeRef};

/// The default lifetime, in seconds: short, so a hand-minted token is not a
/// long-lived credential by accident.
pub const DEFAULT_LIFETIME_SECS: u64 = 3600;
/// The longest lifetime (and the furthest `--not-before`) `bynk token` signs.
pub const MAX_LIFETIME_SECS: u64 = 365 * 86_400;
/// How far in the past `--expired` puts `exp`. The emitted verifier allows no
/// clock skew, but a deployment's clock may differ from this machine's, so the
/// token is expired by a margin no reasonable skew closes.
pub const EXPIRED_BY_SECS: u64 = 3600;

/// What the user asked for, as the CLI parsed it.
#[derive(Debug, Default)]
pub struct Request<'a> {
    pub actor: Option<&'a str>,
    pub sub: Option<&'a str>,
    pub lifetime: Option<&'a str>,
    pub expired: bool,
    pub not_before: Option<&'a str>,
}

/// `bynk token`: list the project's Bearer actors, or mint one's token.
pub fn run(root: &Path, req: &Request) -> ExitCode {
    match run_inner(root, req) {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("bynk: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_inner(root: &Path, req: &Request) -> Result<String, String> {
    // Reject a bad lifetime before the (slower) project analysis.
    let lifetime = match req.lifetime {
        Some(s) => parse_duration(s, "--lifetime")?,
        None => DEFAULT_LIFETIME_SECS,
    };
    let not_before = req
        .not_before
        .map(|s| parse_duration(s, "--not-before"))
        .transpose()?;

    let analysis = crate::openapi::analyse(root);
    let errors = crate::openapi::errors_of(&analysis);
    if !errors.is_empty() {
        let mut msg = String::from("the project does not check; fix these first:");
        for e in &errors {
            msg.push_str("\n  ");
            msg.push_str(e);
        }
        return Err(msg);
    }
    let actors = collect_actors(root, &analysis);

    let Some(name) = req.actor else {
        return Ok(list(&actors));
    };
    let found = resolve(&actors, name)?;
    let env_var = bearer_env_var(found)?;
    let sub = req
        .sub
        .ok_or_else(|| format!("`--sub <ID>` is required to mint a token for `{name}`"))?;
    validate_sub(found, sub)?;

    let secret = match std::env::var(&env_var) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            return Err(format!(
                "`{env_var}` is not set (or is empty): it is the secret the `{name}` actor \
                 declares, and `bynk token` reads it from the environment — never an argument"
            ));
        }
    };

    let now = unix_now()?;
    let (nbf, exp) = if req.expired {
        (None, now.saturating_sub(EXPIRED_BY_SECS))
    } else {
        let nbf = not_before.map(|d| now + d);
        (nbf, nbf.unwrap_or(now) + lifetime)
    };
    Ok(mint(&secret, sub, nbf, exp))
}

// ---------------------------------------------------------------------
// The project's actors
// ---------------------------------------------------------------------

/// One declared actor, with what resolving its identity type needs.
struct Found {
    unit: String,
    decl: ActorDecl,
    /// The types visible to the actor's unit, by name.
    types: BTreeMap<String, TypeDecl>,
    /// The base actor, for a refinement (`actor Admin = User where …`).
    base: Option<Box<Found>>,
}

/// `unit_sources` paths are project-relative, so they resolve against `root`.
fn collect_actors(root: &Path, analysis: &bynk_ide::ProjectDiagnostics) -> Vec<Found> {
    // Every unit's own items: actors, and types for a commons-declared identity.
    let mut own: BTreeMap<String, (Vec<ActorDecl>, BTreeMap<String, TypeDecl>)> = BTreeMap::new();
    for (unit, paths) in &analysis.unit_sources {
        for path in paths {
            let Ok(src) = std::fs::read_to_string(root.join(path)) else {
                continue;
            };
            let Ok(tokens) = bynk_syntax::lexer::tokenize(&src) else {
                continue;
            };
            let Ok(parsed) = bynk_syntax::parser::parse_unit(&tokens, &src) else {
                continue;
            };
            let items = match parsed {
                SourceUnit::Context(c) => c.items,
                SourceUnit::Commons(c) => c.items,
                _ => continue,
            };
            let entry = own.entry(unit.clone()).or_default();
            for item in items {
                match item {
                    CommonsItem::Actor(a) => entry.0.push(a),
                    CommonsItem::Type(t) => {
                        entry.1.insert(t.name.name.clone(), t);
                    }
                    _ => {}
                }
            }
        }
    }
    let mut out = Vec::new();
    for (unit, (actors, own_types)) in &own {
        // The unit's combined view (its own, `uses`d and consumed types) when
        // the analysis has one; its own declarations otherwise.
        let mut types: BTreeMap<String, TypeDecl> = own_types.clone();
        if let Some(info) = analysis.boundary_info.get(unit) {
            for (n, t) in &info.types {
                types.entry(n.clone()).or_insert_with(|| (**t).clone());
            }
        }
        for a in actors {
            let base = a.refinement.as_ref().and_then(|r| {
                let b = actors.iter().find(|b| b.name.name == r.base.name)?;
                Some(Box::new(Found {
                    unit: unit.clone(),
                    decl: b.clone(),
                    types: types.clone(),
                    base: None,
                }))
            });
            out.push(Found {
                unit: unit.clone(),
                decl: a.clone(),
                types: types.clone(),
                base,
            });
        }
    }
    out.sort_by(|a, b| (&a.unit, &a.decl.name.name).cmp(&(&b.unit, &b.decl.name.name)));
    out
}

fn scheme_of(a: &ActorDecl) -> Option<Scheme> {
    Scheme::from_name(a.auth.as_ref()?.name.as_str())
}

fn qualified(f: &Found) -> String {
    format!("{}.{}", f.unit, f.decl.name.name)
}

/// The Bearer actors, one per line with the secret each reads. Names only: no
/// secret value is ever read here.
fn list(actors: &[Found]) -> String {
    let rows: Vec<(String, String)> = actors
        .iter()
        .filter(|f| f.base.is_none() && scheme_of(&f.decl) == Some(Scheme::Bearer))
        .filter_map(|f| {
            let var = f.decl.scheme_arg("secret")?.value.as_str()?.to_string();
            Some((qualified(f), var))
        })
        .collect();
    if rows.is_empty() {
        return "no Bearer actors in this project".to_string();
    }
    let width = rows.iter().map(|(a, _)| a.len()).max().unwrap_or(0);
    let mut out = String::from("Bearer actors (mint with `bynk token <ACTOR> --sub <ID>`):");
    for (actor, var) in rows {
        out.push_str(&format!("\n  {actor:<width$}  reads ${var}"));
    }
    out
}

/// The actor `name` names — `Actor` or `unit.Actor` — or why it names none.
fn resolve<'a>(actors: &'a [Found], name: &str) -> Result<&'a Found, String> {
    let hits: Vec<&Found> = actors
        .iter()
        .filter(|f| f.decl.name.name == name || qualified(f) == name)
        .collect();
    match hits.as_slice() {
        [one] => Ok(one),
        [] => {
            let known: Vec<String> = actors.iter().map(|f| f.decl.name.name.clone()).collect();
            Err(if known.is_empty() {
                format!("no actor `{name}`: the project declares no actors")
            } else {
                format!(
                    "no actor `{name}`; the project declares {}",
                    known.join(", ")
                )
            })
        }
        many => Err(format!(
            "`{name}` is declared in more than one context; name one of {}",
            many.iter()
                .map(|f| qualified(f))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The env var a mintable actor's secret is read from, or why it is not mintable.
fn bearer_env_var(f: &Found) -> Result<String, String> {
    let name = &f.decl.name.name;
    if let Some(base) = &f.base {
        return Err(format!(
            "`{name}` is a refinement of `{}`: its seam also checks claims the token must carry, \
             and `bynk token` signs only `sub` and the time claims. Mint with the base actor \
             `{}` to test authentication, not the refinement's authorisation",
            base.decl.name.name, base.decl.name.name
        ));
    }
    match scheme_of(&f.decl) {
        Some(Scheme::Bearer) => f
            .decl
            .scheme_arg("secret")
            .and_then(|a| a.value.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("`{name}` names no signing secret")),
        Some(Scheme::Signature) => Err(format!(
            "`{name}` is a `Signature` actor, which `bynk token` does not support: it \
             authenticates a request by an HMAC over that request's body, so there is no \
             standalone credential to mint — the signature depends on the body being sent"
        )),
        Some(Scheme::Oidc) => Err(format!(
            "`{name}` is an `Oidc` actor, which `bynk token` does not support: its tokens are \
             minted by the identity provider and signed with the provider's private key, which \
             the project does not hold"
        )),
        Some(other) => Err(format!(
            "`{name}` uses `auth = {}`, which `bynk token` does not support: it mints \
             credentials for `Bearer` actors only",
            other.as_str()
        )),
        None => Err(format!(
            "`{name}` declares no `auth` scheme: `bynk token` mints credentials for `Bearer` \
             actors only"
        )),
    }
}

/// `--sub` must be a value the actor's identity type admits — the emitted seam
/// answers `401` to a `sub` that fails the refinement — and non-empty, which the
/// verifier requires of any `sub`.
fn validate_sub(f: &Found, sub: &str) -> Result<(), String> {
    let name = &f.decl.name.name;
    if sub.is_empty() {
        return Err(format!(
            "`--sub` is empty: the verifier refuses a token with no `sub`, so no `{name}` seam \
             would accept it"
        ));
    }
    let Some(TypeRef::Named(id)) = &f.decl.identity else {
        return Ok(());
    };
    let Some(decl) = f.types.get(&id.name) else {
        return Ok(());
    };
    if let Some(pred) = bynk_check::checker::string_refinement_violation(decl, sub) {
        return Err(format!(
            "`--sub {sub:?}` is not a valid `{}` (the `{name}` actor's identity): it fails \
             `{}`, so the seam would answer 401",
            id.name,
            pred.name()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Durations
// ---------------------------------------------------------------------

/// `30s`, `15m`, `1h`, `30d`: a positive whole number and a unit, at most a year.
fn parse_duration(s: &str, flag: &str) -> Result<u64, String> {
    let bad = || {
        format!(
            "{flag} `{s}` is not a duration: write a whole number and a unit, like `90s`, `15m`, \
             `1h` or `30d`"
        )
    };
    let split = s.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
    let (digits, unit) = s.split_at(split);
    let n: u64 = digits.parse().map_err(|_| bad())?;
    let mult = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return Err(bad()),
    };
    let secs = n.checked_mul(mult).ok_or_else(bad)?;
    if secs == 0 {
        return Err(format!("{flag} `{s}` must be longer than zero"));
    }
    if secs > MAX_LIFETIME_SECS {
        return Err(format!(
            "{flag} `{s}` is longer than the 365 days `bynk token` signs: a hand-minted token \
             should be short-lived"
        ));
    }
    Ok(secs)
}

fn unix_now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "the system clock is before 1970".to_string())
}

// ---------------------------------------------------------------------
// Signing
// ---------------------------------------------------------------------

/// An HS256 JWT over `{sub, exp[, nbf]}`. The header and claim order match
/// `jwt_signer.ts`'s `JSON.stringify` output, so a token is byte-comparable
/// with the test runtime's.
pub fn mint(secret: &str, sub: &str, nbf: Option<u64>, exp: u64) -> String {
    let header = r#"{"alg":"HS256","typ":"JWT"}"#;
    let sub_json = serde_json::to_string(sub).expect("a string serialises");
    let payload = match nbf {
        Some(nbf) => format!(r#"{{"sub":{sub_json},"exp":{exp},"nbf":{nbf}}}"#),
        None => format!(r#"{{"sub":{sub_json},"exp":{exp}}}"#),
    };
    let signing_input = format!(
        "{}.{}",
        b64url(header.as_bytes()),
        b64url(payload.as_bytes())
    );
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let sig = ring::hmac::sign(&key, signing_input.as_bytes());
    format!("{signing_input}.{}", b64url(sig.as_ref()))
}

/// Unpadded base64url (RFC 4648 §5), as the verifier's decoder reads it.
fn b64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_the_rfc_vectors_unpadded() {
        assert_eq!(b64url(b""), "");
        assert_eq!(b64url(b"f"), "Zg");
        assert_eq!(b64url(b"fo"), "Zm8");
        assert_eq!(b64url(b"foo"), "Zm9v");
        assert_eq!(b64url(b"foob"), "Zm9vYg");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
    }

    /// RFC 7515 §A.1's HS256 example: the signing input and key are the RFC's, so
    /// this pins the HMAC and the base64url independently of the verifier.
    #[test]
    fn hmac_matches_rfc_7515_a1() {
        let key_b64 = "AyM1SysPpbyDfgZld3umj1qzKObwVMkoqQ-EstJQLr_T-1qS0gZH75aKtMN3Yj0iPS4hcgUuTwjAzZr1Z9CAow";
        // The RFC's key is raw bytes; decode our own encoding of it to compare.
        let mut bytes = Vec::new();
        let mut acc = 0u32;
        let mut bits = 0;
        for c in key_b64.chars() {
            let v = match c {
                'A'..='Z' => c as u32 - 'A' as u32,
                'a'..='z' => c as u32 - 'a' as u32 + 26,
                '0'..='9' => c as u32 - '0' as u32 + 52,
                '-' => 62,
                _ => 63,
            };
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                bytes.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        let input = "eyJ0eXAiOiJKV1QiLA0KICJhbGciOiJIUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ";
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &bytes);
        let sig = ring::hmac::sign(&key, input.as_bytes());
        assert_eq!(
            b64url(sig.as_ref()),
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        );
    }

    #[test]
    fn durations_parse_and_refuse() {
        assert_eq!(parse_duration("90s", "--lifetime"), Ok(90));
        assert_eq!(parse_duration("15m", "--lifetime"), Ok(900));
        assert_eq!(parse_duration("1h", "--lifetime"), Ok(3600));
        assert_eq!(parse_duration("365d", "--lifetime"), Ok(MAX_LIFETIME_SECS));
        for bad in [
            "",
            "h",
            "30",
            "0s",
            "1.5h",
            "-1h",
            "3w",
            "366d",
            "99999999999999999999d",
        ] {
            assert!(parse_duration(bad, "--lifetime").is_err(), "{bad:?}");
        }
    }

    #[test]
    fn payload_order_matches_the_test_signer() {
        let t = mint("k", "me", None, 5);
        let payload = t.split('.').nth(1).unwrap();
        assert_eq!(payload, b64url(br#"{"sub":"me","exp":5}"#));
        let t = mint("k", "me", Some(3), 5);
        assert_eq!(
            t.split('.').nth(1).unwrap(),
            b64url(br#"{"sub":"me","exp":5,"nbf":3}"#)
        );
    }
}
