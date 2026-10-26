// #2473: the `bynk approve --list --format json` listing, what changed since
// the last approval, how a driver's output reports a pending requirement, and
// the review markup. Pure: no editor, no driver.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import {
  annotateChanges,
  approveArgs,
  parseApprovalListing,
  pendingMessage,
  pendingPackages,
  pendingSignature,
  rememberApproved,
  renderApproveHtml,
  reportsUnapproved,
} from "../../src/approveReport";

const FIXTURE = fs.readFileSync(path.resolve(__dirname, "../../../test/fixtures/approve-list.json"), "utf8");
const listing = () => parseApprovalListing(FIXTURE);

describe("bynk approve --list (#2473)", () => {
  it("parses the driver's listing", () => {
    const l = listing();
    assert.strictEqual(l.pending, 2);
    const jose = l.requirements[0];
    assert.strictEqual(jose.package, "jose");
    assert.strictEqual(jose.state, "pending");
    assert.strictEqual(jose.change, "new");
    assert.deepStrictEqual(jose.chains[0], ["this project", "acme/money", "acme/money:tokens"]);
  });

  it("reads an unknown state as pending, and rejects a document with no requirements", () => {
    const l = parseApprovalListing(JSON.stringify({ requirements: [{ package: "x", range: "1", state: "odd" }] }));
    assert.strictEqual(l.requirements[0].state, "pending");
    assert.throws(() => parseApprovalListing("{}"));
  });

  it("groups pending rows per package, sorted", () => {
    const l = parseApprovalListing(
      JSON.stringify({
        requirements: [
          { package: "b", range: "1", state: "pending" },
          { package: "a", range: "1", state: "pending" },
          { package: "a", range: "2", state: "pending" },
          { package: "c", range: "1", state: "approved" },
        ],
      }),
    );
    const packages = pendingPackages(l);
    assert.deepStrictEqual(
      packages.map((p) => [p.package, p.rows.map((r) => r.range)]),
      [["a", ["1", "2"]], ["b", ["1"]]],
    );
  });

  it("says a moved range is a change, from what it last saw approved", () => {
    const approved = parseApprovalListing(
      JSON.stringify({ requirements: [{ package: "jose", range: "^5.2.0", state: "approved" }] }),
    );
    const memory = rememberApproved(approved, {});
    assert.deepStrictEqual(memory, { jose: ["^5.2.0"] });
    // The lock then drops the approved row and asks about the new range.
    const moved = parseApprovalListing(
      JSON.stringify({ requirements: [{ package: "jose", range: "^6.0.0", state: "pending", change: "new" }] }),
    );
    const annotated = annotateChanges(moved, memory);
    assert.strictEqual(annotated.requirements[0].change, "range_changed");
    assert.deepStrictEqual(annotated.requirements[0].previousRanges, ["^5.2.0"]);
    // While it is pending the memory is kept; once approved, it is replaced.
    assert.deepStrictEqual(rememberApproved(moved, memory), { jose: ["^5.2.0"] });
    const done = parseApprovalListing(
      JSON.stringify({ requirements: [{ package: "jose", range: "^6.0.0", state: "approved" }] }),
    );
    assert.deepStrictEqual(rememberApproved(done, memory), { jose: ["^6.0.0"] });
  });

  it("leaves a package it never saw approved as new", () => {
    const annotated = annotateChanges(listing(), {});
    assert.ok(annotated.requirements.filter((r) => r.state === "pending").every((r) => r.change === "new"));
  });

  it("detects an unapproved requirement in driver output", () => {
    assert.ok(
      reportsUnapproved(
        "src/a.bynk:1:1: error[bynk.requires.unapproved]: the adapter requires npm package `jose`\n",
      ),
    );
    assert.ok(reportsUnapproved("error: bynk.requires.unapproved: needs approval"));
    assert.ok(!reportsUnapproved("src/a.bynk:1:1: error[bynk.fake.rebuild]: nope\n"));
    assert.ok(!reportsUnapproved(""));
  });

  it("tells a changed pending set from an unchanged one", () => {
    const l = listing();
    assert.strictEqual(pendingSignature(l), "jose@^6.0.0\nleft-pad@^1.3.0");
    assert.ok(pendingMessage(l).includes("`jose`, `left-pad`"));
  });

  it("approves one package, or all", () => {
    assert.deepStrictEqual(approveArgs("jose"), ["approve", "jose"]);
    assert.deepStrictEqual(approveArgs(undefined), ["approve", "--all"]);
  });

  it("renders a button per pending package, approve-all secondary, approved folded", () => {
    const html = renderApproveHtml(listing());
    assert.ok(html.includes('data-action="approve" data-package="jose"'));
    assert.ok(html.includes('data-action="approve" data-package="left-pad"'));
    assert.ok(html.includes('data-action="approveAll"'));
    assert.ok(!html.includes('data-action="approve" data-package="zod"'), "an approved package has no button");
    assert.ok(html.includes("Already approved (1)"));
    assert.ok(html.includes("acme/money") && html.includes("→"));
  });

  it("offers no approve-all for a single package, and escapes driver text", () => {
    const l = parseApprovalListing(
      JSON.stringify({
        requirements: [
          { package: '<img src=x onerror="1">', range: "^1", state: "pending", chains: [["a<b"]], url: "" },
        ],
      }),
    );
    const html = renderApproveHtml(l);
    assert.ok(!html.includes('data-action="approveAll"'));
    assert.ok(!html.includes("<img"));
    assert.ok(!html.includes("a<b"));
  });

  it("says what changed when a range moved", () => {
    const l = annotateChanges(
      parseApprovalListing(JSON.stringify({ requirements: [{ package: "jose", range: "^6.0.0", state: "pending" }] })),
      { jose: ["^5.2.0"] },
    );
    const html = renderApproveHtml(l);
    assert.ok(html.includes("range changed, was <code>^5.2.0</code>"));
  });

  it("renders a clean state", () => {
    const html = renderApproveHtml(parseApprovalListing(JSON.stringify({ requirements: [] })));
    assert.ok(html.includes("Nothing awaits approval"));
  });
});
