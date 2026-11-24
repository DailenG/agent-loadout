// The "Read only" indicator (M38, #4): a lock and a label, with a tooltip
// that says in plain words why this account cannot write the file or folder
// and names its placeholder path. Loadout shows read-only contexts and never
// writes them; the controls that would write are disabled with the same text
// (`readOnlyTooltip`), never hidden.
//
// Two forms, as the Recommended badge: `text` (lock + "Read only"), focusable
// so the reason shows on keyboard focus and is read by screen readers; and
// `mark` (the lock alone) in dense chip rows, where the chip's own control
// takes the focus and names the reason with `aria-describedby={reasonId}`.

import { useId } from "react";
import { t, type MessageKey } from "@/i18n/en";
import type { ReadOnly, ReadOnlyKind } from "@/ipc/bindings";

const KIND_KEYS = {
  acl: "readOnly.kind.acl",
  attribute: "readOnly.kind.attribute",
  owner: "readOnly.kind.owner",
  share: "readOnly.kind.share",
} as const satisfies Record<ReadOnlyKind, MessageKey>;

/** Why the file or folder is read only, as the end of a sentence ("it is marked read only"). */
export function readOnlyReason(kind: ReadOnlyKind): string {
  return t(KIND_KEYS[kind]);
}

/** The whole explanation: the path (when given), the reason, and that Loadout leaves it as it is. */
export function readOnlyTooltip(readOnly: ReadOnly, path?: string | null): string {
  const reason = readOnlyReason(readOnly.kind);
  return path ? t("readOnly.tooltipPath", { path, reason }) : t("readOnly.tooltip", { reason });
}

export function LockIcon() {
  return (
    <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true" focusable="false">
      <rect x="3" y="7" width="10" height="7" rx="1.5" stroke="currentColor" strokeWidth="1.5" fill="none" />
      <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" stroke="currentColor" strokeWidth="1.5" fill="none" />
    </svg>
  );
}

type ReadOnlyBadgeProps = {
  readOnly: ReadOnly | null | undefined;
  /** The file or folder, under its placeholder root; named in the tooltip. */
  path?: string | null;
  /** Replaces the default tooltip (a project, several clients). */
  tooltip?: string;
  /** Replaces the default label "Read only". */
  label?: string;
  variant?: "text" | "mark";
  /** Required with `mark`: the id the chip's control describes itself by. */
  reasonId?: string;
  /** `text` only: false inside another control (a card's summary button), so there is no nested focus stop. */
  focusable?: boolean;
};

export function ReadOnlyBadge({ readOnly, path, tooltip, label, variant = "text", reasonId, focusable = true }: ReadOnlyBadgeProps) {
  const ownId = useId();
  if (!readOnly) {
    return null;
  }
  const reason = tooltip ?? readOnlyTooltip(readOnly, path);
  const text = label ?? t("readOnly.label");
  if (variant === "mark") {
    return (
      <>
        <span
          className="read-only-mark"
          role="img"
          aria-label={text}
          data-tooltip={reason}
          data-tooltip-on-parent-focus=""
        >
          <LockIcon />
        </span>
        {reasonId ? (
          <span id={reasonId} hidden>
            {reason}
          </span>
        ) : null}
      </>
    );
  }
  return (
    <>
      <span
        className="status-badge tone-warning read-only-badge"
        tabIndex={focusable ? 0 : undefined}
        data-tooltip={reason}
        aria-describedby={ownId}
      >
        <LockIcon />
        <span>{text}</span>
      </span>
      <span id={ownId} hidden>
        {reason}
      </span>
    </>
  );
}

/** A line of explanation next to controls a read-only target disables. */
export function ReadOnlyNote({ id, children }: { id?: string; children: string }) {
  return (
    <p className="read-only-note" role="note" id={id}>
      <LockIcon />
      <span>{children}</span>
    </p>
  );
}
