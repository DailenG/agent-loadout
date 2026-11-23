#!/usr/bin/env node
// Builds the deployable site from site/ (no dependencies, Node 18+).
//
//   node site/tools/build.mjs --out <dir>   copy site/ to <dir>, minus design/,
//                                           tools/ and previews/, with the site URL
//                                           and the package channels filled in
//   node site/tools/build.mjs --sync        render the channel blocks into
//                                           site/index.html itself
//
// The site URL lives in one place: site/data/site.json ("url"). Text files may
// use %SITE_URL% (absolute, with a trailing slash) and %SITE_PATH% (the path
// part, for pages served at other depths such as 404.html).
// Package channels come from site/data/channels.json and are rendered between
// <!-- channels:OS --> and <!-- /channels --> markers.

import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, extname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SITE = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const EXCLUDE = new Set(["design", "tools", "previews"]);
const TEXT = new Set([".html", ".xml", ".txt", ".webmanifest"]);

const args = process.argv.slice(2);
const outIndex = args.indexOf("--out");
const out = outIndex >= 0 ? resolve(args[outIndex + 1] || "") : null;
const sync = args.includes("--sync");
if (!out && !sync) {
  console.error("usage: build.mjs --out <dir> | --sync");
  process.exit(2);
}

const config = JSON.parse(readFileSync(join(SITE, "data/site.json"), "utf8"));
const siteUrl = new URL(config.url);
if (!siteUrl.pathname.endsWith("/")) throw new Error("site.json url must end with /");
const channels = JSON.parse(readFileSync(join(SITE, "data/channels.json"), "utf8")).channels;

const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

function renderChannel(c, indent) {
  if (!["pending", "live"].includes(c.status)) throw new Error(`channel ${c.id}: status must be pending or live`);
  const live = c.status === "live";
  const label = live ? "Available" : "Pending";
  const note = live
    ? (c.url ? `<p class="note"><a href="${esc(c.url)}">${esc(c.name)} package page</a></p>` : "")
    : `<p class="note">${esc(c.pending)}</p>`;
  return [
    `<li class="chan" data-channel="${esc(c.id)}" data-status="${c.status}">`,
    `  <div class="chan__head"><b>${esc(c.name)}</b><span class="status status--${c.status}">${label}</span></div>`,
    `  <div class="term"><span class="prompt" aria-hidden="true">${esc(c.prompt)}</span><code>${esc(c.command)}</code><button class="copy" type="button" data-copy="${esc(c.command)}" hidden>Copy</button></div>`,
    note ? `  ${note}` : null,
    `</li>`,
  ].filter((line) => line !== null).map((line) => indent + line).join("\n");
}

function renderChannels(html) {
  return html.replace(/([ \t]*)<!-- channels:(\w+) -->[\s\S]*?<!-- \/channels -->/g, (_, indent, os) => {
    const rows = channels.filter((c) => c.os === os).map((c) => renderChannel(c, indent));
    return [`${indent}<!-- channels:${os} -->`, ...rows, `${indent}<!-- /channels -->`].join("\n");
  });
}

function fill(text, file) {
  const result = renderChannels(text)
    .replaceAll("%SITE_URL%", siteUrl.href)
    .replaceAll("%SITE_PATH%", siteUrl.pathname);
  const left = result.match(/%SITE_[A-Z]+%/);
  if (left) throw new Error(`${file}: unknown placeholder ${left[0]}`);
  return result;
}

if (sync) {
  const index = join(SITE, "index.html");
  const before = readFileSync(index, "utf8");
  const after = renderChannels(before);
  if (after !== before) writeFileSync(index, after);
  console.log(after === before ? "index.html: channels up to date" : "index.html: channels updated");
}

if (out) {
  if (out === SITE || out.startsWith(SITE + "\\") || out.startsWith(SITE + "/")) throw new Error("--out must be outside site/");
  if (existsSync(out)) rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  for (const name of readdirSync(SITE)) {
    if (EXCLUDE.has(name)) continue;
    cpSync(join(SITE, name), join(out, name), { recursive: true });
  }
  let files = 0;
  let bytes = 0;
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) { walk(path); continue; }
      if (TEXT.has(extname(name))) writeFileSync(path, fill(readFileSync(path, "utf8"), relative(out, path)));
      files++;
      bytes += statSync(path).size;
    }
  };
  walk(out);
  console.log(`built ${files} files, ${(bytes / 1024).toFixed(1)} KiB, for ${siteUrl.href} into ${out}`);
}
