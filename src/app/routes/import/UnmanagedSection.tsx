// The read-only "not managed by Loadout" list (#3, #9): foreign MCP entries
// of the client files in one scope, per client and file, each with "Import…"
// that opens the import view with that entry picked. The MCP page shows it
// for the sidebar's scope, the Tools Manage dialog for one client.

import { useId } from "react";
import { orderByRecommended } from "@/app/clients";
import { projectRelativePath } from "@/app/routes/projects/labels";
import { useActiveScope } from "@/app/scope/ActiveScope";
import { t } from "@/i18n/en";
import type { UnmanagedGroup, UnmanagedSummary } from "@/ipc/bindings";
import { importScopeOf, type ImportTarget } from "./model";
import { useUnmanagedSummary } from "./unmanaged";

/** The sidebar scope's not-managed entries. */
export function useScopeUnmanaged() {
  const { scope } = useActiveScope();
  return useUnmanagedSummary(importScopeOf(scope));
}

/** Unmanaged entries per client id. */
export function unmanagedByClient(summary: UnmanagedSummary | undefined): Map<string, number> {
  const counts = new Map<string, number>();
  for (const group of summary?.groups ?? []) {
    counts.set(group.client, (counts.get(group.client) ?? 0) + group.entries.length);
  }
  return counts;
}

export type UnmanagedSectionProps = {
  summary: UnmanagedSummary | undefined;
  /** Client names by id (an unknown id shows as itself). */
  clientNames: ReadonlyMap<string, string>;
  /** Only this client's groups (Tools > Manage). */
  client?: string;
  onImport: (target: ImportTarget) => void;
};

function groupTitle(group: UnmanagedGroup, clientName: string, projectName: string | null): string {
  if (group.scope === "user") {
    return clientName;
  }
  return t(group.scope === "project_local" ? "import.unmanaged.groupLocal" : "import.unmanaged.groupProject", {
    client: clientName,
    project: projectName ?? group.projectId ?? "",
  });
}

export function UnmanagedSection({ summary, clientNames, client, onImport }: UnmanagedSectionProps) {
  const headingId = useId();
  const { project } = useActiveScope();
  const groups = orderByRecommended(
    (summary?.groups ?? []).filter((group) => !client || group.client === client),
    (group) => group.client,
  );
  const total = groups.reduce((sum, group) => sum + group.entries.length, 0);
  if (!summary || total === 0) {
    return null;
  }
  return (
    <section className="import-unmanaged" aria-labelledby={headingId}>
      <h4 id={headingId} className="apply-drawer__section-title">
        {t("import.unmanaged.title", { count: total })}
      </h4>
      <p className="apply-drawer__note">{t("import.unmanaged.description")}</p>
      {groups.map((group) => {
        const name = clientNames.get(group.client) ?? group.client;
        const title = groupTitle(group, name, project?.id === group.projectId ? project.name : null);
        const key = `${group.client}-${group.scope}-${group.projectId ?? ""}`;
        return (
          <div key={key} className="import-unmanaged__group" role="group" aria-label={title}>
            <div className="import-unmanaged__group-head">
              <strong>{title}</strong>
              <span className="status-badge tone-neutral">
                {t("import.unmanaged.count", { count: group.entries.length })}
              </span>
            </div>
            <ul className="import-unmanaged__files" aria-label={t("import.unmanaged.filesAria", { name: title })}>
              {group.files.map((file) => (
                <li key={file} title={file}>
                  {projectRelativePath(file, project?.id === group.projectId ? project?.path : undefined)}
                </li>
              ))}
            </ul>
            <ul className="import-unmanaged__entries">
              {group.entries.map((entry) => (
                <li key={entry.id} className="import-unmanaged__entry">
                  <span className="import-unmanaged__name">{entry.name}</span>
                  {entry.new ? <span className="status-badge tone-warning">{t("import.unmanaged.new")}</span> : null}
                  <button
                    className="secondary-button secondary-button--compact"
                    type="button"
                    aria-label={t("import.unmanaged.importAria", { name: entry.name, client: name })}
                    onClick={() => onImport({ name: entry.name, findingId: entry.id })}
                  >
                    {t("import.unmanaged.import")}
                  </button>
                </li>
              ))}
            </ul>
          </div>
        );
      })}
    </section>
  );
}
