//! The eight tools `bynk mcp` serves: their definitions for `tools/list` and
//! their bodies for `tools/call`. The names, inputs and answers match the VS
//! Code language-model tools (#2463, `vscode-bynk/src/lmTools*.ts`), so an agent
//! sees one tool set wherever it runs.
//!
//! Every tool is read-only and answers from the project on disk, analysed on
//! each call through `bynk-ide`'s project analysis (the one the language server
//! uses), so it sees the file an agent has just edited.

use std::path::{Path, PathBuf};

use bynk_check::index::{SymbolKey, SymbolKind};
use bynk_syntax::span::Span;
use serde_json::{Value, json};

use super::render::{self, DiagnosticLine, Locator, MAX_CHARS, Severity};
use super::workspace::{Project, Workspace, line_col, offset_of, offset_of_name};

/// What a tool call came to.
pub enum Outcome {
    /// No tool by that name.
    Unknown,
    Text(String),
    /// The tool could not answer; the text says why and what to try.
    Failed(String),
}

/// Sent to the client at `initialize`, as a hint on when to reach for the tools.
pub const INSTRUCTIONS: &str = "Bynk is a young, pre-1.0 language, so remembered syntax is \
unreliable. These tools answer from the installed toolchain: call bynk_check (or \
bynk_get_diagnostics) after editing .bynk files, bynk_explain_diagnostic on any bynk.* code, \
and bynk_describe_symbol, bynk_get_documentation, bynk_get_architecture and \
bynk_get_wire_contract to learn a project before changing it, and bynk_search_book for the Book. Positions are 1-based and paths \
are relative to the workspace.";

const DIAGNOSTIC_INDEX: &str = "https://bynk-lang.org/book/reference/diagnostics/";
const BOOK_URL: &str = "https://bynk-lang.org";

fn tool(name: &str, title: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": format!(
            "Bynk is a young, pre-1.0 language: do not rely on remembered syntax. {description}"
        ),
        "inputSchema": { "type": "object", "properties": properties, "required": required },
        "annotations": {
            "title": title,
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}

const FILE_REL: &str = "A file path relative to the workspace root (or absolute).";

