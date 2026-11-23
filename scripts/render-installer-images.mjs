// Renders the NSIS installer bitmaps (#1, M28) from `src-tauri/icons/icon.svg`
// on the brand background:
// - `src-tauri/windows/sidebar.bmp`, 164x314, the Welcome and Finish pages;
// - `src-tauri/windows/header.bmp`, 150x57, the header of the other pages
//   (the uninstaller reuses it).
// NSIS needs uncompressed 24-bit BMPs. A headless Chromium-family browser
// (Edge ships with Windows) draws each image from HTML; this script crops its
// PNG screenshot and writes the BMP, with no npm dependencies.
//
//   node scripts/render-installer-images.mjs          render, then check
//   node scripts/render-installer-images.mjs --check  check the committed files
//
// Set LOADOUT_BROWSER to a browser executable to skip the lookup. The
// wordmark uses the system UI font (Segoe UI on Windows), so render on
// Windows to match the installer.

import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { inflateSync } from "node:zlib";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = join(root, "src-tauri", "windows");

// The icon's own gradient stops; the background uses its dark end.
const BRAND_DARK = "#22184f";
const BRAND_DEEP = "#140e33";
const BRAND_VIOLET = "#5a3fd0";

const IMAGES = [
  {
    file: "sidebar.bmp",
    width: 164,
    height: 314,
    body: (icon) => `
      <div class="frame" style="
        background:
          radial-gradient(120px 120px at 82px 104px, ${BRAND_VIOLET}55, transparent 70%),
          linear-gradient(180deg, ${BRAND_DARK} 0%, ${BRAND_DEEP} 100%);
        flex-direction: column; justify-content: flex-start; padding-top: 58px; gap: 14px;">
        <img src="${icon}" width="92" height="92" alt="">
        <span style="font-size: 22px; font-weight: 600; letter-spacing: 0.01em;">Loadout</span>
      </div>`,
  },
  {
    file: "header.bmp",
    width: 150,
    height: 57,
    body: (icon) => `
      <div class="frame" style="
        background: linear-gradient(90deg, ${BRAND_DARK} 0%, ${BRAND_DEEP} 100%);
        justify-content: flex-start; padding-left: 10px; gap: 8px;">
        <img src="${icon}" width="40" height="40" alt="">
        <span style="font-size: 18px; font-weight: 600; letter-spacing: 0.01em;">Loadout</span>
      </div>`,
  },
];

// Chromium enforces a minimum window size, so the page renders in a larger
// viewport and the image is cropped from its top-left corner.
const VIEWPORT = { width: 800, height: 600 };

function findBrowser() {
  if (process.env.LOADOUT_BROWSER) {
    return process.env.LOADOUT_BROWSER;
  }
  const candidates =
    process.platform === "win32"
      ? [
          join(process.env["ProgramFiles(x86)"] ?? "", "Microsoft", "Edge", "Application", "msedge.exe"),
          join(process.env.ProgramFiles ?? "", "Microsoft", "Edge", "Application", "msedge.exe"),
          join(process.env.ProgramFiles ?? "", "Google", "Chrome", "Application", "chrome.exe"),
          join(process.env.LOCALAPPDATA ?? "", "Google", "Chrome", "Application", "chrome.exe"),
        ]
      : process.platform === "darwin"
        ? [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
          ]
        : ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/bin/microsoft-edge"];
  const found = candidates.find((path) => path && existsSync(path));
  if (!found) {
    throw new Error("render-installer-images: no Edge or Chrome found; set LOADOUT_BROWSER");
  }
  return found;
}

function page(image, icon) {
  return `<!doctype html>
<html><head><meta charset="utf-8"><style>
  html, body { margin: 0; padding: 0; background: ${BRAND_DEEP}; }
  .frame {
    box-sizing: border-box; display: flex; align-items: center;
    width: ${image.width}px; height: ${image.height}px; overflow: hidden;
    color: #ffffff; font-family: "Segoe UI", system-ui, sans-serif;
  }
  img { display: block; flex: 0 0 auto; }
</style></head><body>${image.body(icon)}</body></html>`;
}

function screenshot(browser, htmlPath, pngPath, profileDir) {
  execFileSync(
    browser,
    [
      "--headless=new",
      "--disable-gpu",
      // Grayscale antialiasing: subpixel (ClearType) text leaves color
      // fringes once NSIS stretches the image for high-DPI screens.
      "--disable-lcd-text",
      "--hide-scrollbars",
      "--no-first-run",
      "--no-default-browser-check",
      "--force-device-scale-factor=1",
      `--user-data-dir=${profileDir}`,
      `--window-size=${VIEWPORT.width},${VIEWPORT.height}`,
      `--screenshot=${pngPath}`,
      pathToFileURL(htmlPath).href,
    ],
    { stdio: "ignore", timeout: 60_000 },
  );
  if (!existsSync(pngPath)) {
    throw new Error(`render-installer-images: the browser wrote no screenshot for ${htmlPath}`);
  }
}

