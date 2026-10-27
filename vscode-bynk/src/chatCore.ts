// #2465: the pure core of the `@bynk` chat participant (`chatParticipant.ts`) —
// what each slash command gathers from the toolchain, the prompt it builds, the
// tool-calling loop against the model, and the sources it cites. No `vscode`
// import, so the unit tests drive it with a stubbed model (see
// tsconfig.test.json).
//
// DECISION A: the model is whichever the user picked in chat
// (`request.model`); the extension holds no keys and makes no choice.
// DECISION B: `/fix` returns its edit as a proposal (`FixProposal`) that the
// caller shows as a diff behind a button; nothing here changes a file.
// DECISION C: every answer cites its sources. They are collected from the
// toolchain's own output (Book links, `path:line` locations), not taken from
// the model's say-so, and an answer with none says so.

/** One part of a model's streamed reply. */
export type ModelPart =
  | { kind: "text"; value: string }
  | { kind: "toolCall"; id: string; name: string; input: object };

/** A conversation message the core builds; the glue maps it onto the API. */
export type ChatMsg =
  | { role: "user" | "assistant"; text: string }
  | { role: "assistant"; toolCall: { id: string; name: string; input: object } }
  | { role: "user"; toolResult: { id: string; text: string } };

/** The model behind `request.model`, reduced to what the core needs. */
export interface ModelLike {
  send(
    messages: readonly ChatMsg[],
    toolNames: readonly string[],
  ): Promise<AsyncIterable<ModelPart>>;
}

/** A diagnostic in the editor, 1-based, with a workspace-relative path. */
export interface EditorDiagnostic {
  path: string;
  line: number;
  column: number;
  severity: "error" | "warning" | "info" | "hint";
  code: string | undefined;
  message: string;
}

/** What the editor shows the participant when a command is run. */
export interface EditorContext {
  /** The active `.bynk` file, workspace-relative. */
  file?: string;
  /** The cursor's (or selection start's) 1-based line. */
  line?: number;
  selectedText?: string;
  /** Diagnostics of the active file. */
  diagnostics: readonly EditorDiagnostic[];
  /** The active file's lines, for `/fix` to quote. */
  lines?: readonly string[];
}

export interface ChatDeps {
  model: ModelLike;
  editor: EditorContext;
  /** Run a registered tool and return its text. Throws with the tool's message. */
  invokeTool(name: string, input: object): Promise<string>;
  /** Whether a tool of that name is registered. */
  hasTool(name: string): boolean;
  /** The Bynk tools offered to the model for its own calls. */
  modelTools: readonly string[];
  isCancelled(): boolean;
}

/** An edit `/fix` proposes: replace 1-based lines `startLine..endLine`. */
export interface FixProposal {
  path: string;
  startLine: number;
  endLine: number;
  replacement: string;
  original: string;
}

export interface ChatOutcome {
  /** Markdown to stream, in order. */
  markdown: string[];
  fix?: FixProposal;
  /** The command that produced it, for follow-up suggestions. */
  command: string;
  sources: string[];
}

export const BOOK_INDEX = "https://bynk-lang.org/book/";
export const SEARCH_BOOK_TOOL = "bynk_search_book";
const MAX_TOOL_ROUNDS = 4;
const MAX_EXPLAINED_CODES = 3;
const FIX_CONTEXT_LINES = 3;

/** The grounding rule (DECISION C), common to every command. */
export const SYSTEM_PROMPT = [
  "You are @bynk, a guide to the Bynk programming language inside VS Code.",
  "Bynk is young and pre-1.0: what you remember of its syntax is likely wrong or out of date.",
  "Answer only from the MATERIAL below, which comes from the installed Bynk toolchain, and from tool results.",
  "Cite the source of every claim: a Book link, a diagnostic code, or a `path:line` location, exactly as the material gives it.",
  "If the material does not answer the question, say so plainly rather than guess.",
  "Before you say any Bynk code compiles or is fixed, call the `bynk_check` tool and report what it says.",
  "Keep answers short and in British English.",
].join("\n");

const COMMAND_TASKS: Record<string, string> = {
  explain:
    "Explain the diagnostic(s) or code in the MATERIAL to a learner: what it means, why it happened here, and how to resolve it.",
  learn:
    "Teach the topic as a short lesson (a few paragraphs and one small example) using only the Book material given.",
  architecture:
    "Walk through this project's architecture: its contexts, services, agents and how they connect. Start with the shape, then the details.",
  fix: [
    "Propose a fix for the target diagnostic. The MATERIAL gives the numbered lines to replace.",
    "Reply with one sentence on the cause, then exactly one fenced ```bynk block holding the complete replacement for those lines, with no line numbers.",
    "Change as little as possible.",
  ].join(" "),
};

/** The `bynk.*` diagnostic codes mentioned in `text`, in order, without repeats. */
export function codesIn(text: string): string[] {
  const found = text.match(/\bbynk\.[a-z0-9_]+(?:\.[a-z0-9_]+)*/g) ?? [];
  return [...new Set(found)];
}

