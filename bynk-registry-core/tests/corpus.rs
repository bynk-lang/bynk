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
