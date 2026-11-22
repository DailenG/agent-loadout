// End-to-end smoke test (SPEC 19 item 9, M15): the debug app through
// tauri-driver, in a throwaway profile. Adds a server on the MCP page,
// enables it for Claude Code with its pill, and checks the temp
// `.claude.json`.
//
// The app only ever runs with an environment built from an allowlist
// (`appEnv`): USERPROFILE, HOME, APPDATA, LOCALAPPDATA, XDG_CONFIG_HOME,
// XDG_DATA_HOME, LOADOUT_HOME and WEBVIEW2_USER_DATA_FOLDER inside a fresh
// temp folder, and no client override variable (CLAUDE_CONFIG_DIR, ...);
// `assertProfileInTemp` refuses to start the driver otherwise. Run it in a
// disposable machine (Windows Sandbox) if WebView2 cannot start that way.
//
// Needs: a debug build (`npx tauri build --debug`), `tauri-driver` on PATH
// or in TAURI_DRIVER, and on Windows `msedgedriver` matching the WebView2
// runtime in LOADOUT_E2E_NATIVE_DRIVER or on PATH. Skips with a notice when
// the app or the driver is missing.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { appEnv, assertProfileInTemp, profileDirs } from "./profileGuard";
import { dismissBackupRemoteOffer, Session, waitFor } from "./webdriver";

const REPO = resolve(import.meta.dirname, "../..");
const APP =
  process.env.LOADOUT_E2E_APP ??
  join(REPO, "src-tauri", "target", "debug", `agent-loadout-app${process.platform === "win32" ? ".exe" : ""}`);
const DRIVER = process.env.TAURI_DRIVER ?? "tauri-driver";
const NATIVE_DRIVER = process.env.LOADOUT_E2E_NATIVE_DRIVER;
const PORT = 4444;
const SERVER = `http://127.0.0.1:${PORT}`;

function driverRuns(): boolean {
  const probe = spawnSync(DRIVER, ["--help"], { stdio: "ignore" });
  return probe.error === undefined;
}

const missing = !existsSync(APP)
  ? `no debug build at ${APP}`
  : !driverRuns()
    ? `tauri-driver not found (${DRIVER})`
    : null;
if (missing) {
  console.warn(`smoke_mcp skipped: ${missing}`);
}

/** The app's own data folder Tauri creates under the (Known Folder) local app data. */
const tauriAppData =
  process.platform === "win32" && process.env.LOCALAPPDATA
    ? join(process.env.LOCALAPPDATA, "dev.dailen.loadout")
    : null;

describe.skipIf(missing !== null)("MCP smoke", () => {
  let root = "";
  let driver: ChildProcess | undefined;
  let session: Session | undefined;
  let tauriAppDataExisted = true;
  let passed = false;

  beforeAll(async () => {
    root = mkdtempSync(join(tmpdir(), "loadout-e2e-"));
    const dirs = profileDirs(root);
    Object.values(dirs).forEach((dir) => mkdirSync(dir, { recursive: true }));
    // Claude Code counts as installed when its user file exists.
    writeFileSync(join(dirs.home, ".claude.json"), "{}\n");
    tauriAppDataExisted = tauriAppData === null || existsSync(tauriAppData);

    // Never `...process.env`: a client override variable of the developer's
    // shell (CLAUDE_CONFIG_DIR, ...) would send writes to real files.
    const env = appEnv(process.env, dirs);
    assertProfileInTemp(env);
    const args = NATIVE_DRIVER ? ["--native-driver", NATIVE_DRIVER] : [];
    driver = spawn(DRIVER, args, { stdio: "ignore", env });
    await waitFor(
      async () => {
        try {
          return (await fetch(`${SERVER}/status`)).ok;
        } catch {
          return false;
        }
      },
      "tauri-driver to listen",
    );
    session = await Session.start(SERVER, APP);
    await dismissBackupRemoteOffer(session);
  });

  afterAll(async () => {
    await session?.quit().catch(() => undefined);
    driver?.kill();
    await sleep(1_000);
    if (passed) {
      rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
    } else if (root) {
      // Kept for inspection: the data directory holds the pre-write backups.
      console.warn(`smoke_mcp kept its temp root: ${root}`);
    }
    if (tauriAppData && !tauriAppDataExisted) {
      rmSync(tauriAppData, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
    }
  });

  it("adds a server, enables it for Claude Code and writes the temp home", async () => {
    const app = session as Session;
    const claudeFile = join(root, "home", ".claude.json");

    await app.click("//nav//button[normalize-space(.)='MCP']");
    await app.click("//button[normalize-space(.)='Add']");
    await app.type("//label[span[normalize-space(.)='ID']]//input", "e2e-fake");
    await app.type("//label[span[normalize-space(.)='Command']]//input", "fake-mcp-command");
    // A new server has no client; refuse to save if any client is selected
    // ("All detected clients" or any client's own box), so nothing but the
    // pill below can enable one.
    const clientBoxes = "//div[@role='dialog']//label[contains(@class, 'mcp-form-app')]//input[@type='checkbox']";
    const boxes = await app.selectedAll(clientBoxes);
    if (boxes.length < 2) {
      throw new Error(`refusing to save: expected the client checkboxes, found ${boxes.length}`);
    }
    if (boxes.some(Boolean)) {
      throw new Error("refusing to save: a client is selected");
    }
    await app.click("//div[@role='dialog']//button[normalize-space(.)='Save']");

    await app.click("//*[@aria-label='Expand e2e-fake']");
    const pill = "//button[@aria-label='Claude Code: disabled']";
    await app.find(pill, 10_000).catch(async (error: unknown) => {
      const seen = await app.labels("//button[@data-state]");
      throw new Error(`${String(error)}; pills: ${seen.join(", ")}`);
    });
    await app.click(pill);
    await waitFor(
      () => readFileSync(claudeFile, "utf8").includes("e2e-fake"),
      "the server in ~/.claude.json",
    );
    const written = JSON.parse(readFileSync(claudeFile, "utf8")) as {
      mcpServers?: Record<string, { command?: string }>;
    };
    expect(written.mcpServers?.["e2e-fake"]?.command).toBe("fake-mcp-command");
    await app.find("//button[@aria-label='Claude Code: enabled']");
    passed = true;
  });
});
