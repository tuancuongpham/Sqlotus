// Unit tests of src/problems.ts.
//
//   npm test
import assert from "node:assert/strict";
import { test } from "node:test";
import { groupProblems, nonOverlappingFixes, problemsMarkdown } from "../src/problems.ts";

const p = (line, column, severity, code, message, fix) => ({ start: line * 100 + column, end: line * 100 + column + 1, line, column, severity, code, message, fix });

const list = [
  p(3, 1, "info", "SQ002", "Avoid SELECT *"),
  p(1, 8, "warning", "SQ001", "DELETE without a WHERE clause"),
  p(2, 5, "warning", "SQ003", "Comparison with NULL", { title: "Use IS NULL", start: 205, end: 211, text: "IS NULL" }),
  p(1, 1, "error", "SQ006", "Invalid object name 'x'"),
];

test("grouping and filtering", () => {
  const flat = groupProblems(list, "none");
  assert.equal(flat.length, 1);
  assert.deepEqual(
    flat[0].items.map((x) => `${x.line}:${x.column}`),
    ["1:1", "1:8", "2:5", "3:1"],
  );
  const bySeverity = groupProblems(list, "severity").map((g) => `${g.name}=${g.items.length}`);
  assert.deepEqual(bySeverity, ["error=1", "warning=2", "info=1"]);
  const byRule = groupProblems(list, "rule").map((g) => g.name);
  assert.deepEqual(byRule, ["SQ001", "SQ002", "SQ003", "SQ006"]);
  assert.equal(groupProblems(list, "none", "null")[0].items.length, 1);
  assert.equal(groupProblems(list, "none", "sq00")[0].items.length, 4);
  assert.deepEqual(groupProblems(list, "rule", "nothing like this"), []);
  assert.deepEqual(groupProblems([], "none"), []);
});

test("Markdown export", () => {
  const md = problemsMarkdown([p(1, 2, "warning", "SQ001", "a | b\nc")], "My|Query");
  assert.equal(md, "**My\\|Query** — 1 problem(s)\n\n| Line | Severity | Rule | Problem |\n| ---: | --- | --- | --- |\n| 1:2 | warning | SQ001 | a \\| b c |");
});

test("fix all skips overlapping fixes", () => {
  const fixes = nonOverlappingFixes([
    { fix: { title: "b", start: 10, end: 20, text: "B" } },
    { fix: { title: "a", start: 0, end: 5, text: "A" } },
    { fix: { title: "overlaps b", start: 15, end: 25, text: "X" } },
    { fix: { title: "insert", start: 20, end: 20, text: "N" } },
    {},
  ]);
  assert.deepEqual(
    fixes.map((f) => f.title),
    ["a", "b", "insert"],
  );
});
