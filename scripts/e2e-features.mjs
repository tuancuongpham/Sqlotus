// End-to-end checks of the SQL Prompt-style features against the Docker
// "Shop" database (connection "E2E SQL Server", see scripts/test-db).
//
//   node scripts/e2e-features.mjs [screenshot-dir]
import { mkdirSync } from "node:fs";
import { attach, checker } from "./cdp.mjs";

const outDir = process.argv[2] ?? "e2e-out";
mkdirSync(outDir, { recursive: true });
const ui = await attach(outDir);
const { sleep, evaluate, waitFor, click, press, type, shot, editorText, send } = ui;
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
const suggestText = () => evaluate(`document.querySelector(".suggest-widget.visible")?.innerText ?? ""`);
const statusText = () => evaluate(`document.querySelector(".status-msg")?.textContent ?? ""`);
const tabCount = () => evaluate(`document.querySelectorAll(".tab").length`);

async function newConnectedTab(label, database) {
  await press("Control+N");
  await sleep(300);
  await evaluate(`document.querySelector(".toolbar button").click()`);
  await waitFor(".conn-modal");
  await until(() =>
    evaluate(`(() => { const e = [...document.querySelectorAll(".conn-item")].find((e) => e.textContent.includes(${JSON.stringify(label)})); e?.click(); return !!e; })()`),
  );
  await until(async () => (await evaluate(`document.querySelector(".conn-form input")?.value`)) === label);
  await evaluate(`[...document.querySelectorAll(".modal-actions button")].find((b) => b.textContent === "Connect").click()`);
  return until(async () => {
    const s = await statusText();
    return s.startsWith(`${label} / ${database}`) && s.includes("objects loaded") ? s : null;
  }, 20000);
}

async function setText(text, closeSuggest = true) {
  await click(".editor .view-lines");
  await sleep(150); // let the editor take focus
  await press("Control+A");
  await press("Delete");
  await type(text);
  if (closeSuggest) await press("Escape");
}

async function acceptSuggestion(expect) {
  const s = await until(async () => {
    const t = await suggestText();
    return t.includes(expect) ? t : null;
  }, 8000);
  if (s) await press("Enter");
  await sleep(400);
  return s;
}

/** Put the cursor inside the first occurrence of `word` in the editor. */
async function cursorOn(word) {
  // wait until the editor has rendered the new text
  await until(() => evaluate(`[...document.querySelectorAll(".editor .view-line span span")].some((x) => x.textContent.includes(${JSON.stringify(word)}))`), 3000);
  await sleep(150);
  const box = await evaluate(`(() => {
    const spans = [...document.querySelectorAll(".editor .view-line span span")];
    const s = spans.find((x) => x.textContent.includes(${JSON.stringify(word)}));
    if (!s) return null;
    const r = s.getBoundingClientRect();
    const i = s.textContent.indexOf(${JSON.stringify(word)});
    const w = r.width / Math.max(1, s.textContent.length);
    return { x: r.x + (i + 0.5) * w, y: r.y + r.height / 2 };
  })()`);
  if (!box) return null;
  for (const t of ["mousePressed", "mouseReleased"]) {
    await send("Input.dispatchMouseEvent", { type: t, x: box.x, y: box.y, button: "left", clickCount: 1 });
  }
  return box;
}

const connected = await newConnectedTab("E2E SQL Server", "Shop");
check("connect Shop", !!connected, connected ?? "");

// 1. EXEC lays out parameters with type comments
await setText("EXEC ", false);
await sleep(300);
await type("dbo.GetC");
const execSug = await acceptSuggestion("GetCustomerOrders");
const exec = await editorText();
check(
  "EXEC expands parameters",
  !!execSug && /EXEC dbo\.GetCustomerOrders @CustomerId = 0,\s+-- int\n\s+@From = NULL\s+-- date \(has default\)/.test(exec),
  JSON.stringify(exec),
);
await shot("f1-exec");

// 2. GROUP BY suggests the non-aggregated columns
await setText("SELECT c.Country, COUNT(*) AS n FROM dbo.Customers c GROUP BY");
await type(" ");
await press("Control+Space");
const gb = await acceptSuggestion("c.Country");
check("GROUP BY suggestion", !!gb && (await editorText()).endsWith("GROUP BY c.Country"), JSON.stringify(await editorText()));

// 3. code analysis squiggles
await setText("SELECT o.Totl FROM dbo.Orders o WHERE o.Total = NULL");
const squiggles = await until(async () => {
  const n = await evaluate(`document.querySelectorAll(".editor .squiggly-warning, .editor .squiggly-info").length`);
  return n >= 2 ? n : null;
}, 5000);
check("analysis squiggles", !!squiggles, `${squiggles} markers`);
await shot("f3-squiggles");

