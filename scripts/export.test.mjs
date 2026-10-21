// Unit tests of src/export.ts (Node runs TypeScript directly).
//
//   npm test
import assert from "node:assert/strict";
import { test } from "node:test";
import { NUMERIC, quoteName, sqlLiteral, toCsv, toHtml, toInsert, toJson, toMarkdown, toTsv } from "../src/export.ts";

const rs = (columns, rows = []) => ({ columns: columns.map(([name, typeName]) => ({ name, typeName })), rows, truncated: false });

test("numeric column types", () => {
  for (const t of ["int", "int4", "bigint", "decimal", "decimal(10,2)", "numeric", "money", "float8", "double precision", "integer", "LONGLONG", "NEWDECIMAL", "bit"]) {
    assert.ok(NUMERIC.test(t), t);
  }
  for (const t of ["interval", "nvarchar", "text", "point", "datetime2", "uuid", "LONGTEXT", "inet"]) {
    assert.ok(!NUMERIC.test(t), t);
  }
});

test("SQL literals", () => {
  assert.equal(sqlLiteral(null, "int", "mssql"), "NULL");
  assert.equal(sqlLiteral("42", "int", "mssql"), "42");
  assert.equal(sqlLiteral("-1.50", "decimal", "postgres"), "-1.50");
  assert.equal(sqlLiteral("1e-5", "float", "mysql"), "1e-5");
  // a numeric type with a non-numeric rendering is quoted (PostgreSQL money)
  assert.equal(sqlLiteral("$1,234.00", "money", "postgres"), "'$1,234.00'");
  assert.equal(sqlLiteral("O'Brien", "nvarchar", "mssql"), "N'O''Brien'");
  assert.equal(sqlLiteral("Nguyễn", "text", "postgres"), "'Nguyễn'");
  // MySQL: backslashes are escapes, so they are doubled; other servers keep them
  assert.equal(sqlLiteral("C:\\temp\\new", "VAR_STRING", "mysql"), "'C:\\\\temp\\\\new'");
  assert.equal(sqlLiteral("C:\\temp", "text", "postgres"), "'C:\\temp'");
  assert.equal(sqlLiteral("it's \\'", "VAR_STRING", "mysql"), "'it''s \\\\'''");
  assert.equal(sqlLiteral("0x0A0B", "varbinary", "mssql"), "0x0A0B");
  // no type name: numbers stay numbers, the rest is quoted
  assert.equal(sqlLiteral("7", "", "mssql"), "7");
  assert.equal(sqlLiteral("07a", "", "postgres"), "'07a'");
  // leading zeros in a text column stay text
  assert.equal(sqlLiteral("007", "varchar", "mssql"), "N'007'");
});

test("quoted names", () => {
  assert.equal(quoteName("a]b", "mssql"), "[a]]b]");
  assert.equal(quoteName("a`b", "mysql"), "`a``b`");
  assert.equal(quoteName('a"b', "postgres"), '"a""b"');
});

test("INSERT statements", () => {
  const r = rs([["Id", "int"], ["Na]me", "nvarchar"], ["", "int"]]);
  assert.equal(
    toInsert(r, [["1", "a", null]], "mssql"),
    "INSERT INTO #Results ([Id], [Na]]me], [column])\nVALUES (1, N'a', NULL);",
  );
  assert.ok(toInsert(r, [["1", "a", null]], "mysql").startsWith("INSERT INTO results (`Id`, `Na]me`, `column`)"));
  // 1000 rows per INSERT (the SQL Server limit for VALUES)
  const many = Array.from({ length: 2001 }, (_, i) => [String(i), "x", null]);
  assert.equal(toInsert(r, many, "mssql").match(/INSERT INTO/g).length, 3);
});

test("Markdown table", () => {
  const md = toMarkdown(rs([["n", "int"], ["t", "text"]]), [["1", "a|b\nc"], [null, "C:\\x"]]);
  assert.equal(md, "| n | t |\n| ---: | --- |\n| 1 | a\\|b<br>c |\n| NULL | C:\\\\x |");
});

test("JSON keeps every digit", () => {
  const r = rs([["id", "bigint"], ["amount", "decimal"], ["flag", "bit"], ["name", "nvarchar"], ["", "int"]]);
  const rows = [
    ["9007199254740993", "12345678901.123456789", "1", "x", "5"],
    ["42", "150.50", "0", null, null],
  ];
  const out = JSON.parse(toJson(r, rows));
  // too many digits for a JavaScript number: kept as a string
  assert.equal(out[0].id, "9007199254740993");
  assert.equal(out[0].amount, "12345678901.123456789");
  assert.equal(out[0].column5, 5);
  assert.equal(out[1].id, 42);
  assert.equal(out[1].amount, 150.5);
  // bit stays as the server rendered it, NULL is null
  assert.equal(out[0].flag, "1");
  assert.equal(out[1].name, null);
});

test("HTML table escapes markup", () => {
  const html = toHtml(rs([["<b>", "text"]]), [["a & <script>"], [null]]);
  assert.ok(html.includes("<th>&lt;b&gt;</th>"));
  assert.ok(html.includes("<td>a &amp; &lt;script&gt;</td>"));
  assert.ok(html.includes("<td><i>NULL</i></td>"));
});

test("CSV quoting", () => {
  const csv = toCsv(rs([["a,b", "text"], ["c", "text"]]), [['say "hi"', "x\ny"], [null, "plain"]]);
  assert.equal(csv, '"a,b",c\r\n"say ""hi""","x\ny"\r\n,plain');
});

test("TSV for the clipboard", () => {
  const tsv = toTsv(rs([["a", "text"], ["b", "text"]]), [["x\ty", "1\r\n2"], [null, "z"]], true);
  assert.equal(tsv, "a\tb\r\nx y\t1 2\r\nNULL\tz");
});
