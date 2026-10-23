// End-to-end checks of the productivity features, through the real UI
// against the Docker SQL Server (connection "E2E SQL Server", saved by
// e2e-db.mjs):
//
//   objects reloaded after CREATE / DROP (and only once a transaction ends),
//   F5 running the statement at the cursor, the theme following Windows,
//   Ctrl+wheel zoom, results as text, one row as a form, following foreign
//   keys from a result, table data as INSERT, connection folders, finding a
//   value in every text column, SQLCMD variables, backup and restore
//   scripts, query history kept for N days, the question before committing
//   what procedures changed, and the CSV formula guard.
//
// Every setting changed here is set back through Options at the end, and
// every object created is dropped.
//
//   $env:SQLOTUS_E2E_PASSWORD="..."; node scripts/e2e-productivity.mjs [screenshot-dir]
import { start } from "./e2e-kit.mjs";

const t = await start();
const { ui, check } = t;
const { sleep, evaluate, press, shot, invoke, send } = ui;

const before = new Set(await t.tabTitles());
const original = await invoke("get_settings");
const CONN = "e2e-mssql";

/** Run a step; an exception fails it instead of stopping the suite. */
async function step(name, fn) {
  try {
    await fn();
  } catch (e) {
    check(name, false, String(e?.stack ?? e).split("\n").slice(0, 3).join(" | "));
    await shot(`fail-${name.replace(/\W+/g, "-")}`).catch(() => undefined);
    await evaluate(`document.querySelectorAll(".modal-backdrop").forEach((m) => m.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })))`).catch(() => undefined);
  }
}

/** Set a React-controlled input or select by selector. */
const setField = (selector, value) =>
  evaluate(`(() => {
    const el = document.querySelector(${JSON.stringify(selector)});
    if (!el) return false;
    const proto = el.tagName === "SELECT" ? HTMLSelectElement.prototype : el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(el, ${JSON.stringify(value)});
    el.dispatchEvent(new Event(el.tagName === "SELECT" ? "change" : "input", { bubbles: true }));
    return true;
  })()`);

/** Make a checkbox checked (or not). */
const setCheck = (selector, on) =>
  evaluate(`(() => {
    const el = document.querySelector(${JSON.stringify(selector)});
    if (!el) return false;
    if (el.checked !== ${on}) el.click();
    return el.checked === ${on};
  })()`);

/** Change Options on `page` ("Editor & completion", "Safety"), then Done. */
async function options(page, change) {
  await t.clickButton(".toolbar", "⚙ Options");
  if (!(await t.until(() => evaluate(`!!document.querySelector(".settings-modal")`), 5000))) throw new Error("Options did not open");
  await t.clickButton(".settings-tabs", page);
  await sleep(200);
  await change();
  await sleep(300);
  await t.clickButton(".settings-modal .modal-actions", "Done");
  await t.until(() => evaluate(`!document.querySelector(".settings-modal")`), 5000);
  await sleep(300);
}

const catalogNames = async () => (await invoke("catalog_objects", { connectionId: CONN, database: "Shop" })).map((o) => o.name);
const resultTabs = () => evaluate(`[...document.querySelectorAll(".results-tabs button")].map((b) => b.textContent).filter((x) => x.startsWith("Results"))`);
const headers = () => evaluate(`[...document.querySelectorAll(".grid-head")].map((h) => h.textContent.replace(/[↗🔒▲▼]/g, "").trim())`);
const connected = () => t.until(() => evaluate(`document.querySelector(".toolbar button")?.textContent.startsWith("⇄")`), 20000);
/** Capture what the app copies (reading the clipboard back asks for permission). */
await evaluate(`(() => { window.__clip = ""; navigator.clipboard.writeText = async (x) => { window.__clip = x; }; })()`);
const clip = () => evaluate(`window.__clip`);

// the tab all steps start from
const tab = await t.connectNewTab("E2E SQL Server");
check("connected", !!tab);
const mainTitle = await t.activeTitle();
const back = () => t.selectTab(mainTitle);
await t.runSql("IF OBJECT_ID('dbo.E2E_AutoRefresh') IS NOT NULL DROP TABLE dbo.E2E_AutoRefresh; IF OBJECT_ID('dbo.E2E_InTx') IS NOT NULL DROP TABLE dbo.E2E_InTx; IF OBJECT_ID('dbo.E2E_Find') IS NOT NULL DROP TABLE dbo.E2E_Find");

