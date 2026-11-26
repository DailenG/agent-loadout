// The optional external scanner's part of a review (M50, M51): SkillSpector's
// result beside the built-in findings. It runs only when the user turned it
// on in Settings > Security and it is ready (uvx prepared, or a skillspector
// on PATH), on this computer, as soon as an install or update review opens
// (local folders too: nothing is uploaded) and on a library skill on request.
// One run per review, cancelable. On but not ready: "Not scanned (<reason>)"
// without running anything. A scan that didn't run never blocks; output the
// app can't read is "not understood" and a partial scan "not fully scanned",
// never clean. A dangerous verdict needs the same "... anyway" as the
// built-in scan.

import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type {
  ExternalNotScanned,
  ExternalScanner,
  ExternalScannerStatus,
  ExternalScanResult,
  ScannerAvailability,
} from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport, type CommandResult } from "@/ipc/transport";
import { SEVERITY_LABELS, SEVERITY_TONES, VERDICT_LABELS, VERDICT_TONES } from "./ScanFindings";

export const scannerStatusKey = ["scanner_status"] as const;

/** Settings > Security's scanner state: detection runs `--version` probes, never the network. */
export function useScannerStatus() {
  const { commands } = useTransport();
  return useQuery({
    queryKey: scannerStatusKey,
    queryFn: async () => unwrap(await commands.scannerStatus()),
  });
}

export const SCANNER_NAMES: Record<ExternalScanner, string> = {
  skillspector: "SkillSpector",
  "snyk-agent-scan": "Snyk Agent Scan",
};

/** True when the external scan ran and rated the content dangerous: it needs "... anyway". */
export function isExternalDangerous(result: ExternalScanResult | null | undefined): boolean {
  return result?.status === "scanned" && result.verdict === "dangerous";
}

/** The "... anyway" box's sentence, naming which scanner(s) rated the content dangerous (M49 review F11). */
export function gateDescription(builtin: boolean, external: boolean, kind: "install" | "update"): string {
  if (builtin && external) {
    return t(kind === "install" ? "scan.gate.bothDescription" : "scan.gate.bothUpdateDescription");
  }
  if (external) {
    return t(kind === "install" ? "scan.gate.externalDescription" : "scan.gate.externalUpdateDescription");
  }
  return t(kind === "install" ? "scan.gate.description" : "scan.gate.updateDescription");
}

const NOT_SCANNED_KEYS: Record<ExternalNotScanned, MessageKey> = {
  not_available: "external.reason.notAvailable",
  not_prepared: "external.reason.notPrepared",
  timeout: "external.reason.timeout",
  canceled: "external.reason.canceled",
  failed: "external.reason.failed",
  // Snyk Agent Scan only; never produced while it is disabled.
  not_installed: "external.reason.notInstalled",
  unsupported_platform: "external.reason.unsupportedPlatform",
  no_token: "external.reason.noToken",
  token_unavailable: "external.reason.tokenUnavailable",
  token_rejected: "external.reason.tokenRejected",
  limit_reached: "external.reason.limitReached",
  too_large: "external.reason.tooLarge",
  unreachable: "external.reason.unreachable",
  would_start_server: "external.reason.wouldStartServer",
};

/** "Not scanned (timed out)." for a reason, or the generic failure. */
function notScannedText(reason: ExternalNotScanned | null): string {
  return t("external.notScanned", { reason: t(NOT_SCANNED_KEYS[reason ?? "failed"]) });
}

/** Why a scanner that is on can't run: what `external_scan_*` would answer without running anything. */
const UNAVAILABLE_REASONS: Record<Exclude<ScannerAvailability, "ready">, ExternalNotScanned> = {
  needs_prepare: "not_prepared",
  not_available: "not_available",
};

export type ExternalRun = {
  running: boolean;
  result: ExternalScanResult | null;
  /** A command failure (not a not-scanned result), as a sentence. */
  error: string | null;
  run: () => void;
  cancel: () => void;
};

