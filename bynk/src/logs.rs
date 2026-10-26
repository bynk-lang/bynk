//! `bynk dev` worker logs (#2474): each line a `wrangler dev` prints, with the
//! stack frames that name generated TypeScript rewritten to the `.bynk` source
//! line they came from.
//!
//! The emitter writes a source-map v3 document beside every generated module
//! (`<file>.ts.map`, ADR 0103/0569). A runtime error in a handler prints frames
//! like `at http_GET (/…/.bynk/dev/workers/shop/handlers.ts:57:11)`; that line
//! is useless to someone who wrote `.bynk`. [`Rewriter`] looks the position up
//! in the module's map and puts the `.bynk` location in its place, so the
//! terminal, and the editor's log view, can link straight to the source.
//!
//! The rewriting is the driver's rather than the editor's so the CLI benefits
//! too. It is line-at-a-time and total: a line it does not understand, a module
//! without a map, or a position the map does not cover is passed through as it
//! came.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// How severe a log line is, as far as `wrangler dev`'s output tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Classifies a stream of log lines. Wrangler tags its own messages
/// (`[ERROR]`, `✘`, `[WARNING]`, `▲`) and prints a thrown error as
/// `Error: …` followed by `    at …` frames, so a frame inherits the level of
/// the line it continues.
#[derive(Debug)]
pub struct Classifier {
    last: LogLevel,
}

impl Default for Classifier {
    fn default() -> Self {
        Classifier {
            last: LogLevel::Info,
        }
    }
}

impl Classifier {
    pub fn classify(&mut self, line: &str) -> LogLevel {
        let text = line.trim_start();
        let level = if text.starts_with("at ") {
            self.last
        } else if text.contains("[ERROR]")
            || text.starts_with('✘')
            || text.starts_with("Uncaught")
            || text.contains("Uncaught (in")
            || is_error_headline(text)
        {
            LogLevel::Error
        } else if text.contains("[WARNING]") || text.starts_with('▲') {
            LogLevel::Warn
        } else {
            LogLevel::Info
        };
        self.last = level;
        level
    }
}

