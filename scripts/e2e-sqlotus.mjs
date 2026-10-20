// End-to-end checks for notifications, the Connection Doctor, open
// transaction reminders, read-only connections and grid aggregates.
// Uses the Docker "Shop" database (connection "E2E SQL Server").
//
//   SQLOTUS_E2E_PASSWORD=... node scripts/e2e-sqlotus.mjs [screenshot-dir]
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
  return waitToast(`Connected to ${label}`, 20000);
}

async function setText(text) {
  await click(".editor .view-lines");
  await sleep(150);
  await press("Control+A");
  await press("Delete");
  await type(text);
  await press("Escape");
}

// 1. the explorer is open by default
check("explorer open by default", await evaluate(`document.querySelector(".side-head b")?.textContent === "Object Explorer"`));

// 2. connecting shows progress and success notifications
const connected = await connectNewTab("E2E SQL Server");
check("connect notification", !!connected, connected ?? (await toastText()));
await until(async () => (await evaluate(`document.querySelector(".status-msg").textContent`)).includes("objects loaded"), 15000);

// 3. format and execute notifications
await clearToasts();
await setText("select Id, Name from dbo.Customers order by Id");
await press("Control+K");
await press("Control+Y");
check("format notification", !!(await waitToast("Document formatted", 5000)), await toastText());
await press("F5");
check("execute notification", !!(await waitToast("Query finished", 10000)), await toastText());

// 4. grid aggregates over a Shift+click range
const cell = (r, c) => evaluate(`(() => { const row = [...document.querySelectorAll(".grid-row")][${r}]; const el = row.children[${c}]; const b = el.getBoundingClientRect(); return { x: b.x + 10, y: b.y + b.height / 2 }; })()`);
const a = await cell(0, 1);
const b = await cell(2, 1);
for (const t of ["mousePressed", "mouseReleased"]) await ui.send("Input.dispatchMouseEvent", { type: t, x: a.x, y: a.y, button: "left", clickCount: 1 });
for (const t of ["mousePressed", "mouseReleased"])
  await ui.send("Input.dispatchMouseEvent", { type: t, x: b.x, y: b.y, button: "left", clickCount: 1, modifiers: 8 });
await sleep(300);
const agg = await evaluate(`document.querySelector(".grid-aggregates")?.textContent ?? ""`);
check("grid aggregates", agg.includes("Sum 6") && agg.includes("Avg 2") && agg.includes("Count 3"), agg.trim());
await shot("s4-aggregates");

// 5. open transaction: badge, warning, and a choice when closing the tab
await clearToasts();
await setText("BEGIN TRAN; UPDATE dbo.Customers SET Country = Country WHERE Id = 1");
await press("F5");
const txWarn = await waitToast("transaction still open", 10000);
const badge = await until(() => evaluate(`!!document.querySelector(".tab.active .tab-badge.tx")`), 3000);
check("open transaction warning + badge", !!txWarn && !!badge, txWarn ?? "");
await shot("s5-transaction");
await clearToasts();
await press("Control+W");
const choice = await until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? ""`), 5000);
check("closing asks commit/rollback", (choice ?? "").includes("open transaction"), (choice ?? "").split("\n")[0]);
await evaluate(`[...document.querySelectorAll(".confirm-modal button")].find((x) => x.textContent === "Roll back and close").click()`);
check("rollback notification", !!(await waitToast("Transaction rolled back", 8000)), await toastText());

// 6. read-only connection blocks changes but runs queries
const ro = {
  id: "e2e-readonly",
  name: "E2E Read-only",
  dialect: "mssql",
  host: "localhost,14333",
  port: null,
  database: "Shop",
  auth: "sql",
  user: "sa",
  encrypt: false,
  trustServerCertificate: true,
  color: "#8e44ad",
  readOnly: true,
};
await invoke("save_connection", { config: ro, password, forgetPassword: false });
await connectNewTab("E2E Read-only");
await sleep(500);
await clearToasts();
await setText("DELETE FROM dbo.OrderItems WHERE 1 = 0");
await press("F5");
const blockedWarn = await until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? ""`), 3000);
if (blockedWarn) await evaluate(`[...document.querySelectorAll(".confirm-modal button")].find((x) => x.textContent === "Execute anyway")?.click()`);
const blocked = await waitToast("read-only", 8000);
check("read-only blocks DELETE", !!blocked, blocked ?? (await toastText()));
await clearToasts();
await setText("SELECT COUNT(*) FROM dbo.OrderItems");
await press("F5");
check("read-only allows SELECT", !!(await waitToast("Query finished", 8000)), await toastText());
check("read-only lock on tab", await evaluate(`!!document.querySelector(".tab.active .tab-badge.ro")`));
await press("Control+W");
await sleep(300);
await invoke("delete_connection", { id: ro.id });

// 7. Connection Doctor: a port with nothing listening on this computer
await evaluate(`document.querySelector(".toolbar button").click()`);
await waitFor(".conn-modal");
await evaluate(`document.querySelector(".conn-new").click()`);
await sleep(200);
await evaluate(`(() => {
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
  const field = (label) => [...document.querySelectorAll(".conn-form label")].find((l) => l.firstChild.textContent.trim() === label).querySelector("input");
  const s = field("Server"); set.call(s, "localhost"); s.dispatchEvent(new Event("input", { bubbles: true }));
  const p = field("Port"); set.call(p, "14399"); p.dispatchEvent(new Event("input", { bubbles: true }));
})()`);
await sleep(200);
await evaluate(`[...document.querySelectorAll(".modal-actions button")].find((b) => b.textContent === "Test").click()`);
const doctorText = await until(async () => {
  const t = await evaluate(`document.querySelector(".doctor-modal")?.innerText ?? ""`);
  return t.includes("refused") || t.includes("Port") ? t : null;
}, 30000);
await shot("s7-doctor");
check(
  "connection doctor explains and offers a fix",
  !!doctorText && /does not match|not accepting|Use port/.test(doctorText),
  (doctorText ?? "").split("\n").slice(0, 6).join(" | "),
);
await evaluate(`[...document.querySelectorAll(".doctor-modal button")].find((b) => b.textContent === "Close").click()`);
await sleep(200);
await evaluate(`[...document.querySelectorAll(".conn-modal .modal-actions button")].find((b) => b.textContent === "Close").click()`);

// 8. settings save notification
await clearToasts();
await evaluate(`[...document.querySelectorAll(".toolbar button")].find((b) => b.textContent.includes("Options")).click()`);
await waitFor(".settings-modal");
await sleep(300);
await evaluate(`[...document.querySelectorAll(".settings-modal button")].find((b) => b.textContent === "Done").click()`);
check("settings saved notification", !!(await waitToast("Settings saved", 5000)), await toastText());

const failed = summary();
ui.close();
process.exit(failed ? 1 : 0);
