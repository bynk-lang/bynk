//! The registry protocol's data (packaging slice 4a, #2274, decision D), as
//! `bynk` uses it: the pure data and payloads live in `bynk-registry-core`
//! (first-party registry R2, #2336, decision B), shared with the hosted
//! registry's wasm module; this module adds ed25519 signing and verification
//! over `ring` (already in the build through rustls), and [`ConfigTrust`], the
//! signer walks with `ring` as their verifier.

use bynk_project::lock::{base64_decode, base64_encode};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

pub use bynk_registry_core::protocol::{
    Config, IndexLine, KeyEntry, PROTOCOL, PROTOCOL_DELEGATION, Requirement, Signers, canonical,
    delegation_payload, fingerprint, key_bytes, revocation_payload, rotation_payload,
};

/// The signer walks of [`Config`], verifying with `ring`.
pub trait ConfigTrust {
    /// See [`Config::signers_with`].
    fn signers(&self, chain: &[String]) -> Signers;
    /// See [`Config::chain_from_with`].
    fn chain_from(&self, trusted: &str) -> Option<Vec<String>>;
    /// See [`Config::follow_chain_with`].
    fn follow_chain(&self, trusted: &str) -> Option<&str>;
}

impl ConfigTrust for Config {
    fn signers(&self, chain: &[String]) -> Signers {
        self.signers_with(chain, &verify)
    }

    fn chain_from(&self, trusted: &str) -> Option<Vec<String>> {
        self.chain_from_with(trusted, &verify)
    }

    fn follow_chain(&self, trusted: &str) -> Option<&str> {
        self.follow_chain_with(trusted, &verify)
    }
}

/// Whether `sig` (base64) is `key`'s signature over `payload`.
pub fn verify(key: &str, payload: &[u8], sig: &str) -> bool {
    let (Some(pk), Some(sig)) = (key_bytes(key), base64_decode(sig)) else {
        return false;
    };
    UnparsedPublicKey::new(&ED25519, pk)
        .verify(payload, &sig)
        .is_ok()
}

/// A registry's signing key, from its PKCS#8 document.
pub struct SigningKey(Ed25519KeyPair);

