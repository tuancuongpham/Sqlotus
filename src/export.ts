// Result rows as text: copy (TSV), INSERT statements, CSV, Markdown, JSON,
// HTML and a fixed-width text table. Pure functions, tested by
// scripts/export.test.mjs.
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

/** Binary column types shown as `0x...` by SQL Server and MySQL. */
const BINARY = /^(varbinary|binary|image|blob|tinyblob|mediumblob|longblob|geometry)\b/i;

export function sqlLiteral(v: string | null, typeName: string, dialect: Dialect): string {
  if (v == null) return "NULL";
  // PostgreSQL bit is a bit string and money text ($1,234.56); without a
  // type (a script of several statements) every value is quoted there,
  // which PostgreSQL reads into any column type
  const numeric = typeName
    ? NUMERIC.test(typeName) && !(dialect === "postgres" && /^(bit|money)/i.test(typeName))
    : dialect !== "postgres" && /^-?\d+(\.\d+)?$/.test(v);
  if (numeric && /^-?\d+(\.\d+)?(e-?\d+)?$/i.test(v)) return v;
  if (dialect !== "postgres" && BINARY.test(typeName) && /^0x[0-9a-f]*$/i.test(v)) {
    return dialect === "mysql" ? `X'${v.slice(2)}'` : v;
  }
  // datetime reads 'YYYY-MM-DD hh:mm' by the session's DATEFORMAT; with a T it is ISO 8601
  if (dialect === "mssql" && /^(small)?datetime$/i.test(typeName) && /^\d{4}-\d{2}-\d{2} \d/.test(v)) {
    v = v.replace(" ", "T");
  }
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

/** What a spreadsheet reads as the start of a formula. */
const FORMULA_START = /^[=+\-@\t\r]/;

/**
 * Rows as CSV. With `formulaGuard`, text starting with `=`, `+`, `-` or `@`
 * gets a `'` in front, so a spreadsheet shows it instead of running it as a
 * formula; numbers in numeric columns stay as they are.
 */
export function toCsv(rs: ResultSet, rows: Rows, formulaGuard = false): string {
  const numeric = rs.columns.map((c) => NUMERIC.test(c.typeName));
  const esc = (v: string | null, j = -1) => {
    if (v == null) return "";
    const safe = formulaGuard && !numeric[j] && FORMULA_START.test(v) ? `'${v}` : v;
    return /[",\r\n]/.test(safe) ? `"${safe.replace(/"/g, '""')}"` : safe;
  };
  return [rs.columns.map((c) => esc(c.name)).join(","), ...rows.map((r) => r.map(esc).join(","))].join("\r\n");
}

/** Characters two columns wide in a fixed-width font (CJK, emoji). */
function isWide(cp: number): boolean {
  return (
    (cp >= 0x1100 && cp <= 0x115f) ||
    (cp >= 0x2e80 && cp <= 0xa4cf) ||
    (cp >= 0xac00 && cp <= 0xd7a3) ||
    (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe30 && cp <= 0xfe4f) ||
    (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1f64f) ||
    (cp >= 0x1f900 && cp <= 0x1f9ff) ||
    (cp >= 0x20000 && cp <= 0x3fffd)
  );
}

const MARK = /\p{M}/u;

const charWidth = (ch: string) => (MARK.test(ch) ? 0 : isWide(ch.codePointAt(0)!) ? 2 : 1);

/** Width of text in a fixed-width font: combining marks take no room, wide characters two. */
export function textWidth(s: string): number {
  let w = 0;
  for (const ch of s) w += charWidth(ch);
  return w;
}

/** The start of `s` that fits `width` columns. */
function fitWidth(s: string, width: number): string {
  let w = 0;
  let out = "";
  for (const ch of s) {
    const cw = charWidth(ch);
    if (w + cw > width) break;
    w += cw;
    out += ch;
  }
  return out;
}

/**
 * Rows as a fixed-width text table, as a terminal client prints them:
 * numbers right-aligned, line breaks shown as spaces, values longer than
 * `maxWidth` cut with `…`, and the row count at the end.
 */
export function toText(rs: ResultSet, rows: Rows, maxWidth = 200): string {
  const cell = (v: string | null) => {
    const flat = v == null ? "NULL" : v.replace(/\r\n|[\r\n\t]/g, " ");
    return textWidth(flat) > maxWidth ? `${fitWidth(flat, maxWidth - 1)}…` : flat;
  };
  const heads = rs.columns.map((c) => cell(c.name));
  const cells = rows.map((r) => r.map(cell));
  const widths = heads.map(textWidth);
  for (const r of cells) r.forEach((v, j) => (widths[j] = Math.max(widths[j], textWidth(v))));
  const numeric = rs.columns.map((c) => NUMERIC.test(c.typeName));
  const pad = (v: string, j: number, right: boolean) => {
    const gap = " ".repeat(widths[j] - textWidth(v));
    return right ? gap + v : v + gap;
  };
  const line = (r: string[], header: boolean) => ` ${r.map((v, j) => pad(v, j, !header && numeric[j])).join(" | ")}`.trimEnd();
  const out = [line(heads, true), widths.map((w) => "-".repeat(w + 2)).join("+"), ...cells.map((r) => line(r, false))];
  out.push(`(${rows.length.toLocaleString("en-US")} row${rows.length === 1 ? "" : "s"})`);
  return out.join("\n");
}
