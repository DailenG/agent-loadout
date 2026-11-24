// The optional external scanner's part of a review (M47, M49): Snyk Agent
// Scan's result beside the built-in findings. It runs only when the user
// turned it on in Settings > Security and the scanner is installed: on its
// own for content fetched from a source (Git, a catalog, an update), on
// request for a folder or archive from this computer. One run per review,
// cancelable. A scan that didn't run says "Not scanned (<reason>)" and never
// blocks; output the app can't read is "not understood", never clean. A
// dangerous verdict needs the same "... anyway" as the built-in scan.

import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type { ExternalNotScanned, ExternalScannerStatus, ExternalScanResult, ScanFinding } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport, type CommandResult } from "@/ipc/transport";
import { SEVERITY_LABELS, SEVERITY_TONES, VERDICT_LABELS, VERDICT_TONES } from "./ScanFindings";

export const scannerStatusKey = ["scanner_status"] as const;

/**
 * Whether this build offers the external scanner (mirrors the core's
 * `scan::external::AVAILABLE`). `false` since 0.7.1: Snyk's free tier
 * refuses new accounts, so the Snyk Agent Scan integration is hidden and
 * disabled; nothing here calls the scanner commands. Flip to re-enable.
 */
export const EXTERNAL_SCANNER_AVAILABLE = false;

/** Settings > Security's scanner state (no network, no secret resolution); never asked while unavailable. */
export function useScannerStatus() {
  const { commands } = useTransport();
  return useQuery({
    queryKey: scannerStatusKey,
    queryFn: async () => unwrap(await commands.scannerStatus()),
    enabled: EXTERNAL_SCANNER_AVAILABLE,
  });
}

/** True when the external scan ran and rated the content dangerous: it needs "... anyway". */
export function isExternalDangerous(result: ExternalScanResult | null | undefined): boolean {
  return result?.status === "scanned" && result.verdict === "dangerous";
}

/** The "... anyway" box's sentence, naming which scanner(s) rated the content dangerous (review F11). */
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
  not_installed: "external.reason.notInstalled",
  unsupported_platform: "external.reason.unsupportedPlatform",
  no_token: "external.reason.noToken",
  token_unavailable: "external.reason.tokenUnavailable",
  token_rejected: "external.reason.tokenRejected",
  limit_reached: "external.reason.limitReached",
  too_large: "external.reason.tooLarge",
  unreachable: "external.reason.unreachable",
  timeout: "external.reason.timeout",
  canceled: "external.reason.canceled",
  would_start_server: "external.reason.wouldStartServer",
  failed: "external.reason.failed",
};

/** "not scanned (Snyk limit reached)"'s reason in words. */
export function notScannedReason(reason: ExternalNotScanned): string {
  return t(NOT_SCANNED_KEYS[reason]);
}

/** Snyk's risk names (research-snyk.md section 6) in words; an unknown name shows as sent. */
const RISK_KEYS: Record<string, MessageKey> = {
  prompt_injection_skill_instructions: "external.risk.promptInjection",
  suspicious_download_url: "external.risk.suspiciousDownload",
  malicious_code: "external.risk.maliciousCode",
  insecure_credential_handling: "external.risk.credentials",
  secret_detection: "external.risk.secret",
  direct_money_access: "external.risk.money",
  third_party_content_exposure: "external.risk.thirdPartyContent",
  unverifiable_dependencies: "external.risk.unverifiableDependencies",
  modifying_system_services: "external.risk.systemServices",
  missing_skill_md: "external.risk.missingSkillMd",
};

/** What the excerpt of a finding is: Snyk sends URLs, not file text (M47 mapping). */
function excerptLabel(finding: ScanFinding): string | null {
  if (finding.rule === "suspicious_download_url") {
    return t("external.excerpt.malicious");
  }
  if (finding.rule === "unverifiable_dependencies") {
    return t("external.excerpt.unverifiable");
  }
  return null;
}

export type ExternalRun = {
  running: boolean;
  result: ExternalScanResult | null;
  /** A command failure (not a not-scanned result), as a sentence. */
  error: string | null;
  run: () => void;
  cancel: () => void;
};

/** Where the scanner stands for a review: still asking, unknown (the status failed), off, or on. */
export type ScannerPhase = "checking" | "unknown" | "off" | "on";

/** "On" means switched on and installed: after Remove nothing can run, so nothing is offered. */
export function scannerPhase(status: UseQueryResult<ExternalScannerStatus>): ScannerPhase {
  if (!EXTERNAL_SCANNER_AVAILABLE) {
    return "off";
  }
  if (status.isPending) {
    return "checking";
  }
  if (status.isError) {
    return "unknown";
  }
  return status.data.enabled && status.data.installed ? "on" : "off";
}

let runCounter = 0;

/** A fresh id for one run: `cancel_external_scan(runId)` cancels only that run (M47, review F12). */
function nextRunId(): string {
  runCounter += 1;
  return `run-${Date.now().toString(36)}-${runCounter}`;
}

/**
 * One external scan for a review: started once `autoStart` is true, never
 * otherwise (`run` starts it by hand). Leaving the review while it runs
 * cancels that run only. `run` starts it again (after a cancel, a timeout
 * or a limit).
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
  /** True for a folder or archive from this computer: it runs only on request. */
  manual: boolean;
  /** Install / Update waits: the status is still loading or the scan runs. */
  busy: boolean;
  /** The result on screen (none while a run is in flight). */
  shown: ExternalScanResult | null;
};

/**
 * The external scan of a review: the scanner's phase, then one run started
 * on its own (`auto`) or by hand. While the status loads, Install waits.
 */