// -- 1. objects reload after the script changed them ---------------------------
await step("objects reload after CREATE TABLE", async () => {
  await t.runSql("CREATE TABLE dbo.E2E_AutoRefresh (Id int PRIMARY KEY, Note nvarchar(20))");
  const listed = await t.until(async () => (await catalogNames()).includes("E2E_AutoRefresh"), 15000);
  const status = await t.until(async () => ((await t.statusText()).includes("objects reloaded after the schema change") ? t.statusText() : null), 5000);
  const tree = await t.until(() => evaluate(`[...document.querySelectorAll(".tree-name")].some((n) => n.textContent.includes("E2E_AutoRefresh"))`), 5000);
  check("objects reload after CREATE TABLE", !!listed && !!status && !!tree, `catalog ${!!listed}, status ${status}, explorer ${!!tree}`);
});

await step("objects reload once the transaction ends", async () => {
  const toast = await t.runSql("BEGIN TRAN; CREATE TABLE dbo.E2E_InTx (Id int);");
  await sleep(1500);
  const meanwhile = (await catalogNames()).includes("E2E_InTx");
  await evaluate(`document.querySelector(".tx-commit")?.click()`);
  const after = await t.until(async () => (await catalogNames()).includes("E2E_InTx"), 15000);
  check("objects reload once the transaction ends", /transaction still open/.test(toast ?? "") && !meanwhile && !!after, `toast ${toast}; listed while open ${meanwhile}; after commit ${!!after}`);
});

await step("objects reload after DROP TABLE", async () => {
  await t.runSql("DROP TABLE dbo.E2E_AutoRefresh; DROP TABLE dbo.E2E_InTx;", { answer: "Execute anyway" });
  const gone = await t.until(async () => {
    const names = await catalogNames();
    return !names.includes("E2E_AutoRefresh") && !names.includes("E2E_InTx");
  }, 15000);
  check("objects reload after DROP TABLE", !!gone);
});

// -- 2. F5 runs the statement at the cursor (an option) ---------------------------
await step("F5 runs the statement at the cursor", async () => {
  await options("Editor & completion", () => setCheck(".f5-statement", true));
  await t.setText("SELECT 1 AS first_one;\n\nSELECT 2 AS second_one;");
  await t.cursorToLine(3);
  await t.clearToasts();
  await press("F5");
  await t.waitToast("Query finished", 15000);
  await sleep(300);
  const one = { tabs: await resultTabs(), head: await headers() };
  // everything selected: the whole script
  await t.focusEditor();
  await press("Control+A");
  await t.clearToasts();
  await press("F5");
  await t.waitToast("Query finished", 15000);
  await sleep(300);
  const all = await resultTabs();
  await options("Editor & completion", () => setCheck(".f5-statement", false));
  await t.cursorToLine(3);
  await t.clearToasts();
  await press("F5");
  await t.waitToast("Query finished", 15000);
  await sleep(300);
  const off = await resultTabs();
  check(
    "F5 runs the statement at the cursor",
    one.tabs.length === 1 && one.head.join() === "second_one" && all.length === 2 && off.length === 2,
    JSON.stringify({ one, all, off }),
  );
});

// -- 3. theme follows Windows; Ctrl+wheel zoom -------------------------------------
await step("theme follows Windows", async () => {
  await options("Editor & completion", () => setField(".settings-modal select", "system"));
  const scheme = async (value) => {
    await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value }] });
    return t.until(async () => ((await evaluate(`document.documentElement.dataset.theme`)) === value ? value : null), 3000);
  };
  const light = await scheme("light");
  const dark = await scheme("dark");
  await send("Emulation.setEmulatedMedia", { features: [] });
  await options("Editor & completion", () => setField(".settings-modal select", original.editor.theme));
  const restored = await evaluate(`document.documentElement.dataset.theme`);
  check("theme follows Windows", light === "light" && dark === "dark" && restored === original.editor.theme, `${light} ${dark} ${restored}`);
});

