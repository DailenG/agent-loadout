// The "Recommended" tag on the recommended client (#2), with a tooltip that
// says why. Renders nothing for any other client.

import { isRecommended } from "@/app/clients";
import { t } from "@/i18n/en";

export function RecommendedBadge({ clientId, className = "" }: { clientId: string; className?: string }) {
  if (!isRecommended(clientId)) {
    return null;
  }
  return (
    <span className={`recommended-badge${className ? ` ${className}` : ""}`} data-tooltip={t("recommended.tooltip")}>
      {t("recommended.badge")}
    </span>
  );
}
