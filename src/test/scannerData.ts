// Mock of the optional external scanner (M47 contract, M49 GUI): Snyk Agent
// Scan's status, install with progress and cancel, token reference, switch
// and the external scans. Synthetic values only; the token is a reference
// text, there is no token value anywhere here.

import type {
  ExternalNotScanned,
  ExternalScannerStatus,
  ExternalScanResult,
  ScanFinding,
  ScannerInstallProgress,
  ScannerTokenKind,
} from "@/ipc/bindings";
import type { CommandResult, Commands } from "@/ipc/transport";

const SCANNER = "snyk-agent-scan";
const VERSION = "0.7.1";
/** The Windows x64 asset of v0.7.1 (research-snyk.md). */
export const MOCK_SCANNER_SIZE = 25_480_568;

/** Which external scan answers which command. */
export type ScannerResults = {
  review: ExternalScanResult;
  update: ExternalScanResult;
  skill: ExternalScanResult;
};

export type ScannerMockState = {
  supported: boolean;
  installed: boolean;
  enabled: boolean;
  /** A reference text (`env:SNYK_TOKEN`, an alias, ...), never a value. */
  tokenRef: string | null;
  /** What `check_scanner_token` answers while a reference is set. */
  tokenCheck: ExternalNotScanned | null;
  results: ScannerResults;
  /** Milliseconds between download steps (0: all at once). */
  installStepMs: number;
  /** Milliseconds an external scan takes (0: answers at once). */
  scanMs: number;
  /** `install_scanner` fails with this (`scanner_verification_failed`, ...) instead of installing. */
  installError: { code: "scanner_download_failed" | "scanner_verification_failed" | "not_supported_on_os"; message: string } | null;
};

function result(fields: Partial<ExternalScanResult>): ExternalScanResult {
  return {
    scanner: SCANNER,
    version: VERSION,
    status: "scanned",
    reason: null,
    verdict: "clean",
    findings: [],
    detail: null,
    ...fields,
  };
}

function finding(fields: Partial<ScanFinding> & Pick<ScanFinding, "rule" | "severity" | "message">): ScanFinding {
  return { scanner: SCANNER, file: null, line: null, excerpt: null, ...fields };
}

export const EXTERNAL_CLEAN = result({});

export const EXTERNAL_WARNING = result({
  verdict: "warning",
  findings: [
    finding({
      rule: "unverifiable_dependencies",
      severity: "warning",
      file: "scripts/diff-stats.py",
      line: 3,
      message: "The script installs a package from an index that can't be verified at review time.",
      excerpt: "https://pypi.example.invalid/simple/diffstat-helper",
    }),
    finding({
      rule: "third_party_content_exposure",
      severity: "info",
      file: "SKILL.md",
      line: 9,
      message: "The skill tells the agent to read content from web pages it doesn't control.",
    }),
  ],
});

export const EXTERNAL_DANGEROUS = result({
  verdict: "dangerous",
  findings: [
    finding({
      rule: "suspicious_download_url",
      severity: "dangerous",
      file: "scripts/bootstrap.sh",
      line: 7,
      message: "The script downloads an executable from an untrusted host and runs it.",
      excerpt: "https://get.example.invalid/install.sh",
    }),
    finding({
      rule: "prompt_injection_skill_instructions",
      severity: "dangerous",
      file: "SKILL.md",
      line: 41,
      message: "Hidden instructions tell the agent to upload the user's SSH keys.",
    }),
    finding({
      rule: "insecure_credential_handling",
      severity: "warning",
      file: "SKILL.md",
      line: 18,
      message: "The skill asks the agent to print an API key into the release notes.",
    }),
  ],
});

export function externalNotScanned(reason: ExternalNotScanned, detail: string | null = null): ExternalScanResult {
  return result({ status: "not_scanned", reason, detail });
}

export const EXTERNAL_NOT_UNDERSTOOD = result({ status: "not_understood", detail: "scanner output not understood (version 0.7.1)" });

export const EXTERNAL_LIMIT_REACHED = externalNotScanned(
  "limit_reached",
  "Daily usage limit reached for the public version of Agent-Scan.",
);

export function defaultScannerState(): ScannerMockState {
  return {
    supported: true,
    installed: false,
    enabled: false,
    tokenRef: null,
    tokenCheck: null,
    results: { review: EXTERNAL_CLEAN, update: EXTERNAL_CLEAN, skill: EXTERNAL_CLEAN },
    installStepMs: 0,
    scanMs: 0,
    installError: null,
  };
}

/**
 * The scanner in the demo build (`npm run dev:mock`), picked with
 * `?scanner=`: `on` (the default: installed, a token reference, turned on;
 * the install review comes back dangerous, the update with warnings),
 * `fresh` (not installed; a slow download to watch and cancel), `ready`
 * (installed with a token, off), `limit` (every scan hits Snyk's daily
 * limit), `garbled` (output not understood), `slow` (scans run until
 * canceled) and `unsupported` (no build for this platform).
 */
export function demoScannerState(name: string | null): Partial<ScannerMockState> {
  const on: Partial<ScannerMockState> = {
    installed: true,
    enabled: true,
    tokenRef: "env:SNYK_TOKEN",
    results: { review: EXTERNAL_DANGEROUS, update: EXTERNAL_WARNING, skill: EXTERNAL_CLEAN },
    scanMs: 1_200,
  };
  switch (name) {
    case "fresh":
      return { installStepMs: 1_500 };
    case "ready":
      return { installed: true, tokenRef: "SNYK_TOKEN" };
    case "limit":
      return { ...on, results: { review: EXTERNAL_LIMIT_REACHED, update: EXTERNAL_LIMIT_REACHED, skill: EXTERNAL_LIMIT_REACHED } };
    case "garbled":
      return { ...on, results: { review: EXTERNAL_NOT_UNDERSTOOD, update: EXTERNAL_NOT_UNDERSTOOD, skill: EXTERNAL_NOT_UNDERSTOOD } };
    case "slow":
      return { ...on, scanMs: 10 * 60_000 };
    case "unsupported":
      return { supported: false };
    default:
      return on;
  }
}

