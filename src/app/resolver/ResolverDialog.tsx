// Drift resolver (SPEC 8.2, 13.4): one entry that needs a decision, in three
// columns (last written, current file, desired), with the actions its state
// allows. Each action is the planner's per-entry decision, applied to that
// file in one store commit.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useId } from "react";
import { useChangeNotice } from "@/app/apply/changeNotice";
import { STATE_KEYS } from "@/app/apply/labels";
import { useNotifications } from "@/app/components/notifications";
import { failureText } from "@/app/routes/projects/labels";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type { EntryRef, ResolveAction } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export const ACTION_KEYS = {
  overwrite: "resolver.action.overwrite",
  import_theirs: "resolver.action.importTheirs",
  release: "resolver.action.release",
  skip: "resolver.action.skip",
  recreate: "resolver.action.recreate",
  respect_deletion: "resolver.action.respectDeletion",
  remove_anyway: "resolver.action.removeAnyway",
} as const satisfies Record<ResolveAction, MessageKey>;

const HINT_KEYS = {
  overwrite: "resolver.hint.overwrite",
  import_theirs: "resolver.hint.importTheirs",
  release: "resolver.hint.release",
  skip: "resolver.hint.skip",
  recreate: "resolver.hint.recreate",
  respect_deletion: "resolver.hint.respectDeletion",
  remove_anyway: "resolver.hint.removeAnyway",
} as const satisfies Record<ResolveAction, MessageKey>;

export type ResolverDialogProps = {
  entry: EntryRef;
  clientName: (id: string) => string;
  onClose: () => void;
};

function Column({ title, text, empty }: { title: string; text: string | null; empty: string }) {
  return (
    <section className="resolver-column" aria-label={title}>
      <h4>{title}</h4>
      {text === null ? (
        <p className="resolver-column__empty">{empty}</p>
      ) : (
        <pre className="apply-diff resolver-column__text">{text}</pre>
      )}
    </section>
  );
}

export function ResolverDialog({ entry, clientName, onClose }: ResolverDialogProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const reportChange = useChangeNotice();
  const titleId = useId();
  const detail = useQuery({
    queryKey: ["resolver_detail", entry],
    queryFn: async () => unwrap(await commands.resolverDetail(entry)),
    staleTime: 0,
    gcTime: 0,
  });

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const resolve = useMutation({
    mutationFn: async (action: ResolveAction) => ({
      action,
      result: unwrap(await commands.resolveEntry(entry, action)),
    }),
    onSuccess: ({ action, result }) => {
      if (action !== "skip") {
        reportChange(result, t("resolver.done", { name: entry.name, action: t(ACTION_KEYS[action]) }));
      }
      onClose();
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const data = detail.data;
  const lastWrittenEmpty =
    data?.state === "collision" ? t("resolver.column.neverWritten") : t("resolver.column.unknown");
  return (
    <div className="dialog-backdrop resolver-backdrop" role="presentation" onClick={onClose}>
      <div
        className="tool-manage-dialog resolver-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="tool-manage-dialog__header">
          <div className="tool-manage-dialog__title">
            <h3 id={titleId}>{t("resolver.title", { name: entry.name })}</h3>
            <p>
              {t("resolver.subtitle", {
                client: clientName(entry.client),
                file: data?.project ? `${data.project}: ${data.file}` : entry.file.file,
              })}
              {data ? (
                <span className="status-badge tone-warning resolver-dialog__state">
                  {t(STATE_KEYS[data.state])}
                </span>
              ) : null}
            </p>
          </div>
          <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={t("resolver.close")}>
            ×
          </button>
        </div>

        {detail.isPending ? <p className="resolver-dialog__note">{t("resolver.loading")}</p> : null}
        {detail.isError ? (
          <p className="resolver-dialog__note" role="alert">
            {failureText(detail.error)}
          </p>
        ) : null}
        {data ? (
          <>
            <div className="resolver-columns">
              <Column title={t("resolver.column.lastWritten")} text={data.lastWritten} empty={lastWrittenEmpty} />
              <Column title={t("resolver.column.current")} text={data.current} empty={t("resolver.column.deleted")} />
              <Column title={t("resolver.column.desired")} text={data.desired} empty={t("resolver.column.notDesired")} />
            </div>
            {data.problem ? (
              <p className="resolver-dialog__note" role="alert">
                {errorMessage(data.problem.code)}
              </p>
            ) : null}
            <p className="resolver-dialog__note">{t("resolver.note.file")}</p>
            <div className="resolver-actions" role="group" aria-label={t("resolver.actions")}>
              {data.actions.map((action, index) => (
                <button
                  key={action}
                  type="button"
                  className={
                    index === 0
                      ? "primary-button primary-button--compact"
                      : "secondary-button secondary-button--compact"
                  }
                  title={t(HINT_KEYS[action])}
                  disabled={resolve.isPending}
                  onClick={() => resolve.mutate(action)}
                >
                  {t(ACTION_KEYS[action])}
                </button>
              ))}
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}
