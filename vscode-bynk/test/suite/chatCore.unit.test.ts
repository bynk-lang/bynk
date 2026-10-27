// #2465: unit coverage for `chatCore` (src/chatCore.ts), the pure core of the
// `@bynk` chat participant, driven with a stubbed model — no extension host.

import * as assert from "assert";

import {
  type ChatDeps,
  type ChatMsg,
  type EditorContext,
  type ModelPart,
  codesIn,
  firstCodeBlock,
  runCommand,
  sourcesIn,
} from "../../src/chatCore";

const EXPLANATION =
  "# `bynk.resolve.unknown_type`\n\nA type name is not in scope.\n\nBook: https://bynk-lang.org/book/reference/diagnostics/#bynk-resolve-unknown-type\n";

interface Stub {
  deps: ChatDeps;
  sent: ChatMsg[][];
  toolsOffered: (readonly string[])[];
  invoked: { name: string; input: object }[];
}

/** A model that replies with each script entry in turn. */
function stub(script: ModelPart[][], editor: EditorContext, tools: Record<string, string> = {}): Stub {
  const sent: ChatMsg[][] = [];
  const toolsOffered: (readonly string[])[] = [];
  const invoked: { name: string; input: object }[] = [];
  let turn = 0;
  const deps: ChatDeps = {
    model: {
      async send(messages, toolNames) {
        sent.push([...messages]);
        toolsOffered.push(toolNames);
        const parts = script[turn++] ?? [];
        return (async function* () {
          yield* parts;
        })();
      },
    },
    editor,
    async invokeTool(name, input) {
      invoked.push({ name, input });
      if (!(name in tools)) throw new Error(`no ${name}`);
      return tools[name];
    },
    hasTool: (name) => name in tools,
    modelTools: ["bynk_check"],
    isCancelled: () => false,
  };
  return { deps, sent, toolsOffered, invoked };
}

const text = (value: string): ModelPart => ({ kind: "text", value });

const broken: EditorContext = {
  file: "src/a.bynk",
  line: 3,
  diagnostics: [
    { path: "src/a.bynk", line: 3, column: 10, severity: "error", code: "bynk.resolve.unknown_type", message: "unknown type `Usr`" },
  ],
  lines: ["commons a", "", "fn f(u: Usr) -> Int {", "  1", "}"],
};