await step("Ctrl+wheel zooms the editor", async () => {
  const size = original.editor.fontSize;
  const box = await evaluate(`(() => { const r = document.querySelector(".editor").getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`);
  const wheel = (deltaY) => send("Input.dispatchMouseEvent", { type: "mouseWheel", x: box.x, y: box.y, deltaX: 0, deltaY, modifiers: 2 });
  await wheel(-120);
  await wheel(-120);
  const bigger = await t.until(async () => ((await t.statusText()).includes(`Editor font size ${size + 2}`) ? true : null), 3000);
  await sleep(800);
  const saved = (await invoke("get_settings")).editor.fontSize;
  await wheel(120);
  await wheel(120);
  await sleep(800);
  const restored = (await invoke("get_settings")).editor.fontSize;
  check("Ctrl+wheel zooms the editor", !!bigger && saved === size + 2 && restored === size, `${saved} → ${restored}`);
});

// -- 4, 5, 6: results as text, one row as a form, foreign keys -------------------------
await step("results as text", async () => {
  await t.runSql("SELECT Id, CustomerId, Total FROM dbo.Orders ORDER BY Id");
  await t.clickButton(".view-switch", "Text");
  const shown = await t.until(() => evaluate(`document.querySelector(".text-view")?.innerText ?? ""`), 3000);
  await t.clickButton(".view-switch", "Grid");
  const grid = await t.until(() => evaluate(`!!document.querySelector(".grid")`), 3000);
  check(
    "results as text",
    /^ Id \| CustomerId \| Total/m.test(shown ?? "") && /\(\d+ rows\)/.test(shown ?? "") && !!grid,
    JSON.stringify((shown ?? "").slice(0, 120)),
  );
});

await step("one row as a form", async () => {
  await t.runSql("SELECT * FROM dbo.Customers ORDER BY Id");
  await evaluate(`document.querySelectorAll(".grid-row")[0].children[2].dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }))`);
  await t.clickButton(".grid-toolbar", "Record");
  const first = await t.until(() => evaluate(`document.querySelector(".record-head b")?.textContent ?? null`), 3000);
  const fields = await evaluate(`[...document.querySelectorAll(".record-name")].map((n) => n.firstChild.textContent.trim())`);
  const value = await evaluate(`[...document.querySelectorAll(".record-value")][1]?.textContent`);
  await evaluate(`document.querySelector(".grid").focus()`);
  await press("ArrowDown");
  const second = await t.until(async () => {
    const h = await evaluate(`document.querySelector(".record-head b")?.textContent ?? ""`);
    return h.startsWith("Row 2") ? h : null;
  }, 3000);
  await t.clickButton(".grid-toolbar", "Record");
  check(
    "one row as a form",
    first?.startsWith("Row 1 of 3") && fields.includes("Name") && value === "Nguyễn Văn A" && !!second,
    `${first} / ${second} / ${fields.join(",")} / ${value}`,
  );
});

await step("Ctrl+click a foreign key opens the row", async () => {
  await t.runSql("SELECT * FROM dbo.Orders ORDER BY Id");
  const linked = await t.until(() => evaluate(`[...document.querySelectorAll(".grid-head")].some((h) => h.textContent.includes("↗") && h.textContent.includes("CustomerId"))`), 5000);
  const col = await evaluate(`[...document.querySelectorAll(".grid-head")].findIndex((h) => h.textContent.includes("CustomerId"))`);
  const key = await evaluate(`document.querySelectorAll(".grid-row")[2].children[${col} + 1].textContent`);
  await t.clearToasts();
  await evaluate(`document.querySelectorAll(".grid-row")[2].children[${col} + 1].dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0, ctrlKey: true }))`);
  const opened = await t.until(async () => ((await t.activeTitle()) === `Customers (${key})` ? true : null), 10000);
  await t.waitToast("Query finished", 20000);
  await sleep(300);
  const rows = await t.gridRows();
  check("Ctrl+click a foreign key opens the row", !!linked && !!opened && rows.length === 1 && rows[0][0] === key, `${await t.activeTitle()} ${JSON.stringify(rows)}`);
  await press("Control+W");
  await sleep(300);
  await back();
});

