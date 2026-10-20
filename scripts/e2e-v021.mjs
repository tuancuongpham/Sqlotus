// End-to-end checks for the 0.2.1 tools: "used by", index health, the
// activity monitor with a real blocking chain, editing result rows, copy
// as Markdown/JSON, and production connections. Uses the Docker "Shop" and
// "Big" databases (connection "E2E SQL Server").
//
//   SQLOTUS_E2E_PASSWORD=... node scripts/e2e-v021.mjs [screenshot-dir]
import { mkdirSync } from "node:fs";
import { attach, checker } from "./cdp.mjs";

const outDir = process.argv[2] ?? "e2e-out";
mkdirSync(outDir, { recursive: true });
const password = process.env.SQLOTUS_E2E_PASSWORD;
if (!password) throw new Error("set SQLOTUS_E2E_PASSWORD");
const ui = await attach(outDir);
const { sleep, evaluate, waitFor, click, press, type, shot, invoke } = ui;
const { check, summary } = checker();

async function until(fn, timeout = 10000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const v = await fn();
    if (v) return v;
    await sleep(150);
  }
  return null;
}
const toastText = () => evaluate(`[...document.querySelectorAll(".toast")].map((t) => t.innerText.replace(/\\s+/g, " ")).join(" | ")`);
const waitToast = (needle, timeout = 10000) => until(async () => ((await toastText()).includes(needle) ? toastText() : null), timeout);
const clearToasts = () => evaluate(`document.querySelectorAll(".toast-close").forEach((b) => b.click())`);
const clickButton = (scope, label) =>
  evaluate(`(() => { const b = [...document.querySelectorAll(${JSON.stringify(scope + " button")})].find((x) => x.textContent.trim().startsWith(${JSON.stringify(label)})); b?.click(); return !!b; })()`);
const text = (sel) => evaluate(`document.querySelector(${JSON.stringify(sel)})?.innerText ?? ""`);
const tabTitles = () => evaluate(`[...document.querySelectorAll(".tab")].map((t) => t.textContent)`);
const activeTitle = () => evaluate(`document.querySelector(".tab.active .tab-title")?.textContent ?? ""`);
const selectTab = (title) => evaluate(`[...document.querySelectorAll(".tab")].find((t) => t.querySelector(".tab-title")?.textContent === ${JSON.stringify(title)})?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }))`);
const before = new Set(await tabTitles());

async function connectNewTab(label) {
  await press("Control+N");
  await sleep(300);
  await evaluate(`document.querySelector(".toolbar button").click()`);
  await waitFor(".conn-modal");
  await until(() =>
    evaluate(`(() => { const e = [...document.querySelectorAll(".conn-item")].find((e) => e.textContent.includes(${JSON.stringify(label)})); e?.click(); return !!e; })()`),
  );
  await until(async () => (await evaluate(`document.querySelector(".conn-form input")?.value`)) === label);
  await evaluate(`[...document.querySelectorAll(".modal-actions button")].find((b) => b.textContent === "Connect").click()`);
  const ok = await waitToast(`Connected to ${label}`, 20000);
  await until(async () => (await evaluate(`document.querySelector(".status-msg").textContent`)).includes("objects loaded"), 15000);
  return ok;
}

async function setText(sql) {
  await click(".editor .view-lines");
  await sleep(150);
  await press("Control+A");
  await press("Delete");
  await type(sql);
  await press("Escape");
}

async function run(sql) {
  await clearToasts();
  await setText(sql);
  await press("F5");
}

const gridColumn = (c) => evaluate(`[...document.querySelectorAll(".grid-row")].map((r) => r.children[${c}].textContent)`);

