// End-to-end checks for Source control on the Docker SQL Server: two
// databases (a developer and a teammate) share one folder in a fresh git
// repository. Creates SqlotusGit_A / SqlotusGit_B and drops them at the end.
//
//   SQLOTUS_E2E_PASSWORD=... node scripts/e2e-source-control.mjs [screenshot-dir]
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { attach, checker } from "./cdp.mjs";

const outDir = process.argv[2] ?? "e2e-out";
mkdirSync(outDir, { recursive: true });
if (!process.env.SQLOTUS_E2E_PASSWORD) throw new Error("set SQLOTUS_E2E_PASSWORD");
const ui = await attach(outDir);
const { sleep, evaluate, click, press, shot, invoke, send } = ui;
const { check, summary } = checker();

const root = join(tmpdir(), `sqlotus-e2e-git-${Date.now()}`);
const folder = join(root, "schema");
const busyFolder = join(root, "busy");

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
  evaluate(`(() => { const b = [...document.querySelectorAll(${JSON.stringify(scope + " button")})].find((x) => x.textContent.trim().startsWith(${JSON.stringify(label)}) && !x.disabled); b?.click(); return !!b; })()`);
const text = (sel) => evaluate(`document.querySelector(${JSON.stringify(sel)})?.innerText ?? ""`);
const statusText = () => evaluate(`document.querySelector(".status-msg")?.textContent ?? ""`);
const tabTitles = () => evaluate(`[...document.querySelectorAll(".tab")].map((t) => t.textContent)`);
const before = new Set(await tabTitles());
/** Set a React-controlled input or textarea. */
const fill = (sel, value) =>
  evaluate(`(() => {
    const el = document.querySelector(${JSON.stringify(sel)});
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(el, ${JSON.stringify(value)});
    el.dispatchEvent(new Event("input", { bubbles: true }));
    return true;
  })()`);

async function newTab(database) {
  await press("Control+N");
  await sleep(300);
  await evaluate(`document.querySelector(".toolbar button").click()`);
  await until(() => evaluate(`!!document.querySelector(".conn-modal")`), 5000);
  await until(() =>
    evaluate(`(() => { const e = [...document.querySelectorAll(".conn-item")].find((e) => e.textContent.includes("E2E SQL Server")); e?.click(); return !!e; })()`),
  );
  await until(async () => (await evaluate(`document.querySelector(".conn-form input")?.value`)) === "E2E SQL Server");
  await evaluate(`[...document.querySelectorAll(".modal-actions button")].find((b) => b.textContent === "Connect").click()`);
  await until(async () => (await statusText()).includes("objects loaded"), 20000);
  if (database) {
    const listed = await until(
      () => evaluate(`[...document.querySelectorAll(".toolbar select option")].some((o) => o.value === ${JSON.stringify(database)})`),
      15000,
    );
    if (!listed) {
      const options = await evaluate(`[...document.querySelectorAll(".toolbar select option")].map((o) => o.value).join(",")`);
      throw new Error(`database ${database} is not in the tab's list: ${options}`);
    }
    await evaluate(`(() => { const s = document.querySelector(".toolbar select"); s.value = ${JSON.stringify(database)}; s.dispatchEvent(new Event("change", { bubbles: true })); })()`);
    const switched = await until(async () => (await evaluate(`document.querySelector(".toolbar select")?.value`)) === database, 20000);
    if (!switched) throw new Error(`could not switch the tab to ${database}`);
    await until(async () => (await statusText()).includes(`/ ${database}`), 20000);
  }
}

