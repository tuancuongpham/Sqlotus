// A schema comparison as a report (Markdown or a self-contained HTML page),
// and the filter of the comparison list. Pure functions, tested by
// scripts/compareReport.test.mjs.
import type { ObjectDiff, ObjectRef, SchemaCompareResult } from "./types";

const CHANGE: Record<ObjectDiff["change"], string> = {
  changed: "Different",
  added: "Only in source",
  removed: "Only in target",
};

const qualified = (o: { schema: string; name: string }) => (o.schema ? `${o.schema}.${o.name}` : o.name);

/** Objects whose kind, schema or name contains every word of `filter`. */
export function matchesFilter(o: ObjectRef | ObjectDiff, filter: string): boolean {
  const hay = `${o.kind} ${qualified(o)}`.toLowerCase();
  return filter
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => hay.includes(w));
}

function counts(r: SchemaCompareResult) {
  const n = (c: ObjectDiff["change"]) => r.diff.differences.filter((d) => d.change === c).length;
  return { changed: n("changed"), added: n("added"), removed: n("removed") };
}

export function compareReportMarkdown(r: SchemaCompareResult, when: Date): string {
  const esc = (s: string) => s.replace(/\\/g, "\\\\").replace(/\|/g, "\\|").replace(/\r?\n/g, " ");
  const c = counts(r);
  const lines = [
    `# Schema comparison`,
    "",
    `**Source:** ${esc(r.source)}  `,
    `**Target:** ${esc(r.target)}  `,
    `**Compared:** ${when.toISOString().replace("T", " ").slice(0, 16)} UTC`,
    "",
    `| Different | Only in source | Only in target | Identical | Not readable |`,
    `| ---: | ---: | ---: | ---: | ---: |`,
    `| ${c.changed} | ${c.added} | ${c.removed} | ${r.diff.identical} | ${r.diff.skipped.length} |`,
    "",
  ];
  if (r.diff.differences.length > 0) {
    lines.push("## Differences", "", "| Object | Type | Difference | Details | Warnings |", "| --- | --- | --- | --- | --- |");
    for (const d of r.diff.differences) {
      lines.push(`| ${esc(qualified(d))} | ${d.kind} | ${CHANGE[d.change]} | ${esc(d.details.join("; "))} | ${esc(d.warnings.join("; "))} |`);
    }
    lines.push("");
  } else {
    lines.push("The schemas are identical.", "");
  }
  if (r.diff.identicalObjects.length > 0) {
    lines.push("## Identical", "", r.diff.identicalObjects.map((o) => `${esc(qualified(o))} (${o.kind})`).join(", "), "");
  }
  if (r.diff.skipped.length > 0) {
    lines.push("## Not readable (skipped)", "", r.diff.skipped.map(esc).join(", "), "");
  }
  return lines.join("\n");
}

export function compareReportHtml(r: SchemaCompareResult, when: Date): string {
  const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const c = counts(r);
  const rows = r.diff.differences
    .map(
      (d) =>
        `<tr class="${d.change}"><td>${esc(qualified(d))}</td><td>${d.kind}</td><td>${CHANGE[d.change]}</td><td>${d.details.map(esc).join("<br>")}</td><td>${d.warnings.map((w) => `⚠ ${esc(w)}`).join("<br>")}</td></tr>`,
    )
    .join("\n");
  const identical = r.diff.identicalObjects.map((o) => `<li>${esc(qualified(o))} <small>${o.kind}</small></li>`).join("");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Schema comparison — ${esc(r.source)} → ${esc(r.target)}</title>
<style>
body { font: 14px system-ui, sans-serif; margin: 24px; color: #1f1f1f; }
table { border-collapse: collapse; margin: 12px 0; }
th, td { border: 1px solid #ccc; padding: 4px 8px; text-align: left; vertical-align: top; }
th { background: #f3f3f3; }
tr.changed td:nth-child(3) { color: #8a5a00; }
tr.added td:nth-child(3) { color: #107c10; }
tr.removed td:nth-child(3) { color: #c42b1c; }
small { color: #6b6b6b; }
</style>
</head>
<body>
<h1>Schema comparison</h1>
<p><b>Source:</b> ${esc(r.source)}<br><b>Target:</b> ${esc(r.target)}<br><b>Compared:</b> ${when.toISOString().replace("T", " ").slice(0, 16)} UTC</p>
<table><tr><th>Different</th><th>Only in source</th><th>Only in target</th><th>Identical</th><th>Not readable</th></tr>
<tr><td>${c.changed}</td><td>${c.added}</td><td>${c.removed}</td><td>${r.diff.identical}</td><td>${r.diff.skipped.length}</td></tr></table>
${
  r.diff.differences.length
    ? `<h2>Differences</h2>\n<table><tr><th>Object</th><th>Type</th><th>Difference</th><th>Details</th><th>Warnings</th></tr>\n${rows}\n</table>`
    : "<p>The schemas are identical.</p>"
}
${identical ? `<details><summary>Identical objects (${r.diff.identicalObjects.length})</summary><ul>${identical}</ul></details>` : ""}
${r.diff.skipped.length ? `<h2>Not readable (skipped)</h2><p>${r.diff.skipped.map(esc).join(", ")}</p>` : ""}
</body>
</html>
`;
}
