//! `bynk token` (#2869): a credential for the project's own `Bearer` actor.
//!
//! The driver signs in Rust, so the load-bearing tests mint with the real
//! binary and verify with the *emitted runtime's* verifier
//! (`bynk-emit/src/emitter/runtime/auth.ts`, run by `node`), and compare the
//! signer byte-for-byte with the test runtime's (`test_runtime/jwt_signer.ts`).
//! Those two skip loudly without `node`; `BYNK_REQUIRE_TSC=1` turns the skip into
//! a failure, as the other Node-driven suites do. The error-path tests need no
//! `node`.

use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";
const SECRET: &str = "a-secret with spaces & ünïcode";

fn bynk() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn project() -> PathBuf {
    manifest_dir().join("tests/fixtures/token")
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// `bynk token <args>` in the fixture project, with the given environment (and
/// neither secret inherited from the test's own).
fn token(args: &[&str], env: &[(&str, &str)]) -> Out {
    let mut cmd = Command::new(bynk());
    cmd.arg("token").args(args).arg("--input").arg(project());
    cmd.env_remove("READER_SECRET").env_remove("ADMIN_SECRET");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run bynk");
    Out {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn have_node() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Whether to skip a node-driven test, failing instead when it is required.
fn skip_without_node() -> bool {
    if have_node() {
        return false;
    }
    let required = std::env::var(REQUIRE_ENV).is_ok_and(|v| !v.is_empty() && v != "0");
    assert!(!required, "BYNK TOKEN: no `node`, and {REQUIRE_ENV} is set");
    eprintln!("BYNK TOKEN cross-check skipped: no `node`");
    true
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("token-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run a script under node, printing its stdout.
fn node(dir: &Path, script: &str, args: &[&str]) -> String {
    let file = dir.join("check.mts");
    std::fs::write(&file, script).unwrap();
    let out = Command::new("node")
        .arg("--experimental-strip-types")
        .arg("--no-warnings")
        .arg(&file)
        .args(args)
        .output()
        .expect("run node");
    assert!(
        out.status.success(),
        "node failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A `file://` URL for a runtime source, which node's ESM loader takes on every
/// platform (a bare Windows path, or its `\\?\` canonical form, is not a module
/// specifier).
fn runtime(file: &str) -> String {
    let path = manifest_dir()
        .join("../bynk-emit/src/emitter/runtime")
        .join(file)
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let path = path.trim_start_matches("//?/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

/// The verifier the emitted Worker runs, over each token it is handed.
fn verify_all(name: &str, secret: &str, tokens: &[&str]) -> Vec<String> {
    let dir = scratch(name);
    let script = format!(
        r#"import {{ verifyBearerJwtHs256 }} from "{}";
const [secret, ...tokens] = process.argv.slice(2);
const out = [];
for (const t of tokens) {{
  const r = await verifyBearerJwtHs256(t, secret);
  out.push(r.tag === "Ok" ? `ok:${{r.value.sub}}` : `err:${{r.error}}`);
}}
console.log(JSON.stringify(out));
"#,
        runtime("auth.ts")
    );
    let mut args = vec![secret];
    args.extend_from_slice(tokens);
    serde_json::from_str(&node(&dir, &script, &args)).unwrap()
}

#[test]
fn a_minted_token_verifies_under_the_emitted_runtime() {
    if skip_without_node() {
        return;
    }
    let env = [("READER_SECRET", SECRET)];
    let good = token(&["Reader", "--sub", "me"], &env);
    assert!(good.ok, "{}", good.stderr);
    // Only the token, on one line, so `$(bynk token …)` works.
    assert_eq!(good.stdout.matches('\n').count(), 1, "{:?}", good.stdout);
    assert_eq!(good.stderr, "");
    let good = good.stdout.trim().to_string();

    let expired = token(&["Reader", "--sub", "me", "--expired"], &env);
    let early = token(&["Reader", "--sub", "me", "--not-before", "10m"], &env);
    let long = token(&["Reader", "--sub", "me", "--lifetime", "30d"], &env);
    for t in [&expired, &early, &long] {
        assert!(t.ok, "{}", t.stderr);
    }
    let (expired, early, long) = (
        expired.stdout.trim(),
        early.stdout.trim(),
        long.stdout.trim(),
    );

    let got = verify_all("verify", SECRET, &[&good, expired, early, long]);
    assert_eq!(
        got,
        [
            "ok:me",
            "err:token expired",
            "err:token not yet valid",
            "ok:me"
        ]
    );
    // A non-ASCII `sub` survives as UTF-8 (`AdminId` has no pattern).
    let admin = token(&["Admin", "--sub", "ünï"], &[("ADMIN_SECRET", SECRET)]);
    assert!(admin.ok, "{}", admin.stderr);
    assert_eq!(
        verify_all("utf8", SECRET, &[admin.stdout.trim()]),
        ["ok:ünï"]
    );
    // Another secret is a bad signature, not a quiet accept.
    assert_eq!(
        verify_all("wrong", "other", &[&good]),
        ["err:bad signature"]
    );
}

#[test]
fn the_signer_is_byte_identical_to_the_test_runtimes() {
    if skip_without_node() {
        return;
    }
    // The shared vector: the test runtime's signer over the claims `bynk token`
    // signs, at a pinned `exp`, must equal `bynk::token::mint`'s output.
    let dir = scratch("vector");
    let signer = std::fs::read_to_string(
        manifest_dir().join("../bynk-emit/src/emitter/test_runtime/jwt_signer.ts"),
    )
    .unwrap();
    let script = format!(
        "{signer}\nconst [secret, sub, exp, nbf] = process.argv.slice(2);\n\
         const claims: Record<string, unknown> = {{ sub, exp: Number(exp) }};\n\
         if (nbf) claims.nbf = Number(nbf);\n\
         console.log(await __bynkSignHs256(claims, secret));\n"
    );
    for (sub, nbf) in [
        ("me", None),
        ("a/\"q\"\\", None),
        // Non-ASCII: Latin-1 range, and above U+00FF (which `btoa` rejects) (#2877).
        ("ünï", None),
        ("日本語😀", Some(1_700_000_000u64)),
        ("me", Some(1_700_000_000u64)),
    ] {
        let exp = 1_800_000_000u64;
        let mut args = vec![SECRET.to_string(), sub.to_string(), exp.to_string()];
        args.extend(nbf.map(|n| n.to_string()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let want = node(&dir, &script, &refs);
        assert_eq!(
            bynk::token::mint(SECRET, sub, nbf, exp),
            want,
            "{sub:?} {nbf:?}"
        );
    }
}

#[test]
fn a_missing_secret_names_the_variable_and_never_an_argument() {
    let out = token(&["Reader", "--sub", "me"], &[]);
    assert!(!out.ok);
    assert!(out.stderr.contains("READER_SECRET"), "{}", out.stderr);
    assert_eq!(out.stdout, "");
    // Empty is missing too.
    let out = token(&["Reader", "--sub", "me"], &[("READER_SECRET", "")]);
    assert!(
        !out.ok && out.stderr.contains("READER_SECRET"),
        "{}",
        out.stderr
    );
    // There is no flag to pass it as.
    let out = token(&["Reader", "--sub", "me", "--secret", "x"], &[]);
    assert!(!out.ok && out.stderr.contains("--secret"), "{}", out.stderr);
}

#[test]
fn the_secret_never_reaches_the_output() {
    let out = token(&["Reader", "--sub", "me"], &[("READER_SECRET", SECRET)]);
    assert!(out.ok, "{}", out.stderr);
    assert!(!out.stdout.contains(SECRET) && !out.stderr.contains(SECRET));
    let out = token(&["Reader", "--sub", ""], &[("READER_SECRET", SECRET)]);
    assert!(!out.ok && !out.stderr.contains(SECRET));
}

#[test]
fn sub_is_checked_against_the_identity_type() {
    let env = [("READER_SECRET", SECRET)];
    for sub in ["", "UPPER", "toolongname", "has space"] {
        let out = token(&["Reader", "--sub", sub], &env);
        assert!(!out.ok, "{sub:?} should be refused");
        assert_eq!(out.stdout, "");
    }
    let out = token(&["Reader", "--sub", "Alice"], &env);
    assert!(
        out.stderr.contains("ReaderId") && out.stderr.contains("Matches"),
        "{}",
        out.stderr
    );
    assert!(token(&["Reader", "--sub", "alice"], &env).ok);
    let out = token(&["Reader"], &env);
    assert!(!out.ok && out.stderr.contains("--sub"), "{}", out.stderr);
}

#[test]
fn only_bearer_actors_are_supported_and_each_refusal_says_why() {
    let sig = token(&["Hook", "--sub", "x"], &[]);
    assert!(!sig.ok);
    assert!(
        sig.stderr.contains("Signature") && sig.stderr.contains("body"),
        "{}",
        sig.stderr
    );
    let oidc = token(&["Sso", "--sub", "x"], &[]);
    assert!(!oidc.ok);
    assert!(
        oidc.stderr.contains("Oidc") && oidc.stderr.contains("provider"),
        "{}",
        oidc.stderr
    );
    let unknown = token(&["Nobody", "--sub", "x"], &[]);
    assert!(!unknown.ok);
    assert!(
        unknown.stderr.contains("Nobody") && unknown.stderr.contains("Reader"),
        "{}",
        unknown.stderr
    );
}

/// The claims object of a minted token, decoded.
fn claims_of(token: &str) -> serde_json::Value {
    let payload = token.split('.').nth(1).expect("a JWT payload");
    let (mut bytes, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for c in payload.chars() {
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
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn a_refinement_actor_signs_the_claims_its_predicate_needs() {
    let env = [("READER_SECRET", SECRET)];
    let out = token(&["Moderator", "--sub", "ann"], &env);
    assert!(out.ok, "{}", out.stderr);
    let claims = claims_of(out.stdout.trim());
    assert_eq!(claims["sub"], "ann");
    assert_eq!(claims["mod"], true);
    // A conjunction signs every claim it needs, strings as strings.
    let out = token(&["Ops", "--sub", "ann"], &env);
    assert!(out.ok, "{}", out.stderr);
    let claims = claims_of(out.stdout.trim());
    assert_eq!(claims["role"], "ops");
    assert_eq!(claims["mod"], true);
    // `--sub` is checked against the base actor's identity type.
    let out = token(&["Moderator", "--sub", "UPPER"], &env);
    assert!(!out.ok && out.stderr.contains("ReaderId"), "{}", out.stderr);
}

#[test]
fn claim_flags_add_claims_with_and_without_a_value() {
    let env = [("READER_SECRET", SECRET)];
    let out = token(
        &[
            "Reader", "--sub", "ann", "--claim", "beta", "--claim", "role=ops", "--claim", "n=",
        ],
        &env,
    );
    assert!(out.ok, "{}", out.stderr);
    let claims = claims_of(out.stdout.trim());
    assert_eq!(claims["beta"], true);
    assert_eq!(claims["role"], "ops");
    assert_eq!(claims["n"], "");
    // On a refinement a `--claim` replaces the witness claim, to test the 403.
    let out = token(&["Moderator", "--sub", "ann", "--claim", "mod="], &env);
    assert!(out.ok, "{}", out.stderr);
    assert_eq!(claims_of(out.stdout.trim())["mod"], "");
}

#[test]
fn reserved_and_malformed_claims_are_refused() {
    let env = [("READER_SECRET", SECRET)];
    for name in ["sub", "iat", "exp", "nbf", "iss", "aud"] {
        let out = token(
            &["Reader", "--sub", "ann", "--claim", &format!("{name}=x")],
            &env,
        );
        assert!(!out.ok, "{name}");
        assert!(
            out.stderr.contains("reserved") && out.stderr.contains(name),
            "{}",
            out.stderr
        );
        assert_eq!(out.stdout, "");
    }
    for bad in ["", "=x"] {
        let out = token(&["Reader", "--sub", "ann", "--claim", bad], &env);
        assert!(
            !out.ok && out.stderr.contains("names no claim"),
            "{}",
            out.stderr
        );
    }
    let out = token(
        &["Reader", "--sub", "ann", "--claim", "a", "--claim", "a=1"],
        &env,
    );
    assert!(
        !out.ok && out.stderr.contains("more than once"),
        "{}",
        out.stderr
    );
}

#[test]
fn an_unsatisfiable_predicate_is_refused() {
    // Refused before the secret is read.
    let out = token(&["Never", "--sub", "ann"], &[]);
    assert!(!out.ok);
    assert!(
        out.stderr.contains("`Never`") && out.stderr.contains("predicate"),
        "{}",
        out.stderr
    );
    assert_eq!(out.stdout, "");
}

#[test]
fn a_predicate_on_a_reserved_claim_is_refused() {
    let out = token(&["Rooted", "--sub", "ann"], &[]);
    assert!(!out.ok);
    assert!(
        out.stderr.contains("`Rooted`") && out.stderr.contains("`sub`"),
        "{}",
        out.stderr
    );
    assert_eq!(out.stdout, "");
}

#[test]
fn the_lifetime_is_validated() {
    let env = [("READER_SECRET", SECRET)];
    for lt in ["0s", "366d", "soon", "1.5h", "30"] {
        let out = token(&["Reader", "--sub", "me", "--lifetime", lt], &env);
        assert!(!out.ok, "--lifetime {lt} should be refused");
        assert_eq!(out.stdout, "");
    }
    // `--expired` is the opposite of a lifetime.
    let out = token(
        &["Reader", "--sub", "me", "--expired", "--lifetime", "1h"],
        &env,
    );
    assert!(!out.ok);
}

#[test]
fn bare_token_lists_the_bearer_actors_and_their_secrets() {
    let out = token(&[], &[]);
    assert!(out.ok, "{}", out.stderr);
    assert!(out.stdout.contains("api.Reader") && out.stdout.contains("READER_SECRET"));
    assert!(out.stdout.contains("api.Admin") && out.stdout.contains("ADMIN_SECRET"));
    assert!(!out.stdout.contains("Hook") && !out.stdout.contains("Sso"));
    assert!(!out.stdout.contains("Moderator"));
}
