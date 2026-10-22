// End-to-end checks for the 0.2 tools: commit confirmation for large
// changes, manual commit, values for undeclared variables, estimated plans,
// new analysis rules, Excel export, charts, result comparison and the
// server check. Uses the Docker "Shop" database (connection "E2E SQL Server").
//
//   SQLOTUS_E2E_PASSWORD=... node scripts/e2e-tools.mjs [screenshot-dir]
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { attach, checker } from "./cdp.mjs";
import { helpers } from "./e2e-kit.mjs";

const outDir = process.argv[2] ?? "e2e-out";
mkdirSync(outDir, { recursive: true });
const password = process.env.SQLOTUS_E2E_PASSWORD;
if (!password) throw new Error("set SQLOTUS_E2E_PASSWORD");
const ui = await attach(outDir);
const { sleep, evaluate, waitFor, click, press, type, shot, invoke } = ui;
const { until, toastText, waitToast, clearToasts, clickButton, text: modalText } = helpers(ui);
const { check, summary } = checker();

const tabTitles = () => evaluate(`[...document.querySelectorAll(".tab")].map((t) => t.textContent)`);
const before = new Set(await tabTitles());

async function connectNewTab(label) {
  await press("Control+N");
  await sleep(300);
  await evaluate(`document.querySelector(".toolbar button").click()`);
  await waitFor(".conn-modal");
  await until(() =>
    evaluate(`(() => { const e = [...document.querySelectorAll(".conn-item")].find((e) => e.textContent.includes(${JSON.stringify(label)})); e?.click(); return !!e; })()`),
  );
  // never press Connect on an empty form: that would save a new "localhost" connection
  const ready = await until(async () => (await evaluate(`document.querySelector(".conn-form input")?.value`)) === label);
  if (!ready) throw new Error(`connection ${label} was not selected`);
  await evaluate(`[...document.querySelectorAll(".modal-actions button")].find((b) => b.textContent === "Connect").click()`);
  const ok = await waitToast(`Connected to ${label}`, 20000);
  await until(async () => (await evaluate(`document.querySelector(".status-msg").textContent`)).includes("objects loaded"), 15000);
  return ok;
}

async function setText(text) {
  await click(".editor .view-lines");
  await sleep(150);
  await press("Control+A");
  await press("Delete");
  await type(text);
  await press("Escape");
}

async function run(sql) {
  await clearToasts();
  await setText(sql);
  await press("F5");
}

