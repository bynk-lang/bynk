//! #2196: `bynk/agentStateModel` — the agent state view's request, driven
//! against a real analysis round (the type table comes from `boundary_info`,
//! as the server reads it), plus the `Show State` lens sites and the
//! camelCase params.

struct Scratch(std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str, files: &[(&str, &str)]) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "bynk_lsp_agent_state_request_{tag}_{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, body) in files {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }
    Scratch(dir)
}

const ORDERS_SRC: &str = r#"context orders

type OrderStatus = enum { Pending, Placed, Cancelled }
type OrderError  = enum { AlreadyPlaced, AlreadyCancelled }

agent Order {
  key id: String

  store status: Cell[OrderStatus] = Pending

  on call place() -> Effect[Result[(), OrderError]] {
    match status {
      Pending => {
        status := Placed
        Ok(())
      }
      Placed    => Err(AlreadyPlaced)
      Cancelled => Err(AlreadyCancelled)
    }
  }

  on call cancel() -> Effect[Result[(), OrderError]] {
    match status {
      Pending => {
        status := Cancelled
        Ok(())
      }
      Placed    => Err(AlreadyPlaced)
      Cancelled => Err(AlreadyCancelled)
    }
  }
}

agent Counter {
  key id: String

  store total: Cell[Int]

  on call bump() -> Effect[Int] {
    total := total + 1
    total
  }
}
"#;

fn types(root: &std::path::Path) -> bynk_ide::ContextBoundaryInfo {
    let roots = bynk_ide::AnalysisRoots::SingleTree(root.to_path_buf());
    let sources = bynk_testkit::read_project_sources(&roots);
    let diag = bynk_ide::diagnose_project_with(&roots, &sources, &bynk_ide::SchemaLock::Off);
    diag.boundary_info
        .get("orders")
        .expect("boundary_info for orders")
        .clone()
}

#[test]
fn the_cursor_anywhere_in_an_agent_selects_its_whole_state_model() {
    let root = scratch("anywhere", &[("orders.bynk", ORDERS_SRC)]);
    let info = types(&root.0);
    // A cursor inside `cancel`'s body still answers for the whole agent.
    let offset = ORDERS_SRC.find("status := Cancelled").unwrap();
    let (agent, model) =
        bynk_lsp::agent_state_request::agent_state_at(ORDERS_SRC, offset, &info.types)
            .expect("an agent encloses this offset");
    let wire = bynk_lsp::agent_state_request::to_wire(&agent, &model, &[], ORDERS_SRC);
    assert_eq!(wire.agent, "Order");
    let names: Vec<&str> = wire.handlers.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["place", "cancel"]);
    assert_eq!(wire.machines.len(), 1);
    let m = &wire.machines[0];
    assert_eq!(m.states, ["Pending", "Placed", "Cancelled"]);
    assert_eq!(m.initial.as_deref(), Some("Pending"));
    assert_eq!(m.reachable, ["Pending", "Placed", "Cancelled"]);
    // #2219: nothing leaves an order `Pending` after a commit, so only the
    // genesis commit ever moves out of it.
    assert_eq!(m.reachable_after_commit, ["Placed", "Cancelled"]);
    assert_eq!(m.edges.len(), 6, "three paths per handler");

    // The wire shape is what the client mirrors: camelCase, tagged targets.
    let json = serde_json::to_value(&wire).unwrap();
    assert_eq!(json["machines"][0]["fieldRange"]["start"]["line"], 8);
    assert_eq!(
        json["machines"][0]["reachableAfterCommit"],
        serde_json::json!(["Placed", "Cancelled"])
    );
    assert_eq!(json["machines"][0]["edges"][0]["to"]["kind"], "State");
    assert_eq!(json["machines"][0]["edges"][0]["to"]["state"], "Placed");
    assert_eq!(json["machines"][0]["edges"][1]["to"]["kind"], "Unchanged");
    // The edge range is the write, for click-to-code.
    let write_line = ORDERS_SRC[..ORDERS_SRC.find("status := Placed").unwrap()]
        .matches('\n')
        .count();
    assert_eq!(
        json["machines"][0]["edges"][0]["range"]["start"]["line"],
        write_line
    );
}