/// `TypeError: x is not a function` — an uncaught error's first line: a
/// capitalised identifier ending `Error`, then a colon.
fn is_error_headline(text: &str) -> bool {
    let Some((name, _)) = text.split_once(':') else {
        return false;
    };
    name.ends_with("Error")
        && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A decoded source-map v3 document. Only what a frame needs: the `sources`
/// and, per generated line, the segments that carry a source position.
#[derive(Debug, PartialEq, Eq)]
pub struct SourceMap {
    sources: Vec<String>,
    /// Per generated line (0-based), segments in column order.
    lines: Vec<Vec<Segment>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Segment {
    column: u32,
    source: u32,
    line: u32,
    source_column: u32,
}

/// A position in an original source: 1-based line and column, as editors and
/// stack frames count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Original {
    pub source: String,
    pub line: u32,
    pub column: u32,
}

impl SourceMap {
    /// Parse a v3 document; `None` for anything else.
    pub fn parse(json: &str) -> Option<SourceMap> {
        let doc: serde_json::Value = serde_json::from_str(json).ok()?;
        if doc.get("version")?.as_u64()? != 3 {
            return None;
        }
        let root = doc.get("sourceRoot").and_then(|r| r.as_str()).unwrap_or("");
        let sources = doc
            .get("sources")?
            .as_array()?
            .iter()
            .map(|s| {
                let s = s.as_str().unwrap_or("");
                if root.is_empty() || Path::new(s).is_absolute() {
                    s.to_string()
                } else {
                    format!("{}/{s}", root.trim_end_matches('/'))
                }
            })
            .collect();
        let lines = decode_mappings(doc.get("mappings")?.as_str()?)?;
        Some(SourceMap { sources, lines })
    }

    /// The original position of a generated one (1-based line and column): the
    /// last segment on that line at or before the column. A line the map has no
    /// segment for has no original; the emitter maps every generated line a
    /// statement spans, so that is generated glue.
    pub fn original(&self, line: u32, column: u32) -> Option<Original> {
        let segments = self.lines.get(line.checked_sub(1)? as usize)?;
        let column0 = column.saturating_sub(1);
        let seg = segments
            .iter()
            .take_while(|s| s.column <= column0)
            .last()
            .or_else(|| segments.first())?;
        Some(Original {
            source: self.sources.get(seg.source as usize)?.clone(),
            line: seg.line + 1,
            column: seg.source_column + 1,
        })
    }
}

fn decode_mappings(mappings: &str) -> Option<Vec<Vec<Segment>>> {
    let (mut source, mut line, mut source_column) = (0i64, 0i64, 0i64);
    let mut out = Vec::new();
    for generated_line in mappings.split(';') {
        let mut column = 0i64;
        let mut segments = Vec::new();
        for raw in generated_line.split(',').filter(|s| !s.is_empty()) {
            let fields = decode_vlq(raw)?;
            column += *fields.first()?;
            // One field is a generated-only position: nothing to map back to.
            if fields.len() < 4 {
                continue;
            }
            source += fields[1];
            line += fields[2];
            source_column += fields[3];
            segments.push(Segment {
                column: u32::try_from(column).ok()?,
                source: u32::try_from(source).ok()?,
                line: u32::try_from(line).ok()?,
                source_column: u32::try_from(source_column).ok()?,
            });
        }
        out.push(segments);
    }
    Some(out)
}

fn decode_vlq(segment: &str) -> Option<Vec<i64>> {
    const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut values = Vec::new();
    let (mut value, mut shift) = (0i64, 0u32);
    for byte in segment.bytes() {
        let digit = BASE64.iter().position(|&b| b == byte)? as i64;
        if shift > 40 {
            return None;
        }
        value |= (digit & 0x1f) << shift;
        if digit & 0x20 != 0 {
            shift += 5;
        } else {
            let negative = value & 1 == 1;
            let magnitude = value >> 1;
            values.push(if negative { -magnitude } else { magnitude });
            (value, shift) = (0, 0);
        }
    }
    (shift == 0).then_some(values)
}

/// One stack frame's location, split out of a line.
#[derive(Debug, PartialEq, Eq)]
struct Frame<'a> {
    /// The line up to the location.
    head: &'a str,
    path: &'a str,
    line: u32,
    column: u32,
    /// The line from the end of the location: the closing `)`, if any.
    tail: &'a str,
}

/// Split `    at fn (file:///x/y.ts:12:5)` / `    at /x/y.ts:12:5` into its
/// location. `None` for any line that is not a stack frame.
fn parse_frame(line: &str) -> Option<Frame<'_>> {
    let body = line.trim_start();
    let indent = line.len() - body.len();
    let rest = body.strip_prefix("at ")?;
    let (loc_start, loc_end) = if rest.ends_with(')') {
        let open = rest.rfind(" (")? + 2;
        (indent + 3 + open, line.len() - 1)
    } else {
        let skip = if rest.starts_with("async ") { 6 } else { 0 };
        (indent + 3 + skip, line.len())
    };
    let loc = line.get(loc_start..loc_end)?;
    let (rest, column) = loc.rsplit_once(':')?;
    let (path, ln) = rest.rsplit_once(':')?;
    Some(Frame {
        head: &line[..loc_start],
        path,
        line: ln.parse().ok()?,
        column: column.parse().ok()?,
        tail: &line[loc_end..],
    })
}

/// `file:///c:/x/y.ts` and `file:///x/y.ts` to a plain path.
fn strip_file_scheme(path: &str) -> String {
    let Some(rest) = path.strip_prefix("file://") else {
        return path.to_string();
    };
    // `file:///c:/x` carries a drive letter; `file:///x` is rooted.
    let drive = rest.strip_prefix('/').filter(|r| {
        let b = r.as_bytes();
        b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
    });
    drive.unwrap_or(rest).to_string()
}

/// A loaded map and the mtime it was read at; `None` for a file that did not parse.
type CachedMap = (SystemTime, Option<std::sync::Arc<SourceMap>>);

/// Rewrites frames against the source maps under a build dir, loading each map
/// on first use and again when it changes: a rebuild rewrites them in place.
pub struct Rewriter {
    build_dir: PathBuf,
    cache: Mutex<HashMap<PathBuf, CachedMap>>,
}

