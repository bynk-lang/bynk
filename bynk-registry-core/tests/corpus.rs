//! The admission corpus (#2321, decision F): the cases the R0 conformance
//! suite sends a registry (`bynk registry conformance`) — a good upload, a
//! mismatched name, a mismatched version, junk, a traversing path, a path
//! dependency — plus a registry dependency and an adapter's npm requirement.
//!
//! Each runs through `admit_json` here. With `BYNK_PARITY_DIR` set, the test
//! also writes every case's upload, expectation and native result there, for
//! `scripts/registry-core-parity.mjs` to replay through the `.wasm` on Node and
//! compare byte for byte.

use bynk_registry_core::{Tree, abi::admit_json, pack};

const MANIFEST: &str = "[package]\nname = \"acme/probe\"\nversion = \"0.0.1\"\n";
const GOOD: &str = r#"{"name":"acme/probe","version":"0.0.1"}"#;

fn upload(manifest: &str, files: &[(&str, &str)]) -> Vec<u8> {
    let mut tree = Tree::new();
    tree.insert("bynk.toml", manifest.as_bytes().to_vec());
    for (p, b) in files {
        tree.insert(*p, b.as_bytes().to_vec());
    }
    pack(&tree).0
}

const PROBE: &str = "commons probe\n\nfn value() -> Int {\n  1\n}\n";
const ADAPTER: &str = "adapter tokens {\n  binding \"./tokens.binding.ts\" requires { \"jose\": \"^5\" }\n\n  exports capability { Jwt }\n\n  capability Jwt {\n    fn sign(sub: String, secret: String) -> Effect[String]\n  }\n\n  provides Jwt = JoseJwt\n}\n";

/// `(id, upload, expected, outcome)` where outcome is a substring the result
/// must contain.
fn corpus() -> Vec<(&'static str, Vec<u8>, &'static str, &'static str)> {
    let good = upload(MANIFEST, &[("src/probe.bynk", PROBE)]);
    let mut traversal = b"bynk-tree-v1\n../escape.txt\x005\x00hello".to_vec();
    traversal.extend_from_slice(b"bynk.toml\x000\x00");
    vec![
        ("good", good.clone(), GOOD, "\"ok\""),
        ("good-unchecked", good.clone(), "", "\"ok\""),
        (
            "other-package",
            good.clone(),
            r#"{"name":"acme/probex","version":"0.0.1"}"#,
            "request named",
        ),
        (
            "other-version",
            good.clone(),
            r#"{"name":"acme/probe","version":"0.0.4"}"#,
            "request named",
        ),
        ("junk", b"not an archive".to_vec(), "", "malformed"),
        ("traversal", traversal, "", "climbs out"),
        (
            "path-dependency",
            upload(
                &format!(
                    "{MANIFEST}\n[dependencies]\nhelper = {{ package = \"acme/helper\", path = \"../helper\" }}\n"
                ),
                &[("src/probe.bynk", PROBE)],
            ),
            "",
            "path or git dependency",
        ),
        (
            "registry-dependency",
            upload(
                &format!(
                    "{MANIFEST}\n[dependencies]\nhelper = {{ package = \"acme/helper\", version = \"^1\" }}\n"
                ),
                &[("src/probe.bynk", PROBE)],
            ),
            "",
            "\"package\":\"acme/helper\"",
        ),
        (
            "npm-requirement",
            upload(MANIFEST, &[("src/tokens.bynk", ADAPTER)]),
            "",
            "\"package\":\"jose\"",
        ),
        (
            "hidden-and-out-left-out",
            upload(
                MANIFEST,
                &[
                    ("src/probe.bynk", PROBE),
                    (".env", "SECRET=1"),
                    ("out/x.ts", "built"),
                ],
            ),
            "",
            "\"ok\"",
        ),
        ("bad-expectation", good, "{", "bad_request"),
    ]
}