try {
  check("connect", !!(await connectNewTab("E2E SQL Server")));
  const tabA = await activeTitle();
  if (!(await evaluate(`!!document.querySelector(".explorer-panel")`))) await press("F8");

  // 1. used by: a table and a column
  await evaluate(`[...document.querySelectorAll(".tree-item")].find((e) => e.querySelector(".tree-name")?.textContent.trim() === "dbo.Customers")?.click()`);
  await sleep(300);
  await clickButton(".tree-actions", "Used by");
  const refs = await until(async () => {
    const t = await text(".problems.references");
    return t.includes("use(s) of") ? t : null;
  }, 8000);
  check("used by: table", !!refs && refs.includes("vCustomerTotals") && refs.includes("foreign key"), (refs ?? "").replace(/\n/g, " | ").slice(0, 160));
  await evaluate(`document.querySelector(".problems.references .problems-head button")?.click()`);
  await evaluate(`[...document.querySelectorAll(".tree-item")].find((e) => e.querySelector(".tree-name")?.textContent.trim() === "dbo.Customers")?.querySelector(".tree-toggle")?.click()`);
  await sleep(300);
  await evaluate(`[...document.querySelectorAll(".tree-leaf")].find((e) => e.textContent.startsWith("·Name") || e.textContent.includes("Name nvarchar"))?.querySelector(".leaf-action")?.click()`);
  const colRefs = await until(async () => {
    const t = await text(".problems.references");
    return t.includes("Customers.Name") ? t : null;
  }, 8000);
  check("used by: column", !!colRefs && colRefs.includes("vCustomerTotals"), (colRefs ?? "").replace(/\n/g, " | ").slice(0, 160));
  await evaluate(`document.querySelector(".problems.references .problems-head button")?.click()`);
  await shot("u1-used-by");

  // 2. edit rows: change a value, add and delete a row, all saved in one transaction
  await run("SELECT * FROM dbo.Customers ORDER BY Id");
  await waitToast("Query finished", 8000);
  await clickButton(".grid-toolbar", "✎ Edit");
  const editing = await until(() => evaluate(`!!document.querySelector(".grid-edit-bar")`), 5000);
  check("edit mode", !!editing, await toastText());
  const cellXY = (r, c) =>
    evaluate(`(() => { const el = [...document.querySelectorAll(".grid-row")][${r}].children[${c}]; const b = el.getBoundingClientRect(); return { x: b.x + 8, y: b.y + b.height / 2 }; })()`);
  const dbl = async (r, c) => {
    const p = await cellXY(r, c);
    for (const t of ["mousePressed", "mouseReleased"]) await ui.send("Input.dispatchMouseEvent", { type: t, x: p.x, y: p.y, button: "left", clickCount: 1 });
    for (const t of ["mousePressed", "mouseReleased"]) await ui.send("Input.dispatchMouseEvent", { type: t, x: p.x, y: p.y, button: "left", clickCount: 2 });
    await sleep(200);
  };
  // row 3 (John Smith), column 3 = Country
  await dbl(2, 3);
  await press("Control+A");
  await type("GB");
  await press("Enter");
  await sleep(200);
  check("changed cell is marked", await evaluate(`[...document.querySelectorAll(".grid-row")][2].children[3].classList.contains("cell-changed")`));
  await clickButton(".grid-edit-bar", "＋ Row");
  await sleep(300);
  await dbl(3, 2);
  await type("E2E Row");
  await press("Enter");
  await sleep(200);
  await clickButton(".grid-edit-bar", "Script");
  const script = await until(() => text(".edit-script"), 5000);
  check(
    "script preview",
    !!script && script.includes("UPDATE dbo.Customers SET Country = 'GB' WHERE Id = 3 AND Country = 'US'") && script.includes("INSERT INTO dbo.Customers (Name) VALUES (N'E2E Row')"),
    (script ?? "").replace(/\n/g, " | "),
  );
  await clickButton(".edit-script-modal .modal-actions", "Close");
  await clickButton(".grid-edit-bar", "Save");
  check("edits saved", !!(await waitToast("2 change(s) saved", 10000)), await toastText());
  await until(async () => (await gridColumn(2)).includes("E2E Row"), 8000);
  const names = await gridColumn(2);
  const countries = await gridColumn(3);
  check("result reloaded with the changes", names.length === 4 && countries[2] === "GB", `${names} / ${countries}`);
  // put the data back: change GB → US, delete the new row
  await clickButton(".grid-toolbar", "✎ Edit");
  await until(() => evaluate(`!!document.querySelector(".grid-edit-bar")`), 5000);
  await dbl(2, 3);
  await press("Control+A");
  await type("US");
  await press("Enter");
  const rowNum = await evaluate(`(() => { const el = [...document.querySelectorAll(".grid-row")][3].children[0]; const b = el.getBoundingClientRect(); return { x: b.x + 5, y: b.y + b.height / 2 }; })()`);
  for (const t of ["mousePressed", "mouseReleased"]) await ui.send("Input.dispatchMouseEvent", { type: t, x: rowNum.x, y: rowNum.y, button: "left", clickCount: 1 });
  await sleep(200);
  await clickButton(".grid-edit-bar", "Delete row");
  await clickButton(".grid-edit-bar", "Save");
  check("update + delete saved", !!(await waitToast("2 change(s) saved", 10000)), await toastText());
  await until(async () => (await gridColumn(2)).length === 3, 8000);
  check("data restored", JSON.stringify(await gridColumn(3)) === '["VN","VN","US"]', JSON.stringify(await gridColumn(3)));
  // a result from a join cannot be edited
  await run("SELECT c.Name, o.Total FROM dbo.Customers c JOIN dbo.Orders o ON o.CustomerId = c.Id");
  await waitToast("Query finished", 8000);
  await clearToasts();
  await clickButton(".grid-toolbar", "✎ Edit");
  check("join result is refused", !!(await waitToast("cannot be edited", 5000)), await toastText());

  // 3. copy as Markdown / JSON
  await evaluate(`navigator.clipboard.writeText = async (t) => { window.__copied = t; }`);
  await run("SELECT Id, Name FROM dbo.Customers WHERE Id <= 2 ORDER BY Id");
  await waitToast("Query finished", 8000);
  const copyAs = (v) => evaluate(`(() => { const s = document.querySelector(".copy-as"); s.value = ${JSON.stringify(v)}; s.dispatchEvent(new Event("change", { bubbles: true })); })()`);
  await copyAs("markdown");
  await sleep(300);
  const md = await evaluate(`window.__copied ?? ""`);
  check("copy as Markdown", md.startsWith("| Id | Name |\n| ---: | --- |\n| 1 | Nguyễn Văn A |"), JSON.stringify(md.slice(0, 80)));
  await copyAs("json");
  await sleep(300);
  const json = JSON.parse((await evaluate(`window.__copied ?? "[]"`)) || "[]");
  check("copy as JSON", json.length === 2 && json[0].Id === 1 && json[1].Name === "Trần Thị B", JSON.stringify(json));

  // 4. activity monitor with a real blocking chain
  await run("BEGIN TRAN; UPDATE dbo.Customers SET Country = Country WHERE Id = 1");
  await waitToast("transaction still open", 8000);
  check("connect second tab", !!(await connectNewTab("E2E SQL Server")));
  const tabB = await activeTitle();
  await setText("SELECT * FROM dbo.Customers WHERE Id = 1");
  await press("F5");
  await sleep(1500);
  check("second query is blocked", await evaluate(`!!document.querySelector(".tab.active .spinner")`));
  await clickButton(".toolbar", "Activity");
  const chain = await until(async () => {
    const t = await text(".blocking-chains");
    return t.includes("blocks 1 session") ? t : null;
  }, 12000);
  await shot("u4-activity");
  check("blocking chain shown", !!chain, (chain ?? (await text(".activity-modal"))).replace(/\n/g, " | ").slice(0, 200));
  await evaluate(`document.querySelector(".blocking-row")?.click()`);
  await sleep(300);
  await clickButton(".activity-detail", "Kill session");
  await sleep(200);
  await clickButton(".activity-confirm", "Kill session");
  check("blocker killed", !!(await waitToast("was ended", 8000)), await toastText());
  const released = await until(async () => !(await evaluate(`!!document.querySelector(".tab.active .spinner")`)), 10000);
  check("blocked query finished", !!released);
  await clickButton(".activity-modal .modal-actions", "Close");
  // the killed tab reconnects by itself
  await selectTab(tabA);
  await sleep(300);
  await run("SELECT 1 AS alive");
  const closedNotice = await waitToast("closed this connection", 8000);
  check("killed session reported", !!closedNotice, await toastText());
  await run("SELECT 1 AS alive");
  check("next run reconnects", !!(await waitToast("Query finished", 15000)), await toastText());
  await selectTab(tabB);

  // 5. index health on a database with duplicates and a missing index
  await evaluate(`(() => { const s = document.querySelector(".toolbar select"); s.value = "Big"; s.dispatchEvent(new Event("change", { bubbles: true })); })()`);
  await until(async () => (await evaluate(`document.querySelector(".status-msg").textContent`)).includes("Big:"), 15000);
  await clickButton(".explorer-panel .panel-search-row", "Index health");
  const health = await until(async () => {
    const t = await text(".health-modal");
    return t.includes("Duplicate indexes") ? t : null;
  }, 20000);
  await shot("u5-index-health");
  check(
    "index health findings",
    !!health && health.includes("Missing indexes") && health.includes("Overlapping indexes") && health.includes("IX_dup2"),
    (health ?? "").replace(/\n/g, " | ").slice(0, 240),
  );
  await evaluate(`[...document.querySelectorAll(".health-item")].find((e) => e.textContent.includes("IX_dup2"))?.querySelector("input")?.click()`);
  await clickButton(".health-modal .modal-actions", "Open");
  const opened = await until(async () => {
    const s = await invoke("load_session");
    return s?.tabs?.find((t) => t.text.includes("DROP INDEX IX_dup2 ON dbo.Leaf0_0;"))?.text ?? null;
  }, 10000);
  check("index script opened, not run", !!opened);

  // 6. production connection asks before changes
  const prod = {
    id: "e2e-prod",
    name: "E2E Production",
    dialect: "mssql",
    host: "localhost,14333",
    port: null,
    database: "Shop",
    auth: "sql",
    user: "sa",
    encrypt: false,
    trustServerCertificate: true,
    color: "#c0392b",
    environment: "prod",
    manualCommit: true,
  };
  await invoke("save_connection", { config: prod, password, forgetPassword: false });
  await connectNewTab("E2E Production");
  check("production badge", await evaluate(`!!document.querySelector(".tab.active .tab-badge.prod")`));
  await run("UPDATE dbo.Customers SET Country = Country WHERE Id = 1");
  const ask = await until(async () => {
    const t = await text(".confirm-modal");
    return t.includes("PRODUCTION") ? t : null;
  }, 5000);
  check("production change asks first", !!ask, (ask ?? "").replace(/\n/g, " | "));
  await clickButton(".confirm-modal", "Cancel");
  check("cancelled, nothing run", !!(await waitToast("Execution cancelled", 5000)), await toastText());
  await run("SELECT COUNT(*) FROM dbo.Customers");
  check("reads run without asking", !!(await waitToast("Query finished", 8000)), await toastText());
  await invoke("delete_connection", { id: prod.id });
} finally {
  for (let i = 0; i < 12; i++) {
    const own = (await tabTitles()).findIndex((t) => !before.has(t));
    if (own < 0) break;
    await evaluate(`document.querySelectorAll(".tab")[${own}].dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }))`);
    await sleep(200);
    await press("Control+W");
    await sleep(400);
    await clickButton(".confirm-modal", "Roll back and close");
    await sleep(300);
  }
  await invoke("delete_connection", { id: "e2e-prod" }).catch(() => undefined);
}

const failed = summary();
ui.close();
process.exit(failed ? 1 : 0);
