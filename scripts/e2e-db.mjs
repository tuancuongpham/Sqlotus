// End-to-end test through the app UI against the Docker test databases
// (see scripts/test-db/README.md). Saves three connections named "E2E ...".
//
//   $env:SQLOTUS_E2E_PASSWORD="..."; node scripts/e2e-db.mjs [screenshot-dir]
import { mkdirSync } from "node:fs";
import { attach, checker } from "./cdp.mjs";

const outDir = process.argv[2] ?? "e2e-out";
mkdirSync(outDir, { recursive: true });
const password = process.env.SQLOTUS_E2E_PASSWORD;
if (!password) throw new Error("set SQLOTUS_E2E_PASSWORD");

const ui = await attach(outDir);
const { sleep, evaluate, waitFor, click, press, type, shot, editorText, invoke } = ui;
const { check, summary } = checker();

const servers = [
  {
    config: { id: "e2e-mssql", name: "E2E SQL Server", dialect: "mssql", host: "localhost,14333", port: null, database: "Shop", auth: "sql", user: "sa", encrypt: false, trustServerCertificate: true, color: "#c0392b" },
    query: "SELECT Id, Name FROM dbo.Customers ORDER BY Id",
    joinPrefix: "SELECT * FROM dbo.Orders o JOIN ",
    joinExpect: "dbo.Customers c ON c.Id = o.CustomerId",
  },
  {
    config: { id: "e2e-pg", name: "E2E PostgreSQL", dialect: "postgres", host: "localhost", port: 54329, database: "shop", auth: "sql", user: "postgres", encrypt: false, trustServerCertificate: true, color: "#2980b9" },
    query: "SELECT id, name FROM customers ORDER BY id",
    joinPrefix: "SELECT * FROM orders o JOIN ",
    joinExpect: "JOIN customers c ON c.id = o.customer_id",
  },
  {
    config: { id: "e2e-mysql", name: "E2E MySQL", dialect: "mysql", host: "127.0.0.1", port: 33069, database: "shop", auth: "sql", user: "root", encrypt: false, trustServerCertificate: true, color: "#27ae60" },
    query: "SELECT id, name FROM customers ORDER BY id",
    joinPrefix: "SELECT * FROM orders o JOIN ",
    joinExpect: "JOIN customers c ON c.id = o.customer_id",
  },
];

const statusText = () => evaluate(`document.querySelector(".status-msg")?.textContent ?? ""`);
const toolbarConn = () => evaluate(`document.querySelector(".toolbar button")?.textContent ?? ""`);
const gridRows = (n = 5) =>
  evaluate(`[...document.querySelectorAll(".grid-row")].slice(0, ${n}).map((r) => [...r.children].slice(1).map((c) => c.textContent))`);
const suggestText = () => evaluate(`document.querySelector(".suggest-widget.visible")?.innerText ?? ""`);

async function waitUntil(fn, timeout = 15000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const v = await fn();
    if (v) return v;
    await sleep(150);
  }
  return null;
}

async function clearEditor() {
  await click(".editor .view-lines");
  await press("Control+A");
  await press("Delete");
}

async function typeQuery(sql) {
  await clearEditor();
  await type(sql);
  await press("Escape");
}

async function execute() {
  const before = await evaluate(`document.querySelector(".results-status")?.textContent ?? ""`);
  await press("F5");
  return waitUntil(async () => {
    const running = await evaluate(`!!document.querySelector(".tab.active .spinner")`);
    const s = await evaluate(`document.querySelector(".results-status")?.textContent ?? ""`);
    return !running && s && s !== before ? s : null;
  });
}

// tabs that were open before the run are left alone
const tabTitles = () => evaluate(`[...document.querySelectorAll(".tab")].map((t) => t.textContent)`);
const before = new Set(await tabTitles());

for (const s of servers) {
  const label = s.config.name;
  await invoke("save_connection", { config: s.config, password, forgetPassword: false });

  // connect a new tab through the dialog
  await press("Control+N");
  await sleep(300);
  await evaluate(`document.querySelector(".toolbar button").click()`);
  await waitFor(".conn-modal");
  // the list loads asynchronously; clicking Connect on the empty form would
  // save a new default connection instead
  const picked = await waitUntil(() =>
    evaluate(`(() => {
      const e = [...document.querySelectorAll(".conn-item")].find((e) => e.textContent.includes(${JSON.stringify(label)}));
      e?.click();
      return !!e;
    })()`),
  );
  if (!picked) throw new Error(`connection ${label} not listed`);
  // never press Connect on an empty form: that would save a new "localhost" connection
  if (!(await waitUntil(async () => (await evaluate(`document.querySelector(".conn-form input")?.value`)) === label))) {
    throw new Error(`connection ${label} was not selected`);
  }
  await evaluate(`[...document.querySelectorAll(".modal-actions button")].find((b) => b.textContent === "Connect").click()`);
  const connected = await waitUntil(async () => ((await toolbarConn()).includes(label) ? true : null));
  check(`${label}: connect`, !!connected, `${await toolbarConn()} ${await statusText()}`);
  const loaded = await waitUntil(async () => {
    const st = await statusText();
    if (!st.startsWith(`${label} / `)) return null;
    return st.includes("objects loaded") || st.includes("failed") ? st : null;
  });
  check(`${label}: catalog loaded`, !!loaded && loaded.includes("objects loaded"), loaded ?? "");

  // run a query and read the grid
  await typeQuery(s.query);
  const st = await execute();
  const rows = await gridRows();
  check(`${label}: execute`, rows?.[0]?.[0] === "1" && rows?.[0]?.[1] === "Nguyễn Văn A", `${st} ${JSON.stringify(rows?.slice(0, 2))}`);
  await shot(`${s.config.id}-1-results`);

  // FK join suggestion after JOIN + space, accepted with Enter
  await clearEditor();
  await type(s.joinPrefix);
  const sug = await waitUntil(async () => {
    const t = await suggestText();
    return t.includes(" ON ") ? t : null;
  }, 8000);
  await shot(`${s.config.id}-2-join-suggest`);
  check(`${label}: join suggestion`, !!sug, (sug ?? "").split("\n").slice(0, 2).join(" | "));
  await press("Enter");
  await sleep(300);
  const joined = await editorText();
  check(`${label}: join inserted`, joined.includes(s.joinExpect), JSON.stringify(joined));
}