/**
 * Where the scanner stands for a review: still asking, unknown (the status
 * failed), off, on but not runnable here (`unavailable`: nothing runs), or on.
 */
export type ScannerPhase = "checking" | "unknown" | "off" | "unavailable" | "on";

export function scannerPhase(status: UseQueryResult<ExternalScannerStatus>): ScannerPhase {
  if (status.isPending) {
    return "checking";
  }
  if (status.isError) {
    return "unknown";
  }
  if (!status.data.enabled) {
    return "off";
  }
  return status.data.availability === "ready" ? "on" : "unavailable";
}

let runCounter = 0;

/** A fresh id for one run: `cancel_external_scan(runId)` cancels only that run (M47, M49 review F12). */
function nextRunId(): string {
  runCounter += 1;
  return `run-${Date.now().toString(36)}-${runCounter}`;
}

/**
 * One external scan: started once `autoStart` is true, never otherwise.
 * Leaving the review while it runs cancels that run only. `run` starts it
 * again (after a cancel, a timeout or a failure).
 */
export function useExternalScan(start: (runId: string) => Promise<CommandResult<ExternalScanResult>>, autoStart: boolean): ExternalRun {
  const { commands } = useTransport();
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<ExternalScanResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const started = useRef(false);
  const current = useRef<string | null>(null);
  const mounted = useRef(true);
  const startRef = useRef(start);
  startRef.current = start;

  const run = useCallback(() => {
    if (current.current !== null) {
      return;
    }
    const runId = nextRunId();
    started.current = true;
    current.current = runId;
    setRunning(true);
    // A new run's outcome replaces the last one: a failure must not leave an older result on screen.
    setResult(null);
    setError(null);
    void startRef
      .current(runId)
      .then((answer) => {
        if (mounted.current) {
          setResult(unwrap(answer));
        }
      })
      .catch((failure: unknown) => {
        if (mounted.current) {
          setError(failure instanceof IpcFailure ? errorMessage(failure.code) : t("app.error.unknown"));
        }
      })
      .finally(() => {
        current.current = null;
        if (mounted.current) {
          setRunning(false);
        }
      });
  }, []);

  const cancel = useCallback(() => {
    if (current.current !== null) {
      void commands.cancelExternalScan(current.current);
    }
  }, [commands]);

  useEffect(() => {
    if (autoStart && !started.current) {
      run();
    }
  }, [autoStart, run]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      // Deferred: a remount (React's strict mode) keeps the run; leaving the review cancels it.
      setTimeout(() => {
        if (!mounted.current && current.current !== null) {
          void commands.cancelExternalScan(current.current);
        }
      }, 0);
    };
  }, [commands]);

  return { running, result, error, run, cancel };
}

export type ReviewScan = {
  phase: ScannerPhase;
  scan: ExternalRun;
  /** The configured run timeout, for "up to N seconds". */
  timeoutSecs: number | null;
  /** The version that will run (ready only), named before a result arrives. */
  version: string | null;
  /** While `unavailable`: why nothing runs. */
  unavailableReason: ExternalNotScanned | null;
  /** Install / Update waits: the status is still loading or the scan runs. */
  busy: boolean;
  /** On and ready but no result to carry (the run failed): the backend refuses Install / Update until Scan again. */
  needsScan: boolean;
  /** The result on screen (none while a run is in flight). */
  shown: ExternalScanResult | null;
};

/**
 * The external scan of a review or a library skill: the scanner's phase,
 * then one run as soon as it is on and ready. While the status loads,
 * Install waits; while it is off or not ready, nothing is called.
 */