/// `tools/list`.
pub fn definitions() -> Vec<Value> {
    vec![
        tool(
            "bynk_get_diagnostics",
            "Bynk diagnostics",
            "Lists the errors and warnings in the Bynk project on disk, with workspace-relative \
             `path:line:column` locations (1-based) and their `bynk.*` codes. Call it before and \
             after editing `.bynk` files to see what is wrong and whether an edit fixed it. Pass \
             `file` for one file; omit it for every project in the workspace.",
            json!({ "file": { "type": "string", "description": FILE_REL } }),
            &[],
        ),
        tool(
            "bynk_check",
            "Check the Bynk project",
            "Type-checks the whole Bynk project and returns every error and warning with \
             `path:line:column` and its `bynk.*` code. It writes nothing. Use it to confirm the \
             project compiles after you edit it, before telling the user the code works.",
            json!({ "file": {
                "type": "string",
                "description": "Any file in the project to check; omit when the workspace holds one project.",
            } }),
            &[],
        ),
        tool(
            "bynk_explain_diagnostic",
            "Explain Bynk diagnostic",
            "Explains a Bynk compiler diagnostic code (such as `bynk.resolve.unknown_type`): what \
             it means, an example, and a link to the Bynk Book. The text comes from the installed \
             toolchain, so it matches the compiler that reported the error. Use it whenever you \
             meet a `bynk.*` code you are about to fix or explain.",
            json!({ "code": {
                "type": "string",
                "description": "The diagnostic code, e.g. `bynk.resolve.unknown_type`.",
            } }),
            &["code"],
        ),
        tool(
            "bynk_describe_symbol",
            "Describe Bynk symbol",
            "Describes a Bynk symbol: its type or signature, its doc comment, and where it is \
             defined. Give `file` and `line` (1-based) with either `column` (1-based) or the \
             symbol's `name` on that line; or give `name` (and optionally `file`, without `line`) \
             to find a declaration by name across the workspace or in that file.",
            json!({
                "file": { "type": "string", "description": FILE_REL },
                "line": { "type": "integer", "minimum": 1, "description": "1-based line in `file`." },
                "column": { "type": "integer", "minimum": 1, "description": "1-based column on `line`." },
                "name": {
                    "type": "string",
                    "description": "The symbol's name: found on `line` when `file` and `line` are \
                                    given, otherwise searched for in `file`, or across the \
                                    workspace without it.",
                },
            }),
            &[],
        ),
        tool(
            "bynk_get_documentation",
            "Bynk documentation",
            "Returns the reference documentation for the Bynk unit (context or commons) a file \
             belongs to: every type, function, capability, service, agent and handler, with its \
             signature, its doc comment and its `path:line:column`. Use it to learn what a unit \
             declares before calling or changing it.",
            json!({ "file": {
                "type": "string",
                "description": "A `.bynk` file path relative to the workspace root (or absolute).",
            } }),
            &["file"],
        ),
        tool(
            "bynk_get_architecture",
            "Bynk architecture",
            "Summarises a Bynk project's architecture: each context and adapter, the services and \
             agents it declares, the capabilities it uses or provides, and which other contexts \
             it consumes. Use it to orient yourself in a project or to answer what a context \
             exposes. `file` picks the project (any file in it); omit it when the workspace holds \
             one project.",
            json!({ "file": {
                "type": "string",
                "description": "Any `.bynk` file in the project, relative to the workspace root (or absolute).",
            } }),
            &[],
        ),
        tool(
            "bynk_search_book",
            "Search the Bynk Book",
            "Searches the Bynk Book (the language guide, reference and specification) and returns \
             the best-matching sections with their page links. The text is embedded in the \
             installed toolchain, so it matches the compiler the user runs, and every result \
             names the Book version. Use it before writing Bynk you are not sure of, and to look \
             up a concept or keyword (such as `refined types`, `agent`, `given`).",
            json!({ "query": {
                "type": "string",
                "description": "The words to look for, e.g. `refined types` or `service handler`.",
            } }),
            &["query"],
        ),
        tool(
            "bynk_get_wire_contract",
            "Bynk wire contract",
            "Describes the wire contract of the Bynk service handler at a position: its route or \
             trigger, the request parameters and their types, the HTTP responses it can produce, \
             and the boundary types with their fields and validation. Use it to write a client, a \
             test, or a change that must stay compatible. Give a `line` inside the handler.",
            json!({
                "file": {
                    "type": "string",
                    "description": "A `.bynk` file path relative to the workspace root (or absolute).",
                },
                "line": { "type": "integer", "minimum": 1, "description": "A 1-based line inside the handler." },
                "column": { "type": "integer", "minimum": 1, "description": "Optional 1-based column." },
            }),
            &["file", "line"],
        ),
    ]
}

/// `tools/call`.
pub fn call(ws: &Workspace, name: &str, args: &Value) -> Outcome {
    let result = match name {
        "bynk_get_diagnostics" => get_diagnostics(ws, args),
        "bynk_check" => check(ws, args),
        "bynk_explain_diagnostic" => explain(args),
        "bynk_describe_symbol" => describe_symbol(ws, args),
        "bynk_get_documentation" => get_documentation(ws, args),
        "bynk_get_architecture" => get_architecture(ws, args),
        "bynk_get_wire_contract" => get_wire_contract(ws, args),
        "bynk_search_book" => search_book(args),
        _ => return Outcome::Unknown,
    };
    match result {
        Ok(text) => Outcome::Text(text),
        Err(text) => Outcome::Failed(text),
    }
}

// -- arguments

fn opt_str(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
        Some(Value::String(_)) => Ok(None),
        Some(_) => Err(format!("`{key}` must be a string.")),
    }
}

fn req_str(args: &Value, key: &str) -> Result<String, String> {
    opt_str(args, key)?.ok_or_else(|| format!("`{key}` is required."))
}

fn opt_pos(args: &Value, key: &str) -> Result<Option<usize>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n >= 1 => Ok(Some(n as usize)),
            _ => Err(format!("`{key}` must be an integer of at least 1.")),
        },
    }
}

// -- shared lookups

/// The project that holds `file`, analysed, with `file` as a project-relative path.
fn project_of(ws: &Workspace, file: &str) -> Result<(Project, PathBuf), String> {
    let path = ws.resolve_file(file)?;
    let root = ws.pick_project(Some(file))?;
    let project = ws.analyse(&root);
    let rel = project
        .rel(&path)
        .ok_or_else(|| format!("`{file}` is not inside its project."))?;
    if project.text(&rel).is_none() {
        return Err(format!(
            "`{file}` is not one of the project's analysed `.bynk` files (outside its `[paths]`, \
             or not a Bynk source)."
        ));
    }
    Ok((project, rel))
}