export function useReviewScan(start: (runId: string) => Promise<CommandResult<ExternalScanResult>>, auto: boolean): ReviewScan {
  const status = useScannerStatus();
  const phase = scannerPhase(status);
  const scan = useExternalScan(start, auto && phase === "on");
  return {
    phase,
    scan,
    timeoutSecs: status.data?.timeoutSecs ?? null,
    manual: !auto,
    busy: phase === "checking" || scan.running,
    shown: scan.running ? null : scan.result,
  };
}

/** The result of one external scan: verdict and findings, or why it didn't scan. */
export function ExternalScanFindings({ result }: { result: ExternalScanResult }) {
  if (result.status === "not_scanned") {
    return (
      <>
        <p className="external-scan__state">
          {t("external.notScanned", { reason: result.reason ? notScannedReason(result.reason) : t("external.reason.failed") })}
        </p>
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
  if (result.findings.length === 0) {
    return <p className="scan-findings__empty">{t("external.none")}</p>;
  }
  return (
    <ul className="scan-findings__list">
      {result.findings.map((finding, index) => {
        const risk = RISK_KEYS[finding.rule];
        const label = excerptLabel(finding);
        return (
          <li key={`${finding.rule}-${finding.file ?? ""}-${finding.line ?? ""}-${index}`} className="scan-finding">
            <div className="scan-finding__head">
              <span className={`status-badge ${SEVERITY_TONES[finding.severity]}`}>{t(SEVERITY_LABELS[finding.severity])}</span>
              <span className="scan-finding__rule">{risk ? t(risk) : finding.rule}</span>
              {finding.file ? (
                <span className="scan-finding__location">
                  {finding.line === null ? finding.file : `${finding.file}:${finding.line}`}
                </span>
              ) : null}
            </div>
            <p className="scan-finding__message">{finding.message}</p>
            {finding.excerpt ? (
              <div className="external-scan__url">
                {label ? <span>{label}</span> : null}
                <code className="scan-finding__excerpt">{finding.excerpt}</code>
              </div>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

function stateBadge(result: ExternalScanResult): { tone: string; label: string } {
  if (result.status === "not_scanned") {
    return { tone: "tone-neutral", label: t("external.badge.notScanned") };
  }
  if (result.status === "not_understood") {
    return { tone: "tone-warning", label: t("external.badge.notUnderstood") };
  }
  return { tone: VERDICT_TONES[result.verdict], label: t(VERDICT_LABELS[result.verdict]) };
}

/** Not-scanned reasons that come after the request reached Snyk: the content was sent. */
const SENT_REASONS: Partial<Record<ExternalNotScanned, true>> = {
  limit_reached: true,
  too_large: true,
  token_rejected: true,
  timeout: true,
};

/** True when this result means the content left this computer. */
function contentWasSent(result: ExternalScanResult): boolean {
  return result.status !== "not_scanned" || (result.reason !== null && SENT_REASONS[result.reason] === true);
}

/** One line for the live region: what the scan ended with. */
function resultSummary(result: ExternalScanResult): string {
  if (result.status === "not_scanned") {
    return t("external.notScanned", { reason: result.reason ? notScannedReason(result.reason) : t("external.reason.failed") });
  }
  if (result.status === "not_understood") {
    return t("external.notUnderstood", { version: result.version });
  }
  const count = result.findings.length;
  return t("external.live.result", {
    verdict: t(VERDICT_LABELS[result.verdict]),
    count: count === 1 ? t("scan.count.one") : t("scan.count.many", { count }),
  });
}

export type ExternalScanSectionProps = {
  review: ReviewScan;
  /**
   * What the review's main button now offers, announced with the result
   * ("Install anyway needs your confirmation."); none for a report.
   */
  actionNote?: string | null;
  /** The sentence under the heading once content was sent (or is being sent). */
  sentNote?: string;
};

/**
 * "External scan": the scanner's name, its run and its result. One status
 * line is always mounted, so screen readers hear the run start and end; one
 * action button switches between Scan with Snyk, Cancel scan and Scan
 * again, so focus stays on it, and goes to the heading when it disappears.
 */
export function ExternalScanSection({ review, actionNote = null, sentNote = t("external.sentNote") }: ExternalScanSectionProps) {
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const actionFocused = useRef(false);
  const { phase, scan, manual, timeoutSecs } = review;
  const { result, running, error } = scan;
  const done = !running && (result !== null || error !== null);
  const badge = !running && result ? stateBadge(result) : null;
  const count = !running && result?.status === "scanned" ? result.findings.length : null;
  const action = running
    ? { label: t("external.cancel"), onClick: scan.cancel }
    : phase === "on" && !done && manual
      ? { label: t("external.skill.action"), onClick: scan.run }
      : phase === "on" && done && (error !== null || result?.status !== "scanned")
        ? { label: t("external.runAgain"), onClick: scan.run }
        : null;
  const showSent = running || (!running && result !== null && contentWasSent(result));

  const live =
    phase === "checking"
      ? t("external.checking")
      : phase === "unknown"
        ? t("external.unknown")
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
        <span className="external-scan__name">{t("external.scanner", { version: result?.version ?? "" }).trim()}</span>
        {badge ? <span className={`status-badge ${badge.tone}`}>{badge.label}</span> : null}
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
      {phase === "unknown" ? (
        <>
          <p className="external-scan__state">{t("external.unknown")}</p>
          <p className="external-scan__detail">{t("external.notScannedNote")}</p>
        </>
      ) : null}
      {phase === "on" && manual && !running && !done ? (
        <p className="external-scan__detail">{t("external.manualNote")}</p>
      ) : null}
      {showSent ? <p className="external-scan__detail">{sentNote}</p> : null}
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
