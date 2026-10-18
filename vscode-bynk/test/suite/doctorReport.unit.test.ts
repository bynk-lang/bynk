// Unit coverage for `Bynk: Doctor`'s pure core (#2095): parsing `bynk doctor
// --format json`, the actions each remedy offers, and the checklist markup.
//
// The pinned fixture is the driver's own golden, `bynk/tests/golden/doctor.json`
// (blessed by `bynk/tests/doctor.rs`), read in place: a change to the doctor's
// JSON shows up here as well as there.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import {
  DoctorRow,
  offeredCommands,
  parseDoctorReport,
  remedyActions,
  renderDoctorHtml,
} from "../../src/doctorReport";

const GOLDEN = fs.readFileSync(
  path.resolve(__dirname, "../../../../bynk/tests/golden/doctor.json"),
  "utf8",
);

const row = (label: string, remedy: string, level: DoctorRow["level"] = "fail"): DoctorRow => ({
  label,
  level,
  detail: "missing",
  remedy,
});

describe("parseDoctorReport", () => {
  it("reads the driver's golden report", () => {
    const r = parseDoctorReport(GOLDEN);
    assert.strictEqual(r.allOk, false);
    assert.deepStrictEqual(
      r.capabilities.map((c) => `${c.capability}:${c.level}${c.optional ? "?" : ""}`),
      ["compile:ok", "test:ok", "deploy:warn", "editor:note?"],
    );
    const wrangler = r.capabilities[2].rows[1];
    assert.strictEqual(wrangler.label, "wrangler");
    assert.strictEqual(wrangler.level, "warn");
    assert.ok(wrangler.remedy?.startsWith("npm install -g wrangler@4"));
    assert.strictEqual(r.capabilities[0].rows[0].remedy, null);
  });

  it("ignores unknown fields and reads an unknown level as warn", () => {
    const r = parseDoctorReport(
      '{"driver":"1.0.0","new":1,"capabilities":[{"capability":"test","level":"weird","rows":[{"label":"node","level":"ok","detail":"v22","remedy":null,"extra":true}]}]}',
    );
    assert.strictEqual(r.capabilities[0].level, "warn");
    assert.strictEqual(r.capabilities[0].rows[0].level, "ok");
    assert.strictEqual(r.compiler.resolved, false);
  });

  it("refuses text that is not a report", () => {
    assert.throws(() => parseDoctorReport('{"driver":"1"}'));
    assert.throws(() => parseDoctorReport("bynk: error"));
  });
});

describe("remedyActions", () => {
  it("takes the leading command of a remedy", () => {
    assert.deepStrictEqual(remedyActions(row("wrangler", "npm install -g wrangler@4, or clear a stale npx cache (~/.npm/_npx)")), {
      commands: ["npm install -g wrangler@4"],
      urls: [],
    });
    assert.deepStrictEqual(
      remedyActions(row("wrangler", "npm install --save-dev wrangler@4 (in the project)")).commands,
      ["npm install --save-dev wrangler@4"],
    );
  });

  it("also offers a backticked alternative", () => {
    assert.deepStrictEqual(
      remedyActions(row("tsc | tsx", "npm install -g typescript@5.4.2 (or `npm install -g tsx`, which runs tests without type-checking)")).commands,
      ["npm install -g typescript@5.4.2", "npm install -g tsx"],
    );
  });

  it("offers links and settings, but no command for prose", () => {
    assert.deepStrictEqual(remedyActions(row("node", "install Node.js ≥ 22 from https://nodejs.org")), {
      commands: [],
      urls: ["https://nodejs.org"],
    });
    assert.deepStrictEqual(remedyActions(row("bynkc-lsp", "install bynkc-lsp (or download from releases)")), {
      commands: [],
      urls: [],
      setting: "bynk.executablePath",
    });
    assert.strictEqual(
      remedyActions(row("compiler", "fix BYNK_BYNKC, or unset it to use the in-process compiler")).setting,
      "bynk.compilerPath",
    );
    assert.deepStrictEqual(remedyActions(row("cargo", "install Rust via https://rustup.rs")).urls, ["https://rustup.rs"]);
  });

  it("collects every offered command, which is all the host will run", () => {
    assert.deepStrictEqual([...offeredCommands(parseDoctorReport(GOLDEN))], ["npm install -g wrangler@4"]);
  });
});

describe("renderDoctorHtml", () => {
  const report = parseDoctorReport(GOLDEN);
  const html = renderDoctorHtml(report);

  it("groups rows by capability with their levels", () => {
    assert.ok(html.includes("Some tools need attention"));
    for (const heading of ["Compile", "Test", "Dev and deploy", "Editor"]) {
      assert.ok(html.includes(` ${heading}`), heading);
    }
    assert.ok(html.includes('<span class="optional">optional</span>'));
    assert.ok(html.includes('<li class="row warn">'));
    assert.ok(html.includes('<li class="row fail">'));
  });

  it("puts remedy buttons on rows that are not ok, and only there", () => {
    assert.ok(html.includes('data-action="run" data-command="npm install -g wrangler@4"'));
    assert.ok(html.includes('data-action="setting" data-setting="bynk.executablePath"'));
    assert.strictEqual((html.match(/data-action="copy"/g) ?? []).length, 2);
    assert.ok(html.includes('data-action="rerun"'));
  });

  it("names a --only scope and escapes CLI text", () => {
    const evil = parseDoctorReport(
      '{"driver":"<b>","capabilities":[{"capability":"<i>","level":"fail","rows":[{"label":"x","level":"fail","detail":"<script>","remedy":"npm i \\"<x>\\""}]}]}',
    );
    const out = renderDoctorHtml(evil, { only: "test" });
    assert.ok(out.includes("checking <code>test</code> only"));
    // Every `<` from the input arrives escaped: each injected tag appears only
    // in its escaped form, never raw (whatever its case).
    for (const tag of ["<b>", "<i>", "<script>", "<x>"]) {
      assert.ok(out.includes(tag.replace("<", "&lt;").replace(">", "&gt;")), `${tag} is escaped`);
      assert.ok(!out.toLowerCase().includes(tag), `${tag} does not appear raw`);
    }
  });
});