await step("right-click: the rows that reference this one", async () => {
  await t.runSql("SELECT Id, CustomerId FROM dbo.Orders WHERE Id = 1");
  await sleep(500);
  await evaluate(`(() => {
    const cell = document.querySelectorAll(".grid-row")[0].children[1];
    const r = cell.getBoundingClientRect();
    cell.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: r.x + 5, clientY: r.y + 5 }));
  })()`);
  const items = await t.until(() => evaluate(`document.querySelector(".grid-menu") ? [...document.querySelectorAll(".grid-menu button")].map((b) => b.textContent) : null`), 3000);
  await sleep(200);
  // the results are at the bottom of the window: the menu still fits in it
  const inView = await evaluate(`(() => { const r = document.querySelector(".grid-menu").getBoundingClientRect(); return r.top >= 0 && r.bottom <= innerHeight && r.right <= innerWidth; })()`);
  check("the row menu fits in the window", inView);
  await t.clearToasts();
  await t.clickButton(".grid-menu", "← Rows of dbo.OrderItems");
  const opened = await t.until(async () => ((await t.activeTitle()).startsWith("OrderItems by OrderId (1)") ? true : null), 10000);
  await t.waitToast("Query finished", 20000);
  await sleep(300);
  const rows = await t.gridRows();
  check(
    "right-click: the rows that reference this one",
    !!items && items.some((x) => x.includes("Open the dbo.Customers row (Id = 1)")) && !!opened && rows.length > 0,
    `${JSON.stringify(items)} ${await t.activeTitle()} ${rows.length}`,
  );
  await press("Control+W");
  await sleep(300);
  await back();
});

// -- CSV formula guard and the text table, as copied -------------------------------------
await step("CSV formula guard", async () => {
  await t.runSql("SELECT N'=1+2' AS f, -5 AS n, N'+84 90' AS p");
  await setField(".copy-as", "csv");
  const csv = await t.until(clip, 3000);
  await setField(".copy-as", "text");
  await sleep(300);
  const table = await clip();
  check("CSV formula guard", csv === "f,n,p\r\n'=1+2,-5,'+84 90" && table.includes("(1 row)") && table.includes(" =1+2"), JSON.stringify({ csv, table }));
});

// -- 7. table data as INSERT -------------------------------------------------------------
async function explorerTable(name) {
  if (!(await evaluate(`!!document.querySelector(".explorer-panel")`))) await press("F8");
  await t.until(() => evaluate(`!!document.querySelector(".explorer-panel")`), 3000);
  return evaluate(`(() => {
    const item = [...document.querySelectorAll(".tree-item")].find((i) => i.querySelector(".tree-name")?.textContent === "dbo.${name}");
    item?.click();
    return !!item;
  })()`);
}

await step("table data as INSERT", async () => {
  await explorerTable("Customers");
  await sleep(200);
  await t.clickButton(".tree-actions", "Data as INSERT");
  await t.until(() => evaluate(`!!document.querySelector(".data-script-modal")`), 3000);
  await t.clearToasts();
  await t.clickButton(".data-script-modal .modal-actions", "Open script");
  const opened = await t.until(async () => ((await t.activeTitle()) === "Customers (data)" ? true : null), 10000);
  const toast = await t.waitToast("row(s) scripted", 10000);
  await sleep(500);
  const script = await ui.editorText();
  check(
    "table data as INSERT",
    !!opened && /3 row\(s\) scripted/.test(toast ?? "") && script.includes("-- Data of dbo.Customers: 3 row(s)") && script.includes("INSERT INTO dbo.Customers"),
    `${toast} ${JSON.stringify(script.slice(0, 160))}`,
  );
  await press("Control+W");
  await sleep(300);
  await back();
});

// -- 8. connection folders ------------------------------------------------------------------
await step("connection folders", async () => {
  await evaluate(`document.querySelector(".toolbar button").click()`);
  await t.until(() => evaluate(`!!document.querySelector(".conn-modal")`), 5000);
  const pick = () =>
    t.until(async () => {
      await evaluate(`[...document.querySelectorAll(".conn-item")].find((e) => e.textContent.includes("E2E SQL Server"))?.click()`);
      return (await evaluate(`document.querySelector(".conn-form input")?.value`)) === "E2E SQL Server";
    });
  await pick();
  await setField(".conn-group-input", "E2E Folder");
  await t.clickButton(".conn-modal .modal-actions", "Save");
  const grouped = await t.until(() => evaluate(`(() => { const h = [...document.querySelectorAll(".conn-folder")].map((x) => x.textContent); return h.some((x) => x.includes("E2E Folder")) ? h : null; })()`), 5000);
  const visible = () => evaluate(`[...document.querySelectorAll(".conn-item")].some((e) => e.textContent.includes("E2E SQL Server"))`);
  await evaluate(`[...document.querySelectorAll(".conn-folder")].find((x) => x.textContent.includes("E2E Folder")).click()`);
  await sleep(200);
  const folded = !(await visible());
  await evaluate(`[...document.querySelectorAll(".conn-folder")].find((x) => x.textContent.includes("E2E Folder")).click()`);
  await sleep(200);
  const unfolded = await visible();
  await pick();
  await setField(".conn-group-input", "");
  await t.clickButton(".conn-modal .modal-actions", "Save");
  await sleep(500);
  const saved = (await invoke("list_connections")).find((c) => c.id === CONN);
  await t.clickButton(".conn-modal .modal-actions", "Close");
  check("connection folders", !!grouped && folded && unfolded && !saved?.group, `${JSON.stringify(grouped)} folded ${folded} unfolded ${unfolded} group ${saved?.group}`);
});