export function useReviewScan(start: (runId: string) => Promise<CommandResult<ExternalScanResult>>): ReviewScan {
  const status = useScannerStatus();
  const phase = scannerPhase(status);
  const scan = useExternalScan(start, phase === "on");
  const availability = status.data?.availability;
  return {
    phase,
    scan,
    timeoutSecs: status.data?.timeoutSecs ?? null,
    version: status.data?.version ?? null,
    unavailableReason: phase === "unavailable" && availability && availability !== "ready" ? UNAVAILABLE_REASONS[availability] : null,
    busy: phase === "checking" || scan.running,
    needsScan: phase === "on" && !scan.running && scan.result === null,
    shown: scan.running ? null : scan.result,
  };
}

/** The result of one external scan: verdict and findings, or why it didn't scan. */
export function ExternalScanFindings({ result }: { result: ExternalScanResult }) {
  const scanner = SCANNER_NAMES[result.scanner];
  if (result.status === "not_scanned") {
    return (
      <>
        <p className="external-scan__state">{notScannedText(result.reason)}</p>
        {result.detail ? <p className="external-scan__detail">{result.detail}</p> : null}
        <p className="external-scan__detail">{t("external.notScannedNote")}</p>
      </>
    );
  }
  if (result.status === "not_understood") {
    return (
      <>
        <p className="external-scan__state">{t("external.notUnderstood", { version: result.version })}</p>
        <p className="external-scan__detail">{t("external.notUnderstoodNote")}</p>
      </>
    );
  }
  return (
    <>
      {result.incomplete ? <p className="external-scan__state">{t("external.incompleteNote", { scanner })}</p> : null}
      {result.findings.length === 0 ? (
        result.incomplete ? null : <p className="scan-findings__empty">{t("external.none", { scanner })}</p>
      ) : (
        <ul className="scan-findings__list">
          {result.findings.map((finding, index) => (
            <li key={`${finding.rule}-${finding.file ?? ""}-${finding.line ?? ""}-${index}`} className="scan-finding">
              <div className="scan-finding__head">
                <span className={`status-badge ${SEVERITY_TONES[finding.severity]}`}>{t(SEVERITY_LABELS[finding.severity])}</span>
                <code className="scan-finding__rule">{finding.rule}</code>
                {finding.file ? (
                  <span className="scan-finding__location">
                    {finding.line === null ? finding.file : `${finding.file}:${finding.line}`}
                  </span>
                ) : null}
              </div>
              <p className="scan-finding__message">{finding.message}</p>
              {finding.excerpt ? <code className="scan-finding__excerpt">{finding.excerpt}</code> : null}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

/** The header's badges: the state, or the verdict; a partial scan never shows "Clean". */
function stateBadges(result: ExternalScanResult): { tone: string; label: string }[] {
  if (result.status === "not_scanned") {
    return [{ tone: "tone-neutral", label: t("external.badge.notScanned") }];
  }
  if (result.status === "not_understood") {
    return [{ tone: "tone-warning", label: t("external.badge.notUnderstood") }];
  }
  const verdict = { tone: VERDICT_TONES[result.verdict], label: t(VERDICT_LABELS[result.verdict]) };
  if (!result.incomplete) {
    return [verdict];
  }
  const incomplete = { tone: "tone-warning", label: t("external.badge.incomplete") };
  return result.verdict === "clean" ? [incomplete] : [verdict, incomplete];
}

/** One line for the live region: what the scan ended with. */
function resultSummary(result: ExternalScanResult): string {
  if (result.status === "not_scanned") {
    return notScannedText(result.reason);
  }
  if (result.status === "not_understood") {
    return t("external.notUnderstood", { version: result.version });
  }
  const count = result.findings.length;
  const parts = [
    result.incomplete && result.verdict === "clean" ? null : t(VERDICT_LABELS[result.verdict]),
    result.incomplete ? t("external.incomplete") : null,
    count === 1 ? t("scan.count.one") : t("scan.count.many", { count }),
  ];
  return t("external.live.result", { scanner: SCANNER_NAMES[result.scanner], summary: parts.filter(Boolean).join(", ") });
}

export type ExternalScanSectionProps = {
  review: ReviewScan;
  /**
   * What the review's main button now offers, announced with the result
   * ("Install anyway needs your confirmation."); none for a report.
   */
  actionNote?: string | null;
  /** The sentence under the heading while it runs or has a result. */
  note?: string;
};

/**
 * "External scan": the scanner's name, its run and its result. One status
 * line is always mounted, so screen readers hear the run start and end; one
 * action button switches between Cancel scan and Scan again, so focus stays
 * on it, and goes to the heading when it disappears.
 */
export function ExternalScanSection({ review, actionNote = null, note = t("external.localNote") }: ExternalScanSectionProps) {
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const actionFocused = useRef(false);
  const { phase, scan, timeoutSecs, unavailableReason } = review;
  const { result, running, error } = scan;
  const done = !running && (result !== null || error !== null);
  const unavailable = phase === "unavailable" ? notScannedText(unavailableReason) : null;
  const badges =
    !running && result
      ? stateBadges(result)
      : unavailable
        ? [{ tone: "tone-neutral", label: t("external.badge.notScanned") }]
        : [];
  const count = !running && result?.status === "scanned" ? result.findings.length : null;
  const action = running
    ? { label: t("external.cancel"), onClick: scan.cancel }
    : phase === "on" && done && (error !== null || result?.status !== "scanned")
      ? { label: t("external.runAgain"), onClick: scan.run }
      : null;
  const version = result?.version ?? review.version ?? "";

  const live =
    phase === "checking"
      ? t("external.checking")
      : phase === "unknown"
        ? t("external.unknown")
        : unavailable
          ? [unavailable, actionNote].filter(Boolean).join(" ")
          : running
            ? t("external.live.running")
            : error
              ? [error, actionNote].filter(Boolean).join(" ")
              : result
                ? [resultSummary(result), actionNote].filter(Boolean).join(" ")
                : "";

  // The action button went away with the result (a scanned verdict has no Scan again): keep focus in the section.
  const hasAction = action !== null;
  useEffect(() => {
    if (!hasAction && actionFocused.current) {
      actionFocused.current = false;
      headingRef.current?.focus();
    }
  }, [hasAction]);

  return (
    <section className="scan-findings external-scan" aria-labelledby={headingId}>
      <div className="scan-findings__header">
        <h4 id={headingId} ref={headingRef} tabIndex={-1}>
          {t("external.title")}
        </h4>
        <span className="external-scan__name">
          {t("external.scanner", { scanner: SCANNER_NAMES[result?.scanner ?? "skillspector"], version }).trim()}
        </span>
        {badges.map((badge) => (
          <span key={badge.label} className={`status-badge ${badge.tone}`}>
            {badge.label}
          </span>
        ))}
        {count !== null ? (
          <span className="scan-findings__count">
            {count === 1 ? t("scan.count.one") : t("scan.count.many", { count })}
          </span>
        ) : null}
      </div>
      <p className="sr-only" role="status">
        {live}
      </p>
      {phase === "checking" ? <p className="external-scan__detail">{t("external.checking")}</p> : null}
      {phase === "unknown" || unavailable ? (
        <>
          <p className="external-scan__state">{unavailable ?? t("external.unknown")}</p>
          <p className="external-scan__detail">{t("external.notScannedNote")}</p>
        </>
      ) : null}
      {phase === "on" && (running || done) ? <p className="external-scan__detail">{note}</p> : null}
      {running ? (
        <p className="external-scan__running">
          {timeoutSecs ? t("external.running", { seconds: timeoutSecs }) : t("external.live.running")}
        </p>
      ) : null}
      {!running && error ? <p className="dialog-error">{error}</p> : null}
      {!running && result ? <ExternalScanFindings result={result} /> : null}
      {action ? (
        <div className="external-scan__running">
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            onClick={action.onClick}
            onFocus={() => {
              actionFocused.current = true;
            }}
            onBlur={() => {
              actionFocused.current = false;
            }}
          >
            {action.label}
          </button>
        </div>
      ) : null}
    </section>
  );
}
