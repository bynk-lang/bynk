// #852: end-to-end coverage for the sequence-diagram webview's participant
// click-to-code zip. The unit tests (`mermaid-gen.unit.test.ts`) only pin the
// generated Mermaid *text*; the bug this guards lives one layer later, in how
// `main.ts` maps the *rendered* SVG's elements back to participants. Mermaid 11
// emits several `.actor`-classed nodes per participant (see
// `src/webview/participant-map.ts`), so the old `querySelectorAll(".actor")[i]`
// walk mapped participant `i` to an unrelated node.
//
// Verifying that needs a real Mermaid render, i.e. a real webview browser
// context — out of reach for a pure unit test. So this drives the actual
// bundle (`out/webview.js`) inside a live VS Code webview with a known payload,
// sets the `__BYNK_SEQUENCE_DEBUG__` seam so `main.ts` reports the label its
// zip landed on for each participant, and asserts every participant's element
// really is its own box (rendered label === declared name). Under the old zip
// the reported labels are empty/shuffled and this fails.

import * as assert from "assert";

import * as vscode from "vscode";

import type { SequenceModel, SequencePayload } from "../../src/webview/types";

const EXT_ID = "bynk.bynk-vscode";

const R = (line: number) => ({
  start: { line, character: 0 },
  end: { line, character: 4 },
});

// Five participants with distinct, single-word names so a mis-zip (element for
// participant `i` belonging to some other participant, or to a label-less
// `rect`) surfaces as a label≠name mismatch. Deliberately MIXED: a principal
// `Actor` (stick-figure → `.actor-man`) at slot 0, an `Entry` (box, no range —
// not clickable but still rendered), and `Capability` boxes. Mermaid emits the
// stick-figure nodes at the END of DOM order and the boxes regrouped out of
// declaration order, so this only passes if the zip recovers declaration order
// across BOTH element kinds (the crux of #852, now that #876 ships actors).
const MODEL: SequenceModel = {
  participants: [
    { id: 0, kind: "Actor", name: "Caller", range: R(0) },
    { id: 1, kind: "Entry", name: "Entry", range: null },
    { id: 2, kind: "Capability", name: "Clock", range: R(2) },
    { id: 3, kind: "Capability", name: "Store", range: R(3) },
    { id: 4, kind: "Capability", name: "Logger", range: R(4) },
  ],
  messages: [
    { from: 0, to: 1, kind: "Call", label: "check", range: R(1), block: null },
    { from: 1, to: 2, kind: "Call", label: "now", range: R(2), block: null },
    { from: 2, to: 1, kind: "Return", label: "", range: R(2), block: null },
    { from: 1, to: 3, kind: "Call", label: "get", range: R(3), block: null },
    { from: 3, to: 1, kind: "Return", label: "", range: R(3), block: null },
    { from: 1, to: 4, kind: "Send", label: "info", range: R(4), block: null },
  ],
  blocks: [],
};

interface WiringReport {
  type: "wiring";
  participants: { name: string; label: string | null }[];
}

function nonce(): string {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let out = "";
  for (let i = 0; i < 32; i++) out += chars.charAt(Math.floor(Math.random() * chars.length));
  return out;
}

/** The production HTML shell (`webviewHost.renderWebviewHtml`) embeds only the
 *  model global; this mirrors it but also sets the `__BYNK_SEQUENCE_DEBUG__`
 *  seam so the bundle reports its wiring back. Keep the CSP/nonce in sync with
 *  `webviewHost.renderWebviewHtml` — the two shells are intentionally parallel,
 *  differing only in the extra debug global. */
