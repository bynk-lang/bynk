//! The registry protocol's data (packaging slice 4a, #2274, decision D; track
//! `design/tracks/packaging.md` §6.1): a registry's `config.json`, its index
//! lines, the canonical payload a line's signature covers, and ed25519 keys.
//!
//! Signing and verification use `ring` (already in the build through
//! rustls). A key is written `ed25519:<base64 of the 32-byte public key>`, and
//! its fingerprint `ed25519-fp:<base64 of sha256(public key)>`.

use std::collections::BTreeMap;

use bynk_project::lock::{base64_decode, base64_encode};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The protocol version this toolchain speaks.
pub const PROTOCOL: u64 = 1;

/// A registry's `config.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The registry's stable identity, which its signed lines bind.
    pub id: String,
    /// The archive base: absolute, or relative to the index.
    pub dl: String,
    /// Whether requests carry the registry's token.
    pub token_auth: bool,
    /// The key-rotation chain, oldest first.
    pub keys: Vec<KeyEntry>,
}

/// One link of the key-rotation chain: a key, and (after the first) the
/// previous key's signature over `{ "key": …, "registry": id }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEntry {
    pub key: String,
    pub sig: Option<String>,
}

impl Config {
    /// Parse `config.json`.
    pub fn parse(bytes: &[u8]) -> Result<Config, String> {
        let v: Value =
            serde_json::from_slice(bytes).map_err(|e| format!("`config.json` isn't JSON: {e}"))?;
        let protocol = v.get("protocol").and_then(Value::as_u64);
        if protocol != Some(PROTOCOL) {
            return Err(format!(
                "`config.json` speaks protocol {}, and this toolchain speaks {PROTOCOL}",
                protocol.map_or("(none)".to_string(), |p| p.to_string())
            ));
        }
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let id = text("id").ok_or("`config.json` has no `id`")?;
        let dl = text("dl").unwrap_or_else(|| "dl".to_string());
        let token_auth = match text("auth").as_deref() {
            None | Some("none") => false,
            Some("token") => true,
            Some(other) => return Err(format!("`config.json`'s `auth` is `{other}`")),
        };
        let keys = v
            .get("keys")
            .and_then(Value::as_array)
            .ok_or("`config.json` has no `keys`")?
            .iter()
            .map(|k| {
                Some(KeyEntry {
                    key: k.get("key")?.as_str()?.to_owned(),
                    sig: k.get("sig").and_then(Value::as_str).map(str::to_owned),
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or("a `config.json` key entry has no `key`")?;
        if keys.is_empty() {
            return Err("`config.json` lists no key".to_string());
        }
        Ok(Config {
            id,
            dl,
            token_auth,
            keys,
        })
    }

    /// `config.json`'s text.
    pub fn render(&self) -> String {
        let keys: Vec<Value> = self
            .keys
            .iter()
            .map(|k| match &k.sig {
                Some(sig) => json!({ "key": k.key, "sig": sig }),
                None => json!({ "key": k.key }),
            })
            .collect();
        let v = json!({
            "protocol": PROTOCOL,
            "id": self.id,
            "dl": self.dl,
            "auth": if self.token_auth { "token" } else { "none" },
            "keys": keys,
        });
        serde_json::to_string_pretty(&v).expect("JSON") + "\n"
    }

    /// The newest key the chain reaches from `trusted` (a key fingerprint),
    /// verifying each link; `None` when `trusted` isn't on the chain or a
    /// link doesn't verify (`bynk.registry.key_mismatch`).
    pub fn follow_chain(&self, trusted: &str) -> Option<&str> {
        let start = self
            .keys
            .iter()
            .position(|k| fingerprint(&k.key).as_deref() == Some(trusted))?;
        for i in start + 1..self.keys.len() {
            let prev = &self.keys[i - 1].key;
            let link = &self.keys[i];
            let sig = link.sig.as_deref()?;
            if !verify(prev, &rotation_payload(&self.id, &link.key), sig) {
                return None;
            }
        }
        Some(self.keys.last()?.key.as_str())
    }
}

/// The payload a rotation link's signature covers.
pub fn rotation_payload(registry: &str, key: &str) -> Vec<u8> {
    canonical(&json!({ "key": key, "registry": registry }))
}

/// A dependency or npm requirement in an index line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Requirement {
    pub package: String,
    pub range: String,
}

/// One version line of a package's index file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexLine {
    pub name: String,
    pub version: String,
    /// The canonical-tree checksum (`sha256-<base64>`), as the store names it.
    pub checksum: String,
    pub deps: Vec<Requirement>,
    pub npm: Vec<Requirement>,
    /// The minimum `bynk` this version needs, when it says.
    pub toolchain: Option<String>,
    pub yanked: bool,
    pub sig: String,
}

impl IndexLine {
    /// The canonical payload the signature covers: every field but `sig` and
    /// `yanked`, plus the origin registry's `id` — so a line can't be
    /// replayed under another name or from another registry.
    pub fn payload(&self, registry: &str) -> Vec<u8> {
        let reqs = |rs: &[Requirement]| -> Value {
            Value::Array(
                rs.iter()
                    .map(|r| json!({ "package": r.package, "range": r.range }))
                    .collect(),
            )
        };
        let mut obj = BTreeMap::new();
        obj.insert("checksum", json!(self.checksum));
        obj.insert("deps", reqs(&self.deps));
        obj.insert("name", json!(self.name));
        obj.insert("npm", reqs(&self.npm));
        obj.insert("registry", json!(registry));
        obj.insert("toolchain", json!(self.toolchain));
        obj.insert("version", json!(self.version));
        canonical(&serde_json::to_value(obj).expect("JSON"))
    }

    /// The line as one line of JSON.
    pub fn render(&self) -> String {
        let reqs = |rs: &[Requirement]| -> Vec<Value> {
            rs.iter()
                .map(|r| json!({ "package": r.package, "range": r.range }))
                .collect()
        };
        let v = json!({
            "name": self.name,
            "version": self.version,
            "checksum": self.checksum,
            "deps": reqs(&self.deps),
            "npm": reqs(&self.npm),
            "toolchain": self.toolchain,
            "yanked": self.yanked,
            "sig": self.sig,
        });
        serde_json::to_string(&v).expect("JSON")
    }

    /// Parse one index line.
    pub fn parse(line: &str) -> Result<IndexLine, String> {
        let v: Value =
            serde_json::from_str(line).map_err(|e| format!("an index line isn't JSON: {e}"))?;
        let text = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(format!("an index line has no `{k}`"))
        };
        let reqs = |k: &str| -> Result<Vec<Requirement>, String> {
            match v.get(k) {
                None | Some(Value::Null) => Ok(Vec::new()),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|r| {
                        Some(Requirement {
                            package: r.get("package")?.as_str()?.to_owned(),
                            range: r.get("range")?.as_str()?.to_owned(),
                        })
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or(format!("an index line's `{k}` is malformed")),
                Some(_) => Err(format!("an index line's `{k}` isn't a list")),
            }
        };
        Ok(IndexLine {
            name: text("name")?,
            version: text("version")?,
            checksum: text("checksum")?,
            deps: reqs("deps")?,
            npm: reqs("npm")?,
            toolchain: v
                .get("toolchain")
                .and_then(Value::as_str)
                .map(str::to_owned),
            yanked: v.get("yanked").and_then(Value::as_bool).unwrap_or(false),
            sig: text("sig")?,
        })
    }
}

/// `value` as canonical JSON: object keys in byte-wise order (serde_json's
/// map is ordered unless `preserve_order` is on, so keys are re-sorted here
/// regardless), no insignificant whitespace.
pub fn canonical(value: &Value) -> Vec<u8> {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(map) => {
                let sorted: BTreeMap<&String, Value> =
                    map.iter().map(|(k, v)| (k, sort(v))).collect();
                let mut out = serde_json::Map::new();
                for (k, v) in sorted {
                    out.insert(k.clone(), v);
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_vec(&sort(value)).expect("JSON")
}

/// The raw public key of `ed25519:<base64>`.
pub fn key_bytes(key: &str) -> Option<Vec<u8>> {
    let bytes = base64_decode(key.strip_prefix("ed25519:")?)?;
    (bytes.len() == 32).then_some(bytes)
}

/// `ed25519-fp:<base64 of sha256(public key)>`.
pub fn fingerprint(key: &str) -> Option<String> {
    let bytes = key_bytes(key)?;
    let digest = Sha256::digest(&bytes);
    Some(format!("ed25519-fp:{}", base64_encode(&digest)))
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
}