// -- 9. find a value; 12b. the question before committing procedure changes ------------------
await step("find a value in every text column", async () => {
  await back();
  // pasted: typed, Monaco would close the quotes and brackets itself
  await t.paste("CREATE TABLE dbo.E2E_Find (Id int PRIMARY KEY, Note nvarchar(50)); INSERT INTO dbo.E2E_Find VALUES (1, N'needle-e2e 50%'), (2, N'hay');");
  await t.runSql(null);
  await t.until(async () => (await catalogNames()).includes("E2E_Find"), 15000);
  await t.clickButton(".explorer-panel .panel-search-row", "Find value");
  await t.until(() => evaluate(`!!document.querySelector(".find-value-modal")`), 3000);
  await setField(".find-value", "needle-e2e 50%");
  await t.clickButton(".find-value-modal .modal-actions", "Open script");
  const opened = await t.until(async () => ((await t.activeTitle()).startsWith("Find 'needle-e2e") ? true : null), 10000);
  await connected();
  await t.runSql(null);
  await sleep(300);
  const rows = await t.gridRows();
  check(
    "find a value in every text column",
    !!opened && rows.length === 1 && rows[0].slice(0, 3).join("|") === "dbo.E2E_Find|Note|1" && rows[0][3] === "needle-e2e 50%",
    JSON.stringify(rows),
  );
  await press("Control+W");
  await sleep(300);
  await back();
});

