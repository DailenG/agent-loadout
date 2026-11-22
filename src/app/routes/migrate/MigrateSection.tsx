// Settings section that opens the SkillDock migration wizard (SPEC 15.1).

import { useState } from "react";
import { SettingsGroup, SettingsItem } from "@/app/components/SettingsGroup";
import { t } from "@/i18n/en";
import { MigrateWizard } from "./MigrateWizard";

export function MigrateSection({ onOpenPending }: { onOpenPending: () => void }) {
  const [open, setOpen] = useState(false);
  return (
    <SettingsGroup title={t("migrate.section.title")}>
      <SettingsItem label={t("migrate.section.label")} description={t("migrate.section.description")}>
        <button className="secondary-button secondary-button--compact" type="button" onClick={() => setOpen(true)}>
          {t("migrate.section.open")}
        </button>
      </SettingsItem>
      {open ? <MigrateWizard onClose={() => setOpen(false)} onOpenPending={onOpenPending} /> : null}
    </SettingsGroup>
  );
}
