// #2465: the `@bynk` chat participant — `/explain`, `/learn`, `/architecture`
// and `/fix`. It orchestrates the language-model tools of #2463 through
// `vscode.lm.invokeTool` (with the request's `toolInvocationToken`, so the chat
// UI shows each call) rather than repeating them; what each command gathers,
// the prompt and the citation rule are in `chatCore.ts`.
//
// DECISION A: the model is `request.model`, the one the user picked in chat.
// DECISION B: `/fix` shows its edit as a diff behind an **Apply fix** button;
// the click applies it, nothing applies it silently.
// DECISION C: answers end with the sources the toolchain gave, or say they
// have none.
//
// It needs the native language server and the CLI behind those tools, so it is
// desktop-only (`!isWeb` in the manifest); the web entry doesn't register it.

import * as vscode from "vscode";

import {
  type ChatDeps,
  type ChatMsg,
  type EditorContext,
  type FixProposal,
  type ModelLike,
  type ModelPart,
  runCommand,
} from "./chatCore";
import { relPath } from "./lmToolsCore";

const PARTICIPANT_ID = "bynk.chat";
export const APPLY_FIX_COMMAND = "bynk.chat.applyFix";

/** The tools the model may call itself while answering. */
const MODEL_TOOLS = [
  "bynk_check",
  "bynk_get_diagnostics",
  "bynk_describe_symbol",
  "bynk_explain_diagnostic",
  "bynk_search_book",
];

const SEVERITY: Record<vscode.DiagnosticSeverity, "error" | "warning" | "info" | "hint"> = {
  [vscode.DiagnosticSeverity.Error]: "error",
  [vscode.DiagnosticSeverity.Warning]: "warning",
  [vscode.DiagnosticSeverity.Information]: "info",
  [vscode.DiagnosticSeverity.Hint]: "hint",
};

function editorContext(): EditorContext {
  const editor = vscode.window.activeTextEditor;
  const doc = editor?.document;
  if (!editor || !doc || doc.languageId !== "bynk") return { diagnostics: [] };
  const path = relPath(doc.uri);
  const diagnostics = vscode.languages.getDiagnostics(doc.uri).map((d) => {
    const code = typeof d.code === "object" && d.code !== null ? d.code.value : d.code;
    return {
      path,
      line: d.range.start.line + 1,
      column: d.range.start.character + 1,
      severity: SEVERITY[d.severity],
      code: code === undefined ? undefined : String(code),
      message: d.message,
    };
  });
  return {
    file: path,
    line: editor.selection.active.line + 1,
    selectedText: editor.selection.isEmpty ? undefined : doc.getText(editor.selection),
    diagnostics,
    lines: doc.getText().split(/\r?\n/),
  };
}

/** `request.model` as the core's `ModelLike`. */
function modelLike(model: vscode.LanguageModelChat, token: vscode.CancellationToken): ModelLike {
  return {
    async send(messages, toolNames) {
      const mapped = messages.map(toApiMessage);
      const tools = vscode.lm.tools
        .filter((t) => toolNames.includes(t.name))
        .map((t) => ({ name: t.name, description: t.description, inputSchema: t.inputSchema }));
      const response = await model.sendRequest(mapped, { tools }, token);
      return (async function* (): AsyncGenerator<ModelPart> {
        for await (const part of response.stream) {
          if (part instanceof vscode.LanguageModelTextPart) {
            yield { kind: "text", value: part.value };
          } else if (part instanceof vscode.LanguageModelToolCallPart) {
            yield { kind: "toolCall", id: part.callId, name: part.name, input: part.input };
          }
        }
      })();
    },
  };
}