await step("ask before committing what procedures changed", async () => {
  await options("Safety", () => setCheck(".confirm-calls", true));
  await t.runSql("CREATE OR ALTER PROCEDURE dbo.E2E_Touch AS UPDATE dbo.E2E_Find SET Note = N'touched' WHERE Id = 2");
  await t.runSql("CREATE OR ALTER PROCEDURE dbo.E2E_Read AS SELECT COUNT(*) AS n FROM dbo.E2E_Find");
  await t.runSql("EXEC dbo.E2E_Touch");
  const asked = await t.until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? null`), 5000);
  await t.clickButton(".confirm-modal", "Roll back");
  await t.waitToast("rolled back", 10000);
  const note = (await t.runSql("SELECT Note FROM dbo.E2E_Find WHERE Id = 2"), (await t.gridRows())[0]?.[0]);
  // a procedure that only reads is committed without a question
  await t.runSql("EXEC dbo.E2E_Read");
  await sleep(1500);
  const quiet = !(await evaluate(`!!document.querySelector(".confirm-modal")`)) && !(await evaluate(`!!document.querySelector(".tab.active .tab-badge.tx")`));
  await options("Safety", () => setCheck(".confirm-calls", false));
  check(
    "ask before committing what procedures changed",
    /calls EXEC dbo\.E2E_Touch/.test(asked ?? "") && note === "hay" && quiet,
    `${JSON.stringify(asked)} note ${note} quiet ${quiet}`,
  );
});

// -- 10. SQLCMD variables ------------------------------------------------------------------
await step("SQLCMD variables", async () => {
  await t.setText(":setvar Tbl Customers\n:setvar Who \"Phạm\"\nSELECT N'$(Who)' AS who, COUNT(*) AS n FROM dbo.$(Tbl)");
  await t.runSql(null);
  await sleep(300);
  const whole = await t.gridRows();
  // the statement at the cursor keeps the variables defined above it
  await t.cursorToLine(3);
  await t.runSql(null, { statement: true });
  await sleep(300);
  const part = await t.gridRows();
  await t.clickButton(".toolbar", "Format");
  await sleep(800);
  const formatted = await ui.editorText();
  check(
    "SQLCMD variables",
    whole[0]?.join("|") === "Phạm|3" && part[0]?.join("|") === "Phạm|3" && formatted.includes(":setvar Tbl Customers") && formatted.includes("dbo.$(Tbl)"),
    JSON.stringify({ whole, part, formatted: formatted.slice(0, 120) }),
  );
});

// -- 11. backup and restore scripts --------------------------------------------------------
let backupPath = "";
await step("backup script", async () => {
  await t.clickButton(".explorer-panel .panel-search-row", "Backup");
  await t.until(() => evaluate(`!!document.querySelector(".backup-modal")`), 3000);
  backupPath = await t.until(() => evaluate(`document.querySelector(".backup-path")?.value || null`), 10000);
  await t.clickButton(".backup-modal .modal-actions", "Open backup script");
  const opened = await t.until(async () => ((await t.activeTitle()) === "Backup Shop" ? true : null), 10000);
  await connected();
  const toast = await t.runSql(null);
  check("backup script", !!opened && /Query finished/.test(toast ?? "") && !!backupPath, `${backupPath} ${toast}`);
  await press("Control+W");
  await sleep(300);
  await back();
});

const copyExists = async () => (await t.runSql("SELECT COUNT(*) FROM sys.databases WHERE name = N'Shop_E2E_Copy'"), (await t.gridRows())[0]?.[0] === "1");
const dropCopy = () =>
  t.runSql("IF DB_ID(N'Shop_E2E_Copy') IS NOT NULL BEGIN USE master; ALTER DATABASE Shop_E2E_Copy SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE Shop_E2E_Copy; END");

await step("restore script, as a copy", async () => {
  // a copy left by an interrupted run would make the restore refuse (the safe answer)
  if (await copyExists()) await dropCopy();
  await t.clickButton(".explorer-panel .panel-search-row", "Backup");
  await t.until(() => evaluate(`!!document.querySelector(".backup-modal")`), 3000);
  await t.clickButton(".backup-modal .settings-tabs", "Restore");
  await sleep(300);
  await setField(".restore-path", backupPath);
  await t.clickButton(".backup-modal .settings-form", "Read the file");
  const moves = await t.until(() => evaluate(`document.querySelectorAll(".restore-move").length || null`), 10000);
  await setField(".restore-target", "Shop_E2E_Copy");
  await sleep(500);
  const paths = await evaluate(`[...document.querySelectorAll(".restore-move input")].map((i) => i.value)`);
  await t.clickButton(".backup-modal .modal-actions", "Open restore script");
  const opened = await t.until(async () => ((await t.activeTitle()) === "Restore Shop_E2E_Copy" ? true : null), 10000);
  await connected();
  const toast = await t.runSql(null);
  const copy = (await t.runSql("SELECT COUNT(*) FROM Shop_E2E_Copy.dbo.Customers"), (await t.gridRows())[0]?.[0]);
  const dropped = await dropCopy();
  const gone = !(await copyExists());
  check(
    "restore script, as a copy",
    moves === 2 && paths.every((p) => p.includes("Shop_E2E_Copy")) && !!opened && /Query finished/.test(toast ?? "") && copy === "3" && gone,
    `${moves} ${JSON.stringify(paths)} ${toast} copy ${copy} dropped ${dropped} gone ${gone}`,
  );
  await press("Control+W");
  await sleep(300);
  await back();
});

// -- 12a. query history kept for N days ----------------------------------------------------
await step("history kept for N days", async () => {
  // ten years: nothing of the user's history goes
  await options("Editor & completion", () => setField(".history-days", "3650"));
  const kept = (await invoke("get_settings")).editor.historyDays;
  await options("Editor & completion", () => setField(".history-days", String(original.editor.historyDays ?? 0)));
  const restored = (await invoke("get_settings")).editor.historyDays;
  check("history kept for N days", kept === 3650 && restored === (original.editor.historyDays ?? 0), `${kept} → ${restored}`);
});

// -- SQLCMD variables cannot hide a change from the checks before a run -------------------
await step("SQLCMD variables are checked as run", async () => {
  await back();
  await t.paste(':setvar Cmd "DELETE FROM dbo.Orders"\n$(Cmd)');
  await t.clearToasts();
  await press("F5");
  const warned = await t.until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? null`), 5000);
  await t.clickButton(".confirm-modal", "Cancel");
  const cancelled = await t.waitToast("Execution cancelled", 5000);
  check("SQLCMD variables are checked as run", /DELETE dbo\.Orders has no WHERE clause/.test(warned ?? "") && !!cancelled, JSON.stringify(warned));
});

