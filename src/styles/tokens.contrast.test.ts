// WCAG 2.x contrast of the theme tokens (M28c, #12): text pairs need 4.5:1,
// control boundaries and focus rings 3:1, in light and dark. The values come
// from `tokens.css` itself, so a token change that breaks a pair fails here.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// Read from disk (Vitest stubs CSS imports, `?raw` included); Vitest runs from the project root.
const css = readFileSync(resolve(process.cwd(), "src/styles/tokens.css"), "utf8");

type Rgba = [number, number, number, number];
type Theme = "light" | "dark";

/** The custom properties of the first rule whose selector is exactly `selector`. */
function tokens(selector: string): Map<string, string> {
  const start = css.indexOf(`\n${selector} {`);
  if (start < 0) {
    throw new Error(`no ${selector} rule in tokens.css`);
  }
  const body = css.slice(css.indexOf("{", start) + 1, css.indexOf("}", start));
  const map = new Map<string, string>();
  for (const match of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
    map.set(match[1], match[2].trim());
  }
  return map;
}

const LIGHT = tokens(":root");
const DARK = new Map([...LIGHT, ...tokens(':root[data-theme="dark"]')]);

function parse(value: string): Rgba {
  const hex = value.match(/^#([0-9a-f]{6})$/i);
  if (hex) {
    const n = Number.parseInt(hex[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1];
  }
  const rgba = value.match(/^rgba?\(([^)]+)\)$/);
  if (rgba) {
    const [r, g, b, a = 1] = rgba[1].split(",").map((part) => Number(part.trim()));
    return [r, g, b, a];
  }
  throw new Error(`unsupported color ${value}`);
}

function color(theme: Theme, value: string): Rgba {
  const map = theme === "light" ? LIGHT : DARK;
  const resolved = value.startsWith("--") ? map.get(value) : value;
  if (!resolved) {
    throw new Error(`no ${value} in the ${theme} tokens`);
  }
  return resolved.startsWith("var(") ? color(theme, resolved.slice(4, -1)) : parse(resolved);
}

/** `top` composited over an opaque `bottom`. */
function over(top: Rgba, bottom: Rgba): Rgba {
  const a = top[3];
  return [0, 1, 2].map((i) => top[i] * a + bottom[i] * (1 - a)).concat(1) as Rgba;
}

function luminance([r, g, b]: Rgba): number {
  const channel = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function ratio(a: Rgba, b: Rgba): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** A background token, optionally a translucent fill laid over `--surface`. */
function background(theme: Theme, token: string): Rgba {
  const surface = color(theme, "--surface");
  return over(color(theme, token), surface);
}

const TEXT_PAIRS: Array<[string, string]> = [
  ["--muted", "--surface"],
  ["--muted", "--app-bg"],
  ["--muted", "--surface-muted"],
  ["--heading", "--surface"],
  ["--accent-text", "--surface"],
  ["--accent-text", "--accent-soft"],
  ["--accent-text", "--button-primary-soft"],
  ["--success-text", "--surface"],
  ["--success-text", "--success-soft"],
  ["--warning-text", "--surface"],
  ["--warning-text", "--warning-soft"],
  ["--danger-text", "--surface"],
  ["--danger", "--surface"],
];

const UI_PAIRS: Array<[string, string]> = [
  ["--line-control", "--surface"],
  ["--line-control", "--app-bg"],
  ["--focus-ring", "--surface"],
  ["--focus-ring", "--app-bg"],
  ["--accent-text", "--surface"],
];

describe.each<Theme>(["light", "dark"])("%s theme tokens", (theme) => {
  it.each(TEXT_PAIRS)("%s text on %s is at least 4.5:1", (text, bg) => {
    const base = background(theme, bg);
    expect(ratio(over(color(theme, text), base), base)).toBeGreaterThanOrEqual(4.5);
  });

  it.each(UI_PAIRS)("%s next to %s is at least 3:1", (ui, bg) => {
    const base = background(theme, bg);
    expect(ratio(over(color(theme, ui), base), base)).toBeGreaterThanOrEqual(3);
  });
});

describe("primary buttons", () => {
  it("keep white text at 4.5:1 across the light gradient and its hover", () => {
    const white: Rgba = [255, 255, 255, 1];
    for (const stop of ["--button-primary-top", "--button-primary", "--button-primary-hover-top", "--button-primary-hover"]) {
      expect(ratio(white, color("light", stop)), stop).toBeGreaterThanOrEqual(4.5);
    }
  });
});

describe("tooltips", () => {
  // `[data-tooltip]::before` and `.app-tooltip` draw this text on an opaque `--tooltip-surface`, with no blur.
  it.each<[Theme, string]>([
    ["light", "#374151"],
    ["dark", "#ededed"],
  ])("%s tooltip text %s is at least 4.5:1 on an opaque surface", (theme, text) => {
    const surface = color(theme, "--tooltip-surface");
    expect(surface[3]).toBe(1);
    expect(ratio(color(theme, text), surface)).toBeGreaterThanOrEqual(4.5);
  });
});
