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

/// The protocol version a registry without delegation speaks.
pub const PROTOCOL: u64 = 1;

/// #2316 (D): the protocol version a `config.json` with `delegations` or
/// `revoked` must declare; this toolchain reads both.
pub const PROTOCOL_DELEGATION: u64 = 2;

/// A registry's `config.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The registry's stable identity, which its signed lines bind.
    pub id: String,
    /// The archive base: absolute, or relative to the index.
    pub dl: String,
    /// #2287: the publish API's base, absolute or relative to the index; a
    /// registry without one takes no publishes over HTTP.
    pub api: Option<String>,
    /// Whether requests carry the registry's token.
    pub token_auth: bool,
    /// The key-rotation chain, oldest first.
    pub keys: Vec<KeyEntry>,
    /// #2316: keys a chain key delegated to sign index lines (and nothing
    /// else), each `sig` a chain key's signature over [`delegation_payload`].
    pub delegations: Vec<KeyEntry>,
    /// #2316: delegates a chain key revoked, each `sig` a chain key's
    /// signature over [`revocation_payload`].
    pub revoked: Vec<KeyEntry>,
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
        if !matches!(protocol, Some(PROTOCOL | PROTOCOL_DELEGATION)) {
            return Err(format!(
                "`config.json` speaks protocol {}, and this toolchain speaks {PROTOCOL} and {PROTOCOL_DELEGATION}",
                protocol.map_or("(none)".to_string(), |p| p.to_string())
            ));
        }
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let id = text("id").ok_or("`config.json` has no `id`")?;
        let dl = text("dl").unwrap_or_else(|| "dl".to_string());
        let api = text("api");
        let token_auth = match text("auth").as_deref() {
            None | Some("none") => false,
            Some("token") => true,
            Some(other) => return Err(format!("`config.json`'s `auth` is `{other}`")),
        };
        let entries = |field: &str, required: bool| -> Result<Vec<KeyEntry>, String> {
            let Some(list) = v.get(field) else {
                return if required {
                    Err(format!("`config.json` has no `{field}`"))
                } else {
                    Ok(Vec::new())
                };
            };
            list.as_array()
                .ok_or(format!("`config.json`'s `{field}` isn't a list"))?
                .iter()
                .map(|k| {
                    Some(KeyEntry {
                        key: k.get("key")?.as_str()?.to_owned(),
                        sig: k.get("sig").and_then(Value::as_str).map(str::to_owned),
                    })
                })
                .collect::<Option<Vec<_>>>()
                .ok_or(format!("a `config.json` `{field}` entry has no `key`"))
        };
        let keys = entries("keys", true)?;
        if keys.is_empty() {
            return Err("`config.json` lists no key".to_string());
        }
        let delegations = entries("delegations", false)?;
        let revoked = entries("revoked", false)?;
        if (!delegations.is_empty() || !revoked.is_empty()) && protocol != Some(PROTOCOL_DELEGATION)
        {
            return Err(format!(
                "`config.json` lists `delegations` or `revoked`, which need `\"protocol\": {PROTOCOL_DELEGATION}`"
            ));
        }
        Ok(Config {
            id,
            dl,
            api,
            token_auth,
            keys,
            delegations,
            revoked,
        })
    }

    /// `config.json`'s text.
    pub fn render(&self) -> String {
        let list = |entries: &[KeyEntry]| -> Vec<Value> {
            entries
                .iter()
                .map(|k| match &k.sig {
                    Some(sig) => json!({ "key": k.key, "sig": sig }),
                    None => json!({ "key": k.key }),
                })
                .collect()
        };
        let keys = list(&self.keys);
        let delegating = !self.delegations.is_empty() || !self.revoked.is_empty();
        let mut v = json!({
            "protocol": if delegating { PROTOCOL_DELEGATION } else { PROTOCOL },
            "id": self.id,
            "dl": self.dl,
            "auth": if self.token_auth { "token" } else { "none" },
            "keys": keys,
        });
        if let Some(api) = &self.api {
            v["api"] = json!(api);
        }
        if !self.delegations.is_empty() {
            v["delegations"] = json!(list(&self.delegations));
        }
        if !self.revoked.is_empty() {
            v["revoked"] = json!(list(&self.revoked));
        }
        serde_json::to_string_pretty(&v).expect("JSON") + "\n"
    }

    /// #2316 (B): the keys that may sign an index line, given the chain
    /// keys a client trusts (`chain`, from [`Config::follow_chain`]'s walk):
    /// those chain keys, plus every delegate whose delegation a chain key
    /// signed and no chain-key-signed revocation names. `revoked` lists the
    /// delegates a valid revocation names; `ignored`, the delegations and
    /// revocations whose own signature doesn't verify (each is ignored).
    pub fn signers(&self, chain: &[String]) -> Signers {
        let by_chain = |payload: &[u8], sig: Option<&str>| {
            sig.is_some_and(|sig| chain.iter().any(|k| verify(k, payload, sig)))
        };
        let mut out = Signers {
            keys: chain.to_vec(),
            ..Signers::default()
        };
        for r in &self.revoked {
            if self.keys.iter().any(|k| k.key == r.key) {
                out.ignored.push(format!(
                    "a revocation of the chain key {} (only delegates can be revoked)",
                    r.key
                ));
            } else if by_chain(&revocation_payload(&self.id, &r.key), r.sig.as_deref()) {
                out.revoked.push(r.key.clone());
            } else {
                out.ignored.push(format!(
                    "a revocation of {} that no trusted key signed",
                    r.key
                ));
            }
        }
        for d in &self.delegations {
            if !by_chain(&delegation_payload(&self.id, &d.key), d.sig.as_deref()) {
                out.ignored.push(format!(
                    "a delegation to {} that no trusted key signed",
                    d.key
                ));
            } else if !out.revoked.contains(&d.key) && !out.keys.contains(&d.key) {
                out.keys.push(d.key.clone());
            }
        }
        out
    }

    /// The chain keys reached from `trusted` (a fingerprint), oldest first:
    /// the set a client accepts, and the set that may delegate and revoke.
    pub fn chain_from(&self, trusted: &str) -> Option<Vec<String>> {
        self.follow_chain(trusted)?;
        let start = self
            .keys
            .iter()
            .position(|k| fingerprint(&k.key).as_deref() == Some(trusted))?;
        Some(self.keys[start..].iter().map(|k| k.key.clone()).collect())
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

/// #2316: the payload a delegation's signature covers.
pub fn delegation_payload(registry: &str, key: &str) -> Vec<u8> {
    canonical(&json!({ "delegate": key, "registry": registry }))
}

/// #2316: the payload a revocation's signature covers.
pub fn revocation_payload(registry: &str, key: &str) -> Vec<u8> {
    canonical(&json!({ "registry": registry, "revoke": key }))
}

/// #2316: who may sign a registry's index lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signers {
    /// The trusted chain keys, then the valid, unrevoked delegates.
    pub keys: Vec<String>,
    /// Delegates a valid revocation names.
    pub revoked: Vec<String>,
    /// Delegations and revocations ignored because no trusted key signed them.
    pub ignored: Vec<String>,
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