// 4. hover over a table shows its columns
await setText("SELECT * FROM dbo.Orders");
const box = await cursorOn("Orders");
if (box) {
  // Monaco shows hovers after the pointer rests over a word
  for (const dx of [-30, -15, -5, 0, 1]) {
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: box.x + dx, y: box.y });
    await sleep(60);
  }
}
const hoverText = await until(async () => {
  const t = await evaluate(`[...document.querySelectorAll(".monaco-hover")].map((h) => h.innerText).join(" ")`);
  return t.includes("dbo.Orders") && t.includes("CustomerId") ? t : null;
}, 6000);
check("hover shows columns", !!hoverText, (hoverText ?? "").slice(0, 80).replace(/\n/g, " "));
await shot("f4-hover");

// 5. expand wildcards (Ctrl+B, Ctrl+W) and qualify names (Ctrl+B, Ctrl+Q)
await setText("SELECT * FROM Customers");
await press("Control+B");
await press("Control+W");
await sleep(500);
check("expand wildcards", (await editorText()).startsWith("SELECT Id, Name, Country, CreatedAt FROM Customers"), JSON.stringify(await editorText()));
await press("Control+B");
await press("Control+Q");
await sleep(500);
check("qualify object names", (await editorText()).endsWith("FROM dbo.Customers"), JSON.stringify(await editorText()));
await press("Control+Z");
await sleep(200);
check("refactor is undoable", (await editorText()).endsWith("FROM Customers"), JSON.stringify(await editorText()));

// 6. parameter hints
await setText("SELECT DATEADD(day, 1, ");
const hint = await until(async () => {
  const t = await evaluate(`document.querySelector(".parameter-hints-widget.visible")?.innerText ?? ""`);
  return t.includes("DATEADD") ? t : null;
}, 5000);
check("parameter hints", !!hint, (hint ?? "").replace(/\n/g, " ").slice(0, 60));
await press("Escape");

// 7. F12 opens the definition, Alt+F12 scripts as ALTER
const before = await tabCount();
await setText("EXEC dbo.GetCustomerOrders 1");
await cursorOn("GetCustomerOrders");
await press("F12");
await until(async () => (await tabCount()) > before, 8000);
await sleep(600);
const def = await editorText();
check("F12 opens definition", def.startsWith("CREATE PROCEDURE dbo.GetCustomerOrders"), JSON.stringify(def.slice(0, 60)));
await press("Control+W");
await sleep(400);
await cursorOn("GetCustomerOrders");
await press("Alt+F12");
await until(async () => (await tabCount()) > before, 8000);
await sleep(600);
const alter = await editorText();
check("Alt+F12 scripts ALTER", alter.startsWith("ALTER PROCEDURE dbo.GetCustomerOrders"), JSON.stringify(alter.slice(0, 60)));
await press("Control+W");
await sleep(400);

// 8. execution warning for DELETE without WHERE; cancel it
await setText("DELETE FROM dbo.OrderItems");
const statusBefore = await evaluate(`document.querySelector(".results-status")?.textContent ?? ""`);
await press("F5");
const modal = await until(() => evaluate(`document.querySelector(".confirm-modal")?.innerText ?? ""`), 5000);
await shot("f8-warning");
check("execution warning shown", !!modal && modal.includes("no WHERE"), (modal ?? "").split("\n").slice(0, 2).join(" | "));
await evaluate(`[...document.querySelectorAll(".confirm-modal button")].find((b) => b.textContent === "Cancel").click()`);
await sleep(500);
const statusAfter = await evaluate(`document.querySelector(".results-status")?.textContent ?? ""`);
check("cancel does not execute", statusAfter === statusBefore && !(await evaluate(`!!document.querySelector(".confirm-modal")`)));

// 9. object explorer: SELECT TOP 100 opens a tab and runs it
// open the explorer (F8 toggles it)
if (!(await evaluate(`!!document.querySelector(".explorer-panel")`))) await press("F8");
await waitFor(".explorer-panel");
await sleep(500);
await evaluate(`[...document.querySelectorAll(".tree-item")].find((e) => e.textContent.includes("Customers"))?.click()`);
await sleep(300);
await shot("f9-explorer");
const tabsBefore = await tabCount();
await evaluate(`[...document.querySelectorAll(".tree-actions button")].find((b) => b.textContent === "Select top 100")?.click()`);
const rows = await until(async () => {
  if ((await tabCount()) <= tabsBefore) return null;
  const r = await evaluate(`document.querySelectorAll(".grid-row").length`);
  return r > 0 ? r : null;
}, 15000);
check("explorer Select top 100", !!rows, `${rows} rows; ${JSON.stringify((await editorText()).slice(0, 40))}`);
await shot("f9-explorer-run");

// 10. copy result rows as INSERT statements
// capture what gets written: reading the clipboard back prompts for permission
await evaluate(`(() => { window.__clip = ""; navigator.clipboard.writeText = async (t) => { window.__clip = t; }; })()`);
await evaluate(`(() => { const s = document.querySelector(".copy-as"); s.value = "insert"; s.dispatchEvent(new Event("change", { bubbles: true })); })()`);
await sleep(300);
const clip = await evaluate(`window.__clip`);
check("copy as INSERT", clip.startsWith("INSERT INTO #Results ([Id], [Name]") && clip.includes("N'Nguyễn Văn A'"), clip.split("\n")[0]);

