// Shared helpers for the end-to-end suites (scripts/e2e-*.mjs): waiting for
// the app, reading notifications and dialogs, connecting a tab, and putting
// SQL into the editor in the one way that works reliably with Monaco.
//
//   import { start } from "./e2e-kit.mjs";
//   const t = await start();            // attaches to the running app
//   await t.connectNewTab("E2E SQL Server");
//   await t.runSql("SELECT 1");
//   t.check("ran", ...);
//   process.exit(t.summary() ? 1 : 0);
import { mkdirSync } from "node:fs";
import { attach, checker } from "./cdp.mjs";

/**
 * Attach to the app (started with --remote-debugging-port=9222) and return
 * the driver, the checker and the helpers. Screenshots go to `outDir`.
 */
export async function start(outDir = process.argv[2] ?? "e2e-out") {
  mkdirSync(outDir, { recursive: true });
  const password = process.env.SQLOTUS_E2E_PASSWORD;
  if (!password) throw new Error("set SQLOTUS_E2E_PASSWORD");
  const ui = await attach(outDir);
  return { ui, password, ...checker(), ...helpers(ui) };
}

export function helpers(ui) {
  const { sleep, evaluate, click, press, send } = ui;
  const flat = (s) => s.replace(/\s+/g, " ").trim();

  /** Poll `fn` until it returns something truthy (returned) or time runs out (null). */
  const until = async (fn, timeout = 10000) => {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      const v = await fn();
      if (v) return v;
      await sleep(150);
    }
    return null;
  };

  // -- notifications and dialogs -------------------------------------------
  const toastText = () => evaluate(`[...document.querySelectorAll(".toast")].map((t) => t.innerText.replace(/\\s+/g, " ")).join(" | ")`);
  const waitToast = (needle, timeout = 10000) =>
    until(async () => {
      const t = await toastText();
      return t.includes(needle) ? t : null;
    }, timeout);
  const clearToasts = () => evaluate(`document.querySelectorAll(".toast-close").forEach((b) => b.click())`);
  /** Click the first button under `scope` whose label starts with `label`; false when there is none. */
  const clickButton = (scope, label) =>
    evaluate(`(() => {
      const b = [...document.querySelectorAll(${JSON.stringify(scope + " button")})].find((x) => x.textContent.trim().startsWith(${JSON.stringify(label)}));
      b?.click();
      return !!b;
    })()`);
  const text = (selector) => evaluate(`document.querySelector(${JSON.stringify(selector)})?.innerText ?? ""`);
  const statusText = () => text(".status-msg");

  // -- tabs ----------------------------------------------------------------
  // A tab is known by its name (`.tab-title`): the rest of its label changes
  // as it connects and runs.
  const tabTitles = () => evaluate(`[...document.querySelectorAll(".tab .tab-title")].map((t) => t.textContent)`);
  const activeTitle = () => evaluate(`document.querySelector(".tab.active .tab-title")?.textContent ?? ""`);
  /** Make the tab named `name` active; true once it is. Tabs react to mousedown, not click. */
  const selectTab = async (name) => {
    await evaluate(`[...document.querySelectorAll(".tab")]
      .find((t) => t.querySelector(".tab-title")?.textContent === ${JSON.stringify(name)})
      ?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }))`);
    return !!(await until(async () => (await activeTitle()) === name, 3000));
  };

  /** Open a new tab and connect it to the saved connection named `label`. */
  const connectNewTab = async (label) => {
    await press("Control+N");
    await sleep(300);
    await evaluate(`document.querySelector(".toolbar button").click()`);
    await until(() => evaluate(`!!document.querySelector(".conn-modal")`), 5000);
    await until(() =>
      evaluate(`(() => { const e = [...document.querySelectorAll(".conn-item")].find((e) => e.textContent.includes(${JSON.stringify(label)})); e?.click(); return !!e; })()`),
    );
    // never press Connect on an empty form: that would save a new "localhost" connection
    const ready = await until(async () => (await evaluate(`document.querySelector(".conn-form input")?.value`)) === label);
    if (!ready) throw new Error(`connection ${label} was not selected`);
    await clearToasts();
    await clickButton(".conn-modal .modal-actions", "Connect");
    const ok = await waitToast(`Connected to ${label}`, 20000);
    await until(async () => (await statusText()).includes("objects loaded"), 15000);
    return ok;
  };

  // -- the editor ------------------------------------------------------------
  // Monaco takes text asynchronously, so typing key by key can accept a
  // completion (Enter) or auto-close a bracket, and after a dialog the focus
  // is elsewhere. So: focus the editor, select all, insert the text in one
  // go, and wait until the editor shows it; retry up to three times.
  const focusEditor = async () => {
    await click(".editor .view-lines");
    await sleep(150);
    await evaluate(`document.querySelector(".editor .native-edit-context, .editor textarea")?.focus()`);
  };
  const setText = async (sql) => {
    for (let attempt = 0; attempt < 3; attempt++) {
      await focusEditor();
      await press("Control+A");
      await press("Delete");
      await sleep(100);
      await send("Input.insertText", { text: sql });
      if (await until(async () => flat(await ui.editorText()) === flat(sql), 3000)) {
        await press("Escape");
        return true;
      }
    }
    throw new Error(`the editor did not take the text:\n got  ${JSON.stringify(flat(await ui.editorText()).slice(0, 200))}\n want ${JSON.stringify(flat(sql).slice(0, 200))}`);
  };
  /**
   * Replace the editor text by pasting `text`, as Ctrl+V does: one edit,
   * however long the text (Input.insertText goes through Monaco's typing
   * path, character by character, and a long text takes minutes there).
   * Resolves once the cursor reached the end of the pasted text.
   */
  const paste = async (sql) => {
    await focusEditor();
    await press("Control+A");
    await press("Delete");
    await sleep(100);
    await evaluate(`(() => {
      const data = new DataTransfer();
      data.setData("text/plain", ${JSON.stringify(sql)});
      document.querySelector(".editor .native-edit-context, .editor textarea")
        .dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
    })()`);
    const lines = sql.split("\n");
    const end = `Ln ${lines.length}, Col ${lines[lines.length - 1].length + 1}`;
    return until(async () => (await text(".statusbar")).includes(end), 10000);
  };
  /** Put the cursor at the start of line `line` (1-based) of the editor. */
  const cursorToLine = async (line) => {
    await focusEditor();
    await press("Control+Home");
    for (let i = 1; i < line; i++) await press("ArrowDown");
    await press("Home");
  };

  /**
   * Run `sql` (or, with `statement`, the statement at the cursor via
   * Shift+F5, the text already in the editor) and wait until it finished.
   * Execution warnings are answered with `answer` ("Execute anyway" by
   * default; null leaves them open). Returns the finishing notification.
   */
  const runSql = async (sql, { answer = "Execute anyway", statement = false } = {}) => {
    if (sql != null) await setText(sql);
    await clearToasts();
    await press(statement ? "Shift+F5" : "F5");
    if (answer) {
      const warned = await until(() => evaluate(`!!document.querySelector(".confirm-modal")`), 1500);
      if (warned) await clickButton(".confirm-modal", answer);
    }
    return until(async () => {
      const t = await toastText();
      return /Query finished|finished with errors|transaction still open|cancelled|failed/.test(t) ? t : null;
    }, 30000);
  };

  /** Rows of the first result grid, as arrays of cell texts (row-number column left out). */
  const gridRows = () => evaluate(`[...document.querySelectorAll(".grid-row")].map((r) => [...r.children].slice(1).map((c) => c.textContent))`);

  /**
   * Close every tab opened since `before` (a set of tab names from
   * tabTitles()), answering open-transaction questions with Roll back. A tab
   * that was open before is never closed.
   */
  const closeTabsSince = async (before) => {
    for (let i = 0; i < 30; i++) {
      const own = (await tabTitles()).find((name) => !before.has(name));
      if (!own) break;
      // Ctrl+W closes the active tab: never press it unless the active one is ours
      if (!(await selectTab(own))) break;
      await press("Control+W");
      await sleep(300);
      await evaluate(`[...document.querySelectorAll(".modal button")].find((b) => /roll back|discard/i.test(b.textContent))?.click()`);
      await sleep(300);
    }
  };

  return {
    flat,
    until,
    toastText,
    waitToast,
    clearToasts,
    clickButton,
    text,
    statusText,
    tabTitles,
    activeTitle,
    selectTab,
    connectNewTab,
    focusEditor,
    setText,
    paste,
    cursorToLine,
    runSql,
    gridRows,
    closeTabsSince,
  };
}