function debugHtml(webview: vscode.Webview, scriptUri: vscode.Uri, payload: SequencePayload): string {
  const n = nonce();
  const json = JSON.stringify(payload).replace(/</g, "\\u003c");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src ${webview.cspSource} data:; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${n}';">
</head>
<body>
<div id="root">Loading…</div>
<script nonce="${n}">window.__BYNK_SEQUENCE_MODEL__ = ${json}; window.__BYNK_SEQUENCE_DEBUG__ = true;</script>
<script nonce="${n}" src="${scriptUri.toString()}"></script>
</body>
</html>`;
}

describe("sequence-diagram webview participant wiring (#852)", () => {
  let extensionUri: vscode.Uri;

  before(async function () {
    this.timeout(120_000);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();
    extensionUri = ext.extensionUri;
  });

  it("wires each participant to its own rendered box, in declaration order", async function () {
    this.timeout(60_000);

    const panel = vscode.window.createWebviewPanel(
      "bynkSequenceDiagramTest",
      "Sequence Wiring Test",
      vscode.ViewColumn.One,
      { enableScripts: true, localResourceRoots: [vscode.Uri.joinPath(extensionUri, "out")] },
    );

    try {
      const report = new Promise<WiringReport>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("timed out waiting for wiring report")), 30_000);
        panel.webview.onDidReceiveMessage((msg: WiringReport) => {
          if (msg?.type === "wiring") {
            clearTimeout(timer);
            resolve(msg);
          }
        });
      });

      const scriptUri = panel.webview.asWebviewUri(
        vscode.Uri.joinPath(extensionUri, "out", "webview.js"),
      );
      panel.webview.html = debugHtml(panel.webview, scriptUri, { model: MODEL, uri: "file:///x.bynk" });

      const wiring = await report;

      // One reported slot per participant, in declaration order...
      assert.strictEqual(
        wiring.participants.length,
        MODEL.participants.length,
        "one wiring slot per participant",
      );
      // ...and the element each slot landed on is that participant's own box
      // (its rendered label equals its declared name). The old `.actor[i]` walk
      // would report label-less rects / shuffled names here.
      for (let i = 0; i < MODEL.participants.length; i++) {
        assert.strictEqual(
          wiring.participants[i].label,
          MODEL.participants[i].name,
          `participant ${i} (${MODEL.participants[i].name}) wired to its own box`,
        );
      }
    } finally {
      panel.dispose();
    }
  });

  // #2093 (DECISION C): a live refresh posts `{type: "update"}` into the
  // running page instead of replacing its HTML. The page must re-render in
  // place, with a fresh Mermaid render id (a reused id finds the previous
  // SVG), and the participant zip must hold for the new model.
  it("re-renders in place on an update message", async function () {
    this.timeout(60_000);

    const panel = vscode.window.createWebviewPanel(
      "bynkSequenceDiagramTest",
      "Sequence Update Test",
      vscode.ViewColumn.One,
      { enableScripts: true, localResourceRoots: [vscode.Uri.joinPath(extensionUri, "out")] },
    );

    try {
      const reports: WiringReport[] = [];
      let ready = false;
      panel.webview.onDidReceiveMessage((msg: WiringReport | { type: "ready" }) => {
        if (msg?.type === "wiring") reports.push(msg);
        if (msg?.type === "ready") ready = true;
      });
      const until = async (ok: () => boolean, what: string) => {
        const deadline = Date.now() + 30_000;
        while (!ok()) {
          if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
          await new Promise((r) => setTimeout(r, 100));
        }
      };

      const scriptUri = panel.webview.asWebviewUri(
        vscode.Uri.joinPath(extensionUri, "out", "webview.js"),
      );
      panel.webview.html = debugHtml(panel.webview, scriptUri, { model: MODEL, uri: "file:///x.bynk" });
      await until(() => reports.length >= 1 && ready, "the first render");

      const updated: SequenceModel = {
        participants: [
          { id: 0, kind: "Entry", name: "Entry", range: null },
          { id: 1, kind: "Capability", name: "Queue", range: R(1) },
        ],
        messages: [{ from: 0, to: 1, kind: "Send", label: "push", range: R(1), block: null }],
        blocks: [],
      };
      await panel.webview.postMessage({
        type: "update",
        payload: { model: updated, uri: "file:///x.bynk" },
        view: { target: { uri: "file:///x.bynk" }, followsCursor: true },
      });
      await until(() => reports.length >= 2, "the re-render");

      assert.deepStrictEqual(
        reports[1].participants.map((p) => p.label),
        updated.participants.map((p) => p.name),
        "the update's participants are wired to their own boxes",
      );
    } finally {
      panel.dispose();
    }
  });
});
