// What every store change shows (SPEC 8.7): the queries it makes stale are
// refetched, and a toast reports the instant apply that went with it, with
// "Undo this apply" when the run was recorded.

import { useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t } from "@/i18n/en";
import type { ApplyStatusKind, ChangeResult } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";

/** Query keys of the commands a store change makes stale. */
export const storeQueryKeys = {
  servers: ["list_servers"],
  pendingPlan: ["pending_plan"],
  appliedRuns: ["list_applied_runs"],
  tools: ["list_tools"],
  projects: ["list_projects"],
  /** Prefix of every project's matrix (`["effective_matrix", id]`). */
  effective: ["effective_matrix"],
  history: ["list_history"],
  tags: ["list_tags"],
  /** Prefix of `["commit_detail", id]`. */
  commitDetail: ["commit_detail"],
  fileBackups: ["list_file_backups"],
  mergeConflicts: ["list_merge_conflicts"],
  remotes: ["remotes_overview"],
  secrets: ["secrets_overview"],
} as const;

const STATUS_KEYS = {
  done: "apply.status.done",
  file_failed: "apply.status.file_failed",
  needs_decision: "apply.status.needs_decision",
  missing_secrets: "apply.status.missing_secrets",
  trust_required: "apply.status.trust_required",
} as const satisfies Record<ApplyStatusKind, string>;

/** Refetches everything a store change can affect. */
export function useInvalidateStore() {
  const queryClient = useQueryClient();
  return useCallback(async () => {
    await Promise.all(
      Object.values(storeQueryKeys).map((queryKey) =>
        queryClient.invalidateQueries({ queryKey }),
      ),
    );
  }, [queryClient]);
}

/** Undoes an apply run (`loadout rollback`), then refetches and reports. */
export function useUndoApply() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  return useCallback(
    async (applyId: string, confirmed = false) => {
      try {
        const result = unwrap(await commands.undoApply(applyId, confirmed));
        notify({ tone: "success", message: t("apply.undo.done", { id: result.applyId }) });
        return result;
      } catch (error) {
        notify({
          tone: "error",
          message: error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown"),
        });
        throw error;
      } finally {
        await invalidate();
      }
    },
    [commands, invalidate, notify],
  );
}

/**
 * Reports a store change: `message` says what changed ("Saved github"); the
 * apply status adds what is left to do. A recorded run offers "Undo this
 * apply".
 */
export function useChangeNotice() {
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const undo = useUndoApply();
  return useCallback(
    (result: ChangeResult, message: string) => {
      void invalidate();
      const run = result.apply;
      if (!run) {
        notify({ tone: "info", message: t("apply.saved.reviewMode", { message }) });
        return;
      }
      const failed = run.status === "file_failed" || run.status === "missing_secrets";
      notify({
        tone: failed ? "error" : run.status === "done" ? "success" : "info",
        message: t(STATUS_KEYS[run.status], { message }),
        ...(run.recorded
          ? {
              actionLabel: t("apply.undo.action"),
              onAction: () => void undo(run.applyId).catch(() => undefined),
            }
          : {}),
      });
    },
    [invalidate, notify, undo],
  );
}