#[test]
fn an_agent_with_no_state_field_answers_with_no_machines() {
    let root = scratch("no_machine", &[("orders.bynk", ORDERS_SRC)]);
    let info = types(&root.0);
    let offset = ORDERS_SRC.find("total := total").unwrap();
    let (agent, model) =
        bynk_lsp::agent_state_request::agent_state_at(ORDERS_SRC, offset, &info.types)
            .expect("Counter encloses this offset");
    assert_eq!(agent.name.name, "Counter");
    assert!(model.machines.is_empty());
}

#[test]
fn outside_every_agent_there_is_nothing_to_show() {
    let offset = ORDERS_SRC.find("type OrderError").unwrap();
    let types = std::collections::HashMap::new();
    assert!(bynk_lsp::agent_state_request::agent_state_at(ORDERS_SRC, offset, &types).is_none());
}

#[test]
fn there_is_one_show_state_lens_per_agent_on_its_name() {
    let sites = bynk_lsp::agent_state_request::agent_lens_sites(ORDERS_SRC);
    let names: Vec<&str> = sites.iter().map(|s| &ORDERS_SRC[s.start..s.end]).collect();
    assert_eq!(names, ["Order", "Counter"]);
}

#[test]
fn params_deserialize_from_the_camelcase_wire_shape() {
    let p: bynk_lsp::agent_state_request::AgentStateModelParams =
        serde_json::from_value(serde_json::json!({
            "textDocument": { "uri": "file:///p/orders.bynk" },
            "position": { "line": 3, "character": 1 }
        }))
        .expect("camelCase params deserialize");
    assert_eq!(p.position.line, 3);
}

/// #2197: a `transition` an edge can violate crosses the wire on the edge,
/// with its certainty and range; one outside the fragment is listed.
#[test]
fn a_violated_transition_crosses_the_wire_on_its_edge() {
    let src = r#"context commerce

type OrderStatus = enum { Pending, Placed, Paid }

agent Order {
  key id: String

  store status: Cell[OrderStatus] = Pending

  transition paid_is_terminal: old.status is Paid implies new.status is Paid
  transition odd: if old.status == old.status { true } else { false }

  on call place() -> Effect[()] {
    status := Placed
  }

  on call pay() -> Effect[()] {
    status := Paid
  }
}
"#;
    let root = scratch("violation", &[("commerce.bynk", src)]);
    let roots = bynk_ide::AnalysisRoots::SingleTree(root.0.clone());
    let sources = bynk_testkit::read_project_sources(&roots);
    let diag = bynk_ide::diagnose_project_with(&roots, &sources, &bynk_ide::SchemaLock::Off);
    let info = diag.boundary_info.get("commerce").expect("boundary_info");
    let offset = src.find("status := Placed").unwrap();
    let (agent, model) = bynk_lsp::agent_state_request::agent_state_at(src, offset, &info.types)
        .expect("Order encloses this offset");
    let json = serde_json::to_value(bynk_lsp::agent_state_request::to_wire(
        &agent,
        &model,
        &[],
        src,
    ))
    .unwrap();
    let m = &json["machines"][0];
    let v = &m["edges"][0]["violations"][0];
    assert_eq!(v["transition"], "paid_is_terminal");
    assert_eq!(v["certainty"], "Known");
    assert_eq!(v["from"], serde_json::json!(["Paid"]));
    assert_eq!(v["targets"], serde_json::json!(["Placed"]));
    let decl_line = src[..src.find("transition paid_is_terminal").unwrap()]
        .matches('\n')
        .count();
    assert_eq!(v["range"]["start"]["line"], decl_line);
    assert_eq!(m["edges"][1]["violations"], serde_json::json!([]));
    assert_eq!(m["notEvaluated"][0]["name"], "odd");
}

