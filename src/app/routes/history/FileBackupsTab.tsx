// "Client file backups" tab (SPEC 8.6, 13.5): the apply runs that backed up
// client files before writing them, with Restore all and per-file Restore,
// and each file's outcome once restored.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { storeQueryKeys, useInvalidateStore } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import {
  FILE_RESTORE_KEYS,
  FILE_RESTORE_TONES,
  failureText,
  formatTime,
} from "@/app/routes/history/labels";
import { errorMessage, t } from "@/i18n/en";
import type { BackupRunEntry, FileRestoreEntry } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

type RestoreRequest = { run: BackupRunEntry; file: string | null };

export function FileBackupsTab() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const runs = useQuery({
    queryKey: storeQueryKeys.fileBackups,
    queryFn: async () => unwrap(await commands.listFileBackups()),
  });
  // Outcomes per apply id, then per file, from the last restore of each.
  const [outcomes, setOutcomes] = useState<Record<string, Record<string, FileRestoreEntry>>>({});

  const restore = useMutation({
    mutationFn: async ({ run, file }: RestoreRequest) => unwrap(await commands.restoreClientFiles(run.applyId, file)),
    onSuccess: (entries, { run }) => {
      setOutcomes((current) => ({
        ...current,
        [run.applyId]: {
          ...current[run.applyId],
          ...Object.fromEntries(entries.map((entry) => [entry.file, entry])),
        },
      }));
      const restored = entries.filter((entry) => entry.outcome === "restored").length;
      const failed = entries.some((entry) => entry.outcome === "failed");
      notify({
        tone: failed ? "error" : "success",
        message: t("history.backups.done", { restored, total: entries.length }),
      });
      void invalidate();
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  if (runs.isPending) {
    return <p className="history-note">{t("history.backups.loading")}</p>;
  }
  if (runs.isError) {
    return (
      <div className="panel-card empty-state page-empty-state" role="alert">
        <h3>{t("app.error.loadFailed")}</h3>
        <p>{failureText(runs.error)}</p>
        <button className="secondary-button" type="button" onClick={() => void runs.refetch()}>
          {t("app.action.retry")}
        </button>
      </div>
    );
  }
  if (runs.data.length === 0) {
    return (
      <div className="panel-card empty-state page-empty-state">
        <h3>{t("page.empty.title")}</h3>
        <p>{t("history.backups.empty")}</p>
      </div>
    );
  }
  return (
    <div className="history-backups">
      {runs.data.map((run) => {
        const label = t("history.backups.run", { id: run.applyId, time: formatTime(run.time) });
        return (
          <article key={run.applyId} className="panel-card history-backup-run" aria-label={label}>
            <div className="history-backup-run__head">
              <strong>{label}</strong>
              {run.undoOfRestore ? (
                <span className="status-badge tone-neutral">{t("history.backups.undoOfRestore")}</span>
              ) : null}
              <button
                className="secondary-button secondary-button--compact history-backup-run__all"
                type="button"
                disabled={restore.isPending}
                onClick={() => restore.mutate({ run, file: null })}
              >
                {t("history.backups.restoreAll")}
              </button>
            </div>
            <ul className="history-backup-run__files">
              {run.files.map((file) => {
                const outcome = outcomes[run.applyId]?.[file];
                return (
                  <li key={file} className="history-backup-file">
                    <span className="history-mono history-backup-file__path" title={file}>
                      {file}
                    </span>
                    {outcome ? (
                      <span className={`status-badge ${FILE_RESTORE_TONES[outcome.outcome]}`}>
                        {t(FILE_RESTORE_KEYS[outcome.outcome])}
                      </span>
                    ) : null}
                    {outcome?.error ? (
                      <span className="history-backup-file__error">{errorMessage(outcome.error.code)}</span>
                    ) : null}
                    <button
                      className="ghost-button history-backup-file__restore"
                      type="button"
                      aria-label={t("history.backups.restoreFileAria", { file })}
                      disabled={restore.isPending}
                      onClick={() => restore.mutate({ run, file })}
                    >
                      {t("history.backups.restoreFile")}
                    </button>
                  </li>
                );
              })}
            </ul>
          </article>
        );
      })}
    </div>
  );
}
