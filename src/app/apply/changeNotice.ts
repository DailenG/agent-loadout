// What every store change shows (SPEC 8.7): the queries it makes stale are
// refetched, and a toast reports the instant apply that went with it, with
// "Undo this apply" when the run was recorded.

import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t } from "@/i18n/en";
import type { ApplyStatusKind, ChangeResult } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";

/** Query keys of the commands a store change makes stale. */
export const storeQueryKeys = {
  servers: ["list_servers"],
  pendingPlan: ["pending_plan"],
  /** Prefix of `["list_project_servers", id]`. */
  projectServers: ["list_project_servers"],
  /** Store edits Discard pending can throw away. */
  storeChanges: ["pending_store_changes"],
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
  /** Prefix of `["list_skills", scope]`. */
  skills: ["list_skills"],
  /** Prefix of `["list_skill_tool_dirs", scope]`. */
  skillToolDirs: ["list_skill_tool_dirs"],
  /** Prefix of `["list_skill_files", id]` and `["read_skill_file", id, file]`. */
  skillFiles: ["list_skill_files"],
  skillFile: ["read_skill_file"],
  /** Settings > Sources: installs and updates can record trust (M42). */
  sources: ["list_sources"],
  /** Settings > Catalogs and Browse: the catalog list is in the store (`catalogs.toml`, M44/M45). */
  catalogs: ["catalogs_overview"],
  /** Settings > Agent integration: skill removes, restores and syncs change it (M46). */
  agentSkill: ["agent_skill_status"],
} as const;

/**
 * The scans of client files and skill folders behind the not-managed lists
 * and notice (#3, #9). Not store keys: imports, adopts, added projects and
 * acknowledgments refetch them (`useInvalidateImports`); other store
 * changes refetch the summaries once, coalesced (`STORE_COALESCE_MS`);
 * drift triggers refetch the notice's scans only when older than
 * `NOTICE_STALE_MS`.
 */
export const importQueryKeys = {
  /** Prefix of `["unmanaged_summary", kind, id]`: the MCP and Tools lists with the notice off. */
  unmanaged: ["unmanaged_summary"],
  /** `unmanaged_summary({kind:"all"})`: the notice and, with it on, the MCP and Tools lists. */
  unmanagedAll: ["unmanaged_summary_all"],
  /** Prefix of `["scan_unmanaged_skills", scope, client]`. */
  unmanagedSkills: ["scan_unmanaged_skills"],
} as const;

/** At most one full scan per minute from focus and watcher events. */
export const NOTICE_STALE_MS = 60_000;

/** Store changes in quick succession refetch the not-managed summaries once. */
export const STORE_COALESCE_MS = 1_000;

const coalesceTimers = new WeakMap<QueryClient, ReturnType<typeof setTimeout>>();

/** Schedules one refetch of the summaries after the last of a burst of store changes. */
function refetchSummariesSoon(queryClient: QueryClient) {
  clearTimeout(coalesceTimers.get(queryClient));
  coalesceTimers.set(
    queryClient,
    setTimeout(() => {
      coalesceTimers.delete(queryClient);
      for (const queryKey of [importQueryKeys.unmanaged, importQueryKeys.unmanagedAll]) {
        void queryClient.invalidateQueries({ queryKey });
      }
    }, STORE_COALESCE_MS),
  );
}

/** Refetches the not-managed scans after an import, adopt, added projects or acknowledgment. */
export function useInvalidateImports() {
  const queryClient = useQueryClient();
  return useCallback(async () => {
    await Promise.all(
      Object.values(importQueryKeys).map((queryKey) => queryClient.invalidateQueries({ queryKey })),
    );
  }, [queryClient]);
}

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
    refetchSummariesSoon(queryClient);
    await Promise.all(
      Object.values(storeQueryKeys).map((queryKey) =>
        queryClient.invalidateQueries({ queryKey }),
      ),
    );
  }, [queryClient]);
}

/**
 * Undoes an apply run (`loadout rollback`), then refetches and reports. A
 * file whose backup of that run held old secret values and was removed (M35)
 * is named: its previous content cannot come back.
 */
export function useUndoApply() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  return useCallback(
    async (applyId: string, confirmed = false) => {
      try {
        const result = unwrap(await commands.undoApply(applyId, confirmed));
        const unavailable = result.unavailableOriginals;
        notify(
          unavailable.length > 0
            ? {
                tone: "info",
                message: t("apply.undo.doneUnavailable", { id: result.applyId, files: unavailable.join(", ") }),
              }
            : { tone: "success", message: t("apply.undo.done", { id: result.applyId }) },
        );
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

/** The run summary plus how many files were read only and not written (M38), when any. */
function readOnlyNote(summary: string, count: number): string {
  if (count === 0) {
    return summary;
  }
  return `${summary} ${t(count === 1 ? "apply.readOnly.one" : "apply.readOnly.many", { count })}`;
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
        message: readOnlyNote(t(STATUS_KEYS[run.status], { message }), run.files.filter((file) => file.readOnly).length),
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
