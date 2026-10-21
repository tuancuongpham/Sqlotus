// Unit tests of src/compareReport.ts.
//
//   npm test
import assert from "node:assert/strict";
import { test } from "node:test";
import { compareReportHtml, compareReportMarkdown, matchesFilter } from "../src/compareReport.ts";

const result = {
  id: "cmp-1",
  dialect: "mssql",
  source: "Dev / Shop",
  target: "Prod / Shop",
  diff: {
    differences: [
      { key: "table:dbo.orders", kind: "table", schema: "dbo", name: "Orders", change: "changed", details: ["column Note added", "a | b"], sourceText: null, targetText: null, warnings: [] },
      { key: "view:dbo.v", kind: "view", schema: "dbo", name: "v<1>", change: "added", details: [], sourceText: null, targetText: null, warnings: [] },
      { key: "table:dbo.old", kind: "table", schema: "dbo", name: "Old", change: "removed", details: [], sourceText: null, targetText: null, warnings: ["Table dbo.Old is dropped with all its data."] },
    ],
    identical: 2,
    identicalObjects: [
      { key: "table:dbo.customers", kind: "table", schema: "dbo", name: "Customers" },
      { key: "procedure:sales.p", kind: "procedure", schema: "sales", name: "p" },
    ],
    skipped: ["dbo.hidden"],
  },
};
const when = new Date(Date.UTC(2026, 9, 21, 8, 30));

test("filter matches kind, schema and name words", () => {
  const orders = result.diff.differences[0];
  assert.ok(matchesFilter(orders, ""));
  assert.ok(matchesFilter(orders, "ord"));
  assert.ok(matchesFilter(orders, "table dbo.ord"));
  assert.ok(matchesFilter(result.diff.identicalObjects[1], "sales proc"));
  assert.ok(!matchesFilter(orders, "view"));
  assert.ok(!matchesFilter(orders, "orders customers"));
});

test("Markdown report", () => {
  const md = compareReportMarkdown(result, when);
  assert.ok(md.startsWith("# Schema comparison\n\n**Source:** Dev / Shop  \n**Target:** Prod / Shop  \n**Compared:** 2026-10-21 08:30 UTC"));
  assert.ok(md.includes("| 1 | 1 | 1 | 2 | 1 |"), md);
  assert.ok(md.includes("| dbo.Orders | table | Different | column Note added; a \\| b |  |"), md);
  assert.ok(md.includes("| dbo.Old | table | Only in target |  | Table dbo.Old is dropped with all its data. |"), md);
  assert.ok(md.includes("## Identical\n\ndbo.Customers (table), sales.p (procedure)"), md);
  assert.ok(md.includes("## Not readable (skipped)\n\ndbo.hidden"), md);
  const same = compareReportMarkdown({ ...result, diff: { differences: [], identical: 0, identicalObjects: [], skipped: [] } }, when);
  assert.ok(same.includes("The schemas are identical.") && !same.includes("## Identical"), same);
});

test("HTML report is escaped and self-contained", () => {
  const html = compareReportHtml(result, when);
  assert.ok(html.startsWith("<!doctype html>"));
  assert.ok(html.includes("<td>dbo.v&lt;1&gt;</td>"), "names are escaped");
  assert.ok(!html.includes("<script") && !html.includes("http"), "no scripts or external links");
  assert.ok(html.includes("<details><summary>Identical objects (2)</summary>"));
  assert.ok(html.includes('<tr class="removed">'));
});
