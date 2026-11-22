// Entry states and diff line styles shared by the apply drawer, the drift
// resolver, the trust review and the effective matrix.

import type { MessageKey } from "@/i18n/en";
import type { EntryStateKind } from "@/ipc/bindings";

export const STATE_KEYS = {
  synced: "apply.state.synced",
  pending_create: "apply.state.pendingCreate",
  pending_update: "apply.state.pendingUpdate",
  pending_remove: "apply.state.pendingRemove",
  drifted: "apply.state.drifted",
  missing: "apply.state.missing",
  conflict: "apply.state.conflict",
  foreign: "apply.state.foreign",
  collision: "apply.state.collision",
  blocked: "apply.state.blocked",
} as const satisfies Record<EntryStateKind, MessageKey>;

/** The class of one line of a unified diff. */
export function diffLineClass(line: string) {
  if (line.startsWith("+++") || line.startsWith("---")) {
    return "apply-diff__line apply-diff__line--meta";
  }
  if (line.startsWith("@@")) {
    return "apply-diff__line apply-diff__line--hunk";
  }
  if (line.startsWith("+")) {
    return "apply-diff__line apply-diff__line--added";
  }
  if (line.startsWith("-")) {
    return "apply-diff__line apply-diff__line--removed";
  }
  return "apply-diff__line";
}
