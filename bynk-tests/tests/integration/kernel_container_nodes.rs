//! 1A.30 (#3731): the container kernels (`Option`, `Result`, `Set`, `Map`) lower to nodes. A project
//! is emitted through the driver and the exact lines are pinned. The arms' own forms are unit
//! tests in `lower_call/kernel_container.rs`.

/// `every emitted document joined` for the one-context project `src` on Workers.
fn mem(src: &str) -> String {
    let out = bynk_driver::project::compile_files_in_memory(
        &[("ops/jobs.bynk", src)],
        bynk_check::project_model::BuildTarget::Workers,
        Default::default(),
        true,
    )
    .unwrap_or_else(|e| {
        panic!(
            "should compile:\n{src}\n{}",
            bynk_driver::render::render_project_errors(&e.flatten())
        )
    });
    let text = out
        .artefacts
        .docs
        .values()
        .map(|d| d.text())
        .collect::<Vec<_>>()
        .join("\n");
    text
}

fn assert_line(text: &str, line: &str) {
    assert!(
        text.lines().any(|l| l.trim_start() == line),
        "missing line {line:?} in\n{text}"
    );
}

const NODES: &str = r#"context ops.jobs

fn opt_map(o: Option[Int], f: (Int) -> Int) -> Option[Int] { o.map(f) }
fn opt_and_then(o: Option[Int], f: (Int) -> Option[Int]) -> Option[Int] { o.andThen(f) }
fn opt_get(o: Option[Int], d: Int) -> Int { o.getOrElse(d) }
fn opt_some(o: Option[Int]) -> Bool { o.isSome() }
fn opt_none(o: Option[Int]) -> Bool { o.isNone() }
fn opt_ok_or(o: Option[Int], e: String) -> Result[Int, String] { o.okOr(e) }
fn opt_or_else(o: Option[Int], f: () -> Option[Int]) -> Option[Int] { o.orElse(f) }
fn opt_filter(o: Option[Int], p: (Int) -> Bool) -> Option[Int] { o.filter(p) }
fn res_map(r: Result[Int, String], f: (Int) -> Int) -> Result[Int, String] { r.map(f) }
fn res_and_then(r: Result[Int, String], f: (Int) -> Result[Int, String]) -> Result[Int, String] { r.andThen(f) }
fn res_map_err(r: Result[Int, String], f: (String) -> Int) -> Result[Int, Int] { r.mapErr(f) }
fn res_get(r: Result[Int, String], d: Int) -> Int { r.getOrElse(d) }
fn res_ok(r: Result[Int, String]) -> Option[Int] { r.ok() }
fn res_is_ok(r: Result[Int, String]) -> Bool { r.isOk() }
fn res_is_err(r: Result[Int, String]) -> Bool { r.isErr() }
fn set_len(s: Set[String]) -> Int { s.length() }
fn set_has(s: Set[String], x: String) -> Bool { s.contains(x) }
fn set_add(s: Set[String], x: String) -> Set[String] { s.insert(x) }
fn set_list(s: Set[String]) -> List[String] { s.toList() }
fn kset_add(s: Set[Bytes], x: Bytes) -> Set[Bytes] { s.insert(x) }
fn kset_del(s: Set[Bytes], x: Bytes) -> Set[Bytes] { s.remove(x) }
fn map_len(m: Map[String, Int]) -> Int { m.length() }
fn map_keys(m: Map[String, Int]) -> List[String] { m.keys() }
fn map_values(m: Map[String, Int]) -> List[Int] { m.values() }
fn map_get(m: Map[String, Int], k: String) -> Option[Int] { m.get(k) }
fn map_put(m: Map[String, Int], k: String, v: Int) -> Map[String, Int] { m.insert(k, v) }
fn kmap_put(m: Map[Bytes, Int], k: Bytes, v: Int) -> Map[Bytes, Int] { m.insert(k, v) }
"#;

/// Every container op over identifiers, in one context: the exact lines.
#[test]
fn the_container_kernels_print_as_nodes() {
    let text = mem(NODES);
    for line in [
        r#"return ((__o: Option<number>, __f: (x: number) => number) => __o.tag === "Some" ? Some(__f(__o.value)) : None)(o, f);"#,
        r#"return ((__o: Option<number>, __f: (x: number) => Option<number>) => __o.tag === "Some" ? __f(__o.value) : None)(o, f);"#,
        r#"return ((__o: Option<number>, __d: number) => __o.tag === "Some" ? __o.value : __d)(o, d);"#,
        r#"return (o.tag === "Some");"#,
        r#"return (o.tag === "None");"#,
        r#"return ((__o: Option<number>, __e: string) => __o.tag === "Some" ? Ok(__o.value) : Err(__e))(o, e);"#,
        r#"return ((__o: Option<number>, __f: () => Option<number>) => __o.tag === "Some" ? __o : __f())(o, f);"#,
        r#"return ((__o: Option<number>, __p: (x: number) => boolean) => __o.tag === "Some" && __p(__o.value) ? __o : None)(o, p);"#,
        r#"return ((__r: Result<number, string>, __f: (x: number) => number) => __r.tag === "Ok" ? Ok(__f(__r.value)) : __r)(r, f);"#,
        r#"return ((__r: Result<number, string>, __f: (x: number) => Result<number, string>) => __r.tag === "Ok" ? __f(__r.value) : __r)(r, f);"#,
        r#"return ((__r: Result<number, string>, __f: (e: string) => number) => __r.tag === "Err" ? Err(__f(__r.error)) : __r)(r, f);"#,
        r#"return ((__r: Result<number, string>, __d: number) => __r.tag === "Ok" ? __r.value : __d)(r, d);"#,
        r#"return ((__r: Result<number, string>) => __r.tag === "Ok" ? Some(__r.value) : None)(r);"#,
        r#"return (r.tag === "Ok");"#,
        r#"return (r.tag === "Err");"#,
        "return s.size;",
        "return s.has(x);",
        "return (new globalThis.Set<string>(s).add(x) as ReadonlySet<string>);",
        "return ([...s] as readonly string[]);",
        "return __bynkSetInsert<globalThis.Uint8Array>(s, x);",
        "return __bynkSetRemove<globalThis.Uint8Array>(s, x);",
        "return m.size;",
        "return [...m.keys()];",
        "return [...m.values()];",
        "return ((__m: ReadonlyMap<string, number>, __k: string) => __m.has(__k) ? Some(__m.get(__k) as number) : None)(m, k);",
        "return new globalThis.Map(m).set(k, v);",
        "return __bynkMapInsert<globalThis.Uint8Array, number>(m, k, v);",
    ] {
        assert_line(&text, line);
    }
    // The receiver is bracketed only where precedence needs it.
    assert!(
        !text.contains("(s).size") && !text.contains("(m).size") && !text.contains("(m).keys()"),
        "{text}"
    );
}

/// 1A.30 review (nit): a receiver that is itself a node but not an identifier. A conditional
/// receiver is bracketed under the member access, and the whole expression stays a node. The
/// source's own parentheses lower to a `Paren` node and the printer brackets the conditional
/// again, so the receiver prints doubly bracketed: today's bytes, pinned as they are.
#[test]
fn a_conditional_receiver_is_bracketed_and_stays_a_node() {
    let src = r#"context ops.jobs

fn pick(c: Bool, a: Set[String], b: Set[String]) -> Int { (if c { a } else { b }).length() }
"#;
    let text = mem(src);
    assert_line(&text, "return ((c ? a : b)).size;");
}
