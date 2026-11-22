// Catalog keys and status text of Settings > Versioning & Backup
// (SPEC 9.6, 9.8, 13.6 item 4), shared by its dialogs.

import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type { FolderModeDto, GitAuthDto, RemoteKindDto, RemoteStatusDto, SyncTool } from "@/ipc/bindings";

/** The remote name rule the backend enforces, checked before sending. */
export const REMOTE_NAME_PATTERN = /^[A-Za-z0-9_][A-Za-z0-9._-]{0,63}$/;

/** URLs `auth = gh` is offered for (SPEC 9.6). */
export const GITHUB_URL = /^(?:https:\/\/(?:[^@/]+@)?github\.com\/|ssh:\/\/git@github\.com[:/]|git@github\.com:)/i;

export const KIND_KEYS = {
  git: "settings.versioning.kind.git",
  folder: "settings.versioning.kind.folder",
} as const satisfies Record<RemoteKindDto, MessageKey>;

export const MODE_KEYS = {
  repo_per_machine: "settings.versioning.mode.repoPerMachine",
  mirror: "settings.versioning.mode.mirror",
  archive: "settings.versioning.mode.archive",
} as const satisfies Record<FolderModeDto, MessageKey>;

export const MODE_HINT_KEYS = {
  repo_per_machine: "settings.versioning.mode.repoPerMachineHint",
  mirror: "settings.versioning.mode.mirrorHint",
  archive: "settings.versioning.mode.archiveHint",
} as const satisfies Record<FolderModeDto, MessageKey>;

export const AUTH_KEYS = {
  system: "settings.versioning.auth.system",
  gh: "settings.versioning.auth.gh",
  token: "settings.versioning.auth.token",
} as const satisfies Record<GitAuthDto, MessageKey>;

const SYNC_TOOL_KEYS = {
  resilio: "settings.versioning.syncTool.resilio",
  syncthing: "settings.versioning.syncTool.syncthing",
  one_drive: "settings.versioning.syncTool.oneDrive",
  dropbox: "settings.versioning.syncTool.dropbox",
  google_drive: "settings.versioning.syncTool.googleDrive",
} as const satisfies Record<SyncTool, MessageKey>;

/** "Inside a Dropbox folder: supported, one subfolder per machine". */
export function syncToolNote(tool: SyncTool) {
  return t("settings.versioning.syncTool.note", { tool: t(SYNC_TOOL_KEYS[tool]) });
}

export type StatusPart = { text: string; tone: string };

/** `ahead N`, `behind N`, last error (SPEC 9.8), most urgent first. */
export function remoteStatus(status: RemoteStatusDto): StatusPart[] {
  if (status.reachable === false) {
    return [{ text: t("settings.versioning.status.unreachable"), tone: "tone-danger" }];
  }
  if (status.lastError) {
    return [
      {
        text: t("settings.versioning.status.lastError", { error: errorMessage(status.lastError.error.code) }),
        tone: "tone-danger",
      },
    ];
  }
  if (status.ahead === null) {
    return [{ text: t("settings.versioning.status.notPushed"), tone: "tone-neutral" }];
  }
  const behind = status.behind ?? 0;
  if (status.ahead === 0 && behind === 0) {
    return [{ text: t("settings.versioning.status.inSync"), tone: "tone-positive" }];
  }
  const parts: StatusPart[] = [];
  if (status.ahead > 0) {
    parts.push({ text: t("settings.versioning.status.ahead", { count: status.ahead }), tone: "tone-pending-push" });
  }
  if (behind > 0) {
    parts.push({ text: t("settings.versioning.status.behind", { count: behind }), tone: "tone-info" });
  }
  return parts;
}

/** An RFC 3339 time in the user's locale. */
export function formatTime(time: string) {
  const date = new Date(time);
  return Number.isNaN(date.getTime()) ? time : date.toLocaleString();
}
