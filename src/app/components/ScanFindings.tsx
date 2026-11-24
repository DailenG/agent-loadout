// The built-in scan's findings (M40, SPEC 17) as every review shows them:
// severity, rule, file:line, message and the masked excerpt. A `dangerous`
// verdict blocks: the review's primary action becomes "... anyway", enabled
// only once the user ticks that they read the findings (M42).

import { useId } from "react";
import { t, type MessageKey } from "@/i18n/en";
import type { ScanFinding, ScanResult, ScanSeverity, ScanVerdict } from "@/ipc/bindings";

const SEVERITY_LABELS: Record<ScanSeverity, MessageKey> = {
  dangerous: "scan.severity.dangerous",
  warning: "scan.severity.warning",
  info: "scan.severity.info",
};

const SEVERITY_TONES: Record<ScanSeverity, string> = {
  dangerous: "tone-danger",
  warning: "tone-warning",
  info: "tone-neutral",
};

const VERDICT_LABELS: Record<ScanVerdict, MessageKey> = {
  clean: "scan.verdict.clean",
  warning: "scan.verdict.warning",
  dangerous: "scan.verdict.dangerous",
};

const VERDICT_TONES: Record<ScanVerdict, string> = {
  clean: "tone-positive",
  warning: "tone-warning",
  dangerous: "tone-danger",
};

/** True when any of `scans` blocks without "... anyway". */
export function isDangerous(...scans: (ScanResult | null | undefined)[]): boolean {
  return scans.some((scan) => scan?.verdict === "dangerous");
}

/** The engine's note listing files read by path only (M40); it has no file of its own. */
const NOT_CONTENT_SCANNED = "not-content-scanned";

/** Where a finding is: `file:line`, the file, the command line, or nothing (a note about several files). */
export function findingLocation(finding: ScanFinding): string | null {
  if (!finding.file) {
    return finding.rule === NOT_CONTENT_SCANNED ? null : t("scan.location.command");
  }
  return finding.line === null ? finding.file : `${finding.file}:${finding.line}`;
}

export type ScanFindingsProps = {
  scan: ScanResult;
  /** Heading; "Built-in scan" by default. */
  title?: string;
};

export function ScanFindings({ scan, title = t("scan.title") }: ScanFindingsProps) {
  const headingId = useId();
  const count = scan.findings.length;

  return (
    <section className="scan-findings" aria-labelledby={headingId}>
      <div className="scan-findings__header">
        <h4 id={headingId}>{title}</h4>
        <span className={`status-badge ${VERDICT_TONES[scan.verdict]}`}>{t(VERDICT_LABELS[scan.verdict])}</span>
        <span className="scan-findings__count">
          {count === 1 ? t("scan.count.one") : t("scan.count.many", { count })}
        </span>
        {scan.customRules ? <span className="status-badge tone-info">{t("scan.customRules")}</span> : null}
      </div>
      {scan.warnings.map((warning, index) => (
        <p key={`${index}-${warning}`} className="dialog-warning">
          {t("scan.rulesWarning", { warning })}
        </p>
      ))}
      {count === 0 ? (
        <p className="scan-findings__empty">{t("scan.none")}</p>
      ) : (
        <ul className="scan-findings__list">
          {scan.findings.map((finding, index) => (
            <li key={`${finding.rule}-${finding.file ?? ""}-${finding.line ?? ""}-${index}`} className="scan-finding">
              <div className="scan-finding__head">
                <span className={`status-badge ${SEVERITY_TONES[finding.severity]}`}>
                  {t(SEVERITY_LABELS[finding.severity])}
                </span>
                <code className="scan-finding__rule">{finding.rule}</code>
                {findingLocation(finding) ? (
                  <span className="scan-finding__location">{findingLocation(finding)}</span>
                ) : null}
              </div>
              <p className="scan-finding__message">{finding.message}</p>
              {finding.excerpt ? <code className="scan-finding__excerpt">{finding.excerpt}</code> : null}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

export type DangerousGateProps = {
  /** "Install anyway", "Update anyway" or "Save anyway" follows; the box says what it lets through. */
  acknowledged: boolean;
  onChange: (acknowledged: boolean) => void;
  /** The sentence above the box. */
  description?: string;
};

/**
 * Shown when the verdict is dangerous: what that means and the box that
 * enables the "... anyway" action. Nothing is let through without it.
 */
export function DangerousGate({ acknowledged, onChange, description = t("scan.gate.description") }: DangerousGateProps) {
  return (
    <div className="scan-gate" role="note">
      <p>{description}</p>
      <label className="scan-gate__ack">
        <input type="checkbox" checked={acknowledged} onChange={(event) => onChange(event.target.checked)} />
        <span>{t("scan.gate.ack")}</span>
      </label>
    </div>
  );
}
