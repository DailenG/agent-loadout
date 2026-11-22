// Tag dialog (SPEC 9.4, 13.5): names a commit (or the current version) with
// a user tag. The name is checked here first so a typo never reaches the
// store.

import { useMutation } from "@tanstack/react-query";
import { useEffect, useId, useState } from "react";
import { useInvalidateStore } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { TAG_NAME_PATTERN, failureText } from "@/app/routes/history/labels";
import { t } from "@/i18n/en";
import type { HistoryEntry } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type TagDialogProps = {
  /** The commit to tag; `null` tags the current version (HEAD). */
  entry: HistoryEntry | null;
  onClose: () => void;
};

export function TagDialog({ entry, onClose }: TagDialogProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const titleId = useId();
  const hintId = useId();
  const [name, setName] = useState("");
  const [message, setMessage] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const valid = TAG_NAME_PATTERN.test(name);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const create = useMutation({
    mutationFn: async () =>
      unwrap(await commands.createTag(name, entry?.id ?? null, message.trim() === "" ? null : message.trim())),
    onSuccess: (tag) => {
      void invalidate();
      notify({ tone: "success", message: t("history.tag.created", { name: tag.name, id: tag.commit.slice(0, 7) }) });
      onClose();
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  function submit() {
    setSubmitted(true);
    if (valid) {
      create.mutate();
    }
  }

  const showError = submitted && !valid;
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
              {entry ? t("history.tag.title", { id: entry.shortId }) : t("history.tag.titleCurrent")}
            </h3>
            {entry ? <p>{entry.summary}</p> : null}
          </div>
          <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={t("history.close")}>
            ×
          </button>
        </div>
        <form
          className="project-dialog__body"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <label className="project-dialog__field">
            <span>{t("history.tag.name")}</span>
            <input
              className="tool-manage-dialog__search"
              type="text"
              autoComplete="off"
              spellCheck={false}
              value={name}
              aria-invalid={showError}
              aria-describedby={hintId}
              onChange={(event) => setName(event.target.value)}
            />
          </label>
          {showError ? (
            <p id={hintId} className="history-dialog__error" role="alert">
              {t("history.tag.nameInvalid")}
            </p>
          ) : (
            <p id={hintId} className="project-dialog__hint">
              {t("history.tag.nameHint")}
            </p>
          )}
          <label className="project-dialog__field">
            <span>{t("history.tag.message")}</span>
            <input
              className="tool-manage-dialog__search"
              type="text"
              value={message}
              onChange={(event) => setMessage(event.target.value)}
            />
          </label>
          <div className="tool-manage-dialog__actions">
            <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
              {t("history.cancel")}
            </button>
            <button className="primary-button primary-button--compact" type="submit" disabled={create.isPending}>
              {t("history.tag.create")}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