fn diagnostic_lines(ws: &Workspace, project: &Project, only: Option<&Path>) -> Vec<DiagnosticLine> {
    let mut out = Vec::new();
    let mut push = |path: String, text: Option<&str>, d: &bynk_ide::Diagnostic| {
        let (line, column) = text.map_or((1, 1), |t| line_col(t, d.error.span.start));
        out.push(DiagnosticLine {
            path,
            line,
            column,
            severity: match d.severity {
                bynk_syntax::error::Severity::Error => Severity::Error,
                bynk_syntax::error::Severity::Warning => Severity::Warning,
            },
            code: Some(d.error.category.to_string()),
            message: d.error.message.clone(),
        });
    };
    for f in &project.analysis.files {
        if only.is_some_and(|o| o != f.source_path) {
            continue;
        }
        for d in &f.diagnostics {
            push(project.display(ws, &f.source_path), Some(&f.text), d);
        }
    }
    if only.is_none() {
        // Project-level problems (a malformed manifest, a unit cycle) belong to
        // no single file; they are reported against the manifest.
        for d in &project.analysis.unattributed {
            push(project.display(ws, Path::new("bynk.toml")), None, d);
        }
    }
    out
}

// -- the tools

fn get_diagnostics(ws: &Workspace, args: &Value) -> Result<String, String> {
    let file = opt_str(args, "file")?;
    let mut lines = Vec::new();
    let scope;
    if let Some(file) = &file {
        let (project, rel) = project_of(ws, file)?;
        lines = diagnostic_lines(ws, &project, Some(&rel));
        scope = format!("in {}", project.display(ws, &rel));
    } else {
        let roots = ws.project_roots();
        if roots.is_empty() {
            return Err("No `bynk.toml` found in this workspace.".to_string());
        }
        for root in roots {
            lines.extend(diagnostic_lines(ws, &ws.analyse(&root), None));
        }
        scope = "in the workspace".to_string();
    }
    Ok(render::cap_text(
        &render::diagnostics(&lines, &scope),
        MAX_CHARS,
        Some("Pass `file` to see one file."),
    ))
}

fn check(ws: &Workspace, args: &Value) -> Result<String, String> {
    let root = ws.pick_project(opt_str(args, "file")?.as_deref())?;
    let project = ws.analyse(&root);
    let lines = diagnostic_lines(ws, &project, None);
    let where_ = ws.display(&root);
    let scope = if where_ == "." {
        "from `bynk check`".to_string()
    } else {
        format!("from `bynk check` in {where_}")
    };
    Ok(render::cap_text(
        &render::diagnostics(&lines, &scope),
        MAX_CHARS,
        None,
    ))
}

fn explain(args: &Value) -> Result<String, String> {
    let code = req_str(args, "code")?;
    let code = code.trim();
    Ok(match bynk_syntax::diagnostics::explain(code) {
        Some(e) => render::cap_text(
            &render::explanation(e.code, e.blurb, e.example, &e.href()),
            MAX_CHARS,
            None,
        ),
        None => format!(
            "No extended explanation is written for `{code}` yet, or it is not a code this \
             compiler emits. The full index of codes: {DIAGNOSTIC_INDEX}\n"
        ),
    })
}

/// The hover text and definition site(s) at a byte offset, laddered as the
/// language server's hover is for the cases an agent asks about: a resolved
/// symbol (described from its defining file), then a local or parameter, then
/// a declaration of that name anywhere in the project.
fn describe_at(
    ws: &Workspace,
    project: &Project,
    rel: &Path,
    offset: usize,
) -> (Vec<String>, Vec<String>) {
    let a = &project.analysis;
    let at = Locator { ws, project };
    let Some(text) = project.text(rel) else {
        return (vec![], vec![]);
    };
    if let Some((key, _)) = a.index.symbol_at(rel, offset)
        && let Some(def) = a.index.symbols.get(key).and_then(|e| e.def.as_ref())
        && let Some(def_text) = project.text(&def.path)
        && let Some(hover) = bynk_ide::symbols::describe_symbol(def_text, &key.name)
    {
        return (vec![hover], vec![at.at(&def.path, def.span)]);
    }
    if let Some(locals) = a.locals.get(rel)
        && let Some(hover) = bynk_ide::locals_nav::describe_local_at(locals, text, offset)
    {
        return (vec![hover], vec![]);
    }
    // The lexical fallback: the word under the cursor, found as a declaration.
    let Some(word) = word_at(text, offset) else {
        return (vec![], vec![]);
    };
    let mut files: Vec<_> = a.files.iter().collect();
    files.sort_by(|x, y| x.source_path.cmp(&y.source_path));
    for f in files {
        if let Some(hover) = bynk_ide::symbols::describe_symbol(&f.text, word) {
            let defs = bynk_ide::symbols::find_declaration_span(&f.text, word)
                .map(|span| vec![at.at(&f.source_path, span)])
                .unwrap_or_default();
            return (vec![hover], defs);
        }
    }
    (vec![], vec![])
}

