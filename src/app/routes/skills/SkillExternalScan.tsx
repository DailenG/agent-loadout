// "Scan with Snyk" on a library skill (M47 `external_scan_skill`, M49): the
// store copy is sent to Snyk Agent Scan on request and the result shown.
// Report only: nothing changes. Offered only while the scanner is on and
// installed.

import { ExternalScanSection, scannerPhase, useReviewScan, useScannerStatus } from "@/app/components/ExternalScan";
import { SettingsDialog } from "@/app/routes/settings/SettingsDialog";
import { t } from "@/i18n/en";
import type { SkillInfo } from "@/ipc/bindings";
import { useTransport } from "@/ipc/transport";

/** The button in a skill's details; nothing while Snyk Agent Scan is off or not installed. */
export function SkillExternalScanButton({ skill, onOpen }: { skill: SkillInfo; onOpen: () => void }) {
  const scanner = useScannerStatus();
  if (scannerPhase(scanner) !== "on" || !skill.hasContent) {
    return null;
  }
  return (
    <div className="external-scan__running">
      <button
        className="secondary-button secondary-button--compact"
        type="button"
        aria-label={t("external.skill.actionAria", { name: skill.name })}
        onClick={onOpen}
      >
        {t("external.skill.action")}
      </button>
    </div>
  );
}

/** Runs the scan once when opened; closing it while it runs cancels the run. */
export function SkillExternalScanDialog({ skill, onClose }: { skill: SkillInfo; onClose: () => void }) {
  const { commands } = useTransport();
  const external = useReviewScan((runId) => commands.externalScanSkill(skill.id, runId), true);

  return (
    <SettingsDialog
      wide
      portal
      title={t("external.skill.title", { name: skill.name })}
      closeLabel={t("trust.close")}
      initialFocus="dialog"
      onClose={onClose}
      actions={
        <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
          {t("external.skill.close")}
        </button>
      }
    >
      <ExternalScanSection review={external} sentNote={t("external.skill.note")} />
    </SettingsDialog>
  );
}