describe("chatCore (#2465)", () => {
  it("finds diagnostic codes and sources in text", () => {
    assert.deepStrictEqual(codesIn("see bynk.a.b and bynk.a.b, bynk.x_y"), ["bynk.a.b", "bynk.x_y"]);
    assert.deepStrictEqual(sourcesIn(EXPLANATION + "at src/a.bynk:3:10."), [
      "https://bynk-lang.org/book/reference/diagnostics/#bynk-resolve-unknown-type",
      "src/a.bynk:3:10",
    ]);
  });

  it("takes the first fenced block of a reply", () => {
    assert.strictEqual(firstCodeBlock("Because.\n```bynk\nfn f() {}\n```\nmore"), "fn f() {}");
    assert.strictEqual(firstCodeBlock("no code"), undefined);
  });

  it("/explain quotes the toolchain's explanation and cites the Book link", async () => {
    const s = stub([[text("The type `Usr` is not in scope.")]], broken, { bynk_explain_diagnostic: EXPLANATION });
    const out = await runCommand("explain", "", s.deps, () => {});
    assert.deepStrictEqual(s.invoked, [{ name: "bynk_explain_diagnostic", input: { code: "bynk.resolve.unknown_type" } }]);
    // The explanation reaches the model as material, ahead of its answer.
    const prompt = (s.sent[0][0] as { text: string }).text;
    assert.ok(prompt.includes("A type name is not in scope."));
    assert.ok(prompt.includes("Answer only from the MATERIAL"));
    const md = out.markdown.join("");
    assert.ok(md.includes("The type `Usr` is not in scope."));
    assert.ok(md.includes("**Sources**"));
    assert.ok(out.sources.includes("https://bynk-lang.org/book/reference/diagnostics/#bynk-resolve-unknown-type"));
    assert.ok(md.includes("src/a.bynk:3"));
  });

  it("/explain with nothing to explain asks for something, and does not call the model", async () => {
    const s = stub([], { diagnostics: [] });
    const out = await runCommand("explain", "", s.deps, () => {});
    assert.strictEqual(s.sent.length, 0);
    assert.ok(out.markdown.join("").includes("Put the cursor on a diagnostic"));
  });

  it("answers the model's tool calls and feeds the results back", async () => {
    const s = stub(
      [
        [{ kind: "toolCall", id: "c1", name: "bynk_check", input: {} }],
        [text("It compiles: `bynk check` found no problems.")],
      ],
      broken,
      { bynk_explain_diagnostic: EXPLANATION, bynk_check: "No problems from `bynk check`.\n" },
    );
    const out = await runCommand("explain", "", s.deps, () => {});
    assert.deepStrictEqual(s.invoked.map((i) => i.name), ["bynk_explain_diagnostic", "bynk_check"]);
    assert.strictEqual(s.sent.length, 2);
    assert.ok(s.sent[1].some((m) => "toolResult" in m && m.toolResult.id === "c1"));
    assert.ok(out.markdown.join("").includes("It compiles"));
  });

  it("stops offering tools after a bounded number of rounds", async () => {
    const call: ModelPart[] = [{ kind: "toolCall", id: "c", name: "bynk_check", input: {} }];
    const s = stub([call, call, call, call, call, [text("done")]], broken, {
      bynk_explain_diagnostic: EXPLANATION,
      bynk_check: "ok\n",
    });
    await runCommand("explain", "", s.deps, () => {});
    assert.deepStrictEqual(s.toolsOffered.at(-1), []);
  });

  it("refuses a tool call for a tool that was not offered", async () => {
    const s = stub(
      [[{ kind: "toolCall", id: "c1", name: "other_extension_tool", input: {} }], [text("Done.")]],
      broken,
      { bynk_explain_diagnostic: EXPLANATION, other_extension_tool: "secret\n" },
    );
    await runCommand("explain", "", s.deps, () => {});
    assert.deepStrictEqual(s.invoked.map((i) => i.name), ["bynk_explain_diagnostic"]);
    const result = s.sent[1].find((m) => "toolResult" in m) as { toolResult: { text: string } };
    assert.ok(result.toolResult.text.includes("not available"));
  });

  it("stops after a bounded number of rounds even if the model keeps calling tools", async () => {
    const call: ModelPart[] = [{ kind: "toolCall", id: "c", name: "bynk_check", input: {} }];
    const s = stub(Array.from({ length: 50 }, () => call), broken, {
      bynk_explain_diagnostic: EXPLANATION,
      bynk_check: "ok\n",
    });
    await runCommand("explain", "", s.deps, () => {});
    assert.ok(s.sent.length <= 6, `${s.sent.length} model turns`);
  });

  it("keeps the text a model says alongside its tool calls", async () => {
    const s = stub(
      [[text("Checking. "), { kind: "toolCall", id: "c1", name: "bynk_check", input: {} }], [text("Done.")]],
      broken,
      { bynk_explain_diagnostic: EXPLANATION, bynk_check: "ok\n" },
    );
    await runCommand("explain", "", s.deps, () => {});
    assert.ok(s.sent[1].some((m) => "text" in m && m.role === "assistant" && m.text === "Checking. "));
  });

  it("reports a failed tool call to the model instead of throwing", async () => {
    const s = stub(
      [[{ kind: "toolCall", id: "c1", name: "bynk_check", input: {} }], [text("Could not check.")]],
      broken,
      { bynk_explain_diagnostic: EXPLANATION },
    );
    await runCommand("explain", "", s.deps, () => {});
    const result = s.sent[1].find((m) => "toolResult" in m) as { toolResult: { text: string } };
    assert.ok(result.toolResult.text.startsWith("The tool failed"));
  });

  it("/learn uses the Book search tool when it exists", async () => {
    const book = "## Services\nA service handles requests.\nBook: https://bynk-lang.org/book/services/\n";
    const s = stub([[text("A service handles requests.")]], { diagnostics: [] }, { bynk_search_book: book });
    const out = await runCommand("learn", "services", s.deps, () => {});
    assert.deepStrictEqual(s.invoked, [{ name: "bynk_search_book", input: { query: "services" } }]);
    assert.deepStrictEqual(out.sources, ["https://bynk-lang.org/book/services/"]);
  });

  it("/learn without the Book search tool says so and links the Book, without asking the model", async () => {
    const s = stub([], { diagnostics: [] });
    const out = await runCommand("learn", "services", s.deps, () => {});
    assert.strictEqual(s.sent.length, 0);
    const md = out.markdown.join("");
    assert.ok(md.includes("no Book material"));
    assert.deepStrictEqual(out.sources, ["https://bynk-lang.org/book/"]);
    assert.ok(md.includes("**Sources**"));
  });

  it("/learn needs a topic", async () => {
    const s = stub([], { diagnostics: [] });
    const out = await runCommand("learn", "  ", s.deps, () => {});
    assert.ok(out.markdown.join("").includes("Give a topic"));
  });

  it("/architecture walks the toolchain's architecture", async () => {
    const arch = "# Architecture\nContext `shop` at src/shop.bynk:1:1\n";
    const s = stub([[text("There is one context, shop.")]], broken, { bynk_get_architecture: arch });
    const out = await runCommand("architecture", "", s.deps, () => {});
    assert.deepStrictEqual(s.invoked, [{ name: "bynk_get_architecture", input: { file: "src/a.bynk" } }]);
    assert.ok(out.markdown.join("").includes("src/shop.bynk:1:1"));
  });

  it("/fix proposes the edit as a diff and applies nothing", async () => {
    const reply = "The type is misspelt.\n```bynk\ncommons a\n\nfn f(u: User) -> Int {\n  1\n}\n```";
    const s = stub([[text(reply)]], broken, { bynk_explain_diagnostic: EXPLANATION });
    const streamed: string[] = [];
    const out = await runCommand("fix", "", s.deps, (m) => streamed.push(m));
    assert.deepStrictEqual(out.fix, {
      path: "src/a.bynk",
      startLine: 1,
      endLine: 5,
      replacement: "commons a\n\nfn f(u: User) -> Int {\n  1\n}",
      original: broken.lines!.join("\n"),
    });
    const md = out.markdown.join("");
    assert.ok(md.includes("```diff"));
    assert.ok(md.includes("- fn f(u: Usr) -> Int {"));
    assert.ok(md.includes("+ fn f(u: User) -> Int {"));
    // The model's raw reply is not streamed; the diff is.
    assert.ok(!streamed.join("").includes("```bynk"));
  });

  it("/fix proposes nothing when the reply has no code, or no error exists", async () => {
    const s = stub([[text("I am not sure.")]], broken, { bynk_explain_diagnostic: EXPLANATION });
    const out = await runCommand("fix", "", s.deps, () => {});
    assert.strictEqual(out.fix, undefined);
    assert.ok(out.markdown.join("").includes("no change is proposed"));
    const none = await runCommand("fix", "", stub([], { diagnostics: [] }).deps, () => {});
    assert.ok(none.markdown.join("").includes("no error to fix"));
  });

  it("admits when nothing backs an answer", async () => {
    const s = stub([[text("Something.")]], { file: "x.bynk", line: 1, diagnostics: [] }, { bynk_describe_symbol: "Nothing known.\n" });
    const out = await runCommand("explain", "thing", s.deps, () => {});
    assert.ok(out.markdown.join("").includes("No toolchain source backed this answer"));
  });
});