/// The identifier under `offset`, if any.
fn word_at(text: &str, offset: usize) -> Option<&str> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let offset = offset.min(text.len());
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| offset + i);
    (start < end).then(|| &text[start..end])
}

fn describe_symbol(ws: &Workspace, args: &Value) -> Result<String, String> {
    let file = opt_str(args, "file")?;
    let line = opt_pos(args, "line")?;
    let column = opt_pos(args, "column")?;
    let name = opt_str(args, "name")?;
    if let (Some(file), Some(line)) = (&file, line) {
        let (project, rel) = project_of(ws, file)?;
        let text = project.text(&rel).unwrap_or_default();
        let offset = match (column, &name) {
            (Some(c), _) => offset_of(text, line, Some(c))?,
            (None, Some(n)) => offset_of_name(text, line, n)?,
            (None, None) => offset_of(text, line, None)?,
        };
        let (hovers, defs) = describe_at(ws, &project, &rel, offset);
        let at = Locator {
            ws,
            project: &project,
        }
        .at(&rel, Span::new(offset, offset));
        return Ok(render::cap_text(
            &render::symbol(&at, &hovers, &defs),
            MAX_CHARS,
            None,
        ));
    }
    let Some(name) = name else {
        return Err("Give `name`, or `file` and `line` (with `column` or `name`).".to_string());
    };
    // `file` without `line` narrows the search to that file.
    let (projects, only): (Vec<Project>, Option<PathBuf>) = match &file {
        Some(f) => {
            let (project, rel) = project_of(ws, f)?;
            (vec![project], Some(rel))
        }
        None => (
            ws.project_roots().iter().map(|r| ws.analyse(r)).collect(),
            None,
        ),
    };
    let needle = name.to_lowercase();
    // (project index, key, definition file, definition span); exact names win.
    let mut exact: Vec<(usize, SymbolKey, PathBuf, Span)> = Vec::new();
    let mut partial: Vec<(usize, SymbolKey, PathBuf, Span)> = Vec::new();
    for (i, project) in projects.iter().enumerate() {
        for (key, entry) in &project.analysis.index.symbols {
            let Some(def) = &entry.def else { continue };
            if only.as_ref().is_some_and(|rel| *rel != def.path) {
                continue;
            }
            let hit = (i, key.clone(), def.path.clone(), def.span);
            if key.name == name || key.name.ends_with(&format!(".{name}")) {
                exact.push(hit);
            } else if key.name.to_lowercase().contains(&needle) {
                partial.push(hit);
            }
        }
    }
    let mut matches = if exact.is_empty() { partial } else { exact };
    matches.sort_by(|a, b| (&a.1.name, &a.1.unit, a.0).cmp(&(&b.1.name, &b.1.unit, b.0)));
    if matches.is_empty() {
        let where_ = match (&only, projects.first()) {
            (Some(rel), Some(p)) => p.display(ws, rel),
            _ => "the workspace".to_string(),
        };
        return Ok(format!("No Bynk symbol named `{name}` in {where_}.\n"));
    }
    let mut parts = Vec::new();
    for (i, _key, path, span) in matches.iter().take(5) {
        let project = &projects[*i];
        let (hovers, defs) = describe_at(ws, project, path, span.start);
        let at = Locator { ws, project }.at(path, *span);
        parts.push(render::symbol(&at, &hovers, &defs));
    }
    let tail = if matches.len() > 5 {
        format!(
            "\n{} more match(es) not shown; give `file` and `line`.\n",
            matches.len() - 5
        )
    } else {
        String::new()
    };
    Ok(render::cap_text(
        &(parts.join("\n") + &tail),
        MAX_CHARS,
        None,
    ))
}

/// `bynk_search_book`: the Book embedded in this toolchain, searched by the same
/// code as `bynk book --search`. Unlike the VS Code tool there is no hosted
/// fallback: this server never reaches the network, so a build made without the
/// bundle says so and points at the hosted Book.
fn search_book(args: &Value) -> Result<String, String> {
    let query = req_str(args, "query")?;
    let Some(book) = crate::book::embedded() else {
        return Err(format!(
            "No Book is bundled in this build of `bynk`. Read it at {BOOK_URL}/book/ (or \
             {BOOK_URL}/llms-full.txt).\n"
        ));
    };
    Ok(render::cap_text(
        &crate::book::render_markdown(&book, query.trim()),
        MAX_CHARS,
        None,
    ))
}