const settings = await invoke("get_settings");
await invoke("save_settings", { settings: { ...settings, safety: { confirmRowsOver: 2, promptVariables: true } } });
try {
  check("connect", !!(await connectNewTab("E2E SQL Server")), await toastText());

  // 1. more rows than the limit: asked to commit; roll back keeps the data
  await run("UPDATE dbo.Customers SET Country = Country WHERE Id <= 3");
  const ask = await until(async () => {
    const t = await modalText(".confirm-modal");
    return t.includes("Commit these changes") ? t : null;
  }, 10000);
  await shot("t1-commit-question");
  check("large change asks before commit", !!ask && ask.includes("changed 3 rows"), (ask ?? (await toastText())).replace(/\n/g, " | "));
  await clickButton(".confirm-modal", "Roll back");
  check("roll back from the question", !!(await waitToast("Transaction rolled back", 8000)), await toastText());
  check("no transaction left", !(await evaluate(`!!document.querySelector(".tab.active .tab-badge.tx")`)));

  // 2. within the limit: committed automatically, with a notice
  await run("UPDATE dbo.Customers SET Country = Country WHERE Id = 1");
  const auto = await waitToast("changed and committed", 8000);
  check("small change commits automatically", !!auto && auto.includes("1 row(s) changed"), auto ?? (await toastText()));

  // 3. undeclared variable: asked for a value
  await run("SELECT Id, Name FROM dbo.Customers WHERE Id = @CustomerId");
  const varsDialog = await until(() => evaluate(`!!document.querySelector(".variables-modal")`), 5000);
  check("asks for undeclared variable", !!varsDialog, await modalText(".variables-modal"));
  if (varsDialog) {
    // the last value is remembered: replace it
    await press("Control+A");
    await type("2");
    await press("Enter");
  }
  await waitToast("Query finished", 8000);
  const ids = await evaluate(`[...document.querySelectorAll(".grid-row")].map((r) => r.children[1].textContent)`);
  check("runs with the given value", JSON.stringify(ids) === '["2"]', JSON.stringify(ids));

  // 4. estimated plan (Ctrl+L)
  await clearToasts();
  await setText("SELECT c.Name, o.Total FROM dbo.Customers c JOIN dbo.Orders o ON o.CustomerId = c.Id WHERE o.Total > 100");
  await press("Control+L");
  const nodes = await until(async () => {
    const n = await evaluate(`document.querySelectorAll(".plan-view .plan-node").length`);
    return n > 0 ? n : null;
  }, 15000);
  await shot("t4-plan");
  check("estimated plan tree", (nodes ?? 0) >= 2, `${nodes} nodes · ${await toastText()}`);
  check("plan tab active", (await evaluate(`document.querySelector(".results-tabs .plan-tab")?.className ?? ""`)).includes("active"));

  // 5. new analysis rules
  const diag = async (text) =>
    (await invoke("diagnostics", { args: { connectionId: "e2e-mssql", database: "Shop", dialect: "mssql", text, offset: 0 } })).map((d) => d.code);
  const d1 = await diag("SELECT Id FROM dbo.Customers WHERE YEAR(CreatedAt) = 2024");
  const d2 = await diag("SELECT Id FROM dbo.Customers WHERE Country = N'VN'");
  const d3 = await diag("SELECT Id FROM dbo.Customers WHERE Country NOT IN (SELECT Country FROM dbo.Customers)");
  check("rule SQ010 function on column", d1.includes("SQ010"), d1.join(","));
  check("rule SQ011 implicit conversion", d2.includes("SQ011"), d2.join(","));
  check("rule SQ009 NOT IN nullable", d3.includes("SQ009"), d3.join(","));

  // 6. Excel workbook
  const xlsx = `${process.env.TEMP}\\sqlotus-e2e.xlsx`;
  rmSync(xlsx, { force: true });
  await invoke("export_xlsx", {
    path: xlsx,
    columns: [{ name: "Id", typeName: "int" }, { name: "Name", typeName: "nvarchar" }],
    rows: [["1", "Nguyễn Văn A"], ["2", null]],
    open: false,
  });
  check("Excel workbook written", existsSync(xlsx) && readFileSync(xlsx).subarray(0, 2).toString() === "PK");

  // 7. chart and compare
  await run("SELECT Id, Name, Id * 10 AS Score FROM dbo.Customers ORDER BY Id");
  await waitToast("Query finished", 8000);
  await clickButton(".grid-toolbar", "Chart");
  const canvas = await until(() => evaluate(`!!document.querySelector(".chart-modal canvas")`), 5000);
  await sleep(300);
  await shot("t7-chart");
  check("chart of the result", !!canvas);
  await clickButton(".chart-modal", "Close");
  await clickButton(".grid-toolbar", "📌 Pin");
  check("pin result", !!(await waitToast("pinned for comparison", 5000)));
  await run("SELECT Id, Name, Id * 10 AS Score FROM dbo.Customers WHERE Id <= 2 ORDER BY Id");
  await waitToast("Query finished", 8000);
  await clickButton(".grid-toolbar", "Compare");
  const cmp = await until(() => modalText(".compare-modal"), 5000);
  await shot("t7-compare");
  check("compare with pinned result", (cmp ?? "").includes("Removed 1") && (cmp ?? "").includes("Same 2"), (cmp ?? "").split("\n").slice(0, 6).join(" | "));
  await clickButton(".compare-modal", "Close");
  await press("Control+W");
  await sleep(400);

  // 8. manual commit connection: changes stay open until Commit / Roll back
  const manual = {
    id: "e2e-manual",
    name: "E2E Manual commit",
    dialect: "mssql",
    host: "localhost,14333",
    port: null,
    database: "Shop",
    auth: "sql",
    user: "sa",
    encrypt: false,
    trustServerCertificate: true,
    color: "#d35400",
    manualCommit: true,
  };
  await invoke("save_connection", { config: manual, password, forgetPassword: false });
  await connectNewTab("E2E Manual commit");
  await run("UPDATE dbo.Customers SET Country = Country WHERE Id = 1");
  const open = await waitToast("transaction still open", 8000);
  const buttons = await until(() => evaluate(`!!document.querySelector(".toolbar .tx-rollback")`), 3000);
  check("manual commit keeps the transaction open", !!open && !!buttons, open ?? (await toastText()));
  await evaluate(`document.querySelector(".toolbar .tx-rollback").click()`);
  check("toolbar roll back", !!(await waitToast("Transaction rolled back", 8000)), await toastText());
  check("badge cleared", !(await evaluate(`!!document.querySelector(".tab.active .tab-badge.tx")`)));
  await press("Control+W");
  await sleep(400);
  await invoke("delete_connection", { id: manual.id });

  // 10. ER diagram and test data from the Object Explorer
  check("connect for diagram", !!(await connectNewTab("E2E SQL Server")));
  if (!(await evaluate(`!!document.querySelector(".explorer-panel")`))) await press("F8");
  await waitFor(".explorer-panel");
  await clickButton(".explorer-panel .panel-search-row", "Diagram");
  const tables = await until(async () => {
    const n = await evaluate(`document.querySelectorAll(".diagram-table").length`);
    return n > 0 ? n : null;
  }, 15000);
  const relations = await evaluate(`document.querySelectorAll(".diagram-svg path[marker-end]").length`);
  const overlaps = await evaluate(`(() => { const r = [...document.querySelectorAll(".diagram-table > rect")].map((e) => e.getBoundingClientRect()); let n = 0; for (let i = 0; i < r.length; i++) for (let j = i + 1; j < r.length; j++) { const a = r[i], b = r[j]; if (a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom) n++; } return n; })()`);
  await shot("t10-diagram");
  check("ER diagram of the database", tables === 4 && relations === 2 && overlaps === 0, `${tables} tables, ${relations} relations, ${overlaps} overlaps`);
  await clickButton(".diagram-modal .modal-actions", "Close");
  await sleep(200);
  await evaluate(`[...document.querySelectorAll(".tree-item")].find((e) => e.querySelector(".tree-name")?.textContent.trim() === "dbo.Orders")?.click()`);
  await sleep(300);
  await clickButton(".tree-actions", "Test data");
  const preview = await until(async () => {
    const t = await evaluate(`document.querySelector(".testdata-preview pre")?.textContent ?? ""`);
    return t.includes("INSERT INTO dbo.Orders") ? t : null;
  }, 15000);
  await shot("t10-testdata");
  const customerIds = [...(preview ?? "").matchAll(/^\s*(?:VALUES )?\((\d+),/gm)].map((m) => Number(m[1]));
  check("test data preview uses existing customers", customerIds.length === 5 && customerIds.every((id) => id >= 1 && id <= 3), JSON.stringify(customerIds));
  await clickButton(".testdata-modal .modal-actions", "Open script");
  // the editor only renders visible lines: read the full text from the saved session
  const opened = await until(async () => {
    const session = await invoke("load_session");
    return session?.tabs?.find((t) => t.text.includes("Test data for dbo.Orders"))?.text ?? null;
  }, 10000);
  check("test data script opens in a new tab", !!opened && (opened.match(/^\s*\(|VALUES \(/gm) ?? []).length === 100, `${(opened ?? "").split(String.fromCharCode(10)).length} lines`);

  // 9. server check from the connection dialog (no error needed)
  await evaluate(`document.querySelector(".toolbar button").click()`);
  await waitFor(".conn-modal");
  await until(() =>
    evaluate(`(() => { const e = [...document.querySelectorAll(".conn-item")].find((e) => e.textContent.includes("E2E SQL Server")); e?.click(); return !!e; })()`),
  );
  await sleep(300);
  await clickButton(".conn-modal .modal-actions", "🩺 Check server");
  const check9 = await until(async () => {
    const t = await modalText(".doctor-modal");
    return t.includes("Server check") && !t.includes("Checking the server") ? t : null;
  }, 40000);
  await shot("t9-server-check");
  check("server check without an error", !!check9 && /Firewall|firewall|SQL Server/.test(check9), (check9 ?? "").split("\n").slice(0, 8).join(" | "));
  await clickButton(".doctor-modal .modal-actions", "Close");
  await sleep(200);
  await clickButton(".conn-modal .modal-actions", "Close");
} finally {
  await invoke("save_settings", { settings });
  // close the tabs this run opened
  for (let i = 0; i < 10; i++) {
    const own = (await tabTitles()).findIndex((t) => !before.has(t));
    if (own < 0) break;
    await evaluate(`document.querySelectorAll(".tab")[${own}].dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }))`);
    await sleep(200);
    // close only the tab just selected: tabs react to mousedown, and Ctrl+W closes the active one
    if (!(await evaluate(`!!document.querySelectorAll(".tab")[${own}]?.classList.contains("active")`))) break;
    await press("Control+W");
    await sleep(300);
    await clickButton(".confirm-modal", "Roll back and close");
  }
}

const failed = summary();
ui.close();
process.exit(failed ? 1 : 0);
