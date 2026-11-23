// The Import page's Skills section (#3): the unmanaged skill folders of the
// active scope, every client or one, adopted into the store.

import { useQuery } from "@tanstack/react-query";
import { useId, useState } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { orderByRecommended } from "@/app/clients";
import { useActiveScope } from "@/app/scope/ActiveScope";
import { t } from "@/i18n/en";
import { unwrap, useTransport } from "@/ipc/transport";
import { AdoptPanel } from "./AdoptPanel";

export type SkillsImportSectionProps = {
  /** Accepted for the Import page's shared section contract; adopts make no pending apply. */
  onOpenPending?: () => void;
};

export function SkillsImportSection(_props: SkillsImportSectionProps) {
  const { commands } = useTransport();
  const { scope, project } = useActiveScope();
  const titleId = useId();
  const [client, setClient] = useState<string | null>(null);
  const skillList = useQuery({
    queryKey: [...storeQueryKeys.skills, scope],
    queryFn: async () => unwrap(await commands.listSkills(scope)),
  });
  const clients = orderByRecommended(skillList.data?.clients ?? [], (entry) => entry.id);
  const scopeKey = scope.kind === "user" ? "user" : `project:${scope.id}`;

  return (
    <section className="adopt-section" aria-labelledby={titleId}>
      <h2 id={titleId} className="adopt-section__title">
        {t("adopt.section.title")}
      </h2>
      <p className="adopt-section__description">
        {project
          ? t("adopt.section.descriptionProject", { name: project.name })
          : t("adopt.section.description")}
      </p>
      {clients.length > 0 ? (
        <div className="skills-source-tabs adopt-filter" role="group" aria-label={t("adopt.client.aria")}>
          {[{ id: null, name: t("adopt.client.all") }, ...clients].map((entry) => (
            <button
              key={entry.id ?? "all"}
              className={`skills-source-tab${client === entry.id ? " is-selected" : ""}`}
              type="button"
              aria-pressed={client === entry.id}
              onClick={() => setClient(entry.id)}
            >
              {entry.name}
            </button>
          ))}
        </div>
      ) : null}
      {/* A new scope or client is a new scan with fresh choices. */}
      <AdoptPanel key={`${scopeKey}:${client ?? "all"}`} scope={scope} client={client} />
    </section>
  );
}
