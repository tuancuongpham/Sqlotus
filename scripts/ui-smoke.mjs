// UI smoke test against the running desktop app, over the raw Chrome
// DevTools Protocol (WebView2 exposes it when remote debugging is on).
//
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"; npx tauri dev
//   node scripts/ui-smoke.mjs [screenshot-dir]
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const outDir = process.argv[2] ?? "smoke-out";
mkdirSync(outDir, { recursive: true });

const targets = await (await fetch("http://127.0.0.1:9222/json/list")).json();
const target = targets.find((t) => t.type === "page" && t.url.includes("localhost:1420"));
if (!target) throw new Error("app page not found");

const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((res, rej) => {
  ws.onopen = res;
  ws.onerror = rej;
});
let seq = 0;
const pending = new Map();
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
  }
};
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++seq;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.text + " " + (r.exceptionDetails.exception?.description ?? ""));
  return r.result.value;
};
const waitFor = async (selector, timeout = 5000) => {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await evaluate(`!!document.querySelector(${JSON.stringify(selector)})`)) return true;
    await sleep(100);
  }
  return false;
};
const click = async (selector) => {
  const box = await evaluate(`(() => {
    const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find((e) => e.offsetParent !== null);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.x + Math.min(r.width / 2, 40), y: r.y + r.height / 2 };
  })()`);
  if (!box) throw new Error(`no element ${selector}`);
  for (const type of ["mousePressed", "mouseReleased"]) {
    await send("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button: "left", clickCount: 1 });
  }
};
const clickText = (text) =>
  evaluate(`(() => {
    const el = [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === ${JSON.stringify(text)});
    el?.click();
    return !!el;
  })()`);

const MOD = { Alt: 1, Control: 2, Shift: 8 };
const KEYS = {
  Escape: 27,
  Tab: 9,
  Delete: 46,
  Enter: 13,
  F5: 116,
};
/** press("Control+Shift+H") */
const press = async (combo) => {
  const parts = combo.split("+");
  const key = parts.pop();
  const modifiers = parts.reduce((m, p) => m | MOD[p], 0);
  const vk = KEYS[key] ?? key.toUpperCase().charCodeAt(0);
  const code = KEYS[key] ? key : `Key${key.toUpperCase()}`;
  const base = { modifiers, key: KEYS[key] ? key : key.toLowerCase(), code, windowsVirtualKeyCode: vk };
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...base });
  await send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
};
const type = async (text) => {
  for (const ch of text) {
    await send("Input.dispatchKeyEvent", { type: "keyDown", text: ch, key: ch, unmodifiedText: ch });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: ch });
  }
};
const shot = async (name) => {
  const { data } = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
};
const editorText = async () =>
  ((await evaluate(`document.querySelector(".editor .view-lines")?.innerText ?? ""`)) ?? "").replace(/ /g, " ");

const results = [];
const check = (name, ok, extra = "") => {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${extra ? " — " + extra : ""}`);
};

check("editor rendered", await waitFor(".monaco-editor", 20000));

await press("Control+N");
await sleep(300);
await click(".editor .view-lines");
await type("select a, b, count(*) from dbo.t where x = 1 and y in (1,2) group by a, b");
await press("Escape");
await press("Control+K");
await press("Control+Y");
await sleep(800);
const formatted = await editorText();
check("format (Ctrl+K, Ctrl+Y)", formatted.includes("SELECT a,") && formatted.includes("GROUP BY a,"), JSON.stringify(formatted.slice(0, 140)));
await shot("01-formatted");

await press("Control+A");
await press("Delete");
await type("ssf");
const suggested = await waitFor(".suggest-widget.visible", 5000);
const suggest = suggested ? await evaluate(`document.querySelector(".suggest-widget.visible").innerText`) : "";
check("snippet suggestion ssf", suggest.includes("ssf"), JSON.stringify(suggest.slice(0, 100)));
await shot("02-suggest");
await press("Tab");
await sleep(400);
const expanded = await editorText();
check("snippet expands on Tab", expanded.includes("SELECT * FROM"), JSON.stringify(expanded));

await press("Control+Shift+H");
check("history panel opens", await waitFor(".history-panel", 3000));
await sleep(600);
await shot("03-history");
await press("Control+Shift+H");

await clickText("⚙ Options");
check("options dialog opens", await waitFor(".format-preview", 3000));
await sleep(800);
const preview = await evaluate(`document.querySelector(".format-preview")?.textContent ?? ""`);
check("options format preview", preview.includes("INNER JOIN"), JSON.stringify(preview.slice(0, 80)));
await shot("04-options");
await clickText("Done");
await sleep(300);

await evaluate(`document.querySelector(".toolbar button")?.click()`);
check("connection dialog opens", await waitFor(".conn-modal", 3000));
await shot("05-connect");
await press("Escape");
await sleep(200);
await press("Control+W");

const failed = results.filter((r) => !r.ok).length;
console.log(`\n${results.length - failed}/${results.length} passed`);
ws.close();
process.exit(failed ? 1 : 0);
