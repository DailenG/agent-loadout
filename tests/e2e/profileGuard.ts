// Keeps the e2e app away from the real profile (M15 incident). The app's
// environment is built from an allowlist, never by copying the parent's,
// and checked before the driver starts: every profile variable resolves
// under the OS temp folder, and no client override variable is passed.

import { realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative } from "node:path";

/** Variables that move a client's config folder (the core's adapter table). */
export const ADAPTER_OVERRIDE_VARS = [
  "CLAUDE_CONFIG_DIR",
  "CODEX_HOME",
  "COPILOT_HOME",
  "PI_CONFIG_DIR",
  "PI_CODING_AGENT_DIR",
  "GOOSE_PATH_ROOT",
] as const;

/** Profile variables the app reads; each must be under the temp folder. */
export const PROFILE_VARS = [
  "USERPROFILE",
  "HOME",
  "APPDATA",
  "LOCALAPPDATA",
  "LOADOUT_HOME",
  "XDG_CONFIG_HOME",
  "XDG_DATA_HOME",
  "WEBVIEW2_USER_DATA_FOLDER",
] as const;

/**
 * Taken from the parent: what starting the driver and the app needs, no
 * profile folder. `DISPLAY` lets WebKitWebDriver reach X on Linux CI.
 */
const INHERITED_VARS = ["PATH", "SystemRoot", "TEMP", "TMP", "ComSpec", "PATHEXT", "DISPLAY"] as const;

export type ProfileDirs = {
  home: string;
  appdata: string;
  localappdata: string;
  xdgConfig: string;
  xdgData: string;
  loadout: string;
  webview: string;
};

/** The throwaway profile folders under `root`. */
export function profileDirs(root: string): ProfileDirs {
  return {
    home: join(root, "home"),
    appdata: join(root, "appdata"),
    localappdata: join(root, "localappdata"),
    xdgConfig: join(root, "xdg", "config"),
    xdgData: join(root, "xdg", "data"),
    loadout: join(root, "loadout"),
    webview: join(root, "webview"),
  };
}

/** The app's whole environment: the allowlist from `parent`, plus `dirs`. */
export function appEnv(parent: NodeJS.ProcessEnv, dirs: ProfileDirs): Record<string, string> {
  const env: Record<string, string> = {};
  for (const name of INHERITED_VARS) {
    const value = parent[name];
    if (value !== undefined) {
      env[name] = value;
    }
  }
  return {
    ...env,
    HOME: dirs.home,
    USERPROFILE: dirs.home,
    APPDATA: dirs.appdata,
    LOCALAPPDATA: dirs.localappdata,
    XDG_CONFIG_HOME: dirs.xdgConfig,
    XDG_DATA_HOME: dirs.xdgData,
    LOADOUT_HOME: dirs.loadout,
    WEBVIEW2_USER_DATA_FOLDER: dirs.webview,
  };
}

/**
 * Throws unless `env` is safe to start the app with: no client override
 * variable at all, no `XDG_*` variable other than the checked ones, and
 * every profile variable set to an existing folder under `temp`. Names
 * compare case-insensitively (Windows).
 */
export function assertProfileInTemp(env: Record<string, string | undefined>, temp = realpathSync(tmpdir())) {
  const checked: readonly string[] = PROFILE_VARS;
  const overrides: readonly string[] = ADAPTER_OVERRIDE_VARS;
  for (const [name, value] of Object.entries(env)) {
    const upper = name.toUpperCase();
    if (value !== undefined && overrides.includes(upper)) {
      throw new Error(`refusing to start: ${upper} is set; the app would write that client's real files`);
    }
    if (value !== undefined && upper.startsWith("XDG_") && !checked.includes(upper)) {
      throw new Error(`refusing to start: ${upper} is set and not checked`);
    }
  }
  for (const name of PROFILE_VARS) {
    const value = Object.entries(env).find(([key]) => key.toUpperCase() === name)?.[1];
    const rel = value !== undefined && isAbsolute(value) ? relative(temp, realpathSync(value)) : "..";
    const inside = rel !== "" && !rel.startsWith("..") && !isAbsolute(rel);
    if (!inside) {
      throw new Error(`refusing to start: ${name} (${value ?? "unset"}) is not under ${temp}`);
    }
  }
}
