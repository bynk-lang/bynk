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
    let wire = bynk_lsp::agent_state_request::to_wire(&agent, &model, ORDERS_SRC);
    assert_eq!(wire.agent, "Order");
    let names: Vec<&str> = wire.handlers.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["place", "cancel"]);
    assert_eq!(wire.machines.len(), 1);
    let m = &wire.machines[0];
    assert_eq!(m.states, ["Pending", "Placed", "Cancelled"]);
    assert_eq!(m.initial.as_deref(), Some("Pending"));
    assert_eq!(m.reachable, ["Pending", "Placed", "Cancelled"]);
    assert_eq!(m.edges.len(), 6, "three paths per handler");

    // The wire shape is what the client mirrors: camelCase, tagged targets.
    let json = serde_json::to_value(&wire).unwrap();
    assert_eq!(json["machines"][0]["fieldRange"]["start"]["line"], 8);
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
