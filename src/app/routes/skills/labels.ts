// Catalog keys of the Skills page enums, and the text of a change's link
// notes (SPEC 12.2 statuses).

import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type {
  SkillEntryClass,
  SkillFileChangeKind,
  SkillLinkMethod,
  SkillLinkStatus,
  SkillOrigin,
  SkillSync,
} from "@/ipc/bindings";
import { IpcFailure } from "@/ipc/transport";

export const ORIGIN_KEYS = {
  git: "skill.card.sourceMethod.git",
  local: "skill.card.sourceMethod.local",
  agents_cli: "skill.card.sourceMethod.agents_cli",
  catalog: "skill.card.sourceMethod.catalog",
  authored: "skill.card.sourceMethod.authored",
} as const satisfies Record<SkillOrigin, MessageKey>;

export const ENTRY_CLASS_KEYS = {
  managed: "skills.source.status.managed",
  unmanaged: "skills.source.status.unmanaged",
  mismatch: "skills.source.status.mismatch",
} as const satisfies Record<SkillEntryClass, MessageKey>;

/** Status badge tones of SkillDock's tool folder rows. */
export const ENTRY_CLASS_TONES: Record<SkillEntryClass, string> = {
  managed: "tone-positive",
  unmanaged: "tone-neutral",
  mismatch: "tone-warning",
};

export const LINK_METHOD_KEYS = {
  junction: "skill.card.linkMethod.junction",
  symlink: "skill.card.linkMethod.symlink",
  copy: "skill.card.linkMethod.copy",
} as const satisfies Record<SkillLinkMethod, MessageKey>;

export const LINK_STATUS_KEYS = {
  created: "skill.link.status.created",
  adopted: "skill.link.status.adopted",
  unchanged: "skill.link.status.unchanged",
  updated: "skill.link.status.updated",
  removed: "skill.link.status.removed",
  foreign: "skill.link.status.foreign",
  collision: "skill.link.status.collision",
  changed: "skill.link.status.changed",
  blocked: "skill.link.status.blocked",
} as const satisfies Record<SkillLinkStatus, MessageKey>;

export const CHANGE_KIND_KEYS = {
  added: "skill.update.kind.added",
  modified: "skill.update.kind.modified",
  removed: "skill.update.kind.removed",
} as const satisfies Record<SkillFileChangeKind, MessageKey>;

export function failureText(error: unknown) {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

const ATTENTION: SkillLinkStatus[] = ["foreign", "collision", "changed", "blocked"];

/**
 * The toast text after a change: `message`, plus the client folder entries
 * that need the user (a real folder or another link in the way, an edited
 * copy kept, a blocked entry), named by client and path.
 */
export function syncMessage(sync: SkillSync, message: string): string {
  const notes = sync.notes.filter((note) => ATTENTION.includes(note.status));
  if (notes.length > 0) {
    return t("skills.links.attention", {
      message,
      notes: notes
        .map((note) => `${note.client} ${note.path}: ${t(LINK_STATUS_KEYS[note.status])}`)
        .join("; "),
    });
  }
  if (sync.warnings.length > 0) {
    return t("skills.links.warnings", { message, count: sync.warnings.length });
  }
  return message;
}

/** The first letter shown in a skill's monogram. */
export function monogram(value: string): string {
  const first = Array.from(value.trim()).find((character) => /[\p{L}\p{N}]/u.test(character));
  return (first ?? "?").toLocaleUpperCase();
}
