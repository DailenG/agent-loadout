// The install review (M40, M42, M49): what a git repository or a picked folder
// would install, before anything reaches the store. It shows the source
// and its trust state, the files, the built-in scan's findings and, when
// the user turned it on, Snyk Agent Scan's. Install asks the trust question
// when the source needs it (M39), and a dangerous verdict of either scan is
// installed only through "Install anyway" (M40, M47).

import { useEffect, useState, type ReactNode } from "react";
import { ExternalScanSection, gateDescription, isExternalDangerous, useReviewScan } from "@/app/components/ExternalScan";
import { DangerousGate, isDangerous, ScanFindings } from "@/app/components/ScanFindings";
import { TrustSummary } from "@/app/components/TrustDialog";
import { SettingsDialog } from "@/app/routes/settings/SettingsDialog";
import { errorMessage, t } from "@/i18n/en";
import type { SkillInstallReview } from "@/ipc/bindings";
import { useTransport } from "@/ipc/transport";
import { formatSize } from "../skills/labels";

export type SkillReviewDialogProps = {
  review: SkillInstallReview;
  /** Where the review came from, above the source facts (a catalog entry, M45). */
  origin?: ReactNode;
  isInstalling: boolean;
  /** `allowDangerous` is true only through "Install anyway". */
  onInstall: (allowDangerous: boolean) => void;
  onClose: () => void;
};

export function SkillReviewDialog({ review, origin = null, isInstalling, onInstall, onClose }: SkillReviewDialogProps) {
  const { commands } = useTransport();
  // Content fetched from a source is scanned as soon as the review opens; a folder or archive from this
  // computer only on request (review F2).
  const external = useReviewScan((runId) => commands.externalScanReview(review.token, runId), review.source.kind !== "local");
  const shown = external.shown;
  const [acknowledged, setAcknowledged] = useState(false);
  // The acknowledgement covers the scans shown: a new external result asks again.
  useEffect(() => setAcknowledged(false), [shown]);
  const builtinDangerous = isDangerous(review.scan);
  const externalDangerous = isExternalDangerous(shown);
  const dangerous = builtinDangerous || externalDangerous;
  const actionNote = dangerous
    ? t("external.live.confirm", { action: t("install.review.installAnyway") })
    : t("external.live.available", { action: t("install.review.install") });
  const blocked = review.trust?.state.kind === "blocked" || review.movedTrust?.state.kind === "blocked";
  const source = review.source;
  const commit = source.commit?.slice(0, 7) ?? null;

  return (
    <SettingsDialog
      wide
      title={t("install.review.title", { name: review.name })}
      subtitle={source.url ?? t("install.review.localSource")}
      closeLabel={t("trust.close")}
      initialFocus="dialog"
      dismissible={!isInstalling}
      onClose={onClose}
      actions={
        <>
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            disabled={isInstalling}
            onClick={onClose}
          >
            {t("install.review.cancel")}
          </button>
          {dangerous ? (
            <button
              className="secondary-button secondary-button--compact danger-button"
              type="button"
              disabled={!acknowledged || isInstalling || blocked || external.busy}
              onClick={() => onInstall(true)}
            >
              {isInstalling ? t("install.review.installing") : t("install.review.installAnyway")}
            </button>
          ) : (
            <button
              className="primary-button primary-button--compact"
              type="button"
              disabled={isInstalling || blocked || external.busy}
              onClick={() => onInstall(false)}
            >
              {isInstalling ? t("install.review.installing") : t("install.review.install")}
            </button>
          )}
        </>
      }
    >
      <div className="source-review">
        {origin}
        <dl className="source-review__facts">
          {source.path ? (
            <div>
              <dt>{t("install.review.path")}</dt>
              <dd><code>{source.path}</code></dd>
            </div>
          ) : null}
          {source.gitRef ? (
            <div>
              <dt>{t("install.review.ref")}</dt>
              <dd><code>{source.gitRef}</code></dd>
            </div>
          ) : null}
          {commit ? (
            <div>
              <dt>{t("install.review.commit")}</dt>
              <dd><code>{commit}</code></dd>
            </div>
          ) : null}
        </dl>
        {review.trust ? <TrustSummary info={review.trust} /> : null}
        {review.movedTrust ? <TrustSummary info={review.movedTrust} label={t("trust.summary.moved")} /> : null}
        {blocked ? <p className="dialog-error" role="alert">{errorMessage("source_blocked")}</p> : null}
        <ScanFindings scan={review.scan} />
        {external.phase !== "off" ? <ExternalScanSection review={external} actionNote={actionNote} /> : null}
        {dangerous ? (
          <DangerousGate
            acknowledged={acknowledged}
            onChange={setAcknowledged}
            description={gateDescription(builtinDangerous, externalDangerous, "install")}
          />
        ) : null}
        <section className="source-review__files" aria-label={t("install.review.files")}>
          <h4>{t("install.review.filesCount", { count: review.files.length })}</h4>
          <ul>
            {review.files.map((file) => (
              <li key={file.path}>
                <code>{file.path}</code>
                <span>{formatSize(file.bytes)}</span>
              </li>
            ))}
          </ul>
        </section>
      </div>
    </SettingsDialog>
  );
}