function tokenKind(reference: string | null): ScannerTokenKind | null {
  if (!reference) {
    return null;
  }
  if (reference.startsWith("env:")) {
    return "env";
  }
  if (reference.startsWith("keyring://")) {
    return "keyring";
  }
  if (reference.startsWith("op://")) {
    return "one_password";
  }
  return "alias";
}

const ALIAS = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Whether `reference` is one the core accepts (`TokenRef::parse`); `op-env://` is refused. */
function validReference(reference: string): boolean {
  return (
    ALIAS.test(reference) ||
    /^env:[A-Za-z_][A-Za-z0-9_]*$/.test(reference) ||
    /^keyring:\/\/[^/]+\/.+$/.test(reference) ||
    /^op:\/\/[^/]+\/[^/]+\/.+$/.test(reference)
  );
}

const ok = <T,>(data: T): CommandResult<T> => ({ status: "ok", data });

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** The scanner commands over `state`; `emit` fires the install progress listeners. */
export function scannerCommands(
  state: ScannerMockState,
  emit: (progress: ScannerInstallProgress) => void,
): Pick<
  Commands,
  | "scannerStatus"
  | "installScanner"
  | "removeScanner"
  | "setScannerEnabled"
  | "setScannerToken"
  | "checkScannerToken"
  | "externalScanReview"
  | "externalScanUpdate"
  | "externalScanSkill"
  | "cancelExternalScan"
> {
  // Runs in flight by id; a cancel marks one (or, with null, every run and the install) canceled.
  const canceledRuns = new Set<string>();
  const activeRuns = new Set<string>();
  let installCanceled = false;
  let installing = false;

  const status = (): ExternalScannerStatus => ({
    scanner: SCANNER,
    version: VERSION,
    supported: state.supported,
    installed: state.installed,
    enabled: state.enabled,
    tokenRef: state.tokenRef,
    tokenKind: tokenKind(state.tokenRef),
    sizeBytes: state.supported ? MOCK_SCANNER_SIZE : null,
    timeoutSecs: 60,
  });

  async function run(answer: ExternalScanResult, runId: string): Promise<CommandResult<ExternalScanResult>> {
    if (!state.enabled) {
      return { status: "error", error: { code: "scanner_unavailable", message: "Snyk Agent Scan is off" } };
    }
    if (!state.installed) {
      return ok(externalNotScanned("not_installed"));
    }
    if (!state.tokenRef) {
      return ok(externalNotScanned("no_token"));
    }
    activeRuns.add(runId);
    try {
      if (state.scanMs > 0) {
        // Short steps so a cancel ends the run promptly, as the backend kills the process.
        for (let waited = 0; waited < state.scanMs && !canceledRuns.has(runId); waited += 50) {
          await sleep(50);
        }
      } else {
        await Promise.resolve();
      }
      return ok(canceledRuns.has(runId) ? externalNotScanned("canceled") : answer);
    } finally {
      activeRuns.delete(runId);
      canceledRuns.delete(runId);
    }
  }

  return {
    scannerStatus: async () => ok(status()),
    installScanner: async () => {
      if (!state.supported) {
        return { status: "error", error: { code: "not_supported_on_os", message: "no Snyk Agent Scan build for this platform" } };
      }
      if (installing) {
        return { status: "error", error: { code: "locked", message: "an install is already running" } };
      }
      installing = true;
      installCanceled = false;
      try {
        const total = MOCK_SCANNER_SIZE;
        for (let step = 0; step <= 4; step += 1) {
          if (installCanceled) {
            return { status: "error", error: { code: "canceled", message: "canceled" } };
          }
          emit({ received: Math.round((total * step) / 4), total });
          if (state.installStepMs > 0) {
            await sleep(state.installStepMs);
          }
        }
        if (installCanceled) {
          return { status: "error", error: { code: "canceled", message: "canceled" } };
        }
        if (state.installError) {
          return { status: "error", error: state.installError };
        }
        state.installed = true;
        return ok(status());
      } finally {
        installing = false;
      }
    },
    // Like the backend (M47 review fixes): Remove also turns the switch off; the token reference stays.
    removeScanner: async () => {
      state.installed = false;
      state.enabled = false;
      return ok(status());
    },
    setScannerEnabled: async (enabled) => {
      state.enabled = enabled;
      return ok(status());
    },
    setScannerToken: async (reference) => {
      if (reference !== null && !validReference(reference)) {
        return { status: "error", error: { code: "invalid_ref", message: "not a secret reference" } };
      }
      state.tokenRef = reference;
      return ok(status());
    },
    checkScannerToken: async () =>
      ok(
        state.tokenRef === null
          ? { ok: false, reason: "no_token" }
          : { ok: state.tokenCheck === null, reason: state.tokenCheck },
      ),
    externalScanReview: async (_token, runId) => run(state.results.review, runId),
    externalScanUpdate: async (_id, _latest, runId) => run(state.results.update, runId),
    externalScanSkill: async (_id, runId) => run(state.results.skill, runId),
    cancelExternalScan: async (runId) => {
      if (runId === null) {
        activeRuns.forEach((id) => canceledRuns.add(id));
        installCanceled = true;
      } else if (activeRuns.has(runId)) {
        canceledRuns.add(runId);
      }
      return ok(null);
    },
  };
}
