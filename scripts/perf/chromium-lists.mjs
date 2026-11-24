// Long-list measurements in real Chromium (#17, docs/perf section 11): drives
// headless Edge (or Chrome) through the DevTools protocol against a served
// mock bundle with the perf dataset, and prints one JSON object per run.
//
//   npx vite build --mode mock --outDir <dir>
//   npx vite preview --outDir <dir> --port 4310 --strictPort
//   node scripts/perf/chromium-lists.mjs http://127.0.0.1:4310/ [runs] [browser] [variants]
//
// `variants`: comma-separated extra query strings, run alternately within each
// round (`,cv` compares the default build with `?cv`); each line names its own.
// Per page (Skills, MCP, History with "Load more" until 1000 rows or the end):
// DOM nodes and listeners
// (`Memory.getDOMCounters`), main-thread task ms of the visit
// (`Performance.getMetrics`), and a 3 s scripted scroll of the page's list
// (frames counted with requestAnimationFrame, scrollTop +40 px a frame).
// Window 1088x680, as in the baseline.

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const base = process.argv[2] ?? "http://127.0.0.1:4310/";
const runs = Number(process.argv[3] ?? 3);
const variants = (process.argv[5] ?? "").split(",");
const browserPath =
  process.argv[4] ?? "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const PORT = 9333;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function connect() {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      const page = targets.find((target) => target.type === "page");
      if (page) {
        return page.webSocketDebuggerUrl;
      }
    } catch {
      // Not up yet.
    }
    await sleep(100);
  }
  throw new Error("the browser did not open its DevTools port");
}

function session(url) {
  const socket = new WebSocket(url);
  let next = 0;
  const waiting = new Map();
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    const pending = waiting.get(message.id);
    if (pending) {
      waiting.delete(message.id);
      if (message.error) {
        pending.reject(new Error(message.error.message));
      } else {
        pending.resolve(message.result);
      }
    }
  };
  const opened = new Promise((resolve) => {
    socket.onopen = resolve;
  });
  return {
    opened,
    close: () => socket.close(),
    send(method, params = {}) {
      next += 1;
      const id = next;
      socket.send(JSON.stringify({ id, method, params }));
      return new Promise((resolve, reject) => waiting.set(id, { resolve, reject }));
    },
  };
}

async function evaluate(cdp, expression) {
  const result = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description ?? "evaluation failed");
  }
  return result.result.value;
}

async function taskMs(cdp) {
  const { metrics } = await cdp.send("Performance.getMetrics");
  return metrics.find((metric) => metric.name === "TaskDuration").value * 1000;
}

async function counters(cdp) {
  const { nodes, jsEventListeners } = await cdp.send("Memory.getDOMCounters");
  return { nodes, listeners: jsEventListeners };
}

/** Resolves once `selector` matches and the count held for 10 frames; the ms from `since`. */
const settledScript = (selector, since) => `new Promise((resolve) => {
  const start = ${since};
  let last = -1, steady = 0, first = 0;
  const tick = () => {
    const count = document.querySelectorAll(${JSON.stringify(selector)}).length;
    if (count > 0 && !first) first = performance.now();
    steady = count > 0 && count === last ? steady + 1 : 0;
    last = count;
    if (steady >= 10) resolve({ firstMs: Math.round(first - start), count });
    else requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
})`;

/** Scrolls the list's scroll container for 3 s; frames per second. */
const scrollScript = (selector) => `new Promise((resolve) => {
  let box = document.querySelector(${JSON.stringify(selector)});
  while (box && !/(auto|scroll)/.test(getComputedStyle(box).overflowY)) box = box.parentElement;
  box = box ?? document.scrollingElement;
  box.scrollTop = 0;
  let frames = 0, down = true;
  const start = performance.now();
  const tick = () => {
    frames += 1;
    if (down && box.scrollTop + box.clientHeight >= box.scrollHeight - 2) down = false;
    if (!down && box.scrollTop <= 0) down = true;
    box.scrollTop += down ? 40 : -40;
    if (performance.now() - start < 3000) requestAnimationFrame(tick);
    else resolve(Math.round(frames / ((performance.now() - start) / 1000)));
  };
  requestAnimationFrame(tick);
})`;

async function visit(cdp, label, settleSelector) {
  const before = await taskMs(cdp);
  const now = await evaluate(cdp, "performance.now()");
  await evaluate(
    cdp,
    `[...document.querySelectorAll("nav button, nav a")].find((el) => el.textContent.trim().startsWith(${JSON.stringify(label)})).click()`,
  );
  const settled = await evaluate(cdp, settledScript(settleSelector, now));
  return { ...settled, taskMs: Math.round((await taskMs(cdp)) - before) };
}

async function measure(cdp, selector) {
  await sleep(500);
  const dom = await counters(cdp);
  const before = await taskMs(cdp);
  const fps = await evaluate(cdp, scrollScript(selector));
  return { ...dom, scrollFps: fps, scrollTaskMs: Math.round((await taskMs(cdp)) - before) };
}

async function run(variant) {
  const profile = mkdtempSync(join(tmpdir(), "loadout-perf-"));
  const browser = spawn(browserPath, [
    "--headless=new",
    `--remote-debugging-port=${PORT}`,
    `--user-data-dir=${profile}`,
    "--window-size=1088,680",
    "--no-first-run",
    "about:blank",
  ]);
  try {
    const cdp = session(await connect());
    await cdp.opened;
    await cdp.send("Performance.enable");
    await cdp.send("Page.enable");
    await cdp.send("Page.navigate", { url: `${base}?dataset=perf${variant ? `&${variant}` : ""}` });
    // The new document replaces about:blank; evaluate again until it answers.
    for (;;) {
      const ready = await evaluate(cdp, `location.search.includes("dataset=perf") && document.readyState !== "loading"`).catch(
        () => false,
      );
      if (ready) {
        break;
      }
      await sleep(5);
    }
    const skillsLoad = await evaluate(cdp, settledScript(".skill-card", 0));
    const skills = { load: { ...skillsLoad, taskMs: Math.round(await taskMs(cdp)) }, ...(await measure(cdp, ".skill-card")) };

    const mcpVisit = await visit(cdp, "MCP", ".mcp-server-card");
    const mcp = { visit: mcpVisit, ...(await measure(cdp, ".mcp-server-card")) };

    const historyVisit = await visit(cdp, "History", ".history-list > li");
    for (let page = 0; page < 20; page += 1) {
      const more = await evaluate(
        cdp,
        `new Promise((resolve) => {
          const rows = document.querySelectorAll(".history-list > li").length;
          const button = [...document.querySelectorAll("button")].find((el) => el.textContent.trim() === "Load more");
          if (!button || rows >= 1000) return resolve(false);
          button.click();
          const tick = () => document.querySelectorAll(".history-list > li").length > rows ? resolve(true) : setTimeout(tick, 10);
          tick();
        })`,
      );
      if (!more) {
        break;
      }
    }
    const rows = await evaluate(cdp, `document.querySelectorAll(".history-list > li").length`);
    const history = { visit: historyVisit, rows, ...(await measure(cdp, ".history-list")) };
    cdp.close();
    return { skills, mcp, history };
  } finally {
    browser.kill();
    await sleep(500);
    // The browser may still hold its profile for a moment after exit.
    rmSync(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  }
}

for (let index = 0; index < runs; index += 1) {
  for (const variant of variants) {
    console.log(JSON.stringify({ variant, ...(await run(variant)) }));
  }
}
