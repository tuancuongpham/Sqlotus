// End-to-end checks for schema compare, using the databases left by
// `cargo test -p sqlotus-db --test schema_compare` on the Docker SQL Server:
// SqlotusCmp_src (full), SqlotusCmp_tgt and SqlotusCmp_none (empty).
//
//   SQLOTUS_E2E_PASSWORD=... node scripts/e2e-compare.mjs [screenshot-dir]
import { mkdirSync } from "node:fs";
import { attach, checker } from "./cdp.mjs";

const outDir = process.argv[2] ?? "e2e-out";
mkdirSync(outDir, { recursive: true });
const password = process.env.SQLOTUS_E2E_PASSWORD;
if (!password) throw new Error("set SQLOTUS_E2E_PASSWORD");
const ui = await attach(outDir);
const { sleep, evaluate, press, shot, invoke } = ui;
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
const before = new Set(await tabTitles());

/** Pick connection + database for the n-th side (0 = source, 1 = target). */
async function side(n, connName, db) {
  // the dialog reloads the saved connections when it opens
  const picked = await until(
    () =>
      evaluate(`(() => {
        const sel = document.querySelectorAll(".compare-side")[${n}]?.querySelectorAll("select");
        const conn = sel && [...sel[0].options].find((o) => o.textContent.startsWith(${JSON.stringify(connName)}));
        if (!conn) return false;
        sel[0].value = conn.value; sel[0].dispatchEvent(new Event("change", { bubbles: true })); return true;
      })()`),
    8000,
  );
  if (!picked) throw new Error(`connection ${connName} not listed`);
  if (!db) return;
  const ok = await until(
    () =>
      evaluate(`(() => {
        const sel = document.querySelectorAll(".compare-side")[${n}].querySelectorAll("select")[1];
        if (!sel || ![...sel.options].some((o) => o.value === ${JSON.stringify(db)})) return false;
        sel.value = ${JSON.stringify(db)}; sel.dispatchEvent(new Event("change", { bubbles: true })); return true;
      })()`),
    15000,
  );
  if (!ok) throw new Error(`database ${db} not listed`);
  await sleep(200);
}

async function compareNow() {
  await clearToasts();
  await clickButton(".compare-setup", "Compare");
  return until(async () => {
    const t = await toastText();
    return t.includes("identical") || t.includes("difference(s)") || t.includes("Could not compare") ? t : null;
  }, 30000);
}

async function openCompare() {
  await clickButton(".toolbar", "Compare");
  return until(() => evaluate(`!!document.querySelector(".schema-compare-modal")`), 5000);
}