/// #2198: an agent's handler effects cross the wire, writes with their ranges.
#[test]
fn handler_effects_cross_the_wire() {
    let root = scratch("effects", &[("orders.bynk", ORDERS_SRC)]);
    let info = types(&root.0);
    let offset = ORDERS_SRC.find("total := total").unwrap();
    let (agent, model) =
        bynk_lsp::agent_state_request::agent_state_at(ORDERS_SRC, offset, &info.types).unwrap();
    let agents = ["Order", "Counter"].iter().map(|s| s.to_string()).collect();
    let effects = bynk_lsp::agent_state_request::effects_of(&agent, &agents);
    let json = serde_json::to_value(bynk_lsp::agent_state_request::to_wire(
        &agent, &model, &effects, ORDERS_SRC,
    ))
    .unwrap();
    let bump = &json["effects"][0];
    assert_eq!(bump["handler"], "bump");
    assert_eq!(bump["truncated"], false);
    let row = &bump["rows"][0];
    assert_eq!(row["writes"][0]["field"], "total");
    assert_eq!(row["writes"][0]["op"], ":=");
    let line = ORDERS_SRC[..offset].matches('\n').count();
    assert_eq!(row["writes"][0]["range"]["start"]["line"], line);
    assert_eq!(row["reads"], serde_json::json!(["total"]));
    assert_eq!(row["outcomes"], serde_json::json!(["total"]));
}

const INVARIANTS_SRC: &str = r#"context commerce

type OrderStatus = enum { Pending, Placed, Paid }

agent Order {
  key id: String

  store status: Cell[OrderStatus] = Pending
  store paymentRef: Cell[Option[String]] = None

  ---
  A paid order carries its payment reference.
  ---
  invariant paid_has_ref:
    status is Paid
      implies   paymentRef.isSome()

  transition paid_is_terminal: old.status is Paid implies new.status is Paid

  on call place() -> Effect[()] {
    status := Placed
  }
}

agent Plain {
  key id: String

  store total: Cell[Int]

  on call bump() -> Effect[Int] {
    total := total + 1
    total
  }
}
"#;

/// The wire JSON of the agent enclosing `needle` in `src`, a one-file context
/// named `commerce`.
fn wire_json(tag: &str, src: &str, needle: &str) -> serde_json::Value {
    let root = scratch(tag, &[("commerce.bynk", src)]);
    let roots = bynk_ide::AnalysisRoots::SingleTree(root.0.clone());
    let sources = bynk_testkit::read_project_sources(&roots);
    let diag = bynk_ide::diagnose_project_with(&roots, &sources, &bynk_ide::SchemaLock::Off);
    let info = diag.boundary_info.get("commerce").expect("boundary_info");
    let offset = src.find(needle).unwrap();
    let (agent, model) = bynk_lsp::agent_state_request::agent_state_at(src, offset, &info.types)
        .expect("an agent encloses this offset");
    serde_json::to_value(bynk_lsp::agent_state_request::to_wire(
        &agent,
        &model,
        &[],
        src,
    ))
    .unwrap()
}

/// #3608: both kinds cross the wire in one list, with formatted predicate
/// text, the declaration's range and its documentation.
#[test]
fn invariants_and_transitions_cross_the_wire_in_one_list() {
    let json = wire_json("inv_both", INVARIANTS_SRC, "status := Placed");
    let list = json["invariants"].as_array().expect("an invariants array");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["kind"], "invariant");
    assert_eq!(list[0]["name"], "paid_has_ref");
    // The source spans three lines with irregular spacing; the text is not that.
    assert_eq!(
        list[0]["predicate"],
        "status is Paid implies paymentRef.isSome()"
    );
    assert_eq!(
        list[0]["doc"],
        "A paid order carries its payment reference."
    );
    let line = |needle: &str| {
        INVARIANTS_SRC[..INVARIANTS_SRC.find(needle).unwrap()]
            .matches('\n')
            .count()
    };
    assert_eq!(
        list[0]["range"]["start"]["line"],
        line("invariant paid_has_ref")
    );
    assert_eq!(list[1]["kind"], "transition");
    assert_eq!(list[1]["name"], "paid_is_terminal");
    assert_eq!(
        list[1]["predicate"],
        "old.status is Paid implies new.status is Paid"
    );
    assert_eq!(
        list[1]["range"]["start"]["line"],
        line("transition paid_is_terminal")
    );
    // No documentation: the key is omitted, not null.
    assert!(list[1].get("doc").is_none());
}

