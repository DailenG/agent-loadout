// Loadout site: patch cables, install tabs, copy buttons and the OS-aware
// download button. Everything here is an enhancement: without JS the lists
// read correctly, every install panel is shown, and downloads link to the
// latest GitHub release page.
"use strict";

document.documentElement.classList.add("js");

const REPO = document.documentElement.dataset.repo || "DailenG/agent-loadout";
const RELEASES = `https://github.com/${REPO}/releases/latest`;
const reduced = matchMedia("(prefers-reduced-motion: reduce)");

/* ---------------------------------------------------------------- cables */

// Which store entries are patched into which clients (decoration only).
const PATCHES = {
  github: ["omp", "claude-code", "codex", "cursor"],
  context7: ["omp", "codex", "copilot"],
  playwright: ["omp", "claude-code"],
  sentry: ["claude-code", "gemini"],
  "code-review": ["omp", "claude-code", "codex"],
};
const SVG_NS = "http://www.w3.org/2000/svg";
const rack = document.querySelector(".bay__rack");
const cables = rack && rack.querySelector(".cables");
let drawnOnce = false;

function center(el, box) {
  const r = el.getBoundingClientRect();
  return { x: r.left + r.width / 2 - box.left, y: r.top + r.height / 2 - box.top };
}

function cablePath(d, cls, color, delay) {
  const p = document.createElementNS(SVG_NS, "path");
  p.setAttribute("d", d);
  p.setAttribute("pathLength", "1");
  p.setAttribute("class", cls);
  if (color) p.style.stroke = color;
  p.style.setProperty("--d", `${delay}s`);
  return p;
}

function drawCables() {
  if (!rack || !cables) return;
  cables.replaceChildren();
  const box = rack.getBoundingClientRect();
  const stacked = getComputedStyle(rack).gridTemplateColumns.split(" ").length === 1;
  const store = rack.querySelector(".module--store").getBoundingClientRect();
  const clients = rack.querySelector(".module--clients").getBoundingClientRect();
  const sources = [...rack.querySelectorAll("[data-src]")];
  const animate = !drawnOnce && !reduced.matches;
  const live = new Map();
  let n = 0;

  sources.forEach((src, si) => {
    const color = getComputedStyle(src).getPropertyValue("--c").trim();
    (PATCHES[src.dataset.src] || []).forEach((dstId, di) => {
      const dst = rack.querySelector(`[data-dst="${dstId}"]`);
      if (!dst) return;
      let d;
      if (stacked) {
        // Phones: a vertical bundle from the store's bottom edge to the clients' top edge.
        const spread = (si - (sources.length - 1) / 2) * 14;
        const x1 = store.left + store.width / 2 + spread - box.left;
        const y1 = store.bottom - box.top - 2;
        const x2 = clients.left + clients.width / 2 + spread * 1.6 + (di - 1) * 6 - box.left;
        const y2 = clients.top - box.top + 2;
        const k = (y2 - y1) * 0.6;
        d = `M${x1} ${y1} C${x1} ${y1 + k} ${x2} ${y2 - k} ${x2} ${y2}`;
      } else {
        const a = center(src.querySelector(".jack"), box);
        const b = center(dst.querySelector(".jack"), box);
        const k = (b.x - a.x) * 0.55;
        d = `M${a.x} ${a.y} C${a.x + k} ${a.y} ${b.x - k} ${b.y} ${b.x} ${b.y}`;
      }
      const delay = animate ? 0.25 + n * 0.09 : 0;
      cables.append(
        cablePath(d, "cable cable--glow", color, delay),
        cablePath(d, "cable", color, delay),
        cablePath(d, "pulse", null, delay + 1.2 + (n % 5) * 0.4),
      );
      if (!live.has(dstId)) live.set(dstId, delay + 1.1);
      n++;
    });
  });

  for (const [id, at] of live) {
    const li = rack.querySelector(`[data-dst="${id}"]`);
    if (animate) setTimeout(() => li.classList.add("is-live"), at * 1000);
    else li.classList.add("is-live");
  }
  if (!animate) {
    for (const c of cables.querySelectorAll(".cable")) {
      c.style.animation = "none";
      c.style.strokeDashoffset = "0";
    }
  }
  drawnOnce = true;
}

if (rack) {
  document.fonts.ready.then(drawCables);
  let lastWidth = 0;
  new ResizeObserver((entries) => {
    const width = entries[0].contentRect.width;
    if (drawnOnce && width !== lastWidth) drawCables();
    lastWidth = width;
  }).observe(rack);
  // Theme switches change the cable colors.
  matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => drawnOnce && drawCables());
}

/* ------------------------------------------------------------------ tabs */

// WAI-ARIA tabs with manual activation on click and automatic on arrow keys.
const tablist = document.querySelector('[role="tablist"]');
const tabs = tablist ? [...tablist.querySelectorAll('[role="tab"]')] : [];

function selectTab(tab, focus) {
  for (const t of tabs) {
    const on = t === tab;
    t.setAttribute("aria-selected", String(on));
    t.tabIndex = on ? 0 : -1;
    document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
  }
  if (focus) tab.focus();
}

if (tablist) {
  for (const t of tabs) {
    const panel = document.getElementById(t.getAttribute("aria-controls"));
    panel.setAttribute("role", "tabpanel");
    panel.setAttribute("aria-labelledby", t.id);
  }
  tabs.forEach((tab, i) => {
    tab.addEventListener("click", () => selectTab(tab, true));
    tab.addEventListener("keydown", (e) => {
      const move = { ArrowRight: 1, ArrowLeft: -1 }[e.key];
      let next = null;
      if (move) next = tabs[(i + move + tabs.length) % tabs.length];
      if (e.key === "Home") next = tabs[0];
      if (e.key === "End") next = tabs[tabs.length - 1];
      if (next) {
        e.preventDefault();
        selectTab(next, true);
      }
    });
  });
  tablist.hidden = false;
  selectTab(tabs[0], false);
}

