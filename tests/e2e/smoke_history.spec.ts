// End-to-end smoke test of the History page (SPEC 13.5, 19 item 9, M17):
// the debug app through tauri-driver, in a throwaway profile. The CLI seeds
// a server enabled for Claude Code; the test tags the current version as
// `pre-change` on the History page, changes the server with the CLI,
// restores the tag with "Restore this version", applies it from Pending
// changes and checks the temp `~/.claude.json`; then "Undo this apply" on
// the latest apply brings the changed server back.
//
// Written, not run: M17 shipped this spec without a run; the first run
// confirms the selectors.
//
// The app and the CLI only ever run with an environment built from an
// allowlist (`appEnv`): USERPROFILE, HOME, APPDATA, LOCALAPPDATA,
// XDG_CONFIG_HOME, XDG_DATA_HOME, LOADOUT_HOME and WEBVIEW2_USER_DATA_FOLDER
// inside a fresh temp folder, and no client override variable;
// `assertProfileInTemp` refuses to start anything otherwise. Run it in a
// disposable machine (Windows Sandbox) if WebView2 cannot start that way.
//
// Needs: a debug build (`npx tauri build --debug`), the CLI
// (`cargo build -p agent-loadout`, or LOADOUT_E2E_CLI), `tauri-driver` on
// PATH or in TAURI_DRIVER, and on Windows `msedgedriver` matching the
// WebView2 runtime in LOADOUT_E2E_NATIVE_DRIVER or on PATH. Skips with a
// notice when one is missing.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { appEnv, assertProfileInTemp, profileDirs } from "./profileGuard";
import { Session, waitFor } from "./webdriver";

const REPO = resolve(import.meta.dirname, "../..");
const EXE = process.platform === "win32" ? ".exe" : "";
const APP = process.env.LOADOUT_E2E_APP ?? join(REPO, "src-tauri", "target", "debug", `agent-loadout-app${EXE}`);
const CLI = process.env.LOADOUT_E2E_CLI ?? join(REPO, "target", "debug", `loadout${EXE}`);
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
  : !existsSync(CLI)
    ? `no CLI build at ${CLI}`
    : !driverRuns()
      ? `tauri-driver not found (${DRIVER})`
      : null;
if (missing) {
  console.warn(`smoke_history skipped: ${missing}`);
}

/** The app's own data folder Tauri creates under the (Known Folder) local app data. */
const tauriAppData =
  process.platform === "win32" && process.env.LOCALAPPDATA
    ? join(process.env.LOCALAPPDATA, "dev.dailen.loadout")
    : null;

describe.skipIf(missing !== null)("History smoke", () => {
  let root = "";
  let env: Record<string, string> = {};
  let claudeFile = "";
  let driver: ChildProcess | undefined;
  let session: Session | undefined;
  let tauriAppDataExisted = true;
  let passed = false;

  /** Runs the CLI in the throwaway profile; a failed command fails the test. */
  function cli(args: string[]) {
    const run = spawnSync(CLI, args, { env, encoding: "utf8" });
    if (run.status !== 0) {
      throw new Error(`loadout ${args.join(" ")} failed (${run.status}): ${run.stderr}`);
    }
  }

  beforeAll(async () => {
    root = mkdtempSync(join(tmpdir(), "loadout-e2e-"));
    const dirs = profileDirs(root);
    Object.values(dirs).forEach((dir) => mkdirSync(dir, { recursive: true }));
    // Claude Code counts as installed when its user file exists.
    claudeFile = join(dirs.home, ".claude.json");
    writeFileSync(claudeFile, "{}\n");
    tauriAppDataExisted = tauriAppData === null || existsSync(tauriAppData);

    // Never `...process.env`: a client override variable of the developer's
    // shell (CLAUDE_CONFIG_DIR, ...) would send writes to real files.
    env = appEnv(process.env, dirs);
    assertProfileInTemp(env);
    // Instant mode: the add applies to the temp ~/.claude.json at once.
    cli(["server", "add", "e2e-docs", "--client", "claude-code", "--", "e2e-docs-command"]);
    if (!readFileSync(claudeFile, "utf8").includes("e2e-docs-command")) {
      throw new Error("loadout server add did not write the temp ~/.claude.json");
    }

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
  });

  afterAll(async () => {
    await session?.quit().catch(() => undefined);
    driver?.kill();
    await sleep(1_000);
    if (passed) {
      rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
    } else if (root) {
      // Kept for inspection: the data directory holds the store and its backups.
      console.warn(`smoke_history kept its temp root: ${root}`);
    }
    if (tauriAppData && !tauriAppDataExisted) {
      rmSync(tauriAppData, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
    }
  });

  it("restores a tagged version, applies it, and undoes the apply", async () => {
    const app = session as Session;
    const list = "//ul[@aria-label='Commits']";
    const taggedRow = `${list}/li[.//span[normalize-space(.)='pre-change']]`;
    const claude = () => readFileSync(claudeFile, "utf8");

    // Tag the current version.
    await app.click("//nav//button[normalize-space(.)='History']");
    await app.find(`${list}/li`);
    await app.click("//button[normalize-space(.)='Tag current...']");
    const tagDialog = "//div[@role='dialog' and .//h3[normalize-space(.)='Tag the current version']]";
    await app.type(`${tagDialog}//label[.//span[normalize-space(.)='Name']]//input`, "pre-change");
    await app.click(`${tagDialog}//button[normalize-space(.)='Create tag']`);
    await app.find(taggedRow);

    // Change the server outside the app.
    cli(["server", "rm", "e2e-docs"]);
    cli(["server", "add", "e2e-docs", "--client", "claude-code", "--", "e2e-docs-changed"]);
    expect(claude()).toContain("e2e-docs-changed");
    expect(claude()).not.toContain("e2e-docs-command");

    // Leave and come back: the page refetches the commit list.
    await app.click("//nav//button[normalize-space(.)='Projects']");
    await app.click("//nav//button[normalize-space(.)='History']");
    await app.find(`${list}/li[1][not(.//span[normalize-space(.)='pre-change'])]`);

    // Restore this version: the preview, then the restore commit.
    await app.click(`${taggedRow}//button[contains(@class, 'history-row__main')]`);
    await app.click(`${taggedRow}//div[@role='toolbar']//button[normalize-space(.)='Restore this version']`);
    const restoreDialog = "//div[@role='dialog' and .//h3[starts-with(normalize-space(.), 'Restore version')]]";
    expect(await app.text(`${restoreDialog}//p[contains(@class, 'history-restore__counts')]`)).toMatch(/^Adds \d+/);
    await app.click(`${restoreDialog}//button[normalize-space(.)='Restore' and not(@disabled)]`);

    // The restore asks before applying: Pending changes opens.
    expect(claude()).toContain("e2e-docs-changed");
    await app.click("//button[normalize-space(.)='Apply all' and not(@disabled)]");
    await waitFor(
      () => claude().includes("e2e-docs-command") && !claude().includes("e2e-docs-changed"),
      "the restored server in the temp ~/.claude.json",
    );
    await app.click("//button[@aria-label='Close pending changes']");

    // Undo this apply on the newest apply commit: it never asks.
    const latestApply = `(${list}/li[.//span[normalize-space(.)='Applied']])[1]`;
    await app.click(`${latestApply}//button[contains(@class, 'history-row__main')]`);
    await app.click(`${latestApply}//div[@role='toolbar']//button[normalize-space(.)='Undo this apply']`);
    await waitFor(
      () => claude().includes("e2e-docs-changed") && !claude().includes("e2e-docs-command"),
      "the undone apply in the temp ~/.claude.json",
    );
    passed = true;
  });
});
