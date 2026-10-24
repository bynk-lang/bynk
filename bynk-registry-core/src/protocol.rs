//! The registry protocol's data (packaging slice 4a, #2274, decision D; track
//! `design/tracks/packaging.md` §6.1): a registry's `config.json`, its index
//! lines, and the canonical payloads their signatures cover.
//!
//! I/O- and crypto-free (first-party registry R2, #2336, decision B), so the
//! client, the reference server and the hosted registry's wasm module build
//! the very same bytes. Signature *checks* take a `verify` function: `bynk`
//! passes one over `ring`, and the hosted registry verifies with `bynk.crypto`.
//! A key is written `ed25519:<base64 of the 32-byte public key>`, and its
//! fingerprint `ed25519-fp:<base64 of sha256(public key)>`.

use std::collections::BTreeMap;

use bynk_project::lock::{base64_decode, base64_encode};
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
    /// keys a client trusts (`chain`, from [`Config::follow_chain_with`]'s walk):
    /// those chain keys, plus every delegate whose delegation a chain key
    /// signed and no chain-key-signed revocation names. `revoked` lists the
    /// delegates a valid revocation names; `ignored`, the delegations and
    /// revocations whose own signature doesn't verify (each is ignored).
    pub fn signers_with(&self, chain: &[String], verify: Verify<'_>) -> Signers {
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
    pub fn chain_from_with(&self, trusted: &str, verify: Verify<'_>) -> Option<Vec<String>> {
        self.follow_chain_with(trusted, verify)?;
        let start = self
            .keys
            .iter()
            .position(|k| fingerprint(&k.key).as_deref() == Some(trusted))?;
        Some(self.keys[start..].iter().map(|k| k.key.clone()).collect())
    }

    /// The newest key the chain reaches from `trusted` (a key fingerprint),
    /// verifying each link; `None` when `trusted` isn't on the chain or a
    /// link doesn't verify (`bynk.registry.key_mismatch`).
    pub fn follow_chain_with(&self, trusted: &str, verify: Verify<'_>) -> Option<&str> {
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
pub use crate::admit::Requirement;

/// Whether `sig` (base64) is `key`'s signature over `payload`: the check the
/// signer walks below are given.
pub type Verify<'v> = &'v dyn Fn(&str, &[u8], &str) -> bool;

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
    /// #2330 (A): why the registry withdrew this version — a takedown, after
    /// which its archive is gone. Unsigned, like `yanked`, and a withdrawn
    /// line is also yanked; an older client ignores the field.
    pub withdrawn: Option<String>,
    pub sig: String,
}

/// #2330 (A): the longest `withdrawn` reason, in characters.
pub const WITHDRAWN_MAX: usize = 500;

impl IndexLine {
    /// The canonical payload the signature covers: every field but `sig`,
    /// `yanked` and `withdrawn`, plus the origin registry's `id` — so a line can't be
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
        let mut v = json!({
            "name": self.name,
            "version": self.version,
            "checksum": self.checksum,
            "deps": reqs(&self.deps),
            "npm": reqs(&self.npm),
            "toolchain": self.toolchain,
            "yanked": self.yanked,
            "sig": self.sig,
        });
        if let Some(reason) = &self.withdrawn {
            v["withdrawn"] = json!(reason);
        }
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
        let name = text("name")?;
        let version = text("version")?;
        // An optional field that is present must be a string (`toolchain` may
        // also be `null`): anything else, such as a codec's tagged
        // `{"kind": "Some", …}`, is refused rather than read as absent — a
        // misread `withdrawn` would leave a takedown looking live.
        let optional = |k: &str, null: bool| -> Result<Option<String>, String> {
            match v.get(k) {
                None => Ok(None),
                Some(Value::Null) if null => Ok(None),
                Some(Value::String(s)) => Ok(Some(s.clone())),
                Some(other) => Err(format!(
                    "{name} {version}'s `{k}` is {}, not a string{}",
                    json_kind(other),
                    if null { " or `null`" } else { "" }
                )),
            }
        };
        Ok(IndexLine {
            checksum: text("checksum")?,
            deps: reqs("deps")?,
            npm: reqs("npm")?,
            toolchain: optional("toolchain", true)?,
            yanked: v.get("yanked").and_then(Value::as_bool).unwrap_or(false),
            withdrawn: optional("withdrawn", false)?,
            sig: text("sig")?,
            name,
            version,
        })
    }
}

/// What kind of JSON value `v` is, for an error.
fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "`null`",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
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
