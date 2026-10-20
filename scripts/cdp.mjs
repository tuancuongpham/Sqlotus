// Minimal Chrome DevTools Protocol driver for the running desktop app
// (WebView2 exposes CDP when started with --remote-debugging-port=9222).
import { writeFileSync } from "node:fs";
import { join } from "node:path";

export async function attach(outDir) {
  const targets = await (await fetch("http://127.0.0.1:9222/json/list")).json();
  const target = targets.find((t) => t.type === "page" && t.url.includes("localhost:1420") && t.title === "SqlSupportQuery");
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
  const debug = !!process.env.SQLQ_CDP_DEBUG;
  // A CDP call that never answers means the page is frozen: fail loudly.
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++seq;
      if (debug) console.error(">", method, JSON.stringify(params).slice(0, 120));
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`CDP ${method} timed out (page unresponsive?)`));
      }, 30000);
      pending.set(id, {
        resolve: (v) => (clearTimeout(timer), resolve(v)),
        reject: (e) => (clearTimeout(timer), reject(e)),
      });
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
    F2: 113,
    F5: 116,
    F8: 119,
    F12: 123,
    Space: 32,
    Home: 36,
    End: 35,
    ArrowRight: 39,
    ArrowDown: 40,
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

  const invoke = (cmd, args = {}) =>
    evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)})`);
  return { send, sleep, evaluate, waitFor, click, clickText, press, type, shot, editorText, invoke, close: () => ws.close() };
}

export function checker() {
  const results = [];
  const check = (name, ok, extra = "") => {
    results.push({ name, ok });
    console.log(`${ok ? "PASS" : "FAIL"} ${name}${extra ? " — " + extra : ""}`);
  };
  const summary = () => {
    const failed = results.filter((r) => !r.ok).length;
    console.log(`
${results.length - failed}/${results.length} passed`);
    return failed;
  };
  return { check, summary };
}
