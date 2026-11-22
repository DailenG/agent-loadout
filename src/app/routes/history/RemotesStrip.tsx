// The History page's bottom strip (SPEC 9.8, 9.9, 13.5): each backup
// remote with its state, or "No backup remote" with a way to add one, and
// the fields that conflict after a pull or restore. Conflicts are resolved
// with the CLI in this version.

import { useQuery } from "@tanstack/react-query";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { remoteStatusText } from "@/app/routes/history/labels";
import { t } from "@/i18n/en";
import { unwrap, useTransport } from "@/ipc/transport";

export type RemotesStripProps = {
  onOpenSettings: () => void;
};

export function RemotesStrip({ onOpenSettings }: RemotesStripProps) {
  const { commands } = useTransport();
  const overview = useQuery({
    queryKey: storeQueryKeys.remotes,
    queryFn: async () => unwrap(await commands.remotesOverview()),
  });
  const conflicts = useQuery({
    queryKey: storeQueryKeys.mergeConflicts,
    queryFn: async () => unwrap(await commands.listMergeConflicts()),
  });
  const remotes = overview.data?.remotes;
  const conflicted = conflicts.data ?? [];

  return (
    <section className="history-strip" aria-label={t("history.strip.aria")}>
      {remotes ? (
        <div className="history-strip__line">
          <span className="history-strip__label">{t("history.remotes.label")}</span>
          {remotes.length === 0 ? (
            <>
              <span className="status-badge tone-neutral">{t("history.remotes.none")}</span>
              <button className="ghost-button history-strip__link" type="button" onClick={onOpenSettings}>
                {t("history.remotes.add")}
              </button>
            </>
          ) : (
            <ul className="history-strip__remotes">
              {remotes.map((remote) => (
                <li key={remote.name} className="history-strip__remote">
                  <strong>{remote.name}</strong> <span>{remoteStatusText(remote.status)}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
      {conflicted.length > 0 ? (
        <div className="history-strip__line history-strip__conflicts" role="status">
          <span className="history-strip__label">{t("history.conflicts.label", { count: conflicted.length })}</span>
          <ul className="history-strip__remotes">
            {conflicted.map((conflict, index) => (
              <li key={`${conflict.source}-${conflict.entity}-${conflict.field ?? conflict.file}-${index}`}>
                {t("history.conflicts.item", { entity: conflict.entity, field: conflict.field ?? conflict.file })}
              </li>
            ))}
          </ul>
          <span className="history-strip__hint">{t("history.conflicts.hint")}</span>
        </div>
      ) : null}
    </section>
  );
}