impl Rewriter {
    pub fn new(build_dir: &Path) -> Rewriter {
        Rewriter {
            build_dir: build_dir.to_path_buf(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// `line`, with its frame location (if it is a frame into generated code
    /// that has a map) replaced by the `.bynk` one. `worker` is the worker dir
    /// the line came from, to resolve a path wrangler printed relative to it.
    pub fn rewrite(&self, worker: &str, line: &str) -> String {
        let Some(frame) = parse_frame(line) else {
            return line.to_string();
        };
        let Some(original) = self
            .module_map(worker, frame.path)
            .and_then(|m| m.original(frame.line, frame.column))
        else {
            return line.to_string();
        };
        format!(
            "{}{}:{}:{}{}",
            frame.head, original.source, original.line, original.column, frame.tail
        )
    }

    /// The map for the generated module a frame names, if it has one.
    fn module_map(&self, worker: &str, path: &str) -> Option<std::sync::Arc<SourceMap>> {
        let path = strip_file_scheme(path);
        let map_name = format!("{path}.map");
        let candidates: Vec<PathBuf> = if Path::new(&path).is_absolute() {
            vec![PathBuf::from(&map_name)]
        } else {
            vec![
                self.build_dir.join("workers").join(worker).join(&map_name),
                self.build_dir.join(&map_name),
            ]
        };
        let found = candidates.iter().find(|c| c.is_file())?;
        self.load(found)
    }

    fn load(&self, file: &Path) -> Option<std::sync::Arc<SourceMap>> {
        let mtime = std::fs::metadata(file).ok()?.modified().ok()?;
        let mut cache = self.cache.lock().ok()?;
        if let Some((seen, map)) = cache.get(file)
            && *seen == mtime
        {
            return map.clone();
        }
        let map = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| SourceMap::parse(&text))
            .map(std::sync::Arc::new);
        cache.insert(file.to_path_buf(), (mtime, map.clone()));
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map for a four-line module whose lines 2 and 3 are `.bynk` lines 10
    /// and 11 (0-based 9 and 10), with a first column of 4 on line 3.
    const MAP: &str =
        r#"{"version":3,"file":"h.ts","sources":["/p/src/a.bynk"],"mappings":";AASA;IACA"}"#;

    #[test]
    fn decodes_vlq_with_signs_and_continuations() {
        assert_eq!(decode_vlq("A"), Some(vec![0]));
        assert_eq!(decode_vlq("C"), Some(vec![1]));
        assert_eq!(decode_vlq("D"), Some(vec![-1]));
        assert_eq!(decode_vlq("gB"), Some(vec![16]));
        assert_eq!(decode_vlq("AAMA"), Some(vec![0, 0, 6, 0]));
        // A continuation bit with nothing after it is malformed.
        assert_eq!(decode_vlq("g"), None);
        assert_eq!(decode_vlq("!"), None);
    }

    #[test]
    fn looks_up_a_generated_position() {
        let map = SourceMap::parse(MAP).unwrap();
        assert_eq!(
            map.original(2, 1),
            Some(Original {
                source: "/p/src/a.bynk".into(),
                line: 10,
                column: 1
            })
        );
        // Line 3's segment starts at column 5 (0-based 4): an earlier
        // column on the line still takes it, as does a later one.
        assert_eq!(map.original(3, 40).unwrap().line, 11);
        assert_eq!(map.original(3, 40).unwrap().column, 1);
        // No segment on line 1, and no line 9.
        assert_eq!(map.original(1, 1), None);
        assert_eq!(map.original(9, 1), None);
        assert_eq!(map.original(0, 1), None);
    }

    #[test]
    fn rejects_other_documents() {
        assert!(SourceMap::parse("not json").is_none());
        assert!(SourceMap::parse(r#"{"version":2,"sources":[],"mappings":""}"#).is_none());
        assert!(SourceMap::parse(r#"{"version":3,"sources":[],"mappings":"!!"}"#).is_none());
    }

    #[test]
    fn splits_frames_in_both_node_shapes() {
        let f = parse_frame("    at http_GET (file:///w/h.ts:57:11)").unwrap();
        assert_eq!((f.path, f.line, f.column), ("file:///w/h.ts", 57, 11));
        assert_eq!(f.head, "    at http_GET (");
        assert_eq!(f.tail, ")");
        let f = parse_frame("    at async /w/h.ts:3:9").unwrap();
        assert_eq!((f.path, f.line, f.column), ("/w/h.ts", 3, 9));
        assert_eq!(f.tail, "");
        // A Windows drive letter's colon is not a position.
        let f = parse_frame("  at f (C:\\w\\h.ts:1:2)").unwrap();
        assert_eq!(f.path, "C:\\w\\h.ts");
        assert!(parse_frame("GET / 200 OK (5ms)").is_none());
        assert!(parse_frame("    at native").is_none());
        assert!(parse_frame("    at f (<anonymous>)").is_none());
    }

    #[test]
    fn strips_the_file_scheme() {
        assert_eq!(strip_file_scheme("file:///w/h.ts"), "/w/h.ts");
        assert_eq!(strip_file_scheme("file:///c:/w/h.ts"), "c:/w/h.ts");
        assert_eq!(strip_file_scheme("/w/h.ts"), "/w/h.ts");
    }

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bynk-logs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let worker = dir.join("workers").join("shop");
        std::fs::create_dir_all(&worker).unwrap();
        std::fs::write(worker.join("handlers.ts.map"), MAP).unwrap();
        dir
    }

    #[test]
    fn rewrites_an_absolute_and_a_relative_frame() {
        let build = project("rewrite");
        let rw = Rewriter::new(&build);
        let abs = build.join("workers/shop/handlers.ts");
        let line = format!("    at http_GET (file://{}:2:7)", abs.display());
        assert_eq!(
            rw.rewrite("shop", &line),
            "    at http_GET (/p/src/a.bynk:10:1)"
        );
        // Wrangler prints a path relative to the worker dir, too.
        assert_eq!(
            rw.rewrite("shop", "    at handlers.ts:3:1"),
            "    at /p/src/a.bynk:11:1"
        );
        let _ = std::fs::remove_dir_all(&build);
    }

    #[test]
    fn leaves_everything_else_alone() {
        let build = project("alone");
        let rw = Rewriter::new(&build);
        for line in [
            "GET / 200 OK (5ms)",
            "Error: boom",
            // No map for this module.
            "    at f (file:///elsewhere/runtime.js:5:5)",
            // A map, but no segment on that line.
            "    at handlers.ts:1:1",
            "    at handlers.ts:400:1",
            "",
        ] {
            assert_eq!(rw.rewrite("shop", line), line);
        }
        let _ = std::fs::remove_dir_all(&build);
    }

    #[test]
    fn reloads_a_map_that_changed() {
        let build = project("reload");
        let rw = Rewriter::new(&build);
        let map = build.join("workers/shop/handlers.ts.map");
        assert!(
            rw.rewrite("shop", "    at handlers.ts:2:1")
                .contains(":10:1")
        );
        // A rebuild rewrites the map (a later mtime, a different line).
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&map, MAP.replace(";AASA;", ";AAeA;")).unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&map)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(
            rw.rewrite("shop", "    at handlers.ts:2:1")
                .contains(":16:1")
        );
        let _ = std::fs::remove_dir_all(&build);
    }

    #[test]
    fn classifies_levels_and_lets_frames_inherit() {
        let mut c = Classifier::default();
        assert_eq!(c.classify("GET / 200 OK (5ms)"), LogLevel::Info);
        assert_eq!(
            c.classify("✘ [ERROR] Uncaught (in response) Error: x"),
            LogLevel::Error
        );
        assert_eq!(c.classify("    at f (a.ts:1:1)"), LogLevel::Error);
        assert_eq!(c.classify("▲ [WARNING] heads up"), LogLevel::Warn);
        assert_eq!(c.classify("    at g (a.ts:1:1)"), LogLevel::Warn);
        assert_eq!(
            c.classify("TypeError: x is not a function"),
            LogLevel::Error
        );
        assert_eq!(c.classify("    at h (a.ts:1:1)"), LogLevel::Error);
        assert_eq!(c.classify("hello"), LogLevel::Info);
        // A frame with nothing before it is information.
        assert_eq!(
            Classifier::default().classify("    at h (a.ts:1:1)"),
            LogLevel::Info
        );
    }
}
