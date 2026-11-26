// Tests for generate-sidebar.mjs (web presence track, slices 1–3 and 5): the
// contents checks and the sidebar it builds, on small in-memory Books, so a
// change to the generator can't quietly stop a check from firing.
//
//   node --test site/scripts/generate-sidebar.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";

import { TREES, buildSidebar, completeness, parseSummary, releasePages } from "./generate-sidebar.mjs";

const PAGES = ["index.md", "intro/what.md", "guides/a.md", "guides/b.mdx"];
const SUMMARY = `# Summary

[Introduction](introduction.md)

# Start

- [What](intro/what.md)

# Guides

- Tasks
  - [A](guides/a.md)
  - [B](guides/b.md)
`;

test("a complete contents has no problems", () => {
  assert.deepEqual(completeness(SUMMARY, PAGES), {
    unlisted: [],
    missing: [],
    listedTwice: [],
    sharedRoutes: [],
    malformed: [],
  });
});

test("an .mdx page matches an entry written .md", () => {
  assert.deepEqual(completeness(SUMMARY, PAGES).missing, []);
});

test("a page no contents lists is reported", () => {
  assert.deepEqual(completeness(SUMMARY, [...PAGES, "guides/c.md"]).unlisted, ["guides/c.md"]);
});

test("an entry with no page is reported", () => {
  const text = SUMMARY.replace("guides/b.md", "guides/gone.md");
  assert.deepEqual(completeness(text, PAGES).missing, ["guides/gone.md"]);
});

test("a page listed twice is reported", () => {
  const text = SUMMARY + "  - [A again](guides/a.md)\n";
  assert.deepEqual(completeness(text, PAGES).listedTwice, ["/book/guides/a/"]);
});

test("the Book root listed again under a part counts as twice", () => {
  const text = SUMMARY + "  - [Home](index.md)\n";
  assert.deepEqual(completeness(text, PAGES).listedTwice, ["/book/"]);
});

test("two files that serve one page are reported", () => {
  const pages = [...PAGES, "guides/a/index.md"];
  assert.deepEqual(completeness(SUMMARY, pages).sharedRoutes, [["guides/a.md", "guides/a/index.md"]]);
});

test("a malformed item is reported, not dropped", () => {
  const text = SUMMARY + "  - [C] (guides/c.md)\n";
  const { malformed } = completeness(text, PAGES);
  assert.equal(malformed.length, 1);
  assert.match(malformed[0].text, /\[C\] \(guides\/c\.md\)/);
});

test("a group with nothing beneath it is refused", () => {
  assert.throws(() => buildSidebar(SUMMARY + "  - Empty\n", PAGES), /"Empty" has no pages/);
});

test("plain groups and nested groups start collapsed", () => {
  const sidebar = buildSidebar(SUMMARY, PAGES);
  const guides = sidebar.find((e) => e.label === "Guides");
  assert.deepEqual(guides.items[0], {
    label: "Tasks",
    collapsed: true,
    items: [
      { label: "A", link: "/book/guides/a/" },
      { label: "B", link: "/book/guides/b/" },
    ],
  });
});

const RELEASES = [
  "reference/releases/index.md",
  "reference/releases/unreleased.md",
  "reference/releases/v0-9-0.md",
  "reference/releases/v0-10-0.md",
  "reference/releases/v0-10-2.md",
];
const WITH_RELEASES = `${SUMMARY}
# Reference

- [Releases](reference/releases/index.md)
  - {release pages}
`;

test("release pages sort Unreleased first, then by version, newest first", () => {
  assert.deepEqual(releasePages("reference/releases", RELEASES), [
    "reference/releases/unreleased.md",
    "reference/releases/v0-10-2.md",
    "reference/releases/v0-10-0.md",
    "reference/releases/v0-9-0.md",
  ]);
});

test("{release pages} lists them in the sidebar and in the contents check", () => {
  const pages = [...PAGES, ...RELEASES];
  const sidebar = buildSidebar(WITH_RELEASES, pages);
  // A part holding a single page with children is flattened into that page's
  // items, as `# Guides` is, so the part reads "Reference › Unreleased".
  const reference = sidebar.find((e) => e.label === "Reference");
  assert.deepEqual(
    reference.items.map((i) => i.label),
    ["Overview", "Unreleased", "v0.10.2", "v0.10.0", "v0.9.0"],
  );
  assert.deepEqual(completeness(WITH_RELEASES, pages).unlisted, []);
});

test("{release pages} with no parent page is malformed", () => {
  const { malformed } = parseSummary("# Reference\n\n- {release pages}\n");
  assert.equal(malformed.length, 1);
  assert.match(malformed[0].why, /parent entry/);
});

test("an empty {release pages} folder leaves its parent as a plain link", () => {
  const sidebar = buildSidebar(WITH_RELEASES, [...PAGES, "reference/releases/index.md"]);
  const reference = sidebar.find((e) => e.label === "Reference");
  assert.deepEqual(reference.items, [{ label: "Releases", link: "/book/reference/releases/" }]);
});

// Slice 5: the Reference is a second tree with its own contents, routed under
// /reference/, and held to the same checks.
const REFERENCE = TREES.find((t) => t.name === "Reference");
const REF_PAGES = ["index.md", "types.md", "tooling/index.mdx", "tooling/bynk-cli.md"];
const REF_CONTENTS = `# Summary

[Reference](index.md)

# Language

- [Types](types.md)

# Tooling

- [Tooling](tooling/index.md)
  - [CLI](tooling/bynk-cli.md)
`;

test("a second tree routes its pages under its own base", () => {
  const sidebar = buildSidebar(REF_CONTENTS, REF_PAGES, REFERENCE);
  assert.deepEqual(sidebar[0], { label: "Reference", link: "/reference/" });
  assert.deepEqual(sidebar.find((e) => e.label === "Tooling").items, [
    { label: "Overview", link: "/reference/tooling/" },
    { label: "CLI", link: "/reference/tooling/bynk-cli/" },
  ]);
});

test("the second tree's contents are checked against its own pages", () => {
  const report = completeness(REF_CONTENTS, [...REF_PAGES, "spec/index.md"], REFERENCE.base);
  assert.deepEqual(report.unlisted, ["spec/index.md"]);
  assert.deepEqual(report.missing, []);
});

// Slice 13: the Project pages are a third tree, routed under /project/, with no
// front page of their own (no prefix chapter).
const PROJECT = TREES.find((t) => t.name === "Project");
const PROJECT_PAGES = ["versioning-and-roadmap.md", "contributing/index.md", "contributing/testing.md"];
const PROJECT_CONTENTS = `# Summary

# Project

- [Versioning & roadmap](versioning-and-roadmap.md)

# Contributing

- [Contributing](contributing/index.md)
  - [Testing](contributing/testing.md)
`;

test("the Project tree routes its pages under /project/, without a front page", () => {
  const sidebar = buildSidebar(PROJECT_CONTENTS, PROJECT_PAGES, PROJECT);
  assert.deepEqual(sidebar[0], {
    label: "Project",
    items: [{ label: "Versioning & roadmap", link: "/project/versioning-and-roadmap/" }],
  });
  assert.deepEqual(sidebar[1].items, [
    { label: "Overview", link: "/project/contributing/" },
    { label: "Testing", link: "/project/contributing/testing/" },
  ]);
  const report = completeness(PROJECT_CONTENTS, [...PROJECT_PAGES, "contributing/new.md"], PROJECT.base);
  assert.deepEqual(report.unlisted, ["contributing/new.md"]);
  assert.deepEqual(report.listedTwice, []);
});
