---
title: "`bynk mcp`"
---
`bynk mcp` is a [Model Context Protocol](https://modelcontextprotocol.io/) server
that lets an AI agent ask the installed toolchain about Bynk instead of relying
on what it remembers. Bynk is young and pre-1.0, so a model's training data on it
is thin and dated; these tools give it exact, version-matched answers: the
diagnostics a file has, what a code means, a symbol's type, a unit's
documentation, a project's architecture, and a handler's wire contract.

It speaks MCP over **stdio**, so a client launches it as a subprocess. It is the
same set of tools the [VS Code extension](/docs/tooling/vscode-bynk/) offers to
chat agents inside the editor, for every agent outside it.

```text
bynk mcp [PATH]
```

`PATH` is the workspace the tools may read (default: the current directory).
Clients normally start the server in the project directory; pass `PATH` when
yours does not.

## Tools

Every tool is read-only: none writes a file, runs a subprocess or reaches the
network. Paths in inputs and outputs are relative to `PATH` and lines and columns
are 1-based. A `file` argument must be inside `PATH`; one that resolves outside
it (a `..`, an absolute path elsewhere, a symlink out) is refused.

| Tool | What it returns |
|---|---|
| `bynk_get_diagnostics` | The errors and warnings, with their `bynk.*` codes, for one `file` or every project in the workspace. |
| `bynk_check` | The whole project's diagnostics, as `bynk check` reports them. Run it after editing, before saying the code works. |
| `bynk_explain_diagnostic` | A diagnostic `code`'s explanation, an example and a link to the Book (what [`bynk explain`](/docs/bynk-cli/#bynk-explain) prints). |
| `bynk_describe_symbol` | A symbol's type or signature, doc comment and definition, at `file` + `line` (+ `column` or `name`), or found by `name`. |
| `bynk_get_documentation` | Every declaration of the unit a `file` belongs to, with signature, doc comment and location. |
| `bynk_get_architecture` | The project's contexts and adapters, their services, agents and capabilities, and the `consumes` edges between them. |
| `bynk_get_wire_contract` | The route, request envelope, HTTP responses and boundary types of the service handler at `file` + `line`. |

Results are compact Markdown with `path:line:column` locations, capped at about
12,000 characters; a result that was cut says so and how to ask for less.

Each call analyses the project from the files **on disk**, so an agent that has
just edited a file sees the edit, and unsaved editor buffers are not seen. Where
a workspace holds several projects (several `bynk.toml`), `file` picks the one
you mean.

## Set up a client

Install the `bynk` driver first (see the [README](https://github.com/bynk-lang/bynk#install)),
and check that `bynk mcp` is on the `PATH` the client launches with. If it is
not, give the full path to the binary in the snippets below.

### Claude Code

```sh
claude mcp add bynk -- bynk mcp
```

Add `--scope project` to share the setting through the repository's `.mcp.json`.

### Claude Desktop

In `claude_desktop_config.json` (Settings → Developer → Edit Config), pass the
project directory, because Claude Desktop does not start servers in one:

```json
{
  "mcpServers": {
    "bynk": {
      "command": "bynk",
      "args": ["mcp", "/path/to/your/project"]
    }
  }
}
```

### VS Code (any MCP-capable agent)

You do not need this for Copilot's agent mode: the Bynk extension already offers
the same tools. For another MCP client in VS Code, add `.vscode/mcp.json`:

```json
{
  "servers": {
    "bynk": {
      "type": "stdio",
      "command": "bynk",
      "args": ["mcp"]
    }
  }
}
```

### Cursor

In `.cursor/mcp.json` (or `~/.cursor/mcp.json` for every project):

```json
{
  "mcpServers": {
    "bynk": {
      "command": "bynk",
      "args": ["mcp"]
    }
  }
}
```

### Codex CLI

In `~/.codex/config.toml`:

```toml
[mcp_servers.bynk]
command = "bynk"
args = ["mcp"]
```

### Any other client

Launch `bynk mcp` as a stdio server from the project directory. The server
answers `initialize`, `ping`, `tools/list` and `tools/call`, and negotiates the
protocol revision (2025-06-18, 2025-03-26 or 2024-11-05). It writes protocol
messages to standard output and nothing else; anything meant for a person goes to
standard error.

## How it differs from the editor

The tools answer from the same analysis the [language server](/docs/tooling/bynk-lsp/)
uses, so diagnostics, documentation, architecture and wire contracts agree with
the editor. Two differences:

- the editor sees unsaved buffers; `bynk mcp` sees saved files;
- `bynk_describe_symbol` covers the resolved-symbol, local and
  declaration-by-name cases of hover, not every hover case (a `store` operation
  or a keyword, say, answers "nothing is known").
