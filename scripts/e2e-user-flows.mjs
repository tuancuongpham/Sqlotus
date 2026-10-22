// End-to-end user flows: what a person does with the app, step by step,
// through the real UI against the Docker test servers (connections saved by
// e2e-db.mjs). Each flow covers a fix that only shows when the pieces work
// together:
//
//   1. a new PostgreSQL / MySQL connection typed by hand, `host:port` in Server
//   2. Shift+F5 inside a MySQL procedure written without DELIMITER
//   3. saving an edited row that someone else changed, on a table whose
//      trigger writes an audit log (SQL Server)
//   4. closing a tab that holds an open transaction, and Escape on a warning
//   5. a pasted query nested thousands of levels deep, and a 2 MB script
//   6. "Ignore on this line" while a string is still being typed
//   7. typing with completion, and reopening a closed tab
//
//   $env:SQLOTUS_E2E_PASSWORD="..."; node scripts/e2e-user-flows.mjs [screenshot-dir]
import { start } from "./e2e-kit.mjs";

const t = await start();
const { ui, check, password } = t;
const { sleep, evaluate, press, shot, invoke } = ui;

const before = new Set(await t.tabTitles());
/** The SQL Server tab of flows 3 and 4 (also used to clean up). */
let tabA = null;
const scalar = (rows) => rows?.[0]?.[0] ?? null;

/** Run `sql` and return the rows of its first result. */
async function query(sql) {
  await t.runSql(sql);
  await sleep(300);
  return t.gridRows();
}

// -- 1. connections typed by hand --------------------------------------------

/** Fill the field labelled `label` in the connection form as a person would. */
async function fill(label, value) {
  const found = await evaluate(`(() => {
    const l = [...document.querySelectorAll(".conn-form label")].find((x) => x.firstChild?.textContent.trim() === ${JSON.stringify(label)});
    const el = l?.querySelector("input, select");
    if (!el) return false;
    el.focus();
    if (el.tagName === "SELECT") {
      el.value = ${JSON.stringify(value)};
      el.dispatchEvent(new Event("change", { bubbles: true }));
    }
    return el.tagName;
  })()`);
  if (found === "INPUT") {
    await press("Control+A");
    await press("Delete");
    if (value) await ui.send("Input.insertText", { text: value });
  }
  await sleep(100);
  return !!found;
}

async function typedConnection(name, dialect, server, database, user) {
  await press("Control+N");
  await sleep(300);
  await evaluate(`document.querySelector(".toolbar button").click()`);
  await t.until(() => evaluate(`!!document.querySelector(".conn-modal")`), 5000);
  await t.clickButton(".conn-list", "+ New connection");
  await sleep(200);
  await fill("Server type", dialect);
  await fill("Name", name);
  await fill("Server", server);
  await fill("Port", "");
  await fill("User", user);
  await fill("Password", password);
  await fill("Database", database);
  // keep the password out of the credential store
  await evaluate(`(() => { const c = [...document.querySelectorAll(".conn-form label.check")].find((l) => l.textContent.includes("Remember password"))?.querySelector("input"); if (c?.checked) c.click(); })()`);
  await t.clearToasts();
  await t.clickButton(".conn-modal .modal-actions", "Connect");
  const connected = await t.waitToast(`Connected to ${name}`, 20000);
  const one = connected ? scalar(await query("SELECT 1 AS one")) : null;
  const saved = (await invoke("list_connections")).find((c) => c.name === name);
  if (saved) await invoke("delete_connection", { id: saved.id });
  return { connected: !!connected, one, toasts: await t.toastText() };
}

// -- the flows ----------------------------------------------------------------