/** Run `sql` in the active tab and wait for the result. */
async function run(sql) {
  const flat = (s) => s.replace(/\s+/g, " ").trim();
  let shown = null;
  for (let attempt = 0; attempt < 3 && !shown; attempt++) {
    // after a dialog the focus is elsewhere: put it back into the editor first
    await click(".editor .view-lines");
    await sleep(200);
    await evaluate(`document.querySelector(".editor .native-edit-context, .editor textarea")?.focus()`);
    await press("Control+A");
    await press("Delete");
    await sleep(150);
    // insert the text in one go: typed keys can accept a completion or auto-close a bracket
    await send("Input.insertText", { text: sql });
    // the editor takes text asynchronously: run only once it shows the new statement
    shown = await until(async () => flat(await ui.editorText()) === flat(sql), 3000);
  }
  if (!shown) throw new Error(`the editor did not take the statement:\n got  ${JSON.stringify(flat(await ui.editorText()))}\n want ${JSON.stringify(flat(sql))}`);
  await press("Escape");
  await clearToasts();
  await press("F5");
  // execution warnings (DROP, ROLLBACK IMMEDIATE) and a production confirmation
  for (let i = 0; i < 2; i++) {
    const warn = await until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? ""`), 2000);
    if (!warn) break;
    if (!(await clickButton(".confirm-modal", "Execute anyway"))) await clickButton(".confirm-modal", "Run on production");
    await sleep(300);
  }
  const done = await until(async () => {
    const t = await toastText();
    return t.includes("Query finished") || t.includes("finished with errors") ? t : null;
  }, 30000);
  // a connection with manual commit leaves the DDL open: commit it so other sessions see it
  await clickButton(".toolbar", "✓ Commit");
  return done;
}

async function openSourceControl() {
  await clearToasts();
  await clickButton(".toolbar", "Source control");
  return until(() => evaluate(`!!document.querySelector(".source-control-modal")`), 5000);
}

/** Wait until the dialog shows a status (after Link, Refresh or Commit). */
async function waitStatus() {
  await sleep(400);
  return until(async () => {
    const t = await text(".source-control-modal");
    return t.includes("in sync") && !t.includes("Comparing the database") && !t.includes("Linking") ? t : null;
  }, 30000);
}

const items = () =>
  evaluate(`[...document.querySelectorAll(".source-control-modal .compare-item")].map((e) => e.innerText.replace(/\\s+/g, " ").trim())`);

async function closeDialog() {
  await clickButton(".source-control-modal .modal-actions", "Close");
  await until(() => evaluate(`!document.querySelector(".source-control-modal")`), 3000);
}

async function commit(message) {
  await fill(".sc-commit textarea", message);
  await sleep(100);
  await clearToasts();
  await clickButton(".sc-commit", "Commit");
  // the progress toast says "Committing…"; wait for the outcome
  return until(async () => {
    const t = await toastText();
    return t.includes("Committed") || t.includes("Folder updated") || t.includes("Could not commit") ? t : null;
  }, 30000);
}

/** Get latest: open the script, run it in its tab. */
async function getLatest() {
  await clearToasts();
  await clickButton(".sc-commit", "Get latest");
  const opened = await waitToast("Get latest", 8000);
  await until(async () => (await evaluate(`document.querySelector(".tab.active .tab-title")?.textContent ?? ""`)).startsWith("Get latest"), 8000);
  await until(async () => (await statusText()).includes("objects loaded") || (await statusText()).includes("SqlotusGit"), 20000);
  await sleep(500);
  await evaluate(`document.querySelector(".editor .view-lines")?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }))`);
  await clearToasts();
  await press("F5");
  const warn = await until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? ""`), 3000);
  if (warn) await clickButton(".confirm-modal", "Execute anyway");
  const ran = await until(async () => {
    const t = await toastText();
    return t.includes("Query finished") || t.includes("finished with errors") ? t : null;
  }, 30000);
  return { opened, ran };
}

const dropDb = (db) => `IF DB_ID('${db}') IS NOT NULL BEGIN ALTER DATABASE ${db} SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE ${db}; END`;
const tableSql = "CREATE TABLE dbo.Items (Id int NOT NULL CONSTRAINT PK_Items PRIMARY KEY, Name nvarchar(50) NOT NULL)";
const viewSql = "CREATE VIEW dbo.vItems AS SELECT Id, Name FROM dbo.Items";

/** Unlink every database this script (or an earlier, interrupted run) linked to a temporary folder. */
async function unlinkTestFolders() {
  const conns = await invoke("list_connections");
  const e2e = conns.find((c) => c.name === "E2E SQL Server");
  if (!e2e) return;
  for (const db of ["SqlotusGit_A", "SqlotusGit_B", "Shop", "master"]) {
    const info = await invoke("sc_link_info", { connectionId: e2e.id, database: db }).catch(() => null);
    if (info && info.folder.includes("sqlotus-e2e-git-")) await invoke("sc_unlink", { connectionId: e2e.id, database: db });
  }
}

