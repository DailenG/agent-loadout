// Settings section pointing at the SkillDock migration (SPEC 15.1), whose
// wizard opens from the Import page since 0.3.0 (#3).

import { SettingsGroup, SettingsItem } from "@/app/components/SettingsGroup";
import { t } from "@/i18n/en";

export function MigrateSection({ onOpenImport }: { onOpenImport: () => void }) {
  return (
    <SettingsGroup title={t("migrate.section.title")}>
      <SettingsItem label={t("migrate.section.label")} description={t("migrate.section.movedDescription")}>
        <button className="secondary-button secondary-button--compact" type="button" onClick={onOpenImport}>
          {t("migrate.section.openImport")}
        </button>
      </SettingsItem>
    </SettingsGroup>
  );
}