/// #3608: an agent with neither kind omits the field entirely.
#[test]
fn an_agent_with_no_invariants_omits_the_field() {
    let json = wire_json("inv_none", INVARIANTS_SRC, "total := total");
    assert!(json.get("invariants").is_none(), "{json}");
}

/// #3608: a context split across files keeps each agent's declarations with
/// that agent. An agent is one declaration in one file, so there is nothing to
/// merge, and one file's request never lists another file's declarations.
#[test]
fn a_context_split_across_files_keeps_each_agents_declarations_apart() {
    let a = r#"context shop

type Stage = enum { Open, Closed }

agent Cart {
  key id: String

  store stage: Cell[Stage] = Open

  invariant cart_has_stage: stage is Open implies stage is Open

  on call close() -> Effect[()] {
    stage := Closed
  }
}
"#;
    let b = r#"context shop

type Phase = enum { Idle, Busy }

agent Worker {
  key id: String

  store phase: Cell[Phase] = Idle

  transition busy_stays_busy: old.phase is Busy implies new.phase is Busy

  on call start() -> Effect[()] {
    phase := Busy
  }
}
"#;
    let root = scratch("inv_split", &[("shop_a.bynk", a), ("shop_b.bynk", b)]);
    let roots = bynk_ide::AnalysisRoots::SingleTree(root.0.clone());
    let sources = bynk_testkit::read_project_sources(&roots);
    let diag = bynk_ide::diagnose_project_with(&roots, &sources, &bynk_ide::SchemaLock::Off);
    let info = diag.boundary_info.get("shop").expect("boundary_info");
    let wire = |src: &str, needle: &str| {
        let offset = src.find(needle).unwrap();
        let (agent, model) =
            bynk_lsp::agent_state_request::agent_state_at(src, offset, &info.types).unwrap();
        serde_json::to_value(bynk_lsp::agent_state_request::to_wire(
            &agent,
            &model,
            &[],
            src,
        ))
        .unwrap()
    };
    let cart = wire(a, "stage := Closed");
    let worker = wire(b, "phase := Busy");
    assert_eq!(cart["invariants"].as_array().unwrap().len(), 1);
    assert_eq!(cart["invariants"][0]["kind"], "invariant");
    assert_eq!(cart["invariants"][0]["name"], "cart_has_stage");
    assert_eq!(worker["invariants"].as_array().unwrap().len(), 1);
    assert_eq!(worker["invariants"][0]["kind"], "transition");
    assert_eq!(worker["invariants"][0]["name"], "busy_stays_busy");
}

const CART_SRC: &str = r#"context orders

type Line = { sku: String, qty: Int }

agent Cart {
  key id: String

  store latest: Cell[Line]
  store lines: Cell[List[Line]]
  store blob: Cell[Bytes]
  store prices: Map[String, Int]
  store files: Map[String, Bytes]
  store skus: Set[String]
  store audit: Log[Line]

  on call add(line: Line, data: Bytes) -> Effect[()] {
    latest := line
    lines := [line]
    blob := data
    let _ <- prices.put(line.sku, line.qty)
    let _ <- files.put(line.sku, data)
    let _ <- skus.add(line.sku)
    let _ <- audit.append(line)
    let _ <- prices.remove(line.sku)
    ()
  }
}
"#;