// 11. sort and filter the grid (Customers result is on screen)
const gridCol = (c) => evaluate(`[...document.querySelectorAll(".grid-row")].map((r) => r.children[${c}].textContent)`);
await evaluate(`[...document.querySelectorAll(".grid-head")].find((h) => h.textContent.startsWith("Name")).click()`);
await sleep(200);
const asc = await gridCol(2);
await evaluate(`[...document.querySelectorAll(".grid-head")].find((h) => h.textContent.startsWith("Name")).click()`);
await sleep(200);
const desc = await gridCol(2);
check("grid sort", JSON.stringify(asc) === JSON.stringify([...asc].sort()) && desc[0] === asc[asc.length - 1], `${asc} / ${desc}`);
await click(".grid-filter");
await type("John");
await sleep(300);
const filtered = await gridCol(2);
check("grid filter", filtered.length === 1 && filtered[0] === "John Smith", JSON.stringify(filtered));

// 12. CSV export writes a UTF-8 file with BOM (the save dialog itself is native)
const csvPath = process.env.TEMP + "\\sqlq-e2e.csv";
await ui.invoke("save_text_file", { path: csvPath, contents: "Name\r\nNguyễn Văn A" });
const { readFileSync } = await import("node:fs");
const csv = readFileSync(csvPath);
check("CSV file with BOM", csv[0] === 0xef && csv[1] === 0xbb && csv.toString("utf8").includes("Nguyễn"), `${csv.length} bytes`);

// 13. F2 renames an alias everywhere in the statement
await setText("SELECT o.Id FROM dbo.Orders o WHERE o.Total > 1");
await cursorOn("o");
await press("F2");
const renameBox = await until(() => evaluate(`!!document.querySelector(".rename-box input")`), 4000);
if (renameBox) {
  // the rename box opens with the old name selected
  await until(() => evaluate(`document.activeElement?.closest(".rename-box") != null`), 3000);
  await type("ord");
  await press("Enter");
  await sleep(500);
}
check("F2 rename alias", (await editorText()) === "SELECT ord.Id FROM dbo.Orders ord WHERE ord.Total > 1", JSON.stringify(await editorText()));

// 14. three-part names: objects of another database
await setText("SELECT * FROM Archive.dbo.", false);
const xdb = await until(async () => {
  const t = await suggestText();
  return t.includes("OldOrders") ? t : null;
}, 10000);
check("cross-database completion", !!xdb, (xdb ?? "").split("\n").slice(0, 2).join(" | "));
await press("Escape");

// 15. run on multiple databases
await setText("SELECT COUNT(*) AS objects FROM sys.tables");
await press("Control+Shift+F5");
await waitFor(".multi-modal");
await evaluate(`[...document.querySelectorAll(".multi-modal label")].filter((l) => ["Archive", "Shop"].includes(l.childNodes[1]?.textContent)).forEach((l) => { const i = l.querySelector("input"); if (!i.checked) i.click(); })`);
await sleep(200);
await evaluate(`[...document.querySelectorAll(".multi-modal button")].find((b) => b.textContent.startsWith("Run on")).click()`);
const multiRows = await until(async () => {
  const head = await evaluate(`[...document.querySelectorAll(".grid-head")].map((h) => h.textContent)`);
  if (!head[0]?.startsWith("Database")) return null;
  return gridCol(1);
}, 15000);
await shot("f15-multi");
check("multi-database run", JSON.stringify(multiRows) === JSON.stringify(["Archive", "Shop"]), JSON.stringify(multiRows));

// 16. find invalid objects
if (!(await evaluate(`!!document.querySelector(".explorer-panel")`))) await press("F8");
await waitFor(".explorer-panel");
await evaluate(`[...document.querySelectorAll(".explorer-panel button")].find((b) => b.textContent === "Find invalid objects").click()`);
const problems = await until(() => evaluate(`document.querySelector(".problems")?.innerText ?? ""`), 15000);
await shot("f16-invalid");
check("find invalid objects", (problems ?? "").includes("pBroken") && (problems ?? "").includes("DroppedTable"), (problems ?? "").replace(/\n/g, " | ").slice(0, 120));

// 17. history diff against the current tab
await press("F8");
await press("Control+Shift+H");
await waitFor(".history-item");
await sleep(400);
await evaluate(`document.querySelector(".history-item").click()`);
await sleep(200);
await evaluate(`[...document.querySelectorAll(".search-preview button")].find((b) => b.textContent === "Diff current").click()`);
const diffOk = await until(() => evaluate(`!!document.querySelector(".diff-modal .monaco-diff-editor")`), 5000);
await shot("f17-diff");
check("history diff", !!diffOk);
await evaluate(`[...document.querySelectorAll(".diff-modal button")].find((b) => b.textContent === "Close").click()`);
await press("Control+Shift+H");

if (await evaluate(`!!document.querySelector(".explorer-panel")`)) await press("F8");
await press("Control+W");
await sleep(300);
await press("Control+W");
const failed = summary();
ui.close();
process.exit(failed ? 1 : 0);
