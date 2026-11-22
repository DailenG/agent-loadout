// Restore dialog (SPEC 9.5, 13.5): "Restore this version" and "Restore
// entity...". The preview says what the restore adds, changes and removes
// before anything is written; shared project manifests need an explicit
// confirmation. The restore is one new store commit; client files change
// only when the user applies it from Pending changes.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useId, useState } from "react";
import { useInvalidateStore } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { failureText } from "@/app/routes/history/labels";
import { t, type MessageKey } from "@/i18n/en";
import type { HistoryEntry, RestorePreview, RestoreResult } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type RestoreDialogProps = {
  entry: HistoryEntry;
  /** `true`: "Restore entity..."; the user picks the entities first. */
  pick: boolean;
  onClose: () => void;
  /** The restore made a commit; its apply waits in Pending changes. */
  onRestored: (result: RestoreResult) => void;
};

const GROUPS = [
  ["added", "history.restore.added"],
  ["changed", "history.restore.changed"],
  ["removed", "history.restore.removed"],
] as const satisfies readonly (readonly [keyof RestorePreview, MessageKey])[];

export function RestoreDialog({ entry, pick, onClose, onRestored }: RestoreDialogProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const titleId = useId();
  const [picked, setPicked] = useState<string[]>([]);
  const [confirmed, setConfirmed] = useState(false);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  // A commit that names no entity (a merge, a commit made outside the app):
  // the entities to pick from are those the whole restore would touch.
  const needsCandidates = pick && entry.entities.length === 0;
  const whole = useQuery({
    queryKey: ["restore_preview", entry.id, []],
    queryFn: async () => unwrap(await commands.restorePreview(entry.id, [])),
    enabled: needsCandidates,
  });
  const candidates = needsCandidates
    ? [...new Set([...(whole.data?.added ?? []), ...(whole.data?.changed ?? []), ...(whole.data?.removed ?? [])])]
    : entry.entities;

  const entities = pick ? picked : [];
  const preview = useQuery({
    queryKey: ["restore_preview", entry.id, entities],
    queryFn: async () => unwrap(await commands.restorePreview(entry.id, entities)),
    enabled: !pick || entities.length > 0,
  });
  // Nothing picked yet: no preview (the cached whole-version one would mislead).
  const data = !pick || entities.length > 0 ? preview.data : undefined;
  const needsConfirmation = (data?.sharedManifests.length ?? 0) > 0;

  const restore = useMutation({
    mutationFn: async () =>
      unwrap(await commands.restoreStoreVersion(entry.id, entities, needsConfirmation && confirmed)),
    onSuccess: (result) => {
      void invalidate();
      if (result.tag) {
        notify({ tone: "success", message: t("history.restore.done", { id: entry.shortId, tag: result.tag }) });
      } else {
        notify({ tone: "info", message: t("history.restore.unchanged", { id: entry.shortId }) });
      }
      if (result.commit) {
        onRestored(result);
      } else {
        onClose();
      }
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  function toggle(entity: string, checked: boolean) {
    setPicked((current) =>
      candidates.filter((candidate) => (candidate === entity ? checked : current.includes(candidate))),
    );
  }

  const ready =
    data !== undefined &&
    !preview.isFetching &&
    (!needsConfirmation || confirmed) &&
    (!pick || picked.length > 0);

  return (
    <div className="dialog-backdrop" role="presentation" onClick={onClose}>
      <div
        className="tool-manage-dialog project-dialog history-restore"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="tool-manage-dialog__header">
          <div className="tool-manage-dialog__title">
            <h3 id={titleId}>
              {t(pick ? "history.restore.titleEntity" : "history.restore.titleVersion", { id: entry.shortId })}
            </h3>
            <p>{entry.summary}</p>
          </div>
          <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={t("history.close")}>
            ×
          </button>
        </div>

        <div className="project-dialog__body history-restore__body">
          {pick ? (
            <fieldset className="history-restore__pick">
              <legend>{t("history.restore.pick")}</legend>
              {needsCandidates && whole.isPending ? <p className="project-dialog__hint">{t("history.restore.loading")}</p> : null}
              {needsCandidates && whole.isError ? (
                <p className="history-dialog__error" role="alert">
                  {failureText(whole.error)}
                </p>
              ) : null}
              {candidates.map((entity) => (
                <label key={entity} className="project-dialog__choice">
                  <input
                    type="checkbox"
                    checked={picked.includes(entity)}
                    onChange={(event) => toggle(entity, event.target.checked)}
                  />
                  <span>{entity}</span>
                </label>
              ))}
              {picked.length === 0 ? <p className="project-dialog__hint">{t("history.restore.pickHint")}</p> : null}
            </fieldset>
          ) : null}

          {preview.isFetching ? <p className="project-dialog__hint">{t("history.restore.loading")}</p> : null}
          {preview.isError ? (
            <p className="history-dialog__error" role="alert">
              {failureText(preview.error)}
            </p>
          ) : null}
          {data ? (
            <section className="history-restore__preview" aria-label={t("history.restore.previewAria")}>
              <p className="history-restore__counts">
                {t("history.restore.counts", {
                  added: data.added.length,
                  changed: data.changed.length,
                  removed: data.removed.length,
                })}
              </p>
              {GROUPS.map(([group, label]) => {
                const names = data[group];
                return names.length > 0 ? (
                  <div key={group} className="history-restore__group">
                    <h4>{t(label)}</h4>
                    <ul aria-label={t(label)}>
                      {names.map((name) => (
                        <li key={name}>{name}</li>
                      ))}
                    </ul>
                  </div>
                ) : null;
              })}
              {data.unplaced.length > 0 ? (
                <p className="project-dialog__hint">
                  {t("history.restore.unplaced", { names: data.unplaced.join(", ") })}
                </p>
              ) : null}
              {needsConfirmation ? (
                <div className="history-restore__shared">
                  <p>{t("history.restore.shared")}</p>
                  <ul>
                    {data.sharedManifests.map((file) => (
                      <li key={file} className="history-mono">
                        {file}
                      </li>
                    ))}
                  </ul>
                  <label className="project-dialog__choice">
                    <input
                      type="checkbox"
                      checked={confirmed}
                      onChange={(event) => setConfirmed(event.target.checked)}
                    />
                    <span>{t("history.restore.sharedConfirm")}</span>
                  </label>
                </div>
              ) : null}
              <p className="project-dialog__hint">{t("history.restore.applyHint")}</p>
            </section>
          ) : null}
        </div>

        <div className="tool-manage-dialog__actions">
          <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
            {t("history.cancel")}
          </button>
          <button
            className="primary-button primary-button--compact"
            type="button"
            disabled={!ready || restore.isPending}
            onClick={() => restore.mutate()}
          >
            {t("history.restore.action")}
          </button>
        </div>
      </div>
    </div>
  );
}