try {
  mkdirSync(busyFolder, { recursive: true });
  writeFileSync(join(busyFolder, "app.config"), "not a schema");

  await unlinkTestFolders();
  await newTab("master");
  await run(dropDb("SqlotusGit_A"));
  await run(dropDb("SqlotusGit_B"));
  await run("CREATE DATABASE SqlotusGit_A");
  await run("CREATE DATABASE SqlotusGit_B");
  const created = await run("SELECT COUNT(*) AS n FROM sys.databases WHERE name IN ('SqlotusGit_A', 'SqlotusGit_B')");
  const count = await evaluate(`document.querySelector(".results-body")?.innerText ?? ""`);
  const lastLine = count.trim().split("\n").pop() ?? "";
  check("test databases created", !!created && /\b2\b/.test(lastLine), `${created} :: ${count.replace(/\s+/g, " ")}`);
  for (const db of ["SqlotusGit_A", "SqlotusGit_B"]) {
    await newTab(db);
    await run(tableSql);
    const v = await run(viewSql);
    check(`objects created in ${db}`, !!v && v.includes("Query finished"), v ?? (await toastText()));
  }

  // 1. link A to a new folder; a folder with other files is refused
  await newTab("SqlotusGit_A");
  check("open source control", !!(await openSourceControl()));
  const setupText = await until(async () => ((await text(".source-control-modal")).includes("Link") ? text(".source-control-modal") : null), 5000);
  check("not linked yet", !!setupText && setupText.includes("Keep this database's schema in a folder"));
  await fill(".sc-folder input", busyFolder);
  await clickButton(".sc-setup", "Link");
  const refused = await until(async () => ((await text(".source-control-modal")).includes("already holds other files") ? true : null), 8000);
  check("folder with other files refused", !!refused, await text(".source-control-modal .status-error"));
  await fill(".sc-folder input", folder);
  await clickButton(".sc-setup", "Link");
  const first = await waitStatus();
  check("linked, git repository created", !!first && first.includes("⎇") && existsSync(join(folder, ".git")), first?.slice(0, 200));
  execFileSync("git", ["-C", folder, "config", "user.email", "e2e@example.com"]);
  execFileSync("git", ["-C", folder, "config", "user.name", "Sqlotus E2E"]);
  const firstItems = await items();
  check("first commit: everything is new", firstItems.length === 2 && firstItems.every((i) => i.includes("New in the database")), firstItems.join(" | "));
  await shot("sc1-first");

  // 2. first commit writes one file per object and the snapshot
  const c1 = await commit("Initial schema");
  check("first commit", !!c1 && c1.includes("Committed"), c1 ?? (await toastText()));
  await waitStatus();
  check("files written", existsSync(join(folder, "Tables", "dbo.Items.sql")) && existsSync(join(folder, "Views", "dbo.vItems.sql")) && existsSync(join(folder, "sqlotus-schema.json")));
  check("in sync after commit", (await items()).length === 0 && (await text(".source-control-modal")).includes("are in sync"), (await items()).join(" | "));
  const log1 = execFileSync("git", ["-C", folder, "log", "--format=%s"]).toString().trim();
  check("git history", log1 === "Initial schema", log1);
  await closeDialog();

  // 3. a change in the database is a local change
  await run("ALTER TABLE dbo.Items ADD Note nvarchar(20) NULL");
  await openSourceControl();
  await waitStatus();
  const local = await items();
  check("local change", local.length === 1 && local[0].includes("Items") && local[0].includes("Changed in the database"), local.join(" | "));
  const c2 = await commit("Add Items.Note");
  check("second commit", !!c2 && c2.includes("Committed"), c2 ?? "");
  await waitStatus();
  check("file has the new column", readFileSync(join(folder, "Tables", "dbo.Items.sql"), "utf8").includes("Note"));
  await closeDialog();

  // 4. the teammate links B to the same folder: no common base yet, so differences are conflicts
  await newTab("SqlotusGit_B");
  await openSourceControl();
  await until(async () => (await text(".source-control-modal")).includes("Link"), 5000);
  await fill(".sc-folder input", folder);
  await clickButton(".sc-setup", "Link");
  await waitStatus();
  const conflicts = await items();
  check("unknown direction is a conflict", conflicts.length === 1 && conflicts[0].includes("Differs"), conflicts.join(" | "));
  await evaluate(`(() => { const s = document.querySelector(".source-control-modal .compare-item select"); s.value = "theirs"; s.dispatchEvent(new Event("change", { bubbles: true })); })()`);
  await sleep(200);
  const g1 = await getLatest();
  check("get latest script runs on B", !!g1.ran && g1.ran.includes("Query finished"), g1.ran ?? g1.opened ?? "");
  await openSourceControl();
  await waitStatus();
  check("B in sync", (await items()).length === 0, (await items()).join(" | "));
  await closeDialog();

  // 5. A changes again: B sees it as incoming
  await newTab("SqlotusGit_A");
  await run("ALTER TABLE dbo.Items ADD Price decimal(10, 2) NULL");
  await openSourceControl();
  await waitStatus();
  await commit("Add Items.Price");
  await waitStatus();
  await closeDialog();
  await newTab("SqlotusGit_B");
  await openSourceControl();
  await waitStatus();
  const incoming = await items();
  check("teammate change is incoming", incoming.length === 1 && incoming[0].includes("Changed in the folder"), incoming.join(" | "));
  await shot("sc2-incoming");
  const g2 = await getLatest();
  check("incoming change applied", !!g2.ran && g2.ran.includes("Query finished"), g2.ran ?? "");

  // 6. both sides change the same table: a conflict; a hand-edited file is reported
  await run("ALTER TABLE dbo.Items ADD OnlyInB int NULL");
  await newTab("SqlotusGit_A");
  await run("ALTER TABLE dbo.Items ADD OnlyInA int NULL");
  await openSourceControl();
  await waitStatus();
  await commit("Add Items.OnlyInA");
  await waitStatus();
  await closeDialog();
  writeFileSync(join(folder, "Views", "dbo.vItems.sql"), "-- edited by hand\n");
  await newTab("SqlotusGit_B");
  await openSourceControl();
  await waitStatus();
  const both = await items();
  check("changed on both sides is a conflict", both.some((i) => i.includes("Items") && i.includes("Changed on both sides")), both.join(" | "));
  const note = await text(".source-control-modal .sc-note");
  check("hand-edited file reported", note.includes("dbo.vItems.sql") && note.includes("do not match"), note);
  await fill(".sc-commit textarea", "would commit");
  await sleep(100);
  const commitDisabled = await evaluate(`[...document.querySelectorAll(".sc-commit button")].find((b) => b.textContent.startsWith("Commit")).disabled`);
  check("conflicts are not committed until a side is chosen", commitDisabled === true);
  await shot("sc3-conflict");

  // 7. history and unlink
  await clickButton(".sc-head", "History");
  const history = await until(async () => ((await text(".sc-log")).includes("Add Items.OnlyInA") ? text(".sc-log") : null), 8000);
  check("history lists the commits", !!history && history.includes("Initial schema"), history ?? "");
  await clickButton(".sc-head", "Unlink");
  check("unlinked", !!(await waitToast("Link removed", 5000)) && existsSync(join(folder, "sqlotus-schema.json")));
  await closeDialog();
  await newTab("SqlotusGit_A");
  await openSourceControl();
  await waitStatus();
  await clickButton(".sc-head", "Unlink");
  await waitToast("Link removed", 5000);
  await closeDialog();
} finally {
  // close the tabs this script opened (rolling back anything left open)
  for (let i = 0; i < 20; i++) {
    const own = (await tabTitles()).findIndex((t) => !before.has(t));
    if (own < 0) break;
    await evaluate(`document.querySelectorAll(".tab")[${own}].dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }))`);
    await sleep(200);
    await press("Control+W");
    await sleep(300);
    await clickButton(".confirm-modal", "Roll back and close");
  }
  await unlinkTestFolders().catch(() => undefined);
  await newTab("master").catch(() => undefined);
  await run(dropDb("SqlotusGit_A")).catch(() => undefined);
  await run(dropDb("SqlotusGit_B")).catch(() => undefined);
  const own = (await tabTitles()).findIndex((t) => !before.has(t));
  if (own >= 0) {
    await evaluate(`document.querySelectorAll(".tab")[${own}].dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }))`);
    await press("Control+W");
  }
  rmSync(root, { recursive: true, force: true });
}

const failed = summary();
ui.close();
process.exit(failed ? 1 : 0);
