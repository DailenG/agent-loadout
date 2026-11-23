// The NSIS installer bitmaps (#1, M28; rendered by
// scripts/render-installer-images.mjs): NSIS shows only uncompressed 24-bit
// BMPs, at the sizes Modern UI lays out.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const root = join(__dirname, "..");

function bmpHeader(file: string) {
  const bmp = readFileSync(join(root, "src-tauri", file));
  return {
    magic: bmp.toString("latin1", 0, 2),
    width: bmp.readInt32LE(18),
    height: Math.abs(bmp.readInt32LE(22)),
    bitsPerPixel: bmp.readUInt16LE(28),
    compression: bmp.readUInt32LE(30),
  };
}

describe("installer bitmaps", () => {
  const nsis = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.windows.conf.json"), "utf8")).bundle.windows.nsis;

  it.each([
    ["sidebarImage", 164, 314],
    ["headerImage", 150, 57],
  ])("%s is a %ix%i 24-bit BMP", (key, width, height) => {
    expect(bmpHeader(nsis[key])).toEqual({ magic: "BM", width, height, bitsPerPixel: 24, compression: 0 });
  });
});
