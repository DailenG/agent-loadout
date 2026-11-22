// End-to-end smoke test of the Projects page (SPEC 19 item 9, M16): the
// debug app through tauri-driver, in a throwaway profile. A temp project
// folder is bound with the CLI (the OS folder picker cannot be driven over
// WebDriver); the test then reviews and trusts its manifest on the Projects
// page, checks the temp `<P>/.mcp.json`, opens the effective config matrix
// and a cell's provenance, edits the written entry by hand and resolves the
// drift with Overwrite from Pending changes.
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
import { dismissBackupRemoteOffer, Session, waitFor } from "./webdriver";

const REPO = resolve(import.meta.dirname, "../..");
const EXE = process.platform === "win32" ? ".exe" : "";
const APP = process.env.LOADOUT_E2E_APP ?? join(REPO, "src-tauri", "target", "debug", `agent-loadout-app${EXE}`);
const CLI = process.env.LOADOUT_E2E_CLI ?? join(REPO, "target", "debug", `loadout${EXE}`);
const DRIVER = process.env.TAURI_DRIVER ?? "tauri-driver";
const NATIVE_DRIVER = process.env.LOADOUT_E2E_NATIVE_DRIVER;
const PORT = 4444;
const SERVER = `http://127.0.0.1:${PORT}`;

const MANIFEST = [
  "schema = 1",
  'id = "e2e-app-a1b2"',
  'name = "e2e-app"',
  "",
  "[defaults]",
  'clients = ["claude-code"]',
  "",
  "[servers.e2e-docs]",
  'transport = "stdio"',
  'command = "e2e-docs-command"',
  "",
].join("\n");

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
  console.warn(`smoke_project skipped: ${missing}`);
}

/** The app's own data folder Tauri creates under the (Known Folder) local app data. */
const tauriAppData =
  process.platform === "win32" && process.env.LOCALAPPDATA
    ? join(process.env.LOCALAPPDATA, "dev.dailen.loadout")
    : null;

describe.skipIf(missing !== null)("Projects smoke", () => {
  let root = "";
  let project = "";
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
    project = join(root, "projects", "e2e-app");
    mkdirSync(join(project, ".loadout"), { recursive: true });
    writeFileSync(join(project, ".loadout", "project.toml"), MANIFEST);
    tauriAppDataExisted = tauriAppData === null || existsSync(tauriAppData);

    // Never `...process.env`: a client override variable of the developer's
    // shell (CLAUDE_CONFIG_DIR, ...) would send writes to real files.
    const env = appEnv(process.env, dirs);
    assertProfileInTemp(env);
    // Bound, not trusted: nothing is applied until the review.
    const added = spawnSync(CLI, ["project", "add", project], { env, encoding: "utf8" });
    if (added.status !== 0) {
      throw new Error(`loadout project add failed: ${added.stderr}`);
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
      console.warn(`smoke_project kept its temp root: ${root}`);
    }
    if (tauriAppData && !tauriAppDataExisted) {
      rmSync(tauriAppData, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
    }
  });

  it("trusts a reviewed manifest, shows provenance and resolves drift", async () => {
    const app = session as Session;
    const mcpFile = join(project, ".mcp.json");

    await app.click("//nav//button[normalize-space(.)='Projects']");
    await app.find("//article[@aria-label='e2e-app']");
    expect(existsSync(mcpFile)).toBe(false);

    // The review shows the manifest before anything is trusted.
    await app.click("//article[@aria-label='e2e-app']//button[normalize-space(.)='Review change & trust']");
    const review = "//div[@role='dialog']";
    const content = await app.text(`${review}//pre`);
    expect(content).toContain("e2e-docs-command");
    expect(existsSync(mcpFile)).toBe(false);
    await app.click(`${review}//button[normalize-space(.)='Trust and apply']`);
    await waitFor(() => existsSync(mcpFile) && readFileSync(mcpFile, "utf8").includes("e2e-docs"), "<P>/.mcp.json");

    // Matrix: the project entry for Claude Code, from <P>/.mcp.json.
    await app.click("//article[@aria-label='e2e-app']//button[normalize-space(.)='Effective config']");
    await app.click("//button[starts-with(@aria-label, 'e2e-docs in Claude Code')]");
    const detail = await app.text("//aside[contains(@class, 'matrix-detail')]");
    expect(detail).toContain("<P>/.mcp.json");
    expect(detail).toContain("project:e2e-docs");
    await app.click("//button[normalize-space(.)='Back to projects']");

    // Hand edit: drifted; Overwrite from the resolver restores it.
    const edited = readFileSync(mcpFile, "utf8").replace("e2e-docs-command", "e2e-docs-edited");
    writeFileSync(mcpFile, edited);
    await app.click("//button[contains(@aria-label, 'pending changes')]");
    await app.click("//button[contains(normalize-space(.), '.mcp.json')]");
    await app.click("//button[@aria-label='Resolve e2e-docs']");
    const resolver = "//div[@role='dialog' and .//h3[normalize-space(.)='Resolve e2e-docs']]";
    expect(await app.text(resolver)).toContain("e2e-docs-edited");
    await app.click(`${resolver}//button[normalize-space(.)='Overwrite']`);
    await waitFor(
      () => !readFileSync(mcpFile, "utf8").includes("e2e-docs-edited"),
      "the entry written back into <P>/.mcp.json",
    );
    expect(readFileSync(mcpFile, "utf8")).toContain("e2e-docs-command");
    passed = true;
  });
});
