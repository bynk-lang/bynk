//! The module boundary (first-party registry track §3.4): a plain C ABI, no
//! `wasm-bindgen`, so the build is one `.wasm` file and no glue.
//!
//! ```text
//! alloc(len) -> ptr                       reserve `len` bytes of linear memory
//! dealloc(ptr, len)                       release a buffer from `alloc` or a result
//! admit(ptr, len, exp_ptr, exp_len) -> u64
//! call(ptr, len) -> u64                   a JSON request in, a JSON result out
//! ```
//!
//! `admit` takes the upload archive at `ptr..ptr+len` and, optionally, the
//! expected package as JSON at `exp_ptr..exp_ptr+exp_len` (`{"name":"o/p",
//! "version":"1.0.0"}`; zero length means none). It returns `ptr << 32 | len`
//! of a UTF-8 JSON result in linear memory, which the caller reads and then
//! frees with `dealloc`:
//!
//! ```text
//! {"ok":{"name","version","checksum","deps":[{"package","range"}],
//!        "npm":[…],"archive":"<standard base64>"}}
//! {"error":{"code","message"}}
//! ```
//!
//! `call` (first-party registry R2, #2336, decision B) takes a JSON request
//! `{"op": …, …}` and returns a JSON result the same way, so the hosted
//! registry signs exactly the bytes clients verify:
//!
//! ```text
//! {"op":"line_payload","line":{name,version,checksum,deps,npm,toolchain},"registry":"<id>"}
//!     -> {"ok":{"payload":"<base64>"}}          the canonical bytes a line's signature covers
//! {"op":"line_render","line":{…,"yanked":bool,"withdrawn"?:"<reason>","sig":"<base64>"}}
//!     -> {"ok":{"text":"<one index line>"}}
//! {"op":"signer_info","config":"<config.json text>","key":"ed25519:…"}
//!     -> {"ok":{"id","chain_key","delegated","delegation_payload","delegation_sig","revoked"}}
//! ```
//!
//! `signer_info` does no cryptography: it reports the newest chain key, the
//! delegation of `key` (its payload and signature) and whether a revocation
//! names `key`, for the caller to verify with its own crypto.
//!
//! [`admit_json`] is the whole of it as a safe function, so the native build
//! and the wasm module produce the very same bytes for the very same input.

use serde_json::{Value, json};

use crate::admit::{Requirement, admit};
use crate::protocol::{Config, IndexLine, delegation_payload};

/// Admit `upload` against the `expected` JSON (empty for none) and render the
/// result as the boundary's JSON.
pub fn admit_json(upload: &[u8], expected: &[u8]) -> String {
    let expected = match parse_expected(expected) {
        Ok(e) => e,
        Err(message) => return error("bad_request", &message),
    };
    match admit(
        upload,
        expected.as_ref().map(|(n, v)| (n.as_str(), v.as_str())),
    ) {
        Ok(a) => json!({ "ok": {
            "name": a.name,
            "version": a.version,
            "checksum": a.checksum,
            "deps": requirements(&a.deps),
            "npm": requirements(&a.npm),
            "archive": base64(&a.archive),
        }})
        .to_string(),
        Err(r) => error(r.code, &r.message),
    }
}

/// Answer one JSON request (`call`); see the module docs.
pub fn call_json(request: &[u8]) -> String {
    let v: Value = match serde_json::from_slice(request) {
        Ok(v) => v,
        Err(e) => return error("bad_request", &format!("the request isn't JSON: {e}")),
    };
    let result = match v.get("op").and_then(Value::as_str) {
        Some("line_payload") => line_from(&v, false).and_then(|line| {
            let registry = text(&v, "registry")?;
            Ok(json!({ "payload": base64(&line.payload(&registry)) }))
        }),
        Some("line_render") => line_from(&v, true).map(|line| json!({ "text": line.render() })),
        Some("signer_info") => signer_info(&v),
        Some(other) => Err(format!("no operation `{other}`")),
        None => Err("the request has no `op`".to_string()),
    };
    match result {
        Ok(ok) => json!({ "ok": ok }).to_string(),
        Err(message) => error("bad_request", &message),
    }
}

