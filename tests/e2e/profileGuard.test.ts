// The e2e profile guard, without WebView2 (part of `npm test`).

import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { ADAPTER_OVERRIDE_VARS, appEnv, assertProfileInTemp, profileDirs } from "./profileGuard";

const root = mkdtempSync(join(tmpdir(), "loadout-guard-"));
const dirs = profileDirs(root);
Object.values(dirs).forEach((dir) => mkdirSync(dir, { recursive: true }));

afterAll(() => rmSync(root, { recursive: true, force: true }));

describe("e2e profile guard", () => {
  it("builds the app environment from the allowlist only", () => {
    const parent = {
      PATH: "/usr/bin",
      TEMP: tmpdir(),
      CLAUDE_CONFIG_DIR: join(homedir(), ".claude"),
      CODEX_HOME: join(homedir(), ".codex"),
      XDG_STATE_HOME: join(homedir(), ".local", "state"),
      SOME_TOOL_SETTING: "1",
      HOME: homedir(),
    };
    const env = appEnv(parent, dirs);
    expect(env.PATH).toBe("/usr/bin");
    expect(env.HOME).toBe(dirs.home);
    expect(env.LOADOUT_HOME).toBe(dirs.loadout);
    expect(env.XDG_CONFIG_HOME).toBe(dirs.xdgConfig);
    for (const name of [...ADAPTER_OVERRIDE_VARS, "XDG_STATE_HOME", "SOME_TOOL_SETTING"]) {
      expect(env).not.toHaveProperty(name);
    }
    expect(() => assertProfileInTemp(env)).not.toThrow();
  });

  it("refuses any client override variable, even one under the temp folder", () => {
    for (const name of ADAPTER_OVERRIDE_VARS) {
      const env = { ...appEnv({}, dirs), [name]: dirs.home };
      expect(() => assertProfileInTemp(env)).toThrow(name);
    }
    // Names compare case-insensitively, as Windows does.
    expect(() => assertProfileInTemp({ ...appEnv({}, dirs), claude_config_dir: dirs.home })).toThrow(
      "CLAUDE_CONFIG_DIR",
    );
  });

  it("refuses a profile variable outside the temp folder, or missing", () => {
    for (const name of ["HOME", "USERPROFILE", "LOADOUT_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME"]) {
      expect(() => assertProfileInTemp({ ...appEnv({}, dirs), [name]: homedir() })).toThrow(name);
      const missing: Record<string, string | undefined> = { ...appEnv({}, dirs) };
      delete missing[name];
      expect(() => assertProfileInTemp(missing)).toThrow(name);
    }
  });

  it("refuses an XDG variable it does not check", () => {
    expect(() => assertProfileInTemp({ ...appEnv({}, dirs), XDG_STATE_HOME: dirs.home })).toThrow("XDG_STATE_HOME");
  });
});
