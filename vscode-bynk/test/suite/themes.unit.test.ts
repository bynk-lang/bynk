// The "Bynk Dark" and "Bynk Light" colour themes (themes/*.json): contributed by the
// manifest, covering every scope the TextMate grammar emits and every semantic token
// type the language server and manifest declare, in step with each other, and
// readable (WCAG AA, 4.5:1) on their own backgrounds. Pure file checks, no VS Code.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

const ROOT = path.resolve(__dirname, "../../..");
const read = (rel: string) => JSON.parse(fs.readFileSync(path.join(ROOT, rel), "utf8"));

interface ThemeFile {
  name: string;
  type: "dark" | "light";
  semanticHighlighting?: boolean;
  colors: Record<string, string>;
  tokenColors: { name?: string; scope: string | string[]; settings: { foreground?: string; fontStyle?: string } }[];
  semanticTokenColors: Record<string, unknown>;
}

const pkg = read("package.json");
const contributed: { label: string; uiTheme: string; path: string }[] = pkg.contributes.themes ?? [];
const EXPECTED = [
  { label: "Bynk Dark", uiTheme: "vs-dark", type: "dark" },
  { label: "Bynk Light", uiTheme: "vs", type: "light" },
] as const;

const load = (label: string): ThemeFile => {
  const entry = contributed.find((t) => t.label === label);
  assert.ok(entry, `${label} is contributed`);
  return read(entry.path.replace(/^\.\//, ""));
};

// Every scope the grammar names, from its `name`, `contentName` and capture names.
function grammarScopes(): Set<string> {
  const scopes = new Set<string>();
  const walk = (node: unknown, key = ""): void => {
    if (Array.isArray(node)) return node.forEach((n) => walk(n));
    if (node === null || typeof node !== "object") return;
    for (const [k, v] of Object.entries(node as Record<string, unknown>)) {
      if ((k === "name" || k === "contentName") && typeof v === "string") v.split(" ").forEach((s) => scopes.add(s));
      walk(v, k);
    }
  };
  walk(read("syntaxes/bynk.tmLanguage.json"));
  return scopes;
}

const ruleScopes = (t: ThemeFile): string[] => t.tokenColors.flatMap((r) => (Array.isArray(r.scope) ? r.scope : [r.scope]));
const covers = (rule: string, scope: string) => scope === rule || scope.startsWith(`${rule}.`);

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
const contrast = (a: string, b: string) => {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

describe("Bynk colour themes", () => {
  it("are contributed as a dark and a light theme, each file present and parseable", () => {
    for (const want of EXPECTED) {
      const entry = contributed.find((t) => t.label === want.label);
      assert.ok(entry, `${want.label} is contributed`);
      assert.strictEqual(entry.uiTheme, want.uiTheme);
      const theme = load(want.label);
      assert.strictEqual(theme.name, want.label, "the file's name matches the contributed label");
      assert.strictEqual(theme.type, want.type);
    }
  });

  for (const want of EXPECTED) {
    describe(want.label, () => {
      const theme = () => load(want.label);

      it("opts in to semantic highlighting, or the language server's colours never show", () => {
        assert.strictEqual(theme().semanticHighlighting, true);
      });

      it("colours every TextMate scope the grammar emits, bar the structural meta.* ones", () => {
        const rules = ruleScopes(theme());
        const missing = [...grammarScopes()]
          .filter((s) => s.endsWith(".bynk") && !s.startsWith("meta."))
          .filter((s) => !rules.some((r) => covers(r, s)));
        assert.deepStrictEqual(missing, [], `scopes with no token colour rule: ${missing.join(", ")}`);
      });

      it("colours every semantic token type the manifest declares", () => {
        const declared: string[] = pkg.contributes.semanticTokenTypes.map((t: { id: string }) => t.id);
        const have = Object.keys(theme().semanticTokenColors);
        const missing = declared.filter((id) => !have.includes(id));
        assert.deepStrictEqual(missing, [], `semantic token types with no colour: ${missing.join(", ")}`);
      });

      it("keeps every token colour readable on the editor background (4.5:1)", () => {
        const t = theme();
        const bg = t.colors["editor.background"];
        const low = t.tokenColors
          .filter((r) => r.settings.foreground)
          .map((r) => ({ scope: r.scope, ratio: contrast(r.settings.foreground!, bg) }))
          .filter((x) => x.ratio < 4.5);
        assert.deepStrictEqual(low, [], "token colours below 4.5:1");
        for (const [key, value] of Object.entries(t.semanticTokenColors)) {
          const fg = typeof value === "string" ? value : (value as { foreground?: string }).foreground;
          if (fg) assert.ok(contrast(fg, bg) >= 4.5, `semantic ${key} ${fg} on ${bg}`);
        }
      });

      it("keeps the main workbench text readable on its backgrounds (4.5:1)", () => {
        const c = theme().colors;
        const pairs: [string, string][] = [
          ["editor.foreground", "editor.background"],
          ["editorLineNumber.foreground", "editor.background"],
          ["editorLineNumber.foreground", "sideBar.background"],
          ["sideBar.foreground", "sideBar.background"],
          ["activityBar.inactiveForeground", "activityBar.background"],
          ["statusBar.foreground", "statusBar.background"],
          ["tab.activeForeground", "tab.activeBackground"],
          ["tab.inactiveForeground", "tab.inactiveBackground"],
          ["button.foreground", "button.background"],
          ["badge.foreground", "badge.background"],
          ["input.placeholderForeground", "input.background"],
          ["list.activeSelectionForeground", "list.activeSelectionBackground"],
        ];
        for (const [fg, bg] of pairs) {
          assert.ok(contrast(c[fg], c[bg]) >= 4.5, `${fg} (${c[fg]}) on ${bg} (${c[bg]}) is ${contrast(c[fg], c[bg]).toFixed(2)}:1`);
        }
      });
    });
  }

  it("the two themes colour the same scopes and semantic token types", () => {
    const [dark, light] = EXPECTED.map((e) => load(e.label));
    assert.deepStrictEqual([...new Set(ruleScopes(dark))].sort(), [...new Set(ruleScopes(light))].sort());
    assert.deepStrictEqual(Object.keys(dark.semanticTokenColors).sort(), Object.keys(light.semanticTokenColors).sort());
    assert.deepStrictEqual(Object.keys(dark.colors).sort(), Object.keys(light.colors).sort());
  });
});
