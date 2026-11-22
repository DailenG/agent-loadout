// End-to-end smoke test of the Skills page (SPEC 19 item 9, M19): the debug
// app through tauri-driver, in a throwaway profile. A synthetic local skill
// is added to the store with the CLI (the OS folder picker cannot be driven
// over WebDriver); the test then enables it for Claude Code from the
// skill's "Enable in Tools" pill, checks that the temp
// `home/.claude/skills/<id>` is a link (a junction on Windows) to the store
// copy, sees it as managed on the Claude Code tab, and disables it again.
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
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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

const SKILL_ID = "e2e-notes";
const SKILL_MD = [
  "---",
  `name: ${SKILL_ID}`,
  "description: Synthetic skill for the Skills smoke test.",
  "---",
  "",
  "# e2e notes",
  "",
].join("\n");

function driverRuns(): boolean {
  const probe = spawnSync(DRIVER, ["--help"], { stdio: "ignore" });
  return probe.error === undefined;
}

/** A link (junction or symbolic link), not a folder. */
function isLink(path: string): boolean {
  try {
    return lstatSync(path).isSymbolicLink();
  } catch {
    return false;
  }
}

const missing = !existsSync(APP)
  ? `no debug build at ${APP}`
  : !existsSync(CLI)
    ? `no CLI build at ${CLI}`
    : !driverRuns()
      ? `tauri-driver not found (${DRIVER})`
      : null;
if (missing) {
  console.warn(`smoke_skills skipped: ${missing}`);
}

/** The app's own data folder Tauri creates under the (Known Folder) local app data. */
const tauriAppData =
  process.platform === "win32" && process.env.LOCALAPPDATA
    ? join(process.env.LOCALAPPDATA, "dev.dailen.loadout")
    : null;

describe.skipIf(missing !== null)("Skills smoke", () => {
  let root = "";
  let home = "";
  let env: Record<string, string> = {};
  let driver: ChildProcess | undefined;
  let session: Session | undefined;
  let tauriAppDataExisted = true;
  let passed = false;

  beforeAll(async () => {
    root = mkdtempSync(join(tmpdir(), "loadout-e2e-"));
    const dirs = profileDirs(root);
    Object.values(dirs).forEach((dir) => mkdirSync(dir, { recursive: true }));
    home = dirs.home;
    // Claude Code counts as installed when its user file exists.
    writeFileSync(join(home, ".claude.json"), "{}\n");
    const source = join(root, "downloads", SKILL_ID);
    mkdirSync(source, { recursive: true });
    writeFileSync(join(source, "SKILL.md"), SKILL_MD);
    tauriAppDataExisted = tauriAppData === null || existsSync(tauriAppData);

    // Never `...process.env`: a client override variable of the developer's
    // shell (CLAUDE_CONFIG_DIR, ...) would send links to real folders.
    env = appEnv(process.env, dirs);
    assertProfileInTemp(env);
    const added = spawnSync(CLI, ["skills", "add", source], { env, encoding: "utf8" });
    if (added.status !== 0) {
      throw new Error(`loadout skills add failed: ${added.stderr}`);
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
      console.warn(`smoke_skills kept its temp root: ${root}`);
    }
    if (tauriAppData && !tauriAppDataExisted) {
      rmSync(tauriAppData, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
    }
  });

  it("enables a local skill for Claude Code and the link exists", async () => {
    const app = session as Session;
    const link = join(home, ".claude", "skills", SKILL_ID);
    const card = `//article[@aria-label='${SKILL_ID}']`;

    await app.click("//nav//button[normalize-space(.)='Skills']");
    await app.find(card);
    expect(isLink(link)).toBe(false);

    await app.click(`${card}//button[@aria-label='Expand ${SKILL_ID}']`);
    await app.click(`${card}//button[@aria-label='Enable Claude Code']`);
    await waitFor(() => isLink(link), "the Claude Code skill link");
    expect(readFileSync(join(link, "SKILL.md"), "utf8")).toContain("Synthetic skill");

    // One commit per change: the enable is in the store's history.
    const history = spawnSync(CLI, ["history", "--json"], { env, encoding: "utf8" });
    expect(history.stdout).toContain(`skill: enable ${SKILL_ID} for claude-code`);

    // The Claude Code folder lists it as managed.
    await app.click("//button[@role='tab' and starts-with(@aria-label, 'Claude Code')]");
    const managed = await app.text(card);
    expect(managed).toContain("Managed");
    await app.click("//button[@role='tab' and contains(normalize-space(.), 'Store')]");

    await app.click(`${card}//button[normalize-space(.)='Disable All']`);
    await waitFor(() => !isLink(link) && !existsSync(link), "the link removed");
    passed = true;
  });
});
