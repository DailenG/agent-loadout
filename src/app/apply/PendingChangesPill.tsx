// Header pill for pending changes (SPEC 13.1): shown only while entries wait
// for an apply, a decision, or are blocked; opens the apply drawer.

import { useQuery } from "@tanstack/react-query";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { t } from "@/i18n/en";
import { unwrap, useTransport } from "@/ipc/transport";

export type PendingChangesPillProps = {
  onOpen: () => void;
};

export function PendingChangesPill({ onOpen }: PendingChangesPillProps) {
  const { commands } = useTransport();
  const plan = useQuery({
    queryKey: storeQueryKeys.pendingPlan,
    queryFn: async () => unwrap(await commands.pendingPlan()),
  });
  const pending = plan.data?.pending ?? 0;
  if (pending <= 0) {
    return null;
  }
  const review = plan.data?.applyMode === "review";

  return (
    <button
      className={`apply-pending-pill${review ? " is-review" : ""}`}
      type="button"
      onClick={onOpen}
      aria-label={t("apply.pill.aria", { count: pending })}
      data-tooltip={review ? t("apply.pill.reviewHint") : t("apply.pill.tooltip")}
    >
      <span className="apply-pending-pill__dot" aria-hidden="true" />
      <span>{t("apply.pill.count", { count: pending })}</span>
      {review ? <span className="apply-pending-pill__mode">{t("apply.pill.review")}</span> : null}
    </button>
  );
}
