// The "Recommended" tag on the recommended client (#2), with a tooltip that
// says why. Renders nothing for any other client.
//
// Two forms (M28c): the text badge on the Tools card and the Manage dialog
// title, focusable so the reason shows on keyboard focus; and a star mark in
// dense chip rows (Skills tabs, skill detail pills, the MCP editor's client
// grid), where the chip's own control takes the focus and names the reason
// with `aria-describedby={reasonId}`.

import { useId } from "react";
import { isRecommended } from "@/app/clients";
import { t } from "@/i18n/en";

type RecommendedBadgeProps = {
  clientId: string;
  /**
   * `mark`: the star in a chip row. The chip's control sets
   * `aria-describedby={reasonId}`; the mark renders the hidden reason with
   * that id.
   */
  variant?: "text" | "mark";
  /** Required with `mark`: the id the chip's control describes itself by. */
  reasonId?: string;
};

export function RecommendedBadge({ clientId, variant = "text", reasonId }: RecommendedBadgeProps) {
  const ownId = useId();
  if (!isRecommended(clientId)) {
    return null;
  }
  const reason = t("recommended.tooltip");
  if (variant === "mark") {
    return (
      <>
        <span
          className="recommended-mark"
          role="img"
          aria-label={t("recommended.badge")}
          data-tooltip={reason}
          data-tooltip-on-parent-focus=""
        >
          <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true" focusable="false">
            <path
              d="M8 1.6l1.9 3.9 4.3.6-3.1 3 .7 4.3L8 11.4l-3.8 2 .7-4.3-3.1-3 4.3-.6z"
              fill="currentColor"
            />
          </svg>
        </span>
        <span id={reasonId} hidden>
          {reason}
        </span>
      </>
    );
  }
  return (
    <>
      <span className="recommended-badge" tabIndex={0} data-tooltip={reason} aria-describedby={ownId}>
        {t("recommended.badge")}
      </span>
      <span id={ownId} hidden>
        {reason}
      </span>
    </>
  );
}

/** The `aria-describedby` a chip control sets next to a `mark` badge: the id for the recommended client only. */
export function recommendedReasonId(baseId: string, clientId: string): string | undefined {
  return isRecommended(clientId) ? `${baseId}-recommended` : undefined;
}