/** Decodes an 8-bit, non-interlaced RGB or RGBA PNG into RGB rows. */
function decodePng(buffer) {
  const signature = "89504e470d0a1a0a";
  if (buffer.subarray(0, 8).toString("hex") !== signature) {
    throw new Error("not a PNG");
  }
  let width = 0;
  let height = 0;
  let channels = 0;
  const idat = [];
  for (let offset = 8; offset < buffer.length; ) {
    const length = buffer.readUInt32BE(offset);
    const type = buffer.toString("latin1", offset + 4, offset + 8);
    const data = buffer.subarray(offset + 8, offset + 8 + length);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      const [depth, colorType, , , interlace] = data.subarray(8, 13);
      if (depth !== 8 || interlace !== 0 || (colorType !== 2 && colorType !== 6)) {
        throw new Error(`unsupported PNG (depth ${depth}, color type ${colorType}, interlace ${interlace})`);
      }
      channels = colorType === 6 ? 4 : 3;
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    offset += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const pixels = Buffer.alloc(height * stride);
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)];
    const src = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const row = pixels.subarray(y * stride, (y + 1) * stride);
    const prev = y > 0 ? pixels.subarray((y - 1) * stride, y * stride) : null;
    for (let x = 0; x < stride; x += 1) {
      const left = x >= channels ? row[x - channels] : 0;
      const up = prev ? prev[x] : 0;
      const upLeft = prev && x >= channels ? prev[x - channels] : 0;
      let value = src[x];
      if (filter === 1) value += left;
      else if (filter === 2) value += up;
      else if (filter === 3) value += (left + up) >> 1;
      else if (filter === 4) {
        const p = left + up - upLeft;
        const pa = Math.abs(p - left);
        const pb = Math.abs(p - up);
        const pc = Math.abs(p - upLeft);
        value += pa <= pb && pa <= pc ? left : pb <= pc ? up : upLeft;
      } else if (filter !== 0) {
        throw new Error(`bad PNG filter ${filter}`);
      }
      row[x] = value & 0xff;
    }
  }
  return { width, height, channels, pixels };
}

/** Crops the top-left `width` x `height` and encodes a bottom-up 24-bit BMP. */
function encodeBmp(png, width, height) {
  if (png.width < width || png.height < height) {
    throw new Error(`screenshot ${png.width}x${png.height} is smaller than ${width}x${height}`);
  }
  const rowSize = Math.ceil((width * 3) / 4) * 4;
  const dataSize = rowSize * height;
  const bmp = Buffer.alloc(54 + dataSize);
  bmp.write("BM", 0, "latin1");
  bmp.writeUInt32LE(54 + dataSize, 2);
  bmp.writeUInt32LE(54, 10);
  bmp.writeUInt32LE(40, 14);
  bmp.writeInt32LE(width, 18);
  bmp.writeInt32LE(height, 22);
  bmp.writeUInt16LE(1, 26);
  bmp.writeUInt16LE(24, 28);
  bmp.writeUInt32LE(0, 30);
  bmp.writeUInt32LE(dataSize, 34);
  bmp.writeInt32LE(2835, 38);
  bmp.writeInt32LE(2835, 42);
  for (let y = 0; y < height; y += 1) {
    const target = 54 + (height - 1 - y) * rowSize;
    for (let x = 0; x < width; x += 1) {
      const source = (y * png.width + x) * png.channels;
      bmp[target + x * 3] = png.pixels[source + 2];
      bmp[target + x * 3 + 1] = png.pixels[source + 1];
      bmp[target + x * 3 + 2] = png.pixels[source];
    }
  }
  return bmp;
}

/** Throws unless `path` is an uncompressed 24-bit BMP of the given size. */
function checkBmp(path, width, height) {
  const bmp = readFileSync(path);
  const problems = [];
  if (bmp.toString("latin1", 0, 2) !== "BM") problems.push("not a BMP");
  if (bmp.readInt32LE(18) !== width || Math.abs(bmp.readInt32LE(22)) !== height) {
    problems.push(`size ${bmp.readInt32LE(18)}x${Math.abs(bmp.readInt32LE(22))}, expected ${width}x${height}`);
  }
  if (bmp.readUInt16LE(28) !== 24) problems.push(`${bmp.readUInt16LE(28)}-bit, expected 24-bit`);
  if (bmp.readUInt32LE(30) !== 0) problems.push("compressed, expected BI_RGB");
  if (problems.length > 0) {
    throw new Error(`${path}: ${problems.join("; ")}`);
  }
}

function render() {
  const browser = findBrowser();
  const icon = `data:image/svg+xml;base64,${readFileSync(join(root, "src-tauri", "icons", "icon.svg")).toString("base64")}`;
  const work = mkdtempSync(join(tmpdir(), "loadout-installer-images-"));
  try {
    for (const image of IMAGES) {
      const htmlPath = join(work, `${image.file}.html`);
      const pngPath = join(work, `${image.file}.png`);
      writeFileSync(htmlPath, page(image, icon));
      screenshot(browser, htmlPath, pngPath, join(work, "profile"));
      const png = decodePng(readFileSync(pngPath));
      writeFileSync(join(outDir, image.file), encodeBmp(png, image.width, image.height));
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

if (!process.argv.includes("--check")) {
  render();
}
for (const image of IMAGES) {
  checkBmp(join(outDir, image.file), image.width, image.height);
  console.log(`${image.file}: ${image.width}x${image.height}, 24-bit`);
}
