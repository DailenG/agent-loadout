#!/usr/bin/env node
// WCAG 2.2 contrast check for every text color pair the site uses, in both
// themes. Reads the tokens from site/css/site.css (the dark :root block and
// the light one inside prefers-color-scheme: light) and exits 1 on a failure.
//
//   node site/tools/contrast.mjs
//
// When CSS starts using a new foreground/background combination, add it to
// PAIRS below.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../css/site.css"), "utf8");

function tokens(block) {
  const map = {};
  for (const m of block.matchAll(/--([\w-]+):\s*(#[0-9a-fA-F]{3,8})\s*;/g)) map[m[1]] = m[2];
  return map;
}
const darkBlock = css.match(/:root\s*\{([\s\S]*?)\n\}/)[1];
const lightBlock = css.match(/@media \(prefers-color-scheme: light\)\s*\{\s*:root\s*\{([\s\S]*?)\}\s*\}/)[1];
const dark = tokens(darkBlock);
const light = { ...dark, ...tokens(lightBlock) };

// [foreground, background, where it appears, minimum ratio]
// 4.5 for text, 3 for large text (24px+, or 18.66px+ bold) and UI parts.
const PAIRS = [
  ["text", "bg", "body text, headings", 4.5],
  ["muted", "bg", "ledes, nav links, captions, readouts", 4.5],
  ["accent-text", "bg", "hero emphasis, fine labels on page", 4.5],
  ["text", "panel", "card text, nav hover", 4.5],
  ["muted", "panel", "card body text", 4.5],
  ["accent-text", "panel", "step numbers, file formats, link arrows", 4.5],
  ["text", "panel-2", "module rows, client chips, channel names", 4.5],
  ["muted", "panel-2", "module labels, tab labels", 4.5],
  ["accent-text", "panel-2", "vault aliases", 4.5],
  ["pending-text", "panel-2", "Pending label", 4.5],
  ["ok-text", "panel-2", "Available label", 4.5],
  ["muted", "bg", "secrets box text (on bg)", 4.5],
  ["on-accent", "accent", "primary buttons, selected tab, Recommended chip", 4.5],
  ["on-accent", "accent-hover", "primary button hover", 4.5],
  ["skip-text", "skip-bg", "skip link", 4.5],
  ["scr-text", "scr-bg", "code screens, terminals", 4.5],
  ["scr-muted", "scr-bg", "code comments, screenshot captions", 4.5],
  ["scr-str", "scr-bg", "code strings", 4.5],
  ["scr-key", "scr-bg", "code keys, prompts", 4.5],
  ["scr-add", "scr-bg", "code additions", 4.5],
  ["scr-mod", "scr-bg", "code changes", 4.5],
  ["scr-hl-text", "scr-hl-bg", "highlighted secret references", 4.5],
  ["scr-text", "scr-btn", "copy buttons", 4.5],
  ["focus", "bg", "focus ring on page (non-text)", 3],
  ["focus", "panel", "focus ring on cards (non-text)", 3],
  ["focus", "panel-2", "focus ring on tabs (non-text)", 3],
];

function luminance(hex) {
  let h = hex.slice(1);
  if (h.length === 3) h = [...h].map((c) => c + c).join("");
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16) / 255)
    .map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function ratio(a, b) {
  const [l1, l2] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (l1 + 0.05) / (l2 + 0.05);
}

let failed = 0;
for (const [name, theme] of [["dark", dark], ["light", light]]) {
  console.log(`\n${name} theme`);
  let lowest = Infinity;
  for (const [fg, bg, where, min] of PAIRS) {
    if (!theme[fg] || !theme[bg]) {
      console.log(`  MISSING  --${fg} on --${bg}`);
      failed++;
      continue;
    }
    const r = ratio(theme[fg], theme[bg]);
    if (min >= 3) lowest = Math.min(lowest, r);
    const ok = r >= min;
    if (!ok) failed++;
    console.log(`  ${ok ? "ok  " : "FAIL"} ${r.toFixed(2).padStart(5)}:1 (min ${min})  --${fg} ${theme[fg]} on --${bg} ${theme[bg]}  ${where}`);
  }
  console.log(`  lowest checked ratio: ${lowest.toFixed(2)}:1`);
}
console.log(failed ? `\n${failed} pair(s) below the minimum` : "\nall pairs pass");
process.exit(failed ? 1 : 0);