fn text(v: &Value, k: &str) -> Result<String, String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("the request has no string `{k}`"))
}

/// The request's `line`, as an index line. With `signed`, `yanked` and `sig`
/// are read too; without, they are left empty (a payload never covers them).
fn line_from(v: &Value, signed: bool) -> Result<IndexLine, String> {
    let line = v.get("line").ok_or("the request has no `line`")?;
    let reqs = |k: &str| -> Result<Vec<Requirement>, String> {
        line.get(k)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("the line has no list `{k}`"))?
            .iter()
            .map(|r| {
                Some(Requirement {
                    package: r.get("package")?.as_str()?.to_owned(),
                    range: r.get("range")?.as_str()?.to_owned(),
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| format!("the line's `{k}` is malformed"))
    };
    Ok(IndexLine {
        name: text(line, "name")?,
        version: text(line, "version")?,
        checksum: text(line, "checksum")?,
        deps: reqs("deps")?,
        npm: reqs("npm")?,
        toolchain: line
            .get("toolchain")
            .and_then(Value::as_str)
            .map(str::to_owned),
        yanked: signed && line.get("yanked").and_then(Value::as_bool).unwrap_or(false),
        // #2330: unsigned, like `yanked`, so only a rendered line carries it.
        withdrawn: if signed {
            line.get("withdrawn")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        },
        sig: if signed {
            text(line, "sig")?
        } else {
            String::new()
        },
    })
}

fn signer_info(v: &Value) -> Result<Value, String> {
    let config = Config::parse(text(v, "config")?.as_bytes())?;
    let key = text(v, "key")?;
    let chain_key = config.keys.last().expect("parse checks").key.clone();
    let delegation = config.delegations.iter().find(|d| d.key == key);
    let revoked = config.revoked.iter().any(|r| r.key == key);
    Ok(json!({
        "id": config.id,
        "chain_key": chain_key,
        "delegated": delegation.is_some(),
        "delegation_payload": base64(&delegation_payload(&config.id, &key)),
        "delegation_sig": delegation.and_then(|d| d.sig.clone()),
        "revoked": revoked,
    }))
}

fn error(code: &str, message: &str) -> String {
    json!({ "error": { "code": code, "message": message } }).to_string()
}

fn requirements(rs: &[Requirement]) -> Vec<Value> {
    rs.iter()
        .map(|r| json!({ "package": r.package, "range": r.range }))
        .collect()
}

fn parse_expected(bytes: &[u8]) -> Result<Option<(String, String)>, String> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("the expected package isn't JSON: {e}"))?;
    let field = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("the expected package has no string `{k}`"))
    };
    Ok(Some((field("name")?, field("version")?)))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(target_arch = "wasm32")]
mod exports {
    /// Reserve `len` bytes for the caller to fill.
    #[unsafe(no_mangle)]
    pub extern "C" fn alloc(len: usize) -> *mut u8 {
        let mut buf = Vec::<u8>::with_capacity(len.max(1));
        let ptr = buf.as_mut_ptr();
        std::mem::forget(buf);
        ptr
    }

    /// Release a buffer from [`alloc`], or a result `admit` returned.
    ///
    /// # Safety
    /// `ptr` and `len` must be exactly what `alloc(len)` or `admit` produced.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
        // SAFETY: the caller passes back what `alloc` handed out; capacity is
        // `max(len, 1)` for `alloc`, and exactly `len` for a result.
        drop(unsafe { Vec::from_raw_parts(ptr, 0, len.max(1)) });
    }

    /// Admit the archive at `ptr..ptr+len`; see the module docs.
    ///
    /// # Safety
    /// Both ranges must be live buffers of at least the stated lengths.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn admit(
        ptr: *const u8,
        len: usize,
        expected_ptr: *const u8,
        expected_len: usize,
    ) -> u64 {
        // SAFETY: the caller guarantees the ranges (the function's contract).
        let upload = unsafe { std::slice::from_raw_parts(ptr, len) };
        let expected = if expected_len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(expected_ptr, expected_len) }
        };
        let out = super::admit_json(upload, expected)
            .into_bytes()
            .into_boxed_slice();
        let (ptr, len) = (out.as_ptr() as u64, out.len() as u64);
        std::mem::forget(out);
        ptr << 32 | len
    }
}