/** Book links and `path.bynk:line` locations in a tool's output. */
export function sourcesIn(text: string): string[] {
  const urls = text.match(/https:\/\/bynk-lang\.org\/[^\s)>\]"'`]+/g) ?? [];
  const locs = text.match(/[\w./-]+\.bynk:\d+(?::\d+)?/g) ?? [];
  return [...urls.map((u) => u.replace(/[.,;]+$/, "")), ...locs];
}

function addSources(into: string[], text: string): void {
  for (const s of sourcesIn(text)) if (!into.includes(s)) into.push(s);
}

/** The `Sources` footer, or the plain admission that there are none. */
export function renderSources(sources: readonly string[]): string {
  if (sources.length === 0) {
    return "\n\n*No toolchain source backed this answer; treat it with caution.*\n";
  }
  return `\n\n**Sources**\n${sources.map((s) => `- ${s}`).join("\n")}\n`;
}

/** Run the model to completion, answering its tool calls, and return its text. */
export async function converse(
  deps: ChatDeps,
  messages: ChatMsg[],
  sources: string[],
  emit: (text: string) => void,
): Promise<string> {
  let all = "";
  for (let round = 0; ; round++) {
    const calls: Extract<ModelPart, { kind: "toolCall" }>[] = [];
    // The last round offers no tools, so the model must answer.
    const tools = round < MAX_TOOL_ROUNDS ? deps.modelTools : [];
    let said = "";
    for await (const part of await deps.model.send(messages, tools)) {
      if (deps.isCancelled()) return all;
      if (part.kind === "text") {
        said += part.value;
        all += part.value;
        emit(part.value);
      } else {
        calls.push(part);
      }
    }
    if (calls.length === 0) return all;
    // A model that ignores the missing tools gets no further turn.
    if (round >= MAX_TOOL_ROUNDS) return all;
    // What the model said alongside its calls belongs to the history.
    if (said) messages.push({ role: "assistant", text: said });
    for (const call of calls) {
      let text: string;
      if (!tools.includes(call.name)) {
        // Only the tools offered may run: the prompt carries file contents, so
        // a call for any other registered tool is not one the user's model chose.
        text = `The tool ${call.name} is not available here.`;
      } else {
        try {
          text = await deps.invokeTool(call.name, call.input);
        } catch (e) {
          text = `The tool failed: ${e instanceof Error ? e.message : String(e)}`;
        }
      }
      addSources(sources, text);
      messages.push({ role: "assistant", toolCall: call });
      messages.push({ role: "user", toolResult: { id: call.id, text } });
    }
  }
}

function buildMessages(command: string, material: string, prompt: string): ChatMsg[] {
  const task = COMMAND_TASKS[command] ?? "";
  const ask = prompt.trim() ? `\n\nThe user asks: ${prompt.trim()}` : "";
  return [
    { role: "user", text: `${SYSTEM_PROMPT}\n\nTask: ${task}${ask}\n\nMATERIAL\n${material}` },
  ];
}

/** `/explain`: the toolchain's explanation of the code on the cursor's line,
 *  of a code the user typed, or of the selected symbol. */
async function gatherExplain(deps: ChatDeps, prompt: string, sources: string[]): Promise<string> {
  const { editor } = deps;
  const onLine = editor.diagnostics.filter((d) => d.line === editor.line && d.code);
  const pool = onLine.length > 0 ? onLine : editor.diagnostics.filter((d) => d.code);
  const typed = codesIn(prompt);
  const codes = [...new Set([...typed, ...pool.map((d) => d.code as string)])].slice(
    0,
    MAX_EXPLAINED_CODES,
  );
  const parts: string[] = [];
  for (const d of pool.slice(0, MAX_EXPLAINED_CODES)) {
    parts.push(`Diagnostic at ${d.path}:${d.line}:${d.column}: ${d.severity} ${d.code}: ${d.message}`);
    sources.push(`${d.path}:${d.line}`);
  }
  for (const code of codes) {
    const text = await deps.invokeTool("bynk_explain_diagnostic", { code });
    addSources(sources, text);
    parts.push(text);
  }
  if (codes.length === 0) {
    // No diagnostic to explain: describe the symbol under the cursor instead.
    const input =
      editor.file && editor.line
        ? { file: editor.file, line: editor.line, name: editor.selectedText?.trim() || prompt.trim() || undefined }
        : { name: prompt.trim() };
    if (!("file" in input) && !input.name) {
      return "";
    }
    const text = await deps.invokeTool("bynk_describe_symbol", input);
    addSources(sources, text);
    parts.push(text);
  }
  return parts.join("\n\n");
}

async function gatherLearn(deps: ChatDeps, prompt: string, sources: string[]): Promise<string> {
  if (!deps.hasTool(SEARCH_BOOK_TOOL)) return "";
  const text = await deps.invokeTool(SEARCH_BOOK_TOOL, { query: prompt.trim() });
  addSources(sources, text);
  return text;
}

async function gatherArchitecture(deps: ChatDeps, sources: string[]): Promise<string> {
  const text = await deps.invokeTool("bynk_get_architecture", deps.editor.file ? { file: deps.editor.file } : {});
  addSources(sources, text);
  return text;
}