try {
  for (const [dialect, server, user] of [
    ["postgres", "127.0.0.1:54329", "postgres"],
    ["mysql", "127.0.0.1:33069", "root"],
  ]) {
    const r = await typedConnection(`E2E typed ${dialect}`, dialect, server, "shop", user);
    check(`${dialect}: host:port typed in Server connects`, r.connected && r.one === "1", r.toasts.slice(0, 160));
  }

  // 2. Shift+F5 inside a MySQL procedure: the whole procedure is created, its DELETE does not run
  check("connect MySQL", !!(await t.connectNewTab("E2E MySQL")));
  await t.runSql("DROP PROCEDURE IF EXISTS e2e_clear; DROP TABLE IF EXISTS e2e_staging; CREATE TABLE e2e_staging (id int); INSERT INTO e2e_staging VALUES (1), (2);");
  await t.setText("SELECT 1;\nCREATE PROCEDURE e2e_clear()\nBEGIN\n  SELECT 'clearing';\n  DELETE FROM e2e_staging;\nEND;\nSELECT 2;");
  await t.cursorToLine(5);
  await t.clearToasts();
  await press("Shift+F5");
  const warned = await t.until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? ""`), 2000);
  check("no warning for the DELETE stored in the procedure", !warned, (warned ?? "").replace(/\n/g, " "));
  if (warned) await t.clickButton(".confirm-modal", "Cancel");
  const done = await t.until(async () => {
    const x = await t.toastText();
    return /Query finished|finished with errors/.test(x) ? x : null;
  }, 15000);
  check("Shift+F5 created the procedure", !!done && !done.includes("errors"), (done ?? "").slice(0, 160));
  check("its DELETE did not run", scalar(await query("SELECT COUNT(*) FROM e2e_staging")) === "2");
  check(
    "the procedure exists",
    scalar(await query("SELECT COUNT(*) FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = DATABASE() AND ROUTINE_NAME = 'e2e_clear'")) === "1",
  );
  await t.runSql("DROP PROCEDURE IF EXISTS e2e_clear; DROP TABLE IF EXISTS e2e_staging;");

  // 3. a row changed by someone else, on a table with an audit trigger
  check("connect SQL Server", !!(await t.connectNewTab("E2E SQL Server")));
  tabA = await t.activeTitle();
  await t.runSql(
    "IF OBJECT_ID('dbo.E2E_Audited') IS NOT NULL DROP TABLE dbo.E2E_Audited;\n" +
      "IF OBJECT_ID('dbo.E2E_AuditLog') IS NOT NULL DROP TABLE dbo.E2E_AuditLog;\n" +
      "CREATE TABLE dbo.E2E_Audited (Id int PRIMARY KEY, Name nvarchar(50));\n" +
      "CREATE TABLE dbo.E2E_AuditLog (n int);\n" +
      "INSERT dbo.E2E_Audited VALUES (1, N'Một'), (2, N'Hai');\n" +
      "GO\n" +
      "CREATE TRIGGER dbo.E2E_Audited_Log ON dbo.E2E_Audited AFTER UPDATE AS INSERT dbo.E2E_AuditLog VALUES (1);",
  );
  await invoke("load_catalog_cmd", { connectionId: "e2e-mssql", database: "Shop", refresh: true });
  await query("SELECT Id, Name FROM dbo.E2E_Audited ORDER BY Id");
  await t.clickButton(".grid-toolbar", "✎ Edit");
  check("edit mode", !!(await t.until(() => evaluate(`!!document.querySelector(".grid-edit-bar")`), 5000)), await t.toastText());
  const dbl = async (r, c) => {
    const p = await evaluate(
      `(() => { const el = [...document.querySelectorAll(".grid-row")][${r}].children[${c}]; const b = el.getBoundingClientRect(); return { x: b.x + 8, y: b.y + b.height / 2 }; })()`,
    );
    for (const n of [1, 2]) {
      for (const type of ["mousePressed", "mouseReleased"]) await ui.send("Input.dispatchMouseEvent", { type, x: p.x, y: p.y, button: "left", clickCount: n });
    }
    await sleep(200);
  };
  await dbl(0, 2);
  await press("Control+A");
  await ui.send("Input.insertText", { text: "Một đã sửa" });
  await press("Enter");
  await sleep(200);
  // meanwhile, someone else changes the same row
  check("connect a second tab", !!(await t.connectNewTab("E2E SQL Server")));
  await t.runSql("UPDATE dbo.E2E_Audited SET Name = N'Người khác' WHERE Id = 1");
  check("back on the first tab", await t.selectTab(tabA));
  check("its pending edit is still there", (await evaluate(`document.querySelectorAll(".cell-changed").length`)) === 1);
  await sleep(300);
  await t.clearToasts();
  await t.clickButton(".grid-edit-bar", "Save");
  const conflict = await t.waitToast("someone else", 10000);
  check("the save sees the other change (the trigger's row does not hide it)", !!conflict, (await t.toastText()).slice(0, 200));
  check("nothing was overwritten", scalar(await query("SELECT Name FROM dbo.E2E_Audited WHERE Id = 1")) === "Người khác");
  // a save without a conflict works on the same table
  await query("SELECT Id, Name FROM dbo.E2E_Audited ORDER BY Id");
  await t.clickButton(".grid-toolbar", "✎ Edit");
  await t.until(() => evaluate(`!!document.querySelector(".grid-edit-bar")`), 5000);
  await dbl(1, 2);
  await press("Control+A");
  await ui.send("Input.insertText", { text: "Hai đã sửa" });
  await press("Enter");
  await sleep(200);
  await t.clearToasts();
  await t.clickButton(".grid-edit-bar", "Save");
  check("an ordinary save on the audited table", !!(await t.waitToast("1 change(s) saved", 10000)), await t.toastText());
  await shot("f3-saved");

  // 4. closing a tab with an open transaction rolls it back when asked to
  check("connect a third tab", !!(await t.connectNewTab("E2E SQL Server")));
  await t.runSql("BEGIN TRAN; UPDATE dbo.E2E_Audited SET Name = N'tạm' WHERE Id = 2");
  await t.waitToast("transaction still open", 8000);
  await press("Control+W");
  const asked = await t.until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? ""`), 5000);
  check("closing asks about the open transaction", !!asked && asked.includes("open transaction"), (asked ?? "").replace(/\n/g, " "));
  await t.clickButton(".confirm-modal", "Roll back and close");
  check("rolled back", !!(await t.waitToast("rolled back", 8000)), await t.toastText());
  check("back on the first tab again", await t.selectTab(tabA));
  check("the change was undone", scalar(await query("SELECT Name FROM dbo.E2E_Audited WHERE Id = 2")) === "Hai đã sửa");
  // Escape on an execution warning runs nothing
  await t.setText("DELETE FROM dbo.E2E_AuditLog");
  await t.clearToasts();
  await press("F5");
  const warning = await t.until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? ""`), 5000);
  check("DELETE without WHERE warns", !!warning && warning.includes("no WHERE"), (warning ?? "").replace(/\n/g, " "));
  await press("Escape");
  check("Escape cancels", !!(await t.waitToast("cancelled", 5000)), await t.toastText());
  check("the rows are still there", Number(scalar(await query("SELECT COUNT(*) FROM dbo.E2E_AuditLog"))) >= 1);

  // 5. a pasted query nested thousands of levels deep, and a 2 MB script
  const nested = `SELECT * FROM ${"(SELECT * FROM ".repeat(3000)}dbo.E2E_Audited t${") x".repeat(3000)}`;
  check("a deeply nested query pastes", !!(await t.paste(nested)));
  // the analysis that follows every edit used to overflow the stack and close the app
  await sleep(2500);
  const nestedProblems = await invoke("diagnostics", { args: { connectionId: "e2e-mssql", database: "Shop", dialect: "mssql", text: nested, offset: 10 } });
  check("the app survives its analysis", Array.isArray(nestedProblems) && (await evaluate("1 + 1")) === 2, `${nestedProblems?.length} problem(s)`);
  const script = Array.from({ length: 20000 }, (_, i) => `INSERT INTO dbo.Orders (Id, CustomerId, Total) VALUES (${i}, ${i % 97}, ${i % 1000}.5);`).join("\n");
  const timed = (cmd, args) =>
    evaluate(`(async () => { const s = performance.now(); await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)}); return Math.round(performance.now() - s); })()`);
  const editorArgs = { connectionId: "e2e-mssql", database: "Shop", dialect: "mssql", text: script, offset: script.length };
  const analysisMs = await timed("diagnostics", { args: editorArgs });
  check("analysis of a 2 MB script under 3 s", analysisMs < 3000, `${analysisMs} ms`);
  const checksMs = await timed("modifying_statements", { text: script, dialect: "mssql" });
  check("checks before running a 2 MB script under 3 s", checksMs < 3000, `${checksMs} ms`);
  const completionMs = await timed("complete", { args: editorArgs });
  check("completion at the end of a 2 MB script under 2 s", completionMs < 2000, `${completionMs} ms`);

  // 6. "Ignore on this line" while a string is still being typed
  const typing = "UPDATE dbo.Customers SET Name = 'Ng";
  const found = await invoke("diagnostics", { args: { ...editorArgs, text: typing, offset: typing.length } });
  const sq001 = found.find((d) => d.code === "SQ001");
  check(
    "Ignore on this line goes above, not into the open string",
    !!sq001?.ignore && sq001.ignore.start === 0 && sq001.ignore.text.includes("sqlotus-disable-next-line SQ001"),
    JSON.stringify(sq001?.ignore ?? null),
  );

  // 7. typing with completion, then closing and reopening the tab
  await t.focusEditor();
  await press("Control+A");
  await press("Delete");
  await ui.type("SELECT * FROM dbo.E2E_Aud");
  const suggested = await t.until(() => evaluate(`document.querySelector(".suggest-widget.visible")?.innerText ?? ""`), 5000);
  check("completion offers the new table", !!suggested && suggested.includes("E2E_Audited"), (suggested ?? "").replace(/\n/g, " | ").slice(0, 120));
  await press("Tab");
  await sleep(400);
  const typed = t.flat(await ui.editorText());
  check("Tab inserts it", typed.includes("dbo.E2E_Audited"), typed);
  const title = await t.activeTitle();
  await press("Control+W");
  await sleep(500);
  await press("Control+Shift+T");
  const reopened = await t.until(async () => ((await t.activeTitle()) === title ? t.flat(await ui.editorText()) : null), 8000);
  check("Ctrl+Shift+T brings the tab back with its text", !!reopened && reopened.includes("dbo.E2E_Audited"), reopened ?? "");
} catch (e) {
  check("no exception", false, String(e?.stack ?? e));
} finally {
  // leave the test databases as they were
  try {
    if (tabA && (await t.selectTab(tabA))) {
      await t.runSql(
        "IF OBJECT_ID('dbo.E2E_Audited') IS NOT NULL DROP TABLE dbo.E2E_Audited; IF OBJECT_ID('dbo.E2E_AuditLog') IS NOT NULL DROP TABLE dbo.E2E_AuditLog;",
      );
    }
    // the cached objects too: other suites count the tables of Shop
    await invoke("load_catalog_cmd", { connectionId: "e2e-mssql", database: "Shop", refresh: true });
  } catch {
    // the cleanup is best effort
  }
  await t.closeTabsSince(before);
}

const failed = t.summary();
ui.close();
process.exit(failed ? 1 : 0);