/// #3803: each write says what it stores: the value, and the field once the write has happened.
#[test]
fn writes_report_the_type_they_store() {
    let root = scratch("value_types", &[("orders.bynk", CART_SRC)]);
    let info = types(&root.0);
    let offset = CART_SRC.find("latest := line").unwrap();
    let (agent, model) =
        bynk_lsp::agent_state_request::agent_state_at(CART_SRC, offset, &info.types).unwrap();
    let effects = bynk_lsp::agent_state_request::effects_of(&agent, &Default::default());
    let json = serde_json::to_value(bynk_lsp::agent_state_request::to_wire_with_types(
        &agent,
        &model,
        &effects,
        CART_SRC,
        &info.types,
        (Vec::new(), Vec::new()),
    ))
    .unwrap();
    let writes = json["effects"][0]["rows"][0]["writes"].as_array().unwrap();
    let by = |field: &str, op: &str| {
        writes
            .iter()
            .find(|w| w["field"] == field && w["op"] == op)
            .unwrap_or_else(|| panic!("no {op} write to {field} in {writes:?}"))
    };

    // A set on a record: the cell's own type, as value and field.
    let latest = by("latest", ":=");
    assert_eq!(latest["valueType"], "Line");
    assert_eq!(latest["fieldType"], "Line");
    assert_eq!(latest["valueRef"]["kind"], "Named");
    assert_eq!(latest["valueRef"]["name"], "Line");
    assert_eq!(latest["fieldRef"]["name"], "Line");

    // A set on a list: a generic instantiation, referenced by its codec key.
    let lines = by("lines", ":=");
    assert_eq!(lines["valueType"], "List[Line]");
    assert_eq!(lines["valueRef"]["kind"], "Inst");
    assert_eq!(lines["valueRef"]["key"], "List_Line");

    // A set on `Bytes`: unbounded, and its own reference kind.
    let blob = by("blob", ":=");
    assert_eq!(blob["valueType"], "Bytes");
    assert_eq!(blob["valueRef"]["kind"], "Bytes");

    // A put on a map: the value written and the map it went into.
    let prices = by("prices", "put");
    assert_eq!(prices["valueType"], "Int");
    assert_eq!(prices["fieldType"], "Map[String, Int]");
    assert_eq!(prices["valueRef"]["kind"], "Base");
    assert!(prices.get("fieldRef").is_none(), "a map has no value shape");
    let files = by("files", "put");
    assert_eq!(files["valueType"], "Bytes");
    assert_eq!(files["fieldType"], "Map[String, Bytes]");
    assert_eq!(files["valueRef"]["kind"], "Bytes");

    // An append to a log and an add to a set: the element and the collection.
    let audit = by("audit", "append");
    assert_eq!(audit["valueType"], "Line");
    assert_eq!(audit["fieldType"], "Log[Line]");
    let skus = by("skus", "add");
    assert_eq!(skus["valueType"], "String");
    assert_eq!(skus["fieldType"], "Set[String]");

    // A remove stores no value, and says what the field still is.
    let removed = by("prices", "remove");
    assert!(removed.get("valueType").is_none(), "{removed}");
    assert!(removed.get("valueRef").is_none(), "{removed}");
    assert_eq!(removed["fieldType"], "Map[String, Int]");
}

/// #3803: the writes' references point into the wire contract's own type table.
#[test]
fn the_type_table_is_the_wire_contracts() {
    let root = scratch("value_table", &[("orders.bynk", CART_SRC)]);
    let info = types(&root.0);
    let offset = CART_SRC.find("latest := line").unwrap();
    let (agent, model) =
        bynk_lsp::agent_state_request::agent_state_at(CART_SRC, offset, &info.types).unwrap();
    let index = bynk_check::index::ProjectIndex::default();
    let table = bynk_lsp::wire_contract_request::store_type_table(
        &agent,
        &info.types,
        &root.0,
        &index,
        &Default::default(),
        &["orders".to_string()],
    );
    let json = serde_json::to_value(bynk_lsp::agent_state_request::to_wire_with_types(
        &agent,
        &model,
        &[],
        CART_SRC,
        &info.types,
        table,
    ))
    .unwrap();
    let names: Vec<&str> = json["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Line"), "{names:?}");
    let keys: Vec<&str> = json["instantiations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"List_Line"), "{keys:?}");
}
