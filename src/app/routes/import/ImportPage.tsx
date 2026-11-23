// The Import page (#3): one place to bring an existing setup under Loadout.
// MCP servers from client files (user and known projects), skill folders
// (adopt), project folders the clients know (discover), and the SkillDock
// migration, whose entry point moved here from Settings.

import { useId, useState } from "react";
import { DiscoverProjectsSection } from "@/app/routes/import/projects/DiscoverProjectsSection";
import { SkillsImportSection } from "@/app/routes/import/skills/SkillsImportSection";
import { MigrateWizard } from "@/app/routes/migrate/MigrateWizard";
import { t } from "@/i18n/en";
import { McpImportSection } from "./McpImportSection";
import type { ImportTarget } from "./model";

export type ImportPageProps = {
  /** An entry an "Import…" button points at (MCP page, Tools, the notice). */
  target: ImportTarget | null;
  onOpenPending: () => void;
};

function SkillDockSection({ onOpenPending }: { onOpenPending: () => void }) {
  const titleId = useId();
  const [open, setOpen] = useState(false);
  return (
    <section className="adopt-section import-section" aria-labelledby={titleId}>
      <h2 id={titleId} className="adopt-section__title">
        {t("import.skilldock.title")}
      </h2>
      <p className="adopt-section__description">{t("migrate.section.description")}</p>
      <div>
        <button className="secondary-button secondary-button--compact" type="button" onClick={() => setOpen(true)}>
          {t("migrate.section.open")}
        </button>
      </div>
      {open ? <MigrateWizard onClose={() => setOpen(false)} onOpenPending={onOpenPending} /> : null}
    </section>
  );
}

export function ImportPage({ target, onOpenPending }: ImportPageProps) {
  const mcpTitleId = useId();
  return (
    <div className="import-route">
      <section className="adopt-section import-section" aria-labelledby={mcpTitleId}>
        <h2 id={mcpTitleId} className="adopt-section__title">
          {t("import.mcp.title")}
        </h2>
        <p className="adopt-section__description">{t("import.mcp.description")}</p>
        {/* A new target starts the choices over with that entry picked. */}
        <McpImportSection
          key={target ? `${target.name}/${target.findingId ?? ""}` : ""}
          target={target}
          onOpenPending={onOpenPending}
        />
      </section>
      <SkillsImportSection onOpenPending={onOpenPending} />
      <DiscoverProjectsSection onOpenPending={onOpenPending} />
      <SkillDockSection onOpenPending={onOpenPending} />
    </div>
  );
}
