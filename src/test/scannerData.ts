// Mock of the optional external scanner (M50 contract, M51 GUI): SkillSpector's
// detection status, Prepare with uv's progress lines and cancel, the switch and
// the external scans. Hand-made demo data shaped like the M50 mapping of
// SkillSpector's JSON (rule ids such as E2 and SC4 are SkillSpector's; the
// files, lines and messages are invented). Nothing here runs or downloads.

import type {
  ExternalNotScanned,
  ExternalScannerStatus,
  ExternalScanResult,
  ScanFinding,
  ScannerAvailability,
  ScannerPrepareProgress,
  ScannerRuntime,
} from "@/ipc/bindings";
import type { CommandResult, Commands } from "@/ipc/transport";

/** The version Loadout pins and tests (`skillspector::VERSION`). */
export const TESTED_VERSION = "2.13.7";
const PYTHON = "3.13";

/** Which external scan answers which command. */
export type ScannerResults = {
  review: ExternalScanResult;
  update: ExternalScanResult;
  skill: ExternalScanResult;
};

export type ScannerMockState = {
  availability: ScannerAvailability;
  /** Ready: the form that runs; needs Prepare: `uvx`. */
  runtime: ScannerRuntime | null;
  /** Ready only: the version that runs (another than TESTED_VERSION shows the untested note). */
  version: string | null;
  enabled: boolean;
  timeoutSecs: number;
  /** A 0.7.0 Snyk Agent Scan install remains. */
  snykInstalled: boolean;
  results: ScannerResults;
  /** Milliseconds an external scan takes (0: answers at once). */
  scanMs: number;
  /** What uv prints while preparing, one event per line. */
  prepareLines: string[];
  /** Milliseconds between prepare lines (0: all at once). */
  prepareStepMs: number;
  /** `prepare_scanner` fails with this instead of getting ready. */
  prepareError: { code: "scanner_download_failed" | "scanner_unavailable"; message: string } | null;
};

function result(fields: Partial<ExternalScanResult>): ExternalScanResult {
  return {
    scanner: "skillspector",
    version: TESTED_VERSION,
    status: "scanned",
    reason: null,
    verdict: "clean",
    incomplete: false,
    findings: [],
    detail: null,
    ...fields,
  };
}

function finding(fields: Partial<ScanFinding> & Pick<ScanFinding, "rule" | "severity" | "message">): ScanFinding {
  return { scanner: "skillspector", file: null, line: null, excerpt: null, ...fields };
}

/** SkillSpector's own LOW finding when OSV.dev can't be reached, shown as is. */
const OSV_UNREACHABLE = finding({
  rule: "SC4",
  severity: "info",
  file: "requirements.txt",
  line: null,
  message: "OSV.dev unreachable, using static fallback. Results may be incomplete.",
});

export const EXTERNAL_CLEAN = result({});

export const EXTERNAL_WARNING = result({
  verdict: "warning",
  findings: [
    finding({
      rule: "E2",
      severity: "warning",
      file: "scripts/env_report.py",
      line: 14,
      message: "Reads every environment variable of the process, which can include credentials.",
      excerpt: "for key, val in os.environ.items():",
    }),
    OSV_UNREACHABLE,
  ],
});

export const EXTERNAL_DANGEROUS = result({
  verdict: "dangerous",
  findings: [
    finding({
      rule: "E2",
      severity: "dangerous",
      file: "scripts/sync.py",
      line: 22,
      message: "Collects environment variables and sends them to a remote host.",
      excerpt: "requests.post(URL, json=dict(os.environ))",
    }),
    finding({
      rule: "SC4",
      severity: "warning",
      file: "requirements.txt",
      line: 2,
      message: "requests 2.19.0 has known vulnerabilities on OSV.dev.",
      excerpt: "requests==2.19.0",
    }),
  ],
});

/** No CRITICAL issue, but SkillSpector's recommendation was DO_NOT_INSTALL (M50 mapping). */
export const EXTERNAL_DO_NOT_INSTALL = result({
  verdict: "dangerous",
  findings: [
    finding({
      rule: "DO_NOT_INSTALL",
      severity: "dangerous",
      message: "SkillSpector recommends not installing this skill (risk score 72 of 100)",
    }),
    finding({
      rule: "E2",
      severity: "warning",
      file: "scripts/env_report.py",
      line: 14,
      message: "Reads every environment variable of the process, which can include credentials.",
      excerpt: "for key, val in os.environ.items():",
    }),
  ],
});

