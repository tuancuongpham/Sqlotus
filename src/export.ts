// Result rows as text: copy (TSV), INSERT statements, CSV, Markdown, JSON
// and HTML. Pure functions, tested by scripts/export.test.mjs.
import type { Dialect, ResultSet } from "./types";

type Rows = (string | null)[][];

export function toTsv(rs: ResultSet, rows: Rows, headers: boolean): string {
  const esc = (v: string | null) => (v == null ? "NULL" : v.replace(/\t/g, " ").replace(/\r?\n/g, " "));
  const lines = rows.map((r) => r.map(esc).join("\t"));
  if (headers) lines.unshift(rs.columns.map((c) => c.name).join("\t"));
  return lines.join("\r\n");
}

/** Numeric column types of the three servers (`\b`: `interval` is not `int`). */
export const NUMERIC =
  /^(tinyint|smallint|int|bigint|decimal|numeric|money|smallmoney|float|real|bit|integer|double|serial|bigserial|int[248]|float[48]|LONGLONG|LONG|SHORT|TINY|NEWDECIMAL|DOUBLE|FLOAT)\b/i;

export function sqlLiteral(v: string | null, typeName: string, dialect: Dialect): string {
  if (v == null) return "NULL";
  const numeric = typeName ? NUMERIC.test(typeName) : /^-?\d+(\.\d+)?$/.test(v);
  if (numeric && /^-?\d+(\.\d+)?(e-?\d+)?$/i.test(v)) return v;
  if (typeName === "varbinary" || typeName === "binary") return v;
  // MySQL treats a backslash in a string as an escape character
  const text = dialect === "mysql" ? v.replace(/\\/g, "\\\\") : v;
  const quoted = `'${text.replace(/'/g, "''")}'`;
  return dialect === "mssql" ? `N${quoted}` : quoted;
}

export function quoteName(name: string, dialect: Dialect): string {
  if (dialect === "mssql") return `[${name.replace(/]/g, "]]")}]`;
  if (dialect === "mysql") return `\`${name.replace(/`/g, "``")}\``;
  return `"${name.replace(/"/g, '""')}"`;
}

/** Rows as INSERT statements (1000-row VALUES batches, the SQL Server limit). */
export function toInsert(rs: ResultSet, rows: Rows, dialect: Dialect): string {
  const target = dialect === "mssql" ? "#Results" : "results";
  const cols = rs.columns.map((c) => quoteName(c.name || "column", dialect)).join(", ");
  const parts: string[] = [];
  for (let i = 0; i < rows.length; i += 1000) {
    const values = rows
      .slice(i, i + 1000)
      .map((r) => `(${r.map((v, j) => sqlLiteral(v, rs.columns[j].typeName, dialect)).join(", ")})`)
      .join(",\n       ");
    parts.push(`INSERT INTO ${target} (${cols})\nVALUES ${values};`);
  }
  return parts.join("\n\n");
}

export function toMarkdown(rs: ResultSet, rows: Rows): string {
  const esc = (v: string | null) => (v == null ? "NULL" : v.replace(/\\/g, "\\\\").replace(/\|/g, "\\|").replace(/\r?\n/g, "<br>"));
  const head = `| ${rs.columns.map((c) => esc(c.name)).join(" | ")} |`;
  const numeric = rs.columns.map((c) => NUMERIC.test(c.typeName));
  const sep = `| ${numeric.map((n) => (n ? "---:" : "---")).join(" | ")} |`;
  return [head, sep, ...rows.map((r) => `| ${r.map(esc).join(" | ")} |`)].join("\n");
}

/** A decimal string a JavaScript number holds exactly (at most 15 significant digits). */
function exactNumber(v: string): boolean {
  if (!/^-?\d+(\.\d+)?$/.test(v)) return false;
  const [whole, frac = ""] = v.replace("-", "").split(".");
  return (whole + frac.replace(/0+$/, "")).replace(/^0+/, "").length <= 15;
}

export function toJson(rs: ResultSet, rows: Rows): string {
  const numeric = rs.columns.map((c) => NUMERIC.test(c.typeName) && !/^bit$/i.test(c.typeName));
  const objects = rows.map((r) =>
    Object.fromEntries(
      r.map((v, j) => {
        // a number only when no digit is lost; long decimals stay strings
        const n = v != null && numeric[j] && exactNumber(v) ? Number(v) : v;
        return [rs.columns[j].name || `column${j + 1}`, n];
      }),
    ),
  );
  return JSON.stringify(objects, null, 2);
}

export function toHtml(rs: ResultSet, rows: Rows): string {
  const esc = (v: string | null) => (v == null ? "<i>NULL</i>" : v.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"));
  const head = rs.columns.map((c) => `<th>${esc(c.name)}</th>`).join("");
  const body = rows.map((r) => `<tr>${r.map((v) => `<td>${esc(v)}</td>`).join("")}</tr>`).join("\n");
  return `<table border="1" cellspacing="0" cellpadding="4">\n<thead><tr>${head}</tr></thead>\n<tbody>\n${body}\n</tbody>\n</table>`;
}

export function toCsv(rs: ResultSet, rows: Rows): string {
  const esc = (v: string | null) => (v == null ? "" : /[",\r\n]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v);
  return [rs.columns.map((c) => esc(c.name)).join(","), ...rows.map((r) => r.map(esc).join(","))].join("\r\n");
}
