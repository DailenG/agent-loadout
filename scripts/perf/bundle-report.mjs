// Bundle baseline (#10, docs/perf): builds the frontend with source maps
// into a temp folder and attributes each chunk's minified bytes to the
// packages and source folders they came from.
// Run: node scripts/perf/bundle-report.mjs   (PERF_OUT: output folder)

import { readdirSync, readFileSync, statSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gzipSync } from "node:zlib";
import { build } from "vite";

const out = process.env.PERF_OUT ?? join(tmpdir(), "loadout-perf");
const dist = join(out, "dist-map");
mkdirSync(out, { recursive: true });
await build({ logLevel: "warn", build: { outDir: dist, emptyOutDir: true, sourcemap: true } });

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/** Decodes a mappings string into per-line [column, sourceIndex] segments. */
function decode(mappings) {
  const lines = [];
  let source = 0;
  for (const line of mappings.split(";")) {
    const segments = [];
    let column = 0;
    for (const segment of line.split(",")) {
      if (!segment) continue;
      const values = [];
      let value = 0;
      let shift = 0;
      for (const char of segment) {
        const digit = B64.indexOf(char);
        value += (digit & 31) << shift;
        if (digit & 32) {
          shift += 5;
        } else {
          values.push(value & 1 ? -(value >>> 1) : value >>> 1);
          value = 0;
          shift = 0;
        }
      }
      column += values[0];
      if (values.length > 1) source += values[1];
      segments.push([column, values.length > 1 ? source : -1]);
    }
    lines.push(segments);
  }
  return lines;
}

/** `react-dom`, `@codemirror/view`, or `src/app/routes/mcp` for app code. */
function group(path) {
  const normalized = path.replace(/\\/g, "/");
  const nm = normalized.lastIndexOf("node_modules/");
  if (nm >= 0) {
    const parts = normalized.slice(nm + 13).split("/");
    return parts[0].startsWith("@") ? `${parts[0]}/${parts[1]}` : parts[0];
  }
  const src = normalized.indexOf("src/");
  if (src >= 0) {
    const parts = normalized.slice(src).split("/");
    return parts.slice(0, Math.min(parts.length - 1, 4)).join("/");
  }
  return normalized;
}

const assets = join(dist, "assets");
const report = [];
for (const file of readdirSync(assets).filter((f) => f.endsWith(".js") || f.endsWith(".css"))) {
  const code = readFileSync(join(assets, file), "utf8");
  const entry = { file, bytes: statSync(join(assets, file)).size, gzip: gzipSync(code).length, groups: [] };
  if (file.endsWith(".js")) {
    const map = JSON.parse(readFileSync(join(assets, `${file}.map`), "utf8"));
    const lines = code.split("\n");
    const decoded = decode(map.mappings);
    const bytes = new Map();
    decoded.forEach((segments, lineIndex) => {
      const length = lines[lineIndex]?.length ?? 0;
      segments.forEach(([column, source], i) => {
        const end = i + 1 < segments.length ? segments[i + 1][0] : length;
        const name = source >= 0 ? group(map.sources[source]) : "(unmapped)";
        bytes.set(name, (bytes.get(name) ?? 0) + (end - column));
      });
    });
    entry.groups = [...bytes.entries()].sort((a, b) => b[1] - a[1]);
  }
  report.push(entry);
}

writeFileSync(join(out, "bundle.json"), JSON.stringify(report, null, 2));
for (const entry of report) {
  console.log(`\n${entry.file}: ${(entry.bytes / 1024).toFixed(1)} KiB, gzip ${(entry.gzip / 1024).toFixed(1)} KiB`);
  for (const [name, bytes] of entry.groups.slice(0, 25)) {
    console.log(`  ${(bytes / 1024).toFixed(1).padStart(7)} KiB  ${name}`);
  }
}
