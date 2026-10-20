// UI smoke test against the running desktop app, over the raw Chrome
// DevTools Protocol (WebView2 exposes it when remote debugging is on).
//
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"; npx tauri dev
//   node scripts/ui-smoke.mjs [screenshot-dir]
import { mkdirSync } from "node:fs";
import { attach, checker } from "./cdp.mjs";

const outDir = process.argv[2] ?? "smoke-out";
mkdirSync(outDir, { recursive: true });
const { sleep, evaluate, waitFor, click, clickText, press, type, shot, editorText, close } = await attach(outDir);
const { check, summary } = checker();

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

const failed = summary();
close();
process.exit(failed ? 1 : 0);
