// The Problems list: grouping, Markdown export and fix-all over
// code-analysis diagnostics. Pure functions, tested by
// scripts/problems.test.mjs.
import type { Diagnostic } from "./types";

/** A diagnostic with its 1-based position, as listed in Problems. */
export interface Problem extends Diagnostic {
  line: number;
  column: number;
}

export type ProblemGrouping = "none" | "rule" | "severity";

const SEVERITY_ORDER: Record<Diagnostic["severity"], number> = { error: 0, warning: 1, info: 2 };

/** Problems in groups (one unnamed group when not grouped), each sorted by position. */
export function groupProblems(list: Problem[], by: ProblemGrouping, filter = ""): { name: string; items: Problem[] }[] {
  const f = filter.trim().toLowerCase();
  const shown = list
    .filter((p) => !f || `${p.code} ${p.message}`.toLowerCase().includes(f))
    .sort((a, b) => a.line - b.line || a.column - b.column);
  if (by === "none") return shown.length ? [{ name: "", items: shown }] : [];
  const key = (p: Problem) => (by === "rule" ? p.code : p.severity);
  const groups = new Map<string, Problem[]>();
  for (const p of shown) groups.set(key(p), [...(groups.get(key(p)) ?? []), p]);
  return [...groups.entries()]
    .sort(([a], [b]) => (by === "severity" ? SEVERITY_ORDER[a as Diagnostic["severity"]] - SEVERITY_ORDER[b as Diagnostic["severity"]] : a.localeCompare(b)))
    .map(([name, items]) => ({ name, items }));
}

/** The problems as a Markdown table (for a review or a ticket). */
export function problemsMarkdown(list: Problem[], file: string): string {
  const esc = (s: string) => s.replace(/\\/g, "\\\\").replace(/\|/g, "\\|").replace(/\r?\n/g, " ");
  const rows = [...list]
    .sort((a, b) => a.line - b.line || a.column - b.column)
    .map((p) => `| ${p.line}:${p.column} | ${p.severity} | ${p.code} | ${esc(p.message)} |`);
  return [`**${esc(file)}** — ${list.length} problem(s)`, "", "| Line | Severity | Rule | Problem |", "| ---: | --- | --- | --- |", ...rows].join("\n");
}

/** The fixes of `list` that do not overlap, in text order (UTF-16 offsets). */
export function nonOverlappingFixes(list: Diagnostic[]): NonNullable<Diagnostic["fix"]>[] {
  const fixes = list.flatMap((d) => (d.fix ? [d.fix] : [])).sort((a, b) => a.start - b.start || a.end - b.end);
  const out: NonNullable<Diagnostic["fix"]>[] = [];
  let at = 0;
  for (const f of fixes) {
    if (f.start < at) continue;
    out.push(f);
    at = f.end;
  }
  return out;
}