/* ------------------------------------------------------------------ copy */

for (const button of document.querySelectorAll("[data-copy]")) {
  if (!navigator.clipboard) continue;
  button.hidden = false;
  button.setAttribute("aria-label", `Copy: ${button.dataset.copy}`);
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(button.dataset.copy);
      button.textContent = "Copied";
    } catch {
      button.textContent = "Failed";
    }
    setTimeout(() => { button.textContent = "Copy"; }, 1600);
  });
}

/* -------------------------------------------------------------- download */

function detectOS() {
  // Client hints first (Chromium), then the user agent string, then the
  // legacy platform property.
  const ua = navigator.userAgent || "";
  const hint = (navigator.userAgentData && navigator.userAgentData.platform) || "";
  const source = hint || ua || navigator.platform || "";
  if (/Android|iPhone|iPad|iPod/i.test(ua) || (/Mac/.test(source) && navigator.maxTouchPoints > 1)) return "mobile";
  if (/CrOS|Chrome OS/i.test(source)) return "other";
  if (/Win/i.test(source)) return "windows";
  if (/Mac/i.test(source)) return "macos";
  if (/Linux|X11/i.test(source)) return "linux";
  return "other";
}

// Release assets carry the version in their names, so "latest" is resolved
// through the GitHub API. Any failure keeps the release-page links.
const ASSETS = {
  windows: /^Loadout_.+_x64-setup\.exe$/,
  appimage: /^agent-loadout_.+_amd64\.AppImage$/,
  deb: /^agent-loadout_.+_amd64\.deb$/,
  rpm: /^agent-loadout-.+\.x86_64\.rpm$/,
};
const CACHE_KEY = "loadout-latest-release";
const CACHE_MS = 60 * 60 * 1000;

async function latestRelease() {
  try {
    const cached = JSON.parse(sessionStorage.getItem(CACHE_KEY) || "null");
    if (cached && Date.now() - cached.at < CACHE_MS) return cached.release;
  } catch { /* storage unavailable */ }
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 6000);
  try {
    const res = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
      headers: { Accept: "application/vnd.github+json" },
      signal: controller.signal,
      credentials: "omit",
      referrerPolicy: "no-referrer",
    });
    if (!res.ok) return null;
    const data = await res.json();
    const release = { tag: String(data.tag_name || ""), assets: {} };
    for (const asset of data.assets || []) {
      for (const [key, re] of Object.entries(ASSETS)) {
        if (re.test(asset.name) && /^https:\/\/github\.com\//.test(asset.browser_download_url)) {
          release.assets[key] = { name: asset.name, url: asset.browser_download_url };
        }
      }
    }
    try { sessionStorage.setItem(CACHE_KEY, JSON.stringify({ at: Date.now(), release })); } catch { /* ignore */ }
    return release;
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

const heroButton = document.getElementById("hero-download");
const heroMeta = document.getElementById("hero-meta");
const os = detectOS();

function link(text, href) {
  const a = document.createElement("a");
  a.href = href;
  a.textContent = text;
  return a;
}

function setMeta(...parts) {
  if (!heroMeta) return;
  heroMeta.replaceChildren(...parts);
}

function tabFor(name) {
  return tabs.find((t) => t.id === `t-${name}`);
}

// Open the matching install tab first.
const startTab = { windows: "windows", linux: "linux", macos: "macos" }[os];
if (startTab && tabFor(startTab)) selectTab(tabFor(startTab), false);

// Hero button before the API answers: OS-specific label, release-page link.
if (heroButton) {
  if (os === "windows") {
    heroButton.textContent = "Download for Windows";
    heroButton.href = RELEASES;
    setMeta("Signed per-user installer, x64. ", link("Other downloads", "#install"));
  } else if (os === "linux") {
    heroButton.textContent = "Download for Linux";
    heroButton.href = RELEASES;
    setMeta("AppImage, x86_64. Also ", link(".deb and .rpm", "#install"), ".");
  } else if (os === "macos") {
    heroButton.textContent = "Install on macOS";
    heroButton.href = "#install";
    setMeta("No macOS app yet: the loadout CLI installs with cargo. ", link("How to install", "#install"));
    heroButton.addEventListener("click", () => { const t = tabFor("macos"); if (t) selectTab(t, false); });
  }
}

latestRelease().then((release) => {
  if (!release) return;
  const version = release.tag.replace(/^v/, "");
  for (const el of document.querySelectorAll("[data-release-version]")) {
    if (version) el.textContent = ` (version ${version})`;
  }
  for (const el of document.querySelectorAll("[data-asset]")) {
    const asset = release.assets[el.dataset.asset];
    if (asset) el.href = asset.url;
  }
  for (const el of document.querySelectorAll("[data-asset-name]")) {
    const asset = release.assets[el.dataset.assetName];
    if (asset) el.textContent = asset.name;
  }
  if (!heroButton) return;
  const key = { windows: "windows", linux: "appimage" }[os];
  const asset = key && release.assets[key];
  if (!asset) return;
  heroButton.href = asset.url;
  const v = version ? `Version ${version}. ` : "";
  if (os === "windows") setMeta(`${v}Signed per-user installer, x64. `, link("Other downloads", "#install"));
  else setMeta(`${v}AppImage, x86_64. Also `, link(".deb and .rpm", "#install"), ".");
});