#[cfg(target_arch = "wasm32")]
mod call_export {
    /// Answer the JSON request at `ptr..ptr+len`; see the module docs.
    ///
    /// # Safety
    /// The range must be a live buffer of at least `len` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn call(ptr: *const u8, len: usize) -> u64 {
        // SAFETY: the caller guarantees the range (the function's contract).
        let request = unsafe { std::slice::from_raw_parts(ptr, len) };
        let out = super::call_json(request).into_bytes().into_boxed_slice();
        let (ptr, len) = (out.as_ptr() as u64, out.len() as u64);
        std::mem::forget(out);
        ptr << 32 | len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn junk_and_a_bad_expectation_render_as_errors() {
        let junk = admit_json(b"junk", b"");
        assert!(junk.contains("\"invalid_package\""), "{junk}");
        let bad = admit_json(b"junk", b"{");
        assert!(bad.contains("\"bad_request\""), "{bad}");
    }

    #[test]
    fn a_line_payload_and_render_match_the_protocol_types() {
        let line = IndexLine {
            name: "acme/money".into(),
            version: "1.4.2".into(),
            checksum: "sha256-abc".into(),
            deps: vec![Requirement {
                package: "acme/fx".into(),
                range: "^2".into(),
            }],
            npm: vec![],
            toolchain: None,
            yanked: true,
            withdrawn: Some("a takedown".into()),
            sig: "c2ln".into(),
        };
        let fields = json!({
            "name": "acme/money", "version": "1.4.2", "checksum": "sha256-abc",
            "deps": [{"package": "acme/fx", "range": "^2"}], "npm": [], "toolchain": null,
            "yanked": true, "withdrawn": "a takedown", "sig": "c2ln",
        });
        let payload = call_json(
            json!({"op": "line_payload", "line": fields, "registry": "acme"})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(
            payload,
            json!({"ok": {"payload": base64(&line.payload("acme"))}}).to_string()
        );
        let rendered = call_json(
            json!({"op": "line_render", "line": fields})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(rendered, json!({"ok": {"text": line.render()}}).to_string());
    }

    #[test]
    fn signer_info_reports_without_verifying() {
        let config = r#"{"protocol":2,"id":"bynk","dl":"dl","keys":[{"key":"ed25519:ROOT"}],
            "delegations":[{"key":"ed25519:ONLINE","sig":"c2ln"}],
            "revoked":[{"key":"ed25519:OLD","sig":"eA=="}]}"#;
        let info = |key: &str| {
            call_json(
                json!({"op": "signer_info", "config": config, "key": key})
                    .to_string()
                    .as_bytes(),
            )
        };
        let online: Value = serde_json::from_str(&info("ed25519:ONLINE")).unwrap();
        assert_eq!(online["ok"]["chain_key"], "ed25519:ROOT");
        assert_eq!(online["ok"]["delegated"], true);
        assert_eq!(online["ok"]["delegation_sig"], "c2ln");
        assert_eq!(
            online["ok"]["delegation_payload"],
            base64(&delegation_payload("bynk", "ed25519:ONLINE"))
        );
        let old: Value = serde_json::from_str(&info("ed25519:OLD")).unwrap();
        assert_eq!(old["ok"]["revoked"], true);
        assert_eq!(old["ok"]["delegated"], false);
        assert!(call_json(b"{\"op\":\"nope\"}").contains("bad_request"));
    }
}
