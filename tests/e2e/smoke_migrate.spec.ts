// End-to-end smoke test of the SkillDock migration wizard (SPEC 15.1, 19
// item 9, M20): the debug app through tauri-driver, in a throwaway profile
// whose home holds a copy of the synthetic fixture `fixtures/skilldock-home`.
// The test opens Settings, "Migrate from SkillDock...", reads the dry-run
// report, chooses "Set later" for every flagged value (the app's keyring is
// the OS keyring, which tests never touch), migrates, and checks one
// `migrate: skilldock` commit in the history and a byte-identical
// `~/.skilldock`.
//
// The app and the CLI only ever run with an environment built from an
// allowlist (`appEnv`), and `assertProfileInTemp` refuses to start anything
// otherwise. Run it in a disposable machine (Windows Sandbox) if WebView2
// cannot start that way.
//
// Needs: a debug build (`npx tauri build --debug`), the CLI
// (`cargo build -p agent-loadout`, or LOADOUT_E2E_CLI), `tauri-driver` on
// PATH or in TAURI_DRIVER, and on Windows `msedgedriver` matching the
// WebView2 runtime in LOADOUT_E2E_NATIVE_DRIVER or on PATH. Skips with a
// notice when one is missing.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";
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
const FIXTURE = join(REPO, "fixtures", "skilldock-home");

function driverRuns(): boolean {
  const probe = spawnSync(DRIVER, ["--help"], { stdio: "ignore" });
  return probe.error === undefined;
}

/** A hash of every path and file below `dir`. */
function treeHash(dir: string): string {
  const hash = createHash("sha256");
  const walk = (current: string) => {
    for (const name of readdirSync(current).sort()) {
      const path = join(current, name);
      hash.update(relative(dir, path));
      if (statSync(path).isDirectory()) {
        hash.update("dir");
        walk(path);
      } else {
        hash.update(readFileSync(path));
      }
    }
  };
  walk(dir);
  return hash.digest("hex");
}

const missing = !existsSync(APP)
  ? `no debug build at ${APP}`
  : !existsSync(CLI)
    ? `no CLI build at ${CLI}`
    : !driverRuns()
      ? `tauri-driver not found (${DRIVER})`
      : null;
if (missing) {
  console.warn(`smoke_migrate skipped: ${missing}`);
}

const tauriAppData =
  process.platform === "win32" && process.env.LOCALAPPDATA
    ? join(process.env.LOCALAPPDATA, "dev.dailen.loadout")
    : null;

describe.skipIf(missing !== null)("Migration smoke", () => {
  let root = "";
  let skilldock = "";
  let env: Record<string, string> = {};
  let driver: ChildProcess | undefined;
  let session: Session | undefined;
  let tauriAppDataExisted = true;
  let passed = false;

  beforeAll(async () => {
    root = mkdtempSync(join(tmpdir(), "loadout-e2e-"));
    const dirs = profileDirs(root);
    Object.values(dirs).forEach((dir) => mkdirSync(dir, { recursive: true }));
    cpSync(FIXTURE, dirs.home, { recursive: true });
    skilldock = join(dirs.home, ".skilldock");
    tauriAppDataExisted = tauriAppData === null || existsSync(tauriAppData);

    // Never `...process.env`: a client override variable of the developer's
    // shell would send writes to real files.
    env = appEnv(process.env, dirs);
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
  });

  afterAll(async () => {
    await session?.quit().catch(() => undefined);
    driver?.kill();
    await sleep(1_000);
    if (passed) {
      rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
    } else if (root) {
      console.warn(`smoke_migrate kept its temp root: ${root}`);
    }
    if (tauriAppData && !tauriAppDataExisted) {
      rmSync(tauriAppData, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
    }
  });

  it("migrates the fixture in one commit and leaves ~/.skilldock as it was", async () => {
    const app = session as Session;
    const before = treeHash(skilldock);

    await app.click("//nav//button[normalize-space(.)='Settings']");
    await app.click("//button[normalize-space(.)='Migrate from SkillDock...']");
    const dialog = "//div[@role='dialog' and .//h3[normalize-space(.)='Migrate from SkillDock']]";
    const report = await app.text(`${dialog}//ul[@aria-label='Migration report']`);
    expect(report).toContain("context7");
    await app.click(`${dialog}//button[normalize-space(.)='Next']`);

    // Every flagged value: "Set later" (the OS keyring is never used here).
    for (const label of await app.labels(`${dialog}//button[@role='combobox' and starts-with(@aria-label, 'Destination for')]`)) {
      await app.click(`${dialog}//button[@aria-label='${label}']`);
      await app.click("//*[@role='option' and contains(normalize-space(.), 'Set later')]");
    }
    await app.click(`${dialog}//button[normalize-space(.)='Next']`);
    await app.click(`${dialog}//button[normalize-space(.)='Migrate']`);
    await app.find(`${dialog}//*[contains(normalize-space(.), 'in one commit')]`);

    const history = spawnSync(CLI, ["history", "--json"], { env, encoding: "utf8" });
    const migrations = history.stdout.split("migrate: skilldock").length - 1;
    expect(migrations).toBe(1);
    expect(treeHash(skilldock)).toBe(before);
    passed = true;
  });
});
