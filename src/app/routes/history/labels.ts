// Catalog keys and display helpers of the History page (SPEC 13.5), shared
// by its tabs, dialogs and the remotes strip.

import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type {
  EntityChangeKind,
  FileRestoreOutcome,
  HistoryScope,
  OutcomeKind,
  RemoteStatusDto,
} from "@/ipc/bindings";
import { IpcFailure } from "@/ipc/transport";

export const SCOPE_KEYS = {
  all: "history.scope.all",
  system: "history.scope.system",
  projects: "history.scope.projects",
} as const satisfies Record<HistoryScope, MessageKey>;

export const OUTCOME_KEYS = {
  ok: "history.outcome.ok",
  partial: "history.outcome.partial",
  blocked: "history.outcome.blocked",
  failed: "history.outcome.failed",
} as const satisfies Record<OutcomeKind, MessageKey>;

export const OUTCOME_TONES: Record<OutcomeKind, string> = {
  ok: "tone-positive",
  partial: "tone-warning",
  blocked: "tone-warning",
  failed: "tone-danger",
};

export const CHANGE_KEYS = {
  added: "history.change.added",
  changed: "history.change.changed",
  removed: "history.change.removed",
} as const satisfies Record<EntityChangeKind, MessageKey>;

export const CHANGE_TONES: Record<EntityChangeKind, string> = {
  added: "tone-positive",
  changed: "tone-info",
  removed: "tone-danger",
};

export const FILE_RESTORE_KEYS = {
  restored: "history.backups.outcome.restored",
  unchanged: "history.backups.outcome.unchanged",
  failed: "history.backups.outcome.failed",
} as const satisfies Record<FileRestoreOutcome, MessageKey>;

export const FILE_RESTORE_TONES: Record<FileRestoreOutcome, string> = {
  restored: "tone-positive",
  unchanged: "tone-neutral",
  failed: "tone-danger",
};

/** The names `create_tag` accepts (SPEC 9.4). */
export const TAG_NAME_PATTERN = /^[A-Za-z0-9._-]{1,64}$/;

/** Automatic tags (`auto/pre-restore/...`) are pruned by age, never deleted by hand. */
export function isAutoTag(name: string) {
  return name.startsWith("auto/");
}

export function formatTime(time: string) {
  const date = new Date(time);
  return Number.isNaN(date.getTime()) ? time : date.toLocaleString();
}

export function failureText(error: unknown) {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

/** A remote's state in the strip: reachability, last error, then ahead/behind. */
export function remoteStatusText(status: RemoteStatusDto) {
  if (status.reachable === false) {
    return t("history.remotes.unreachable");
  }
  if (status.lastError) {
    return t("history.remotes.error", { message: errorMessage(status.lastError.error.code) });
  }
  if (status.ahead === null) {
    return t("history.remotes.notPushed");
  }
  const behind = status.behind ?? 0;
  if (status.ahead === 0 && behind === 0) {
    return t("history.remotes.inSync");
  }
  return [
    status.ahead > 0 ? t("history.remotes.ahead", { count: status.ahead }) : "",
    behind > 0 ? t("history.remotes.behind", { count: behind }) : "",
  ]
    .filter(Boolean)
    .join(" ");
}