async function runScriptTab() {
  // the script tab connects to the target; run it and accept the execution warnings
  await until(async () => (await evaluate(`document.querySelector(".tab.active .tab-title")?.textContent ?? ""`)).startsWith("Sync"), 8000);
  await until(() => evaluate(`!!document.querySelector(".tab.active") && !document.querySelector(".tab.active .spinner")`), 5000);
  await until(async () => (await evaluate(`document.querySelector(".toolbar button")?.textContent ?? ""`)).includes("E2E SQL Server"), 15000);
  await clearToasts();
  await evaluate(`document.querySelector(".editor .view-lines")?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }))`);
  await press("F5");
  const warn = await until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? ""`), 3000);
  if (warn) await clickButton(".confirm-modal", "Execute anyway");
  return until(async () => {
    const t = await toastText();
    return t.includes("Query finished") || t.includes("finished with errors") ? t : null;
  }, 30000);
}

const bad = {
  id: "e2e-broken",
  name: "E2E Broken",
  dialect: "mssql",
  host: "localhost,14399",
  port: null,
  database: "master",
  auth: "sql",
  user: "sa",
  encrypt: false,
  trustServerCertificate: true,
};

try {
  // 1. full sync into an empty database
  check("open schema compare", !!(await openCompare()));
  await side(0, "E2E SQL Server", "SqlotusCmp_src");
  await side(1, "E2E SQL Server", "SqlotusCmp_tgt");
  const r1 = await compareNow();
  await shot("c1-compare");
  check("differences found", !!r1 && r1.includes("difference(s)"), r1 ?? "");
  const list = await text(".compare-list");
  check("objects listed as only in source", list.includes("Only in source") && list.includes("Order Lines") && list.includes("vCustomerOrders"), list.replace(/\n/g, " | ").slice(0, 200));
  await evaluate(`[...document.querySelectorAll(".compare-item")].find((e) => e.textContent.includes("Customers"))?.click()`);
  const diffShown = await until(() => evaluate(`!!document.querySelector(".compare-diff-editor .monaco-diff-editor")`), 5000);
  check("side-by-side definition", !!diffShown);
  await clickButton(".schema-compare-modal .modal-actions", "Generate script");
  check("script opened", !!(await waitToast("Sync script", 8000)), await toastText());
  const ran = await runScriptTab();
  check("sync script runs on the target", !!ran && ran.includes("Query finished"), ran ?? (await toastText()));

  // 2. compare again: identical now
  await openCompare();
  await side(0, "E2E SQL Server", "SqlotusCmp_src");
  await side(1, "E2E SQL Server", "SqlotusCmp_tgt");
  const r2 = await compareNow();
  check("identical after sync", !!r2 && r2.includes("identical"), r2 ?? "");

  // 2b. identical objects are listed, the list filters, and the setup can be saved
  await clickButton(".compare-group", "▸");
  await evaluate(`document.querySelector(".compare-toggle")?.click()`);
  const same = await until(() => evaluate(`document.querySelectorAll(".compare-item.identical").length`), 3000);
  check("identical objects listed", same >= 5, String(same));
  await evaluate(`(() => { const el = document.querySelector(".compare-filter"); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(el, "vcustomer"); el.dispatchEvent(new Event("input", { bubbles: true })); })()`);
  const filtered = await until(async () => {
    const n = await evaluate(`document.querySelectorAll(".compare-item.identical").length`);
    return n > 0 && n < same ? n : null;
  }, 3000);
  check("filter narrows the list", filtered === 1, String(filtered));
  check("export report available", await evaluate(`![...document.querySelectorAll(".schema-compare-modal button")].find((b) => b.textContent.startsWith("Export report")).disabled`));
  await clickButton(".compare-saved", "Save");
  const savedName = await until(() => evaluate(`document.querySelector(".compare-saved select").value`), 3000);
  check("comparison saved", !!savedName && savedName.includes("SqlotusCmp_src") && savedName.includes("SqlotusCmp_tgt"), savedName);
  const stored = await invoke("saved_compares");
  check("saved list persisted", stored.some((x) => x.name === savedName && x.source.database === "SqlotusCmp_src"), JSON.stringify(stored));
  await clickButton(".compare-saved", "Delete");
  await sleep(300);
  const after = await invoke("saved_compares");
  check("saved comparison deleted", !after.some((x) => x.name === savedName), JSON.stringify(after));

  // 3. drops are not selected by default
  await side(0, "E2E SQL Server", "SqlotusCmp_none");
  await side(1, "E2E SQL Server", "SqlotusCmp_tgt");
  await compareNow();
  const generate = await evaluate(`[...document.querySelectorAll(".schema-compare-modal .modal-actions button")].find((b) => b.textContent.startsWith("Generate"))?.textContent`);
  check("drops need opting in", generate === "Generate script (0)", generate);
  await clickButton(".compare-summary", "All");
  await clickButton(".schema-compare-modal .modal-actions", "Generate script");
  await waitToast("Sync script", 8000);
  const session = await until(async () => {
    const s = await invoke("load_session");
    return s?.tabs?.find((t) => t.text.includes("DROP TABLE dbo.Customers;"))?.text ?? null;
  }, 8000);
  check("drop script has warnings", !!session && session.includes("WARNINGS") && session.includes("dropped with all its data"));
  const ran2 = await runScriptTab();
  check("drop script runs", !!ran2 && ran2.includes("Query finished"), ran2 ?? "");
  await openCompare();
  await side(0, "E2E SQL Server", "SqlotusCmp_none");
  await side(1, "E2E SQL Server", "SqlotusCmp_tgt");
  const r3 = await compareNow();
  check("target empty again", !!r3 && r3.includes("identical"), r3 ?? "");

  // 4. abnormal: same database, different kinds of server, unreachable server
  await side(1, "E2E SQL Server", "SqlotusCmp_none");
  check("same database note", (await text(".schema-compare-modal")).includes("same database"));
  await side(1, "E2E PostgreSQL", "shop");
  const kind = await text(".schema-compare-modal");
  const disabled = await evaluate(`[...document.querySelectorAll(".compare-setup button")].find((b) => b.textContent === "Compare")?.disabled`);
  check("different kinds refused", kind.includes("same kind of server") && disabled === true);
  await clickButton(".schema-compare-modal .modal-actions", "Close");
  await invoke("save_connection", { config: bad, password, forgetPassword: false });
  await openCompare();
  await side(1, "E2E Broken", null);
  const broken = await until(async () => ((await text(".schema-compare-modal")).includes("cannot list databases") ? true : null), 30000);
  check("unreachable server reported", !!broken);
  await shot("c4-broken");
  await clickButton(".schema-compare-modal .modal-actions", "Close");
} finally {
  await invoke("delete_connection", { id: bad.id }).catch(() => undefined);
  for (let i = 0; i < 10; i++) {
    const own = (await tabTitles()).findIndex((t) => !before.has(t));
    if (own < 0) break;
    await evaluate(`document.querySelectorAll(".tab")[${own}].dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }))`);
    await sleep(200);
    await press("Control+W");
    await sleep(300);
    await clickButton(".confirm-modal", "Roll back and close");
  }
}

const failed = summary();
ui.close();
process.exit(failed ? 1 : 0);