#[test]
fn every_case_has_its_expected_outcome() {
    let dir = std::env::var_os("BYNK_PARITY_DIR").map(std::path::PathBuf::from);
    if let Some(d) = &dir {
        std::fs::create_dir_all(d).unwrap();
    }
    for (id, up, expected, outcome) in corpus() {
        let result = admit_json(&up, expected.as_bytes());
        assert!(result.contains(outcome), "{id}: {result}");
        if let Some(d) = &dir {
            std::fs::write(d.join(format!("{id}.upload")), &up).unwrap();
            std::fs::write(d.join(format!("{id}.expected")), expected).unwrap();
            std::fs::write(d.join(format!("{id}.result")), &result).unwrap();
        }
    }
}

#[test]
fn the_hidden_and_built_files_do_not_reach_the_canonical_archive() {
    let up = upload(
        MANIFEST,
        &[
            ("src/probe.bynk", PROBE),
            (".env", "SECRET=1"),
            ("out/x.ts", "built"),
        ],
    );
    let admitted = bynk_registry_core::admit(&up, None).unwrap();
    let tree = bynk_registry_core::unpack(&admitted.archive).unwrap();
    let paths: Vec<_> = tree.iter().map(|(p, _)| p).collect();
    assert_eq!(paths, ["bynk.toml", "src/probe.bynk"]);
}

/// #2336 (decision B): the `call` operations the hosted registry signs and
/// renders lines with, `(id, request, outcome)`. Written beside the admission
/// corpus as `<id>.call` / `<id>.callresult` for the parity replay.
fn calls() -> Vec<(&'static str, String, &'static str)> {
    let line = r#"{"name":"acme/probe","version":"0.0.1","checksum":"sha256-AAAA","deps":[{"package":"acme/helper","range":"^1"}],"npm":[{"package":"jose","range":"^5"}],"toolchain":null,"yanked":false,"sig":"c2ln"}"#;
    let config = r#"{"protocol":2,"id":"bynk","dl":"dl","keys":[{"key":"ed25519:ROOT"}],"delegations":[{"key":"ed25519:ONLINE","sig":"c2ln"}],"revoked":[{"key":"ed25519:OLD","sig":"eA=="}]}"#;
    let request = |v: serde_json::Value| v.to_string();
    let line_v: serde_json::Value = serde_json::from_str(line).unwrap();
    vec![
        (
            "call-line-payload",
            request(serde_json::json!({"op": "line_payload", "line": line_v, "registry": "bynk"})),
            "\"payload\"",
        ),
        (
            "call-line-render",
            request(serde_json::json!({"op": "line_render", "line": line_v})),
            "\"text\"",
        ),
        (
            // #2330: a withdrawn line renders its reason; the payload ignores it.
            "call-line-render-withdrawn",
            request(serde_json::json!({"op": "line_render", "line": {
                "name": "acme/probe", "version": "0.0.1", "checksum": "sha256-AAAA",
                "deps": [], "npm": [], "toolchain": null, "yanked": true,
                "withdrawn": "a takedown", "sig": "c2ln",
            }})),
            r#"withdrawn\":\"a takedown"#,
        ),
        (
            "call-signer-delegated",
            request(
                serde_json::json!({"op": "signer_info", "config": config, "key": "ed25519:ONLINE"}),
            ),
            "\"delegated\":true",
        ),
        (
            "call-signer-revoked",
            request(
                serde_json::json!({"op": "signer_info", "config": config, "key": "ed25519:OLD"}),
            ),
            "\"revoked\":true",
        ),
        ("call-bad-op", r#"{"op":"nope"}"#.to_string(), "bad_request"),
        ("call-not-json", "{".to_string(), "bad_request"),
    ]
}

#[test]
fn every_call_has_its_expected_outcome() {
    let dir = std::env::var_os("BYNK_PARITY_DIR").map(std::path::PathBuf::from);
    if let Some(d) = &dir {
        std::fs::create_dir_all(d).unwrap();
    }
    for (id, req, outcome) in calls() {
        let result = bynk_registry_core::abi::call_json(req.as_bytes());
        assert!(result.contains(outcome), "{id}: {result}");
        if let Some(d) = &dir {
            std::fs::write(d.join(format!("{id}.call")), &req).unwrap();
            std::fs::write(d.join(format!("{id}.callresult")), &result).unwrap();
        }
    }
}
