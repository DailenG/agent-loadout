// Add folder / Init manifest (SPEC 13.3): a folder with a manifest is bound
// to this machine (and then reviewed before anything applies); a folder
// without one gets a new manifest, Private preselected, Shared on request.

import { useMutation } from "@tanstack/react-query";
import { useEffect, useId, useState } from "react";
import { useChangeNotice } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { LAYOUT_KEYS, failureText } from "@/app/routes/projects/labels";
import { t } from "@/i18n/en";
import type { PickedFolder, ProjectChange } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type FolderDialogProps = {
  folder: PickedFolder;
  /** Which toolbar button picked it; the folder's content decides the form. */
  intent: "add" | "init";
  onClose: () => void;
  /** `review`: the bound project waits for its trust review. */
  onDone: (change: ProjectChange, review: boolean) => void;
};

export function FolderDialog({ folder, intent, onClose, onDone }: FolderDialogProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const reportChange = useChangeNotice();
  const titleId = useId();
  const [shared, setShared] = useState(false);
  const [name, setName] = useState(folder.name);
  const manifest = folder.manifest;

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const add = useMutation({
    mutationFn: async () => unwrap(await commands.addProject(folder.token)),
    onSuccess: (change) => {
      notify({ tone: "info", message: t("projects.folder.added", { name: manifest?.name ?? change.id }) });
      onDone(change, change.trust !== "trusted");
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const init = useMutation({
    mutationFn: async () =>
      unwrap(await commands.initProject(folder.token, shared, name.trim() === "" ? null : name.trim())),
    onSuccess: (change) => {
      reportChange(
        change,
        t("projects.folder.created", {
          layout: t(shared ? "projects.folder.shared" : "projects.folder.private"),
          name: name.trim() || change.id,
        }),
      );
      onDone(change, false);
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const busy = add.isPending || init.isPending;
  return (
    <div className="dialog-backdrop" role="presentation" onClick={onClose}>
      <div
        className="tool-manage-dialog project-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="tool-manage-dialog__header">
          <div className="tool-manage-dialog__title">
            <h3 id={titleId}>
              {t(intent === "init" ? "projects.folder.titleInit" : "projects.folder.titleAdd")}
            </h3>
            <p title={folder.path}>{folder.path}</p>
          </div>
          <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={t("projects.cancel")}>
            ×
          </button>
        </div>

        {manifest ? (
          <div className="project-dialog__body">
            <p>{t("projects.folder.found", { name: manifest.name, id: manifest.id })}</p>
            <p className="project-dialog__hint">{t(LAYOUT_KEYS[manifest.layout])}</p>
            <p className="project-dialog__hint">
              {manifest.known ? t("projects.folder.known") : t("projects.folder.addHint")}
            </p>
          </div>
        ) : (
          <fieldset className="project-dialog__body">
            <legend>{t("projects.folder.none")}</legend>
            <label className="project-dialog__choice">
              <input type="radio" name="layout" checked={!shared} onChange={() => setShared(false)} />
              <span>
                <strong>{t("projects.folder.private")}</strong>
                <span className="project-dialog__hint">{t("projects.folder.privateHint")}</span>
              </span>
            </label>
            <label className="project-dialog__choice">
              <input type="radio" name="layout" checked={shared} onChange={() => setShared(true)} />
              <span>
                <strong>{t("projects.folder.shared")}</strong>
                <span className="project-dialog__hint">{t("projects.folder.sharedHint")}</span>
              </span>
            </label>
            <label className="project-dialog__field">
              <span>{t("projects.folder.name")}</span>
              <input
                className="tool-manage-dialog__search"
                type="text"
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </label>
          </fieldset>
        )}

        <div className="tool-manage-dialog__actions">
          <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
            {t("projects.cancel")}
          </button>
          {manifest ? (
            <button
              className="primary-button primary-button--compact"
              type="button"
              disabled={busy}
              onClick={() => add.mutate()}
            >
              {t("projects.folder.add")}
            </button>
          ) : (
            <button
              className="primary-button primary-button--compact"
              type="button"
              disabled={busy}
              onClick={() => init.mutate()}
            >
              {t("projects.folder.create")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
