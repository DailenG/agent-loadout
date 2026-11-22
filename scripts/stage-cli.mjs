// Builds the `loadout` CLI and stages it where `bundle.externalBin` in
// `src-tauri/tauri.release.conf.json` expects it, so the installers ship it
// next to the app (SPEC 16.1: NSIS puts `loadout.exe` on the user PATH; the
// Linux packages install `/usr/bin/loadout`). Run by that config's
// `beforeBuildCommand`; Tauri sets TAURI_ENV_DEBUG and
// TAURI_ENV_TARGET_TRIPLE for it.

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const debug = process.env.TAURI_ENV_DEBUG === "true";

const hostTriple = /^host: (.+)$/m.exec(execFileSync("rustc", ["-vV"], { encoding: "utf8" }))?.[1];
if (!hostTriple) {
  throw new Error("stage-cli: cannot read the host triple from `rustc -vV`");
}
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || hostTriple;
const cross = triple !== hostTriple;

const args = ["build", "--locked", "-p", "agent-loadout", "--bin", "loadout"];
if (!debug) {
  args.push("--release");
}
if (cross) {
  args.push("--target", triple);
}
execFileSync("cargo", args, { cwd: root, stdio: "inherit" });

const exe = triple.includes("windows") ? ".exe" : "";
const built = join(root, "target", ...(cross ? [triple] : []), debug ? "debug" : "release", `loadout${exe}`);
const staged = join(root, "src-tauri", "binaries", `loadout-${triple}${exe}`);
mkdirSync(dirname(staged), { recursive: true });
copyFileSync(built, staged);
console.log(`stage-cli: ${built} -> ${staged}`);