function toApiMessage(m: ChatMsg): vscode.LanguageModelChatMessage {
  if ("toolCall" in m) {
    const { id, name, input } = m.toolCall;
    return vscode.LanguageModelChatMessage.Assistant([new vscode.LanguageModelToolCallPart(id, name, input)]);
  }
  if ("toolResult" in m) {
    return vscode.LanguageModelChatMessage.User([
      new vscode.LanguageModelToolResultPart(m.toolResult.id, [new vscode.LanguageModelTextPart(m.toolResult.text)]),
    ]);
  }
  return m.role === "user"
    ? vscode.LanguageModelChatMessage.User(m.text)
    : vscode.LanguageModelChatMessage.Assistant(m.text);
}

function depsFor(request: vscode.ChatRequest, token: vscode.CancellationToken): ChatDeps {
  return {
    model: modelLike(request.model, token),
    editor: editorContext(),
    async invokeTool(name, input) {
      const result = await vscode.lm.invokeTool(
        name,
        { input, toolInvocationToken: request.toolInvocationToken },
        token,
      );
      return result.content
        .filter((p): p is vscode.LanguageModelTextPart => p instanceof vscode.LanguageModelTextPart)
        .map((p) => p.value)
        .join("");
    },
    hasTool: (name) => vscode.lm.tools.some((t) => t.name === name),
    modelTools: MODEL_TOOLS.filter((n) => vscode.lm.tools.some((t) => t.name === n)),
    isCancelled: () => token.isCancellationRequested,
  };
}

async function handle(
  request: vscode.ChatRequest,
  _context: vscode.ChatContext,
  stream: vscode.ChatResponseStream,
  token: vscode.CancellationToken,
): Promise<vscode.ChatResult> {
  try {
    const outcome = await runCommand(request.command ?? "explain", request.prompt, depsFor(request, token), (md) =>
      stream.markdown(md),
    );
    if (outcome.fix) {
      const doc = vscode.window.activeTextEditor?.document;
      if (doc) {
        stream.button({
          command: APPLY_FIX_COMMAND,
          title: "Apply fix",
          arguments: [doc.uri.toString(), outcome.fix],
        });
      }
    }
    return { metadata: { command: outcome.command, fixed: outcome.fix !== undefined } };
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    stream.markdown(`I could not complete that: ${message}\n`);
    return { errorDetails: { message } };
  }
}

/** Apply a `/fix` proposal after the user clicks its button — and only if the
 *  lines still read as they did when it was proposed. */
async function applyFix(uri: string, fix: FixProposal): Promise<void> {
  const doc = await vscode.workspace.openTextDocument(vscode.Uri.parse(uri));
  const range = new vscode.Range(fix.startLine - 1, 0, fix.endLine - 1, doc.lineAt(fix.endLine - 1).range.end.character);
  if (doc.getText(range).replace(/\r\n/g, "\n") !== fix.original) {
    void vscode.window.showWarningMessage("The file changed since @bynk proposed this fix; ask again.");
    return;
  }
  const edit = new vscode.WorkspaceEdit();
  edit.replace(doc.uri, range, fix.replacement);
  await vscode.workspace.applyEdit(edit);
  void vscode.window.showInformationMessage("Fix applied. Run `bynk check` (or ask @bynk) to confirm it compiles.");
}

const FOLLOWUPS: Record<string, vscode.ChatFollowup[]> = {
  explain: [{ label: "Propose a fix", command: "fix", prompt: "" }],
  fix: [{ label: "Explain the diagnostic", command: "explain", prompt: "" }],
  learn: [{ label: "Walk through this project", command: "architecture", prompt: "" }],
  architecture: [{ label: "Learn about services", command: "learn", prompt: "services" }],
};

export function registerChatParticipant(context: vscode.ExtensionContext): void {
  const participant = vscode.chat.createChatParticipant(PARTICIPANT_ID, handle);
  participant.iconPath = vscode.Uri.joinPath(context.extensionUri, "assets", "bynk-icon.png");
  participant.followupProvider = {
    provideFollowups: (result) => FOLLOWUPS[String(result.metadata?.command)] ?? [],
  };
  context.subscriptions.push(
    participant,
    vscode.commands.registerCommand(APPLY_FIX_COMMAND, applyFix),
  );
}