/** The lines `/fix` offers for replacement: the target diagnostic's line and a
 *  few either side, so the model sees what surrounds the error. */
export function fixTarget(editor: EditorContext): { diag: EditorDiagnostic; start: number; end: number } | undefined {
  const errors = editor.diagnostics.filter((d) => d.severity === "error");
  const diag = errors.find((d) => d.line === editor.line) ?? errors[0];
  if (!diag || !editor.lines) return undefined;
  const start = Math.max(1, diag.line - FIX_CONTEXT_LINES);
  const end = Math.min(editor.lines.length, diag.line + FIX_CONTEXT_LINES);
  return { diag, start, end };
}

/** The first fenced code block in a reply, if any. */
export function firstCodeBlock(reply: string): string | undefined {
  const m = /```[a-zA-Z]*\r?\n([\s\S]*?)\r?\n?```/.exec(reply);
  return m ? m[1] : undefined;
}

/** A unified-style diff of the replaced lines, for the response. */
export function renderDiff(fix: FixProposal): string {
  const old = fix.original.split("\n").map((l) => `- ${l}`);
  const next = fix.replacement.split("\n").map((l) => `+ ${l}`);
  const head = `@@ ${fix.path}:${fix.startLine}-${fix.endLine} @@`;
  return ["```diff", head, ...old, ...next, "```"].join("\n");
}

/** Run one slash command to completion. `stream` receives Markdown as it is
 *  produced; the outcome carries the whole response and any proposed fix. */
export async function runCommand(
  command: string,
  prompt: string,
  deps: ChatDeps,
  stream: (markdown: string) => void,
): Promise<ChatOutcome> {
  const sources: string[] = [];
  const markdown: string[] = [];
  const say = (s: string) => {
    markdown.push(s);
    stream(s);
  };
  const done = (extra?: Partial<ChatOutcome>): ChatOutcome => ({ markdown, command, sources, ...extra });

  if (command === "learn") {
    if (!prompt.trim()) {
      say("Give a topic, for example `@bynk /learn services`.\n");
      return done();
    }
    const material = await gatherLearn(deps, prompt, sources);
    if (!material) {
      // Nothing to ground a lesson in: point at the Book rather than improvise.
      say(
        `I have no Book material for “${prompt.trim()}” to teach from, and Bynk is too young for me ` +
          `to rely on memory. Search the Book at ${BOOK_INDEX}.\n`,
      );
      sources.push(BOOK_INDEX);
      say(renderSources(sources));
      return done();
    }
    await converse(deps, buildMessages(command, material, prompt), sources, say);
    say(renderSources(sources));
    return done();
  }

  if (command === "architecture") {
    const material = await gatherArchitecture(deps, sources);
    await converse(deps, buildMessages(command, material, prompt), sources, say);
    say(renderSources(sources));
    return done();
  }

  if (command === "fix") {
    const target = fixTarget(deps.editor);
    if (!target || !deps.editor.file || !deps.editor.lines) {
      say("There is no error to fix: open a `.bynk` file whose diagnostics include an error.\n");
      return done();
    }
    const { diag, start, end } = target;
    const original = deps.editor.lines.slice(start - 1, end).join("\n");
    const numbered = deps.editor.lines
      .slice(start - 1, end)
      .map((l, i) => `${start + i}| ${l}`)
      .join("\n");
    const parts = [
      `Target: ${diag.path}:${diag.line}:${diag.column}: ${diag.code ?? "error"}: ${diag.message}`,
      `Lines ${start}-${end} of ${diag.path} (replace exactly these):\n${numbered}`,
    ];
    sources.push(`${diag.path}:${diag.line}`);
    if (diag.code) {
      const explained = await deps.invokeTool("bynk_explain_diagnostic", { code: diag.code });
      addSources(sources, explained);
      parts.push(explained);
    }
    // The reply is held back: the user sees the diff, not raw model output.
    const reply = await converse(deps, buildMessages(command, parts.join("\n\n"), prompt), sources, () => {});
    const replacement = firstCodeBlock(reply);
    const prose = reply.replace(/```[\s\S]*?```/g, "").trim();
    if (prose) say(`${prose}\n\n`);
    if (replacement === undefined) {
      say("I could not turn that into an edit; no change is proposed.\n");
      say(renderSources(sources));
      return done();
    }
    const fix: FixProposal = { path: diag.path, startLine: start, endLine: end, replacement, original };
    say(`${renderDiff(fix)}\n`);
    say(renderSources(sources));
    return done({ fix });
  }

  // /explain, and a bare `@bynk` message (treated as /explain when the editor
  // has something to explain, else a question answered from the toolchain).
  const material = await gatherExplain(deps, prompt, sources);
  if (!material) {
    say("Put the cursor on a diagnostic, select a symbol, or give a code, for example `@bynk /explain bynk.resolve.unknown_type`.\n");
    return done({ command: "explain" });
  }
  await converse(deps, buildMessages("explain", material, prompt), sources, say);
  say(renderSources(sources));
  return done({ command: "explain" });
}
