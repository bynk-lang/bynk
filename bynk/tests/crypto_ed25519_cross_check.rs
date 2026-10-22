//! `bynk.crypto`'s Ed25519 against the toolchain's own verifier (standard-library
//! K4, #2327): a signature made with `Crypto.sign` verifies under
//! `registry::protocol::verify`, and a signature made by the registry's
//! `SigningKey` verifies under `Crypto.verify`.
//!
//! The registry's key is the one `bynk registry init` writes (a `ring` PKCS#8
//! document) and its public key is the 32 raw bytes, so this is also the check
//! that the format the registry stores is the format `Crypto.importSigningKey` and
//! `Crypto.importVerifyKey` take. The binding is the compiled Node one, run by
//! `node`. Skips loudly without `node` or the `bynkc` binary;
//! `BYNK_REQUIRE_TSC=1` turns the skip into a failure, as the other
//! Node-driven suites do.

use std::path::PathBuf;
use std::process::Command;

use bynk::registry::protocol::{SigningKey, key_bytes, verify};
use bynk_project::lock::{base64_decode, base64_encode};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn bynkc() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
        .parent()
        .unwrap()
        .join("bynkc")
}

fn required() -> bool {
    std::env::var(REQUIRE_ENV).is_ok_and(|v| !v.is_empty() && v != "0")
}

fn skip(why: &str) -> bool {
    if required() {
        panic!("CRYPTO CROSS-CHECK: {why}, and {REQUIRE_ENV} is set");
    }
    eprintln!("CRYPTO CROSS-CHECK skipped: {why}");
    true
}

fn have_node() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const PROBE: &str = "context x.probe\n\nconsumes bynk.crypto { Crypto }\n\nservice probe {\n\ton call() -> Effect[Int] given Crypto {\n\t\tEffect.pure(0)\n\t}\n}\n";

/// Run in the compiled output: sign `payload` under `pkcs8` with the binding,
/// then verify `registry_sig` (made by the registry's key) under `public`, and
/// `payload` under the binding's own signature. Prints one JSON object.
const SCRIPT: &str = r#"
import { CryptoProvider } from "./bynk/crypto-node.binding.js";
const [pkcs8, publicKey, payload, registrySig] = process.argv.slice(2).map((b64) => new Uint8Array(Buffer.from(b64, "base64")));
const c = new CryptoProvider();
const ed = { tag: "Ed25519" };
const signing = await c.importSigningKey(ed, pkcs8);
const verifying = await c.importVerifyKey(ed, publicKey);
if (signing.tag !== "Ok" || verifying.tag !== "Ok") {
  console.log(JSON.stringify({ error: [signing, verifying].filter((r) => r.tag === "Err") }));
} else {
  const mine = await c.sign(signing.value, payload);
  console.log(JSON.stringify({
    signature: Buffer.from(mine).toString("base64"),
    verifiesRegistrySignature: await c.verify(verifying.value, payload, registrySig),
    verifiesOwnSignature: await c.verify(verifying.value, payload, mine),
    rejectsOtherPayload: !(await c.verify(verifying.value, new Uint8Array([0]), registrySig)),
  }));
}
"#;

#[test]
fn crypto_and_the_registry_verifier_agree_on_ed25519() {
    if !have_node() {
        skip("`node` is not on PATH");
        return;
    }
    if !bynkc().exists() {
        skip("the `bynkc` binary is not built beside `bynk`");
        return;
    }
    let dir = scratch("crypto-cross-check");
    let project = dir.join("project");
    std::fs::create_dir_all(project.join("src/x")).unwrap();
    std::fs::write(
        project.join("bynk.toml"),
        "[project]\nname = \"cross-check\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.crypto\"]\n",
    )
    .unwrap();
    std::fs::write(project.join("src/x/probe.bynk"), PROBE).unwrap();
    let out = dir.join("out");
    let compiled = Command::new(bynkc())
        .args(["compile", "--platform", "node", "--emit", "js", "-o"])
        .arg(&out)
        .arg(&project)
        .output()
        .expect("run bynkc");
    assert!(
        compiled.status.success(),
        "the probe project does not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    std::fs::write(out.join("cross.mjs"), SCRIPT).unwrap();

    let (key, pkcs8) = SigningKey::generate().expect("generate a registry key");
    let public = key_bytes(&key.public()).expect("the key's public bytes");
    let payload = b"registry-1\nbynk/example 1.4.2 sha256-abc\n".to_vec();
    let registry_sig = key.sign(&payload);

    let ran = Command::new("node")
        .arg("cross.mjs")
        .args([
            base64_encode(&pkcs8),
            base64_encode(&public),
            base64_encode(&payload),
            registry_sig.clone(),
        ])
        .current_dir(&out)
        .output()
        .expect("run node");
    let stdout = String::from_utf8_lossy(&ran.stdout);
    assert!(
        ran.status.success(),
        "node failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"));
    assert!(
        result.get("error").is_none(),
        "the registry's key was not importable: {result}"
    );

    // Crypto.sign -> protocol::verify.
    let mine = result["signature"].as_str().expect("a signature");
    assert!(
        base64_decode(mine).is_some_and(|s| s.len() == 64),
        "an Ed25519 signature is 64 bytes: {mine}"
    );
    assert!(
        verify(&key.public(), &payload, mine),
        "Crypto.sign's signature does not verify under the registry's verifier"
    );
    assert!(
        !verify(&key.public(), b"another payload", mine),
        "the registry's verifier accepted another payload"
    );
    // Ed25519 is deterministic: both implementations produce the same signature.
    assert_eq!(mine, registry_sig, "the two implementations disagree");

    // protocol's signature -> Crypto.verify.
    assert_eq!(result["verifiesRegistrySignature"], true, "{result}");
    assert_eq!(result["verifiesOwnSignature"], true, "{result}");
    assert_eq!(result["rejectsOtherPayload"], true, "{result}");
    let _ = std::fs::remove_dir_all(&dir);
}