/** Not fully scanned with nothing else found: the verdict is clean, the result must never look it. */
export const EXTERNAL_INCOMPLETE = result({
  incomplete: true,
  findings: [
    finding({ rule: "incomplete", severity: "info", message: "not fully scanned (partial, 62% coverage)" }),
  ],
});

/** Not fully scanned and dangerous: still gates. */
export const EXTERNAL_INCOMPLETE_DANGEROUS = result({
  verdict: "dangerous",
  incomplete: true,
  findings: [
    ...EXTERNAL_DANGEROUS.findings,
    finding({ rule: "incomplete", severity: "info", message: "not fully scanned (partial, 80% coverage)" }),
  ],
});

export function externalNotScanned(reason: ExternalNotScanned, detail: string | null = null): ExternalScanResult {
  return result({ status: "not_scanned", reason, detail });
}

export const EXTERNAL_FAILED = externalNotScanned("failed", "Error: could not read <skill>/SKILL.md");

export const EXTERNAL_NOT_UNDERSTOOD = result({
  status: "not_understood",
  detail: `scanner output not understood (version ${TESTED_VERSION})`,
});

/** Lines shaped like uv's output while it fetches SkillSpector (sizes invented). */
export const MOCK_PREPARE_LINES = [
  "Downloading cpython-3.13.9-windows-x86_64-none (download) (21.4MiB)",
  "Downloading botocore (13.1MiB)",
  "Downloading yara-python (1.4MiB)",
  "Downloading skillspector (2.0MiB)",
  "Installed 97 packages in 6.12s",
  `skillspector ${TESTED_VERSION}`,
];

export function defaultScannerState(): ScannerMockState {
  return {
    availability: "not_available",
    runtime: null,
    version: null,
    enabled: false,
    timeoutSecs: 120,
    snykInstalled: false,
    results: { review: EXTERNAL_CLEAN, update: EXTERNAL_CLEAN, skill: EXTERNAL_CLEAN },
    scanMs: 0,
    prepareLines: MOCK_PREPARE_LINES,
    prepareStepMs: 0,
    prepareError: null,
  };
}

/** Ready through uvx with the tested version. */
export const SCANNER_READY: Partial<ScannerMockState> = { availability: "ready", runtime: "uvx", version: TESTED_VERSION };
/** Ready and on. */
export const SCANNER_ON: Partial<ScannerMockState> = { ...SCANNER_READY, enabled: true };
/** uv on PATH, SkillSpector not downloaded yet. */
export const SCANNER_NEEDS_PREPARE: Partial<ScannerMockState> = { availability: "needs_prepare", runtime: "uvx" };

/**
 * The scanner in the demo build (`npm run dev:mock`), picked with
 * `?scanner=`: `on` (the default: ready through uvx and on; the install
 * review comes back dangerous, the update with warnings, a library skill
 * not fully scanned), `off` (ready, off), `prepare` (uv found, Prepare
 * prints uv's lines slowly so it can be watched and canceled), `none`
 * (neither uv nor SkillSpector: setup instructions; also the 0.7.0 Snyk
 * leftover line), `path` (an untested SkillSpector on PATH, on),
 * `incomplete` (every scan not fully scanned), `garbled` (output not
 * understood), `failed` (the scanner failed), `timeout` and `slow` (scans
 * run until canceled).
 */
export function demoScannerState(name: string | null): Partial<ScannerMockState> {
  const on: Partial<ScannerMockState> = {
    ...SCANNER_ON,
    results: { review: EXTERNAL_DANGEROUS, update: EXTERNAL_WARNING, skill: EXTERNAL_INCOMPLETE },
    scanMs: 1_200,
  };
  const every = (answer: ExternalScanResult): ScannerResults => ({ review: answer, update: answer, skill: answer });
  switch (name) {
    case "off":
      return { ...SCANNER_READY };
    case "prepare":
      return { ...SCANNER_NEEDS_PREPARE, prepareStepMs: 1_500 };
    case "none":
      return { snykInstalled: true };
    case "path":
      return { ...on, runtime: "path", version: "2.14.0" };
    case "incomplete":
      return { ...on, results: every(EXTERNAL_INCOMPLETE) };
    case "garbled":
      return { ...on, results: every(EXTERNAL_NOT_UNDERSTOOD) };
    case "failed":
      return { ...on, results: every(EXTERNAL_FAILED) };
    case "timeout":
      return { ...on, results: every(externalNotScanned("timeout")) };
    case "slow":
      return { ...on, scanMs: 10 * 60_000 };
    default:
      return on;
  }
}

