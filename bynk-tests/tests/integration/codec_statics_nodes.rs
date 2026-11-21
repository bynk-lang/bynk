//! 1A.21 (#3722): the `Json`, `Yaml` and `Form` codec statics and the typed decoders lower to
//! nodes. Each case emits a one-commons project and reads the emitted function; the text-exit
//! counter is read where it is real (the driver links a non-test copy of `bynk-emit`, see
//! `text_exit_counters.rs`).

use bynk_check::project_model::BuildTarget;
use bynk_emit::emitter::text_counters;

const TYPES: &str = "type Qty = Int where Positive\n\
    type Row = { id: Int, name: String }\n\
    type Flags = { id: Int, on: Bool }\n\
    type Plan = enum { Free, Pro }\n\
    type Signup = { email: String, plan: Plan = Free }\n\
    type Renamed = { @name(\"Email\") upper: String, n: Int }\n\
    type Env = { body: String, qty: Option[Qty] }\n";

/// `(text exits, the commons' TypeScript)` for the functions `fns`.
fn emit(fns: &str) -> (usize, String) {
    text_counters::reset();
    let src = format!("commons demo.codec\n\n{TYPES}\n{fns}\n");
    let out = bynk_driver::project::compile_files_in_memory(
        &[("demo/codec.bynk", src.as_str())],
        BuildTarget::Bundle,
        Default::default(),
        true,
    )
    .unwrap_or_else(|_| panic!("fixture should compile:\n{src}"));
    let text: String = out
        .artefacts
        .docs
        .iter()
        .filter(|(p, _)| p.to_string_lossy().ends_with("demo/codec.ts"))
        .map(|(_, d)| d.text())
        .collect::<Vec<_>>()
        .join("\n");
    (text_counters::exits(), text)
}

/// The emitted function `name`, from its signature to its closing brace.
fn function<'a>(text: &'a str, name: &str) -> &'a str {
    let start = text
        .find(&format!("export function {name}("))
        .or_else(|| text.find(&format!("export function {name}<")))
        .unwrap_or_else(|| panic!("`{name}` is emitted:\n{text}"));
    let len = text[start..].find("\n}\n").expect("the function closes");
    &text[start..start + len + 2]
}

/// The tail every typed decoder ends with: `Ok`, else the error mapped to `kind`/`path`/`message`
/// (ADR 0047). Pinned once, so a change to the `path` rule or to a message arm is a failure here.
const TAIL: &str = "    const __be = __r.error;\n    return Err({ kind: __be.kind, path: (__be.kind === \"StructuralMismatch\" || __be.kind === \"RefinementViolation\") ? __be.path : \"$\", message: __be.kind === \"StructuralMismatch\" ? `expected ${__be.expected}, got ${String(__be.actual)}` : __be.kind === \"RefinementViolation\" ? __be.violation.message : __be.details });\n  })(s);\n}";

const JSON_PARSE: &str = "    let __j: __JsonValue;\n    try {\n      __j = globalThis.JSON.parse(__s) as __JsonValue;\n    } catch (__e) {\n      return Err({ kind: \"Malformed\", path: \"$\", message: String(__e) });\n    }\n";

/// The decoder of `fn_name` for result type `ty`, in full: parse step `parse`, row pass `pass`,
/// the codec call `des`.
fn decoder(fn_name: &str, ty: &str, parse: &str, pass: &str, des: &str) -> String {
    format!(
        "export function {fn_name}(s: string): Result<{ty}, DecodeError> {{\n  return ((__s: string): Result<{ty}, DecodeError> => {{\n{parse}{pass}    const __r = {des};\n    if (__r.tag === \"Ok\") return Ok(__r.value as {ty});\n{TAIL}"
    )
}

/// Every codec call leaves no text exit (the counter is live on this path, below).
fn assert_node(src: &str) -> String {
    let (exits, text) = emit(src);
    assert_eq!(exits, 0, "`{src}` left a text exit:\n{text}");
    text
}

#[test]
fn json_decode_over_a_record_a_list_an_option_and_a_refined_type() {
    for (f, ty, des) in [
        ("Row", "Row", "__deserialise_Row(__j, \"$\")"),
        (
            "List[Qty]",
            "readonly Qty[]",
            "__deserialise_List_Qty(__j, \"$\")",
        ),
        (
            "Option[Row]",
            "Option<Row>",
            "__deserialise_Option_Row(__j, \"$\")",
        ),
        ("Qty", "Qty", "__deserialise_Qty(__j, \"$\")"),
    ] {
        let text = assert_node(&format!(
            "fn d(s: String) -> Result[{f}, DecodeError] {{ Json.decode[{f}](s) }}"
        ));
        assert_eq!(
            function(&text, "d"),
            decoder("d", ty, JSON_PARSE, "", des),
            "Json.decode[{f}]"
        );
    }
}

#[test]
fn a_decode_notes_the_imports_its_signature_names() {
    let text = assert_node("fn d(s: String) -> Result[Row, DecodeError] { Json.decode[Row](s) }");
    assert!(
        text.contains("type { __JsonValue, __BoundaryError }"),
        "{text}"
    );
    assert!(
        text.contains("type { ValidationError, DecodeError }"),
        "{text}"
    );
}

#[test]
fn json_decode_over_a_rigid_type_parameter_goes_through_the_witness() {
    let text = assert_node(
        "fn t[T: Codable](s: String) -> Result[List[T], DecodeError] { Json.decode[List[T]](s) }",
    );
    let f = function(&text, "t");
    assert!(
        f.contains("    const __r = __bynkCodecList(__codec_T).decode(__j, \"$\");\n"),
        "{f}"
    );
    assert!(f.contains("Ok(__r.value as readonly T[])"), "{f}");
    assert!(f.contains(JSON_PARSE), "{f}");
}

