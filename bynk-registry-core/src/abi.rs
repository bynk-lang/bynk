//! The module boundary (first-party registry track §3.4): a plain C ABI, no
//! `wasm-bindgen`, so the build is one `.wasm` file and no glue.
//!
//! ```text
//! alloc(len) -> ptr                       reserve `len` bytes of linear memory
//! dealloc(ptr, len)                       release a buffer from `alloc` or a result
//! admit(ptr, len, exp_ptr, exp_len) -> u64
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
//! [`admit_json`] is the whole of it as a safe function, so the native build
//! and the wasm module produce the very same bytes for the very same input.

use serde_json::{Value, json};

use crate::admit::{Requirement, admit};

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
}