const ok = <T,>(data: T): CommandResult<T> => ({ status: "ok", data });

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** The scanner commands over `state`; `emit` fires the prepare progress listeners. */
export function scannerCommands(
  state: ScannerMockState,
  emit: (progress: ScannerPrepareProgress) => void,
): Pick<
  Commands,
  "scannerStatus" | "prepareScanner" | "setScannerEnabled" | "externalScanReview" | "externalScanUpdate" | "externalScanSkill" | "cancelExternalScan"
> {
  // Runs in flight by id; a cancel marks one (or, with null, every run and the prepare) canceled.
  const canceledRuns = new Set<string>();
  const activeRuns = new Set<string>();
  let prepareCanceled = false;
  let preparing = false;
  // Like the backend: a null cancel while no prepare runs cancels the next one before it starts.
  let cancelPending = false;

  const status = (): ExternalScannerStatus => {
    const ready = state.availability === "ready";
    return {
      scanner: "skillspector",
      testedVersion: TESTED_VERSION,
      python: PYTHON,
      availability: state.availability,
      runtime: state.availability === "not_available" ? null : state.runtime,
      version: ready ? state.version : null,
      versionTested: !ready || state.version === TESTED_VERSION,
      enabled: state.enabled,
      timeoutSecs: state.timeoutSecs,
      snykInstalled: state.snykInstalled,
    };
  };

  async function run(answer: ExternalScanResult, runId: string): Promise<CommandResult<ExternalScanResult>> {
    if (!state.enabled) {
      return { status: "error", error: { code: "scanner_unavailable", message: "SkillSpector is off" } };
    }
    if (state.availability === "not_available") {
      return ok(externalNotScanned("not_available"));
    }
    if (state.availability === "needs_prepare") {
      return ok(externalNotScanned("not_prepared"));
    }
    activeRuns.add(runId);
    try {
      if (state.scanMs > 0) {
        // Short steps so a cancel ends the run promptly, as the backend kills the process tree.
        for (let waited = 0; waited < state.scanMs && !canceledRuns.has(runId); waited += 50) {
          await sleep(50);
        }
      } else {
        await Promise.resolve();
      }
      // Like the backend: turned off while the run went on, its report is dropped.
      if (!state.enabled) {
        return { status: "error", error: { code: "scanner_unavailable", message: "SkillSpector was turned off during the scan" } };
      }
      return ok(canceledRuns.has(runId) ? externalNotScanned("canceled") : { ...answer, version: state.version ?? answer.version });
    } finally {
      activeRuns.delete(runId);
      canceledRuns.delete(runId);
    }
  }

  return {
    scannerStatus: async () => ok(status()),
    prepareScanner: async () => {
      if (preparing) {
        return { status: "error", error: { code: "locked", message: "a prepare is already running" } };
      }
      if (cancelPending) {
        cancelPending = false;
        return { status: "error", error: { code: "canceled", message: "canceled" } };
      }
      if (state.availability === "not_available") {
        return { status: "error", error: { code: "scanner_unavailable", message: "uv is not on PATH" } };
      }
      preparing = true;
      prepareCanceled = false;
      try {
        for (const line of state.prepareLines) {
          if (state.prepareStepMs > 0) {
            for (let waited = 0; waited < state.prepareStepMs && !prepareCanceled; waited += 50) {
              await sleep(50);
            }
          }
          if (prepareCanceled) {
            return { status: "error", error: { code: "canceled", message: "canceled" } };
          }
          emit({ line });
        }
        if (state.prepareError) {
          return { status: "error", error: state.prepareError };
        }
        // Like the backend: Prepare makes the uvx form ready and never turns SkillSpector on.
        if (state.availability !== "ready") {
          state.availability = "ready";
          state.runtime = "uvx";
          state.version = TESTED_VERSION;
        }
        return ok(status());
      } finally {
        preparing = false;
      }
    },
    setScannerEnabled: async (enabled) => {
      state.enabled = enabled;
      if (!enabled) {
        activeRuns.forEach((id) => canceledRuns.add(id));
      }
      return ok(status());
    },
    externalScanReview: async (_token, runId) => run(state.results.review, runId),
    externalScanUpdate: async (_id, _latest, runId) => run(state.results.update, runId),
    externalScanSkill: async (_id, runId) => run(state.results.skill, runId),
    cancelExternalScan: async (runId) => {
      if (runId === null) {
        activeRuns.forEach((id) => canceledRuns.add(id));
        prepareCanceled = true;
        if (!preparing) {
          cancelPending = true;
        }
      } else if (activeRuns.has(runId)) {
        canceledRuns.add(runId);
      }
      return ok(null);
    },
  };
}