// --- SQL Server specific checks ------------------------------------------------
await evaluate(`[...document.querySelectorAll(".tab")].find((t) => t.title.startsWith("E2E SQL Server"))?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }))`);
await sleep(400);

// Tab after * expands to the column list
await typeQuery("SELECT * FROM dbo.Customers c");
await press("Home");
for (let i = 0; i < 8; i++) await press("ArrowRight");
await press("Tab");
await sleep(700);
const expanded = await editorText();
check("MSSQL: Tab expands *", expanded.includes("SELECT Id, Name, Country, CreatedAt FROM"), JSON.stringify(expanded));

// alias-dot completion
await clearEditor();
await type("SELECT  FROM dbo.Orders o");
await press("Home");
for (let i = 0; i < 7; i++) await press("ArrowRight");
await type("o.");
const cols = await waitUntil(async () => {
  const t = await suggestText();
  return t.includes("CustomerId") ? t : null;
}, 8000);
check("MSSQL: alias column completion", !!cols, (cols ?? "").split("\n").slice(0, 3).join(" | "));
await press("Escape");

// errors show in Messages
await typeQuery("SELECT * FROM NoSuchTable");
await execute();
const msg = await evaluate(`document.querySelector(".msg-error")?.textContent ?? ""`);
check("MSSQL: error message", msg.includes("Msg 208") && msg.includes("NoSuchTable"), msg.split("\n")[0]);
await shot("mssql-3-error");

// multiple result sets and GO batches
await clearEditor();
await type("SELECT 1 AS a; SELECT 2 AS b, 3 AS c");
await press("Escape");
await execute();
const tabsLabels = await evaluate(`[...document.querySelectorAll(".results-tabs button")].map((b) => b.textContent)`);
check("MSSQL: two result sets", tabsLabels.filter((t) => t.startsWith("Results")).length === 2, JSON.stringify(tabsLabels));

// format current text
await typeQuery("select o.Id, c.Name from dbo.Orders o join dbo.Customers c on c.Id = o.CustomerId where o.Total > 100");
await press("Control+K");
await press("Control+Y");
await sleep(600);
const formatted = await editorText();
check("MSSQL: format", formatted.includes("    INNER JOIN") || formatted.includes("    JOIN dbo.Customers c\n        ON c.Id"), JSON.stringify(formatted));

// Object Search
await press("Control+Alt+D");
await waitFor(".search-panel input");
await sleep(200);
await type("CustomerId");
await press("Enter");
const hits = await waitUntil(async () => {
  const r = await evaluate(`[...document.querySelectorAll(".search-results tbody tr")].map((tr) => tr.innerText.replace(/\\s+/g, " "))`);
  return r.length ? r : null;
});
await sleep(500);
await shot("mssql-4-search");
check(
  "MSSQL: Object Search",
  !!hits && hits.some((h) => h.includes("GetCustomerOrders")) && hits.some((h) => h.includes("Orders") && h.includes("column")),
  JSON.stringify(hits),
);
const preview = await evaluate(`document.querySelector(".search-preview pre")?.textContent ?? ""`);
check("MSSQL: search preview shows definition", preview.length > 20, preview.slice(0, 60));
await press("Control+Alt+D");

// Query History finds executed queries
await press("Control+Shift+H");
await waitFor(".history-panel input");
await sleep(200);
await type("NoSuchTable");
const hist = await waitUntil(async () => {
  const n = await evaluate(`document.querySelectorAll(".history-item").length`);
  return n > 0 ? n : null;
});
await shot("mssql-5-history");
check("MSSQL: history search", !!hist, `${hist} entries`);
await press("Control+Shift+H");

// change database from the toolbar
await evaluate(`(() => { const s = document.querySelector(".toolbar select"); s.value = "master"; s.dispatchEvent(new Event("change", { bubbles: true })); })()`);
await typeQuery("SELECT DB_NAME()");
await execute();
const db = await gridRows(1);
check("MSSQL: change database", db?.[0]?.[0] === "master", JSON.stringify(db));

// leave the explorer open, as it is by default
if (!(await evaluate(`!!document.querySelector(".explorer-panel")`))) await press("F8");

// close the tabs this run opened
for (let i = 0; i < 10; i++) {
  const own = (await tabTitles()).findIndex((t) => !before.has(t));
  if (own < 0) break;
  await evaluate(`document.querySelectorAll(".tab")[${own}].click()`);
  await sleep(200);
  await press("Control+W");
  await sleep(300);
}

const failed = summary();
ui.close();
process.exit(failed ? 1 : 0);
