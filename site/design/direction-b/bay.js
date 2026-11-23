// Direction B: draws patch cables between store jacks and client jacks.
// The lists are the content; cables are decoration (aria-hidden).
const PATCHES = {
  github: ["omp", "claude-code", "codex", "cursor"],
  context7: ["omp", "codex", "copilot"],
  playwright: ["omp", "claude-code"],
  sentry: ["claude-code", "gemini"],
  "code-review": ["omp", "claude-code", "codex"],
};
const SVG_NS = "http://www.w3.org/2000/svg";
const rack = document.querySelector(".bay__rack");
const svg = rack?.querySelector(".cables");
const reduced = matchMedia("(prefers-reduced-motion: reduce)");
let drawnOnce = false;

function center(el, box) {
  const r = el.getBoundingClientRect();
  return { x: r.left + r.width / 2 - box.left, y: r.top + r.height / 2 - box.top };
}

function path(d, cls, color, delay) {
  const p = document.createElementNS(SVG_NS, "path");
  p.setAttribute("d", d);
  p.setAttribute("pathLength", "1");
  p.setAttribute("class", cls);
  if (color) p.style.stroke = color;
  p.style.setProperty("--d", `${delay}s`);
  return p;
}

function draw() {
  if (!rack || !svg) return;
  svg.replaceChildren();
  const box = rack.getBoundingClientRect();
  const stacked = getComputedStyle(rack).gridTemplateColumns.split(" ").length === 1;
  const store = rack.querySelector(".module--store").getBoundingClientRect();
  const clients = rack.querySelector(".module--clients").getBoundingClientRect();
  const sources = [...rack.querySelectorAll("[data-src]")];
  const live = new Map();
  let n = 0;

  sources.forEach((src, si) => {
    const color = getComputedStyle(src).getPropertyValue("--c").trim();
    PATCHES[src.dataset.src].forEach((dstId, di) => {
      const dst = rack.querySelector(`[data-dst="${dstId}"]`);
      if (!dst) return;
      let d;
      if (stacked) {
        // Vertical trunk: leave the store's bottom edge, land on the clients' top edge.
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
      const delay = drawnOnce || reduced.matches ? 0 : 0.25 + n * 0.09;
      svg.append(path(d, "cable cable--glow", color, delay), path(d, "cable", color, delay), path(d, "pulse", null, delay + 1.2 + (n % 5) * 0.4));
      if (!live.has(dstId)) live.set(dstId, delay + 1.1);
      n++;
    });
  });

  for (const [id, at] of live) {
    const li = rack.querySelector(`[data-dst="${id}"]`);
    if (drawnOnce || reduced.matches) li.classList.add("is-live");
    else setTimeout(() => li.classList.add("is-live"), at * 1000);
  }
  if (drawnOnce) svg.querySelectorAll(".cable").forEach((c) => (c.style.animation = "none", c.style.strokeDashoffset = "0"));
  drawnOnce = true;
}

if (rack) {
  document.fonts.ready.then(draw);
  new ResizeObserver(() => drawnOnce && draw()).observe(rack);
}

// Install tabs: WAI-ARIA tabs pattern with arrow-key navigation.
const tabs = [...document.querySelectorAll('[role="tab"]')];
function select(tab) {
  for (const t of tabs) {
    const on = t === tab;
    t.setAttribute("aria-selected", String(on));
    t.tabIndex = on ? 0 : -1;
    document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
  }
  tab.focus();
}
tabs.forEach((tab, i) => {
  tab.addEventListener("click", () => select(tab));
  tab.addEventListener("keydown", (e) => {
    const move = { ArrowRight: 1, ArrowLeft: -1 }[e.key];
    if (move) select(tabs[(i + move + tabs.length) % tabs.length]);
    if (e.key === "Home") select(tabs[0]);
    if (e.key === "End") select(tabs[tabs.length - 1]);
  });
});

// Copy buttons.
for (const button of document.querySelectorAll("[data-copy]")) {
  button.addEventListener("click", async () => {
    const label = button.textContent;
    try {
      await navigator.clipboard.writeText(button.dataset.copy);
      button.textContent = "Copied";
    } catch {
      button.textContent = "Select";
    }
    setTimeout(() => { button.textContent = label; }, 1600);
  });
}
