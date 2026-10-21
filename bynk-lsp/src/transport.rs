//! #1667: a framing pump in front of `tower-lsp`'s transport.
//!
//! `tower-lsp` 0.20 reads stdin through a `FramedRead`, and a framed stream
//! ends at its first decode error. So one message whose body isn't valid JSON
//! stopped the whole server, with exit 0 and nothing on stderr. The trigger
//! is reachable from a well-behaved client: `JSON.stringify` writes a lone
//! UTF-16 surrogate in a string as `\udcff`, which `serde_json` rejects.
//!
//! [`pump`] reads the client's frames itself and forwards to the server only
//! bodies that parse, so a bad one can't end the stream:
//!
//! - a body that parses is forwarded unchanged;
//! - a body whose only fault is a lone surrogate escape is repaired (each
//!   lone `\uD800`–`\uDFFF` becomes `\uFFFD`) and forwarded. U+FFFD is one
//!   UTF-16 code unit, like the surrogate it replaces, so the client's
//!   positions into the text stay aligned with the server's;
//! - any other body is logged and skipped. Its `id` can't be recovered from
//!   unparseable JSON, so no error response is possible.
//!
//! A header block without `Content-Length`, or a stream that ends mid-frame
//! (a truncated body), leaves nothing to resynchronise on: the pump stops,
//! with the reason logged, and the server sees end of input.
//!
//! #2239: the pump also stops once it has forwarded `exit`. `tower-lsp` 0.20
//! handles a message that arrives after `exit` by leaving its read loop early,
//! without the clean-up it does at end of input, so `Server::serve` never
//! returned and the process never exited. Ending the server's input after
//! `exit` means it only ever sees end of input next, whatever the client sends.

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// #1771 review: the largest body [`pump`] accepts. A `Content-Length` is
/// otherwise trusted and allocated in full before the body arrives, so one
/// corrupted digit could abort the process. A frame over this is unrecoverable
/// framing, like a missing `Content-Length`. 128 MiB is far above any real
/// message (a `didOpen` carries one file's text).
pub const MAX_BODY: usize = 128 << 20;

/// The longest header line [`pump`] reads, so a client that never sends a
/// newline can't grow the line without limit.
pub const MAX_HEADER_LINE: u64 = 8 << 10;

/// What [`pump`] does with one frame's body.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// The body parses as JSON: forward it unchanged.
    Forward,
    /// The body parses once its lone surrogate escapes are replaced.
    Repaired(Vec<u8>),
    /// The body can't be parsed. The string says why.
    Skip(String),
}

/// Decide what to do with one frame's body.
pub fn classify(body: &[u8]) -> Frame {
    let error = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(_) => return Frame::Forward,
        Err(e) => e,
    };
    if let Ok(text) = std::str::from_utf8(body)
        && let Some(repaired) = repair_lone_surrogates(text)
        && serde_json::from_str::<serde_json::Value>(&repaired).is_ok()
    {
        return Frame::Repaired(repaired.into_bytes());
    }
    Frame::Skip(error.to_string())
}