// -- PostgreSQL procedures that commit by themselves, on a manual-commit connection -------
await step("PostgreSQL procedures that commit run under manual commit", async () => {
  const pg = (await invoke("list_connections")).find((c) => c.id === "e2e-pg");
  if (!pg) throw new Error("the E2E PostgreSQL connection is missing (run e2e-db first)");
  const { hasPassword, ...config } = pg;
  await invoke("save_connection", { config: { ...config, id: "e2e-pg-manual", name: "E2E PG Manual", manualCommit: true }, password: t.password, forgetPassword: false });
  // the procedure, made on the usual connection (it commits each statement)
  await t.connectNewTab("E2E PostgreSQL");
  await t.paste(
    "CREATE TABLE IF NOT EXISTS e2e_commit_log (a int);\nCREATE OR REPLACE PROCEDURE e2e_commit() LANGUAGE plpgsql AS $$ BEGIN INSERT INTO e2e_commit_log VALUES (1); COMMIT; END $$;",
  );
  await t.runSql(null);
  const plainTab = await t.activeTitle();
  await t.connectNewTab("E2E PG Manual");
  const toast = await t.runSql("CALL e2e_commit()");
  const notice = await t.waitToast("commits by itself", 5000);
  const open = await evaluate(`!!document.querySelector(".tab.active .tab-badge.tx")`);
  // created and called in one run: Sqlotus cannot know yet, and says why it failed
  await t.paste("CREATE OR REPLACE PROCEDURE e2e_commit2() LANGUAGE plpgsql AS $$ BEGIN COMMIT; END $$;\nCALL e2e_commit2();");
  await t.runSql(null);
  const explained = await t.waitToast("does not allow inside the", 5000);
  const openAfter = await evaluate(`!!document.querySelector(".tab.active .tab-badge.tx")`);
  check(
    "PostgreSQL procedures that commit run under manual commit",
    /Query finished/.test(toast ?? "") && !!notice && !open && !!explained && !openAfter,
    `${toast} | ${notice} | open ${open} | ${explained} | open ${openAfter}`,
  );
  await press("Control+W");
  await sleep(300);
  await t.selectTab(plainTab);
  await t.runSql("DROP PROCEDURE IF EXISTS e2e_commit(); DROP PROCEDURE IF EXISTS e2e_commit2(); DROP TABLE IF EXISTS e2e_commit_log;");
  await press("Control+W");
  await sleep(300);
  await invoke("delete_connection", { id: "e2e-pg-manual" });
  await back();
});

// -- the statement at the cursor keeps its CTE ----------------------------------------------
await step("the statement at the cursor keeps its CTE", async () => {
  await back();
  await t.runSql("DROP TABLE IF EXISTS dbo.E2E_Cte; CREATE TABLE dbo.E2E_Cte (Id int PRIMARY KEY, Yr int); INSERT dbo.E2E_Cte VALUES (1, 2019), (2, 2019), (3, 2024);");
  // the CTE has the table's name: the DELETE alone would empty the table
  await t.paste("WITH E2E_Cte AS (\n    SELECT * FROM dbo.E2E_Cte WHERE Yr < 2020\n)\n\nDELETE FROM E2E_Cte");
  await t.cursorToLine(5);
  const toast = await t.runSql(null, { statement: true });
  const left = (await t.runSql("SELECT COUNT(*) FROM dbo.E2E_Cte"), (await t.gridRows())[0]?.[0]);
  check("the statement at the cursor keeps its CTE", left === "1", `${left} row(s) left · ${toast}`);
});