fn get_documentation(ws: &Workspace, args: &Value) -> Result<String, String> {
    let file = req_str(args, "file")?;
    let (project, rel) = project_of(ws, &file)?;
    let a = &project.analysis;
    let text = project.text(&rel).unwrap_or_default();
    let snapshots = project.snapshots();
    let siblings: Vec<(&Path, &str)> = bynk_ide::symbols::own_declaration_name(text)
        .and_then(|(name, _)| a.unit_sources.get(&name))
        .into_iter()
        .flatten()
        .filter_map(|p| snapshots.get(p).map(|t| (p.as_path(), t.as_str())))
        .collect();
    let Some(model) = bynk_ide::documentation::documentation_model_merged((&rel, text), siblings)
    else {
        return Ok(format!(
            "No documentation for {}: it is not part of a Bynk project, or the project has not \
             been analysed yet.\n",
            project.display(ws, &rel)
        ));
    };
    let at = Locator {
        ws,
        project: &project,
    };
    let locs: Vec<String> = model
        .entries
        .iter()
        .map(|e| at.at(e.file.as_deref().unwrap_or(&rel), e.span))
        .collect();
    Ok(render::cap_text(
        &render::documentation(&model, &locs),
        MAX_CHARS,
        None,
    ))
}

fn get_architecture(ws: &Workspace, args: &Value) -> Result<String, String> {
    let root = ws.pick_project(opt_str(args, "file")?.as_deref())?;
    let project = ws.analyse(&root);
    let a = &project.analysis;
    let model = bynk_ide::architecture::architecture_model(
        &a.unit_sources,
        &project.snapshots(),
        &a.sequence_info,
    );
    if model.nodes.is_empty() {
        return Ok(format!(
            "No architecture for the project at {}: it declares no context or adapter, or the \
             project has not been analysed yet.\n",
            ws.display(&root)
        ));
    }
    let at = Locator {
        ws,
        project: &project,
    };
    Ok(render::cap_text(
        &render::architecture(&model, &at),
        MAX_CHARS,
        None,
    ))
}

fn get_wire_contract(ws: &Workspace, args: &Value) -> Result<String, String> {
    let file = req_str(args, "file")?;
    let line = opt_pos(args, "line")?.ok_or("`line` is required.")?;
    let column = opt_pos(args, "column")?;
    let (project, rel) = project_of(ws, &file)?;
    let a = &project.analysis;
    let text = project.text(&rel).unwrap_or_default();
    let offset = offset_of(text, line, column)?;
    let none = || {
        format!(
            "No service handler at {}:{line}. Give a line inside a service's `on …` handler.\n",
            project.display(ws, &rel)
        )
    };
    let Some((unit, _)) = bynk_ide::symbols::own_declaration_name(text) else {
        return Ok(none());
    };
    let Some(info) = a.boundary_info.get(&unit) else {
        return Ok(none());
    };
    let expr_types = a.expr_types.get(&rel).map_or(&[][..], |v| v.as_slice());
    let context_count =
        bynk_ide::wire_contract::real_context_count(&a.boundary_info, &a.unit_sources);
    let Some(model) = bynk_ide::wire_contract::wire_contract_at(
        &unit,
        text,
        offset,
        info,
        expr_types,
        &a.ty_intern,
        context_count,
    ) else {
        return Ok(none());
    };
    // A boundary type's declaration site: searched through the unit's own
    // scope order (itself, its `uses`, its `consumes`), as the editor does.
    let order = a
        .doc_scope
        .get(&unit)
        .cloned()
        .unwrap_or_else(|| vec![unit.clone()]);
    let at = Locator {
        ws,
        project: &project,
    };
    let type_locs = |name: &str| {
        order.iter().find_map(|u| {
            let key = SymbolKey {
                unit: u.clone(),
                kind: SymbolKind::Type,
                name: name.to_string(),
            };
            let def = a.index.symbols.get(&key)?.def.as_ref()?;
            Some(at.at(&def.path, def.span))
        })
    };
    let handler_at = at.at(&rel, model.handler_span);
    Ok(render::cap_text(
        &render::wire_contract(&model, &handler_at, &type_locs),
        MAX_CHARS,
        None,
    ))
}