impl SigningKey {
    /// A new key, and its PKCS#8 document (what `bynk registry init` writes).
    pub fn generate() -> Result<(SigningKey, Vec<u8>), String> {
        let rng = ring::rand::SystemRandom::new();
        let doc = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "could not generate a key")?;
        let pair =
            Ed25519KeyPair::from_pkcs8(doc.as_ref()).map_err(|_| "could not read the key")?;
        Ok((SigningKey(pair), doc.as_ref().to_vec()))
    }

    /// The key in a PKCS#8 document.
    pub fn from_pkcs8(doc: &[u8]) -> Result<SigningKey, String> {
        Ed25519KeyPair::from_pkcs8(doc)
            .map(SigningKey)
            .map_err(|_| "the key file isn't an ed25519 PKCS#8 key".to_string())
    }

    /// `ed25519:<base64>`.
    pub fn public(&self) -> String {
        format!("ed25519:{}", base64_encode(self.0.public_key().as_ref()))
    }

    /// The base64 signature over `payload`.
    pub fn sign(&self, payload: &[u8]) -> String {
        base64_encode(self.0.sign(payload).as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(name: &str) -> IndexLine {
        IndexLine {
            name: name.into(),
            version: "1.4.2".into(),
            checksum: "sha256-abc".into(),
            deps: vec![Requirement {
                package: "acme/fx".into(),
                range: "^2".into(),
            }],
            npm: vec![],
            toolchain: None,
            yanked: false,
            sig: String::new(),
        }
    }

    #[test]
    fn a_signature_binds_the_name_and_the_registry() {
        let (key, _) = SigningKey::generate().unwrap();
        let mut l = line("acme/money");
        l.sig = key.sign(&l.payload("acme"));
        assert!(verify(&key.public(), &l.payload("acme"), &l.sig));
        // Replayed from another registry, or under another name.
        assert!(!verify(&key.public(), &l.payload("globex"), &l.sig));
        let mut other = l.clone();
        other.name = "acme/payments".into();
        assert!(!verify(&key.public(), &other.payload("acme"), &l.sig));
        // `yanked` isn't covered: flipping it keeps the signature.
        let mut yanked = l.clone();
        yanked.yanked = true;
        assert!(verify(&key.public(), &yanked.payload("acme"), &l.sig));
        // The line round-trips.
        assert_eq!(IndexLine::parse(&l.render()).unwrap(), l);
    }

    #[test]
    fn the_canonical_payload_is_sorted_and_compact() {
        let p = String::from_utf8(line("acme/money").payload("acme")).unwrap();
        assert_eq!(
            p,
            r#"{"checksum":"sha256-abc","deps":[{"package":"acme/fx","range":"^2"}],"name":"acme/money","npm":[],"registry":"acme","toolchain":null,"version":"1.4.2"}"#
        );
    }

    #[test]
    fn the_key_chain_is_followed_from_the_trusted_key() {
        let (k1, _) = SigningKey::generate().unwrap();
        let (k2, _) = SigningKey::generate().unwrap();
        let (k3, _) = SigningKey::generate().unwrap();
        let config = Config {
            id: "acme".into(),
            dl: "dl".into(),
            api: None,
            token_auth: false,
            keys: vec![
                KeyEntry {
                    key: k1.public(),
                    sig: None,
                },
                KeyEntry {
                    key: k2.public(),
                    sig: Some(k1.sign(&rotation_payload("acme", &k2.public()))),
                },
                KeyEntry {
                    key: k3.public(),
                    sig: Some(k2.sign(&rotation_payload("acme", &k3.public()))),
                },
            ],
            delegations: vec![],
            revoked: vec![],
        };
        let fp1 = fingerprint(&k1.public()).unwrap();
        assert_eq!(config.follow_chain(&fp1), Some(k3.public().as_str()));
        // A link signed by the wrong key breaks the chain.
        let mut broken = config.clone();
        broken.keys[2].sig = Some(k1.sign(&rotation_payload("acme", &k3.public())));
        assert_eq!(broken.follow_chain(&fp1), None);
        // A key not on the chain isn't followed.
        let (stranger, _) = SigningKey::generate().unwrap();
        assert_eq!(
            config.follow_chain(&fingerprint(&stranger.public()).unwrap()),
            None
        );
        assert_eq!(Config::parse(config.render().as_bytes()).unwrap(), config);
    }

    #[test]
    fn delegates_sign_lines_until_revoked_and_only_chain_keys_grant_or_revoke() {
        let (root, _) = SigningKey::generate().unwrap();
        let (d1, _) = SigningKey::generate().unwrap();
        let (d2, _) = SigningKey::generate().unwrap();
        let entry = |k: &SigningKey, sig: String| KeyEntry {
            key: k.public(),
            sig: Some(sig),
        };
        let mut config = Config {
            id: "bynk".into(),
            dl: "dl".into(),
            api: None,
            token_auth: false,
            keys: vec![KeyEntry {
                key: root.public(),
                sig: None,
            }],
            delegations: vec![
                entry(&d1, root.sign(&delegation_payload("bynk", &d1.public()))),
                // A delegate can't delegate.
                entry(&d2, d1.sign(&delegation_payload("bynk", &d2.public()))),
            ],
            revoked: vec![],
        };
        let chain = vec![root.public()];
        let s = config.signers(&chain);
        assert_eq!(s.keys, vec![root.public(), d1.public()]);
        assert_eq!(s.ignored.len(), 1, "{:?}", s.ignored);

        // A delegate can't revoke; the root can.
        config.revoked = vec![entry(
            &d1,
            d1.sign(&revocation_payload("bynk", &d1.public())),
        )];
        assert!(config.signers(&chain).keys.contains(&d1.public()));
        config.revoked = vec![entry(
            &d1,
            root.sign(&revocation_payload("bynk", &d1.public())),
        )];
        let s = config.signers(&chain);
        assert_eq!(s.keys, vec![root.public()]);
        assert_eq!(s.revoked, vec![d1.public()]);

        // A revocation naming a chain key is ignored: only delegates can be
        // revoked.
        let mut chain_revoked = config.clone();
        chain_revoked.revoked = vec![entry(
            &root,
            root.sign(&revocation_payload("bynk", &root.public())),
        )];
        let s = chain_revoked.signers(&chain);
        assert!(s.revoked.is_empty() && s.keys.contains(&root.public()));
        assert!(
            s.ignored.iter().any(|i| i.contains("only delegates")),
            "{:?}",
            s.ignored
        );

        // A delegation is bound to its registry: replayed under another id,
        // it grants nothing.
        let mut other = config.clone();
        other.id = "acme".into();
        other.revoked.clear();
        assert_eq!(other.signers(&chain).keys, vec![root.public()]);

        // Protocol 2 is written, and required, exactly when delegation data
        // is present.
        let text = config.render();
        assert!(text.contains("\"protocol\": 2"), "{text}");
        assert_eq!(Config::parse(text.as_bytes()).unwrap(), config);
        let downgraded = text.replace("\"protocol\": 2", "\"protocol\": 1");
        assert!(Config::parse(downgraded.as_bytes()).is_err());
        config.delegations.clear();
        config.revoked.clear();
        assert!(config.render().contains("\"protocol\": 1"));
    }
}