/// Replace each lone surrogate escape inside a JSON string with `\uFFFD`.
/// `None` when there is none to replace.
///
/// The scan tracks string and escape state, so an escaped backslash
/// (`\\udcff`, a literal backslash then `udcff`) is left alone, and a valid
/// pair (`\ud83d\ude00`) is kept as written.
fn repair_lone_surrogates(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if !in_string {
            if b == b'"' {
                in_string = true;
            }
            out.push(b as char);
            i += 1;
            continue;
        }
        match b {
            b'"' => {
                in_string = false;
                out.push('"');
                i += 1;
            }
            b'\\' if bytes.get(i + 1) == Some(&b'u') => {
                let unit = hex4(bytes, i + 2);
                match unit {
                    Some(0xD800..=0xDBFF) => {
                        // A high surrogate is kept only with a low one after it.
                        let low = (bytes.get(i + 6) == Some(&b'\\')
                            && bytes.get(i + 7) == Some(&b'u'))
                        .then(|| hex4(bytes, i + 8))
                        .flatten();
                        if matches!(low, Some(0xDC00..=0xDFFF)) {
                            out.push_str(&text[i..i + 12]);
                            i += 12;
                        } else {
                            out.push_str("\\uFFFD");
                            changed = true;
                            i += 6;
                        }
                    }
                    Some(0xDC00..=0xDFFF) => {
                        out.push_str("\\uFFFD");
                        changed = true;
                        i += 6;
                    }
                    _ => {
                        out.push_str("\\u");
                        i += 2;
                    }
                }
            }
            b'\\' => {
                // Any other escape: copy it and the escaped byte together, so
                // an escaped quote or backslash can't change the string state.
                out.push('\\');
                if let Some(next) = text[i + 1..].chars().next() {
                    out.push(next);
                    i += 1 + next.len_utf8();
                } else {
                    i += 1;
                }
            }
            _ => {
                let ch = text[i..].chars().next().expect("in bounds");
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    changed.then_some(out)
}

/// #2239: the longest body [`is_exit`] parses. An `exit` notification is a
/// few dozen bytes, so a larger body (a `didOpen`'s text) is never parsed
/// twice.
const MAX_EXIT_BODY: usize = 1024;

/// Whether `body` is the `exit` notification, after which the server reads
/// nothing more.
pub fn is_exit(body: &[u8]) -> bool {
    body.len() <= MAX_EXIT_BODY
        && serde_json::from_slice::<serde_json::Value>(body)
            .is_ok_and(|message| message.get("method").and_then(|m| m.as_str()) == Some("exit"))
}

/// The four hex digits at `at`, as a UTF-16 code unit.
fn hex4(bytes: &[u8], at: usize) -> Option<u32> {
    let digits = std::str::from_utf8(bytes.get(at..at + 4)?).ok()?;
    u32::from_str_radix(digits, 16).ok()
}

/// Read framed messages from `input` and write the usable ones, re-framed, to
/// `output`. Returns when `input` ends or loses its framing, or once `exit`
/// is forwarded (#2239). Dropping `output` then ends the server's input.
pub async fn pump<R, W>(input: R, mut output: W)
where
    R: tokio::io::AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut input = BufReader::new(input);
    loop {
        // Headers, up to the blank line. Only `Content-Length` matters.
        let mut length: Option<usize> = None;
        let mut saw_header = false;
        loop {
            let mut line = String::new();
            let read = (&mut input)
                .take(MAX_HEADER_LINE)
                .read_line(&mut line)
                .await;
            if read.as_ref().is_ok_and(|n| *n as u64 == MAX_HEADER_LINE) && !line.ends_with('\n') {
                tracing::error!(
                    "a client header line is over {MAX_HEADER_LINE} bytes; \
                     the input can't be resynchronised, so it is closed"
                );
                return;
            }
            match read {
                Ok(0) => {
                    if saw_header {
                        tracing::error!("client input ended inside a message's headers");
                    }
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("reading client input failed: {e}");
                    return;
                }
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if saw_header {
                    break;
                }
                continue;
            }
            saw_header = true;
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().ok();
            }
        }
        let Some(length) = length else {
            tracing::error!(
                "a client message had no usable Content-Length header; \
                 the input can't be resynchronised, so it is closed"
            );
            return;
        };
        if length > MAX_BODY {
            tracing::error!(
                "a client message claims a {length}-byte body, over the {MAX_BODY}-byte limit; \
                 the input can't be resynchronised, so it is closed"
            );
            return;
        }
        let mut body = vec![0u8; length];
        if let Err(e) = input.read_exact(&mut body).await {
            tracing::error!(
                "client input ended inside a message body ({length} bytes expected): {e}"
            );
            return;
        }
        let body = match classify(&body) {
            Frame::Forward => body,
            Frame::Repaired(repaired) => {
                tracing::warn!(
                    "replaced lone UTF-16 surrogate escapes in a client message with U+FFFD"
                );
                repaired
            }
            Frame::Skip(reason) => {
                let shown = String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned();
                tracing::error!(
                    "skipped a client message that isn't valid JSON ({reason}): {shown}"
                );
                continue;
            }
        };
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        if output.write_all(header.as_bytes()).await.is_err()
            || output.write_all(&body).await.is_err()
            || output.flush().await.is_err()
        {
            // The server side has gone; nothing more to deliver.
            return;
        }
        if is_exit(&body) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_json_is_forwarded() {
        assert_eq!(classify(br#"{"a":"\ud83d\ude00"}"#), Frame::Forward);
    }

    #[test]
    fn a_lone_surrogate_is_repaired_to_the_replacement_character() {
        let Frame::Repaired(body) = classify(br#"{"text":"a\udcffb"}"#) else {
            panic!("repaired");
        };
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["text"], "a\u{FFFD}b");
    }

    #[test]
    fn a_lone_high_surrogate_is_repaired_and_a_pair_is_kept() {
        let Frame::Repaired(body) = classify(br#"{"t":"\ud83d x \ud83d\ude00"}"#) else {
            panic!("repaired");
        };
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["t"], "\u{FFFD} x \u{1F600}");
    }

    #[test]
    fn an_escaped_backslash_before_u_is_not_a_surrogate_escape() {
        // `\\udcff` is a literal backslash then `udcff`: valid JSON as is.
        assert_eq!(classify(br#"{"t":"\\udcff"}"#), Frame::Forward);
    }

    #[test]
    fn other_invalid_json_is_skipped() {
        assert!(matches!(classify(b"{not json"), Frame::Skip(_)));
    }

    #[test]
    fn a_surrogate_escape_cut_off_at_the_end_is_skipped() {
        assert!(matches!(classify(br#"{"t":"\ud8"#), Frame::Skip(_)));
    }

    /// Run the pump over `input` and return everything it forwarded.
    async fn pumped(input: &[u8]) -> Vec<u8> {
        let (mut server_side, client_side) = tokio::io::duplex(64 * 1024);
        pump(input, client_side).await;
        let mut out = Vec::new();
        server_side.read_to_end(&mut out).await.unwrap();
        out
    }

    /// #1771 review: each unrecoverable-framing path returns (the server
    /// then sees end of input) rather than hanging, spinning or allocating.
    #[tokio::test]
    async fn unrecoverable_framing_ends_the_pump() {
        // A header block with no Content-Length.
        assert!(pumped(b"Content-Type: x\r\n\r\n{}").await.is_empty());
        // A truncated body: 50 bytes promised, 5 sent, then end of input.
        assert!(pumped(b"Content-Length: 50\r\n\r\nhello").await.is_empty());
        // Only blank lines, then end of input.
        assert!(pumped(b"\r\n\r\n\r\n").await.is_empty());
        // A Content-Length over the limit (or past usize) allocates nothing.
        assert!(
            pumped(b"Content-Length: 99999999999\r\n\r\n{}")
                .await
                .is_empty()
        );
        assert!(
            pumped(b"Content-Length: 18446744073709551615\r\n\r\n{}")
                .await
                .is_empty()
        );
        // A header line with no end.
        let endless = vec![b'x'; (MAX_HEADER_LINE as usize) * 2];
        assert!(pumped(&endless).await.is_empty());
    }

    /// The pump forwards good frames, repairs a lone surrogate, skips an
    /// unparseable one, and keeps going: one bad message no longer ends the
    /// stream the server reads.
    #[tokio::test]
    async fn the_pump_skips_a_bad_frame_and_keeps_going() {
        let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let input = [
            frame(r#"{"n":1}"#),
            frame("{not json"),
            frame(r#"{"t":"\udcff"}"#),
            frame(r#"{"n":2}"#),
        ]
        .concat();
        let (server_side, client_side) = tokio::io::duplex(64 * 1024);
        pump(input.as_bytes(), client_side).await;
        let mut out = String::new();
        let mut server_side = server_side;
        server_side.read_to_string(&mut out).await.unwrap();
        assert_eq!(out.matches("Content-Length").count(), 3, "{out}");
        assert!(
            out.contains(r#"{"n":1}"#) && out.contains(r#"{"n":2}"#),
            "{out}"
        );
        assert!(out.contains(r#"\uFFFD"#), "{out}");
        assert!(!out.contains("not json"), "{out}");
    }

    #[test]
    fn only_the_exit_notification_is_exit() {
        assert!(is_exit(br#"{"jsonrpc":"2.0","method":"exit"}"#));
        assert!(!is_exit(br#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#));
        assert!(!is_exit(
            br#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#
        ));
        // `exit` inside a string is not the method.
        assert!(!is_exit(
            br#"{"jsonrpc":"2.0","method":"x","params":{"t":"exit"}}"#
        ));
    }

    /// #2239: the pump ends the server's input once it has forwarded `exit`,
    /// without waiting for the client's input to end, and forwards nothing
    /// after it.
    #[tokio::test]
    async fn the_pump_stops_after_exit() {
        let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let (mut client, input) = tokio::io::duplex(64 * 1024);
        let written = [
            frame(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            frame(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#),
        ]
        .concat();
        client.write_all(written.as_bytes()).await.unwrap();
        // `client` stays open: the pump must return on `exit`, not end of input.
        let (mut server_side, output) = tokio::io::duplex(64 * 1024);
        tokio::time::timeout(std::time::Duration::from_secs(5), pump(input, output))
            .await
            .expect("the pump returns after `exit`");
        let mut out = String::new();
        server_side.read_to_string(&mut out).await.unwrap();
        assert!(out.contains(r#""method":"exit""#), "{out}");
        assert!(!out.contains("initialized"), "{out}");
        drop(client);
    }
}