// -- a procedure named first in a batch runs without EXEC, also with variable values --------
await step("a procedure named first in a batch", async () => {
  await back();
  await t.runSql("CREATE OR ALTER PROCEDURE dbo.E2E_Echo @Value int AS SELECT @Value + 1 AS v");
  await t.paste("dbo.E2E_Echo @Value = @x");
  await t.clearToasts();
  await press("F5");
  const asked = await t.until(() => evaluate(`document.querySelector(".variables-modal")?.innerText ?? null`), 5000);
  await setField(".variable-value", "41");
  await t.clickButton(".variables-modal .modal-actions", "Run");
  const toast = await t.waitToast("Query finished", 10000);
  const rows = await t.gridRows();
  // @Value names the procedure's parameter: only @x is asked for
  check(
    "a procedure named first in a batch",
    !!asked && asked.includes("@x") && !asked.includes("@Value") && rows[0]?.[0] === "42",
    `${JSON.stringify(asked)} ${JSON.stringify(rows)} ${toast}`,
  );
});

// -- PostgreSQL: switching database with an open transaction asks first --------------------
await step("PostgreSQL database switch with an open transaction", async () => {
  await t.connectNewTab("E2E PostgreSQL");
  const pgTab = await t.activeTitle();
  const from = await evaluate(`document.querySelector(".toolbar select[title=Database]")?.value`);
  const other = await evaluate(`[...document.querySelectorAll(".toolbar select[title=Database] option")].map((o) => o.value).find((v) => v && v !== ${JSON.stringify(from)})`);
  await t.runSql("BEGIN; CREATE TEMP TABLE e2e_tx (a int);");
  const open = await t.until(() => evaluate(`!!document.querySelector(".tab.active .tab-badge.tx")`), 5000);
  const ask = () => t.until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? null`), 5000);
  await setField(".toolbar select[title=Database]", other);
  const asked = await ask();
  await t.clickButton(".confirm-modal", "Cancel");
  await sleep(300);
  const kept = (await evaluate(`document.querySelector(".toolbar select[title=Database]")?.value`)) === from && (await evaluate(`!!document.querySelector(".tab.active .tab-badge.tx")`));
  await setField(".toolbar select[title=Database]", other);
  await ask();
  await t.clickButton(".confirm-modal", "Roll back and switch");
  const switched = await t.waitToast(`Switched to database ${other}`, 10000);
  const closed = !(await evaluate(`!!document.querySelector(".tab.active .tab-badge.tx")`));
  check(
    "PostgreSQL database switch with an open transaction",
    !!open && /open transaction/.test(asked ?? "") && kept && !!switched && closed,
    `${from} → ${other} | open ${open} | ${JSON.stringify(asked)} | kept ${kept} | ${switched} | closed ${closed}`,
  );
  await t.selectTab(pgTab);
  await press("Control+W");
  await sleep(300);
  await back();
});

// -- F8 toggles Object Explorer while the editor has the focus ------------------------------
await step("F8 in the editor toggles Object Explorer", async () => {
  await back();
  const shown = () => evaluate(`!!document.querySelector(".explorer-panel")`);
  const before = await shown();
  await t.focusEditor();
  await press("F8");
  const toggled = await t.until(async () => (await shown()) !== before, 3000);
  await t.focusEditor();
  await press("F8");
  const restored = await t.until(async () => (await shown()) === before, 3000);
  check("F8 in the editor toggles Object Explorer", !!toggled && !!restored, `${before} → ${toggled} → ${restored}`);
});

// -- clean up --------------------------------------------------------------------------------
await back();
await t.runSql(
  "DROP PROCEDURE IF EXISTS dbo.E2E_Touch; DROP PROCEDURE IF EXISTS dbo.E2E_Read; DROP PROCEDURE IF EXISTS dbo.E2E_Echo; DROP TABLE IF EXISTS dbo.E2E_Find; DROP TABLE IF EXISTS dbo.E2E_AutoRefresh; DROP TABLE IF EXISTS dbo.E2E_InTx; DROP TABLE IF EXISTS dbo.E2E_Cte;",
);
await t.until(async () => !(await catalogNames()).includes("E2E_Find"), 15000);
const now = await invoke("get_settings");
check(
  "settings are as they were",
  now.editor.theme === original.editor.theme &&
    now.editor.fontSize === original.editor.fontSize &&
    !!now.editor.f5RunsStatement === !!original.editor.f5RunsStatement &&
    (now.editor.historyDays ?? 0) === (original.editor.historyDays ?? 0) &&
    !!now.safety.confirmProcedureCalls === !!original.safety.confirmProcedureCalls,
  JSON.stringify(now.editor),
);
await t.closeTabsSince(before);
process.exit(t.summary() ? 1 : 0);