#[test]
fn json_decode_sql_adds_the_bool_pass_only_for_a_row_with_a_bool_field() {
    let with =
        assert_node("fn u(s: String) -> Result[Flags, DecodeError] { Json.decodeSql[Flags](s) }");
    let pass = "    const __flags = (__o: __JsonValue): void => {\n      if (__o === null || typeof __o !== \"object\" || globalThis.Array.isArray(__o)) return;\n      const __row = __o as { [k: string]: __JsonValue };\n      for (const __k of [\"on\"]) {\n        if (__row[__k] === 0 || __row[__k] === 1) __row[__k] = __row[__k] === 1;\n      }\n    };\n    if (globalThis.Array.isArray(__j)) __j.forEach(__flags);\n    else __flags(__j);\n";
    assert_eq!(
        function(&with, "u"),
        decoder(
            "u",
            "Flags",
            JSON_PARSE,
            pass,
            "__deserialise_Flags(__j, \"$\")"
        )
    );
    let without =
        assert_node("fn v(s: String) -> Result[Row, DecodeError] { Json.decodeSql[Row](s) }");
    assert_eq!(
        function(&without, "v"),
        decoder("v", "Row", JSON_PARSE, "", "__deserialise_Row(__j, \"$\")")
    );
}

#[test]
fn yaml_decode_reads_the_text_with_the_yaml_parser() {
    let text = assert_node("fn y(s: String) -> Result[Row, DecodeError] { Yaml.decode[Row](s) }");
    let parse = "    const __p = __bynkYamlParse(__s);\n    if (__p.tag === \"Err\") return Err(__p.error);\n    const __j: __JsonValue = __p.value;\n";
    assert_eq!(
        function(&text, "y"),
        decoder("y", "Row", parse, "", "__deserialise_Row(__j, \"$\")")
    );
    assert!(text.contains("__bynkYamlParse"), "{text}");
}

/// `Form.decode`'s field table is the one foreign leaf: `workers_entry`'s own source text, carried
/// as a single expression (`VerbatimOrigin::FormFieldSpecs`). Everything around it, the decoder
/// arrow, its parse step and the codec call, is nodes.
#[test]
fn form_decode_is_a_node_decoder_around_the_field_table_carrier() {
    let text =
        assert_node("fn f(s: String) -> Result[Signup, DecodeError] { Form.decode[Signup](s) }");
    assert_eq!(
        function(&text, "f"),
        "export function f(s: string): Result<Signup, DecodeError> {\n  return ((__s: string): Result<Signup, DecodeError> => {\n    const __p = __formDecode(__s, [{ name: \"email\", kind: \"string\", list: false, option: false }, { name: \"plan\", kind: \"enum\", list: false, option: false }]);\n    if (__p.tag === \"Err\") return Err(__p.error);\n    const __j: __JsonValue = __p.value;\n    const __r = __deserialise_Signup(__j, \"$\");\n    if (__r.tag === \"Ok\") return Ok(__r.value as Signup);\n"
            .to_string()
            + TAIL
    );
}

#[test]
fn json_encode_is_a_stringify_over_the_serialiser() {
    let text = assert_node(
        "fn e(r: Row) -> String { Json.encode(r) }\nfn q(r: Qty) -> String { Json.encode(r) }",
    );
    assert_eq!(
        function(&text, "e"),
        "export function e(r: Row): string {\n  return globalThis.JSON.stringify(__serialise_Row(r));\n}"
    );
    assert_eq!(
        function(&text, "q"),
        "export function q(r: Qty): string {\n  return globalThis.JSON.stringify(__serialise_Qty(r));\n}"
    );
}

#[test]
fn json_encode_plain_raises_the_flag_for_one_serialisation() {
    let text = assert_node(
        "fn p(r: Row) -> String { Json.encodePlain(r) }\n\
         fn x(b: Int) -> String { Json.encodePlain(Env { body: Json.encode(b), qty: None }) }",
    );
    let iife = |arg: &str| {
        format!(
            "((__pv = {arg}): string => {{\n    const __g = globalThis as {{ __bynkPlainJson?: boolean }};\n    const __was = __g.__bynkPlainJson;\n    __g.__bynkPlainJson = true;\n    try {{\n      return globalThis.JSON.stringify(__serialise_{});\n    }} finally {{\n      __g.__bynkPlainJson = __was;\n    }}\n  }})()",
            if arg == "r" { "Row(__pv)" } else { "Env(__pv)" }
        )
    };
    assert_eq!(
        function(&text, "p"),
        format!(
            "export function p(r: Row): string {{\n  return {};\n}}",
            iife("r")
        )
    );
    // The argument is the parameter default, evaluated at call time before the body raises the flag, so the inner
    // `Json.encode` stays tagged.
    let x = function(&text, "x");
    assert!(
        x.contains("((__pv = { body: globalThis.JSON.stringify("),
        "{x}"
    );
    assert!(x.contains("qty: None }): string =>"), "{x}");
}

#[test]
fn form_encode_writes_the_serialised_record_and_a_renamed_field_its_table() {
    let text = assert_node("fn g(s: Signup) -> String { Form.encode(s) }");
    assert_eq!(
        function(&text, "g"),
        "export function g(s: Signup): string {\n  return __formString(__serialise_Signup(s));\n}"
    );
    // #3343: a field renamed by `@name` is written under its wire key; the table is
    // `workers_entry` source text, carried as the one `FormFieldSpecs` leaf.
    let text = assert_node("fn r(s: Renamed) -> String { Form.encode(s) }");
    assert!(
        function(&text, "r").contains("__formString(__serialise_Renamed(s), "),
        "{text}"
    );
}
