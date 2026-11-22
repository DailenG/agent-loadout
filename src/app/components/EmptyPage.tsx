// Placeholder body of a page whose content lands in a later milestone.

import { t } from "@/i18n/en";

export function EmptyPage({ message }: { message: string }) {
  return (
    <div className="panel-card empty-state page-empty-state">
      <h3>{t("page.empty.title")}</h3>
      <p>{message}</p>
    </div>
  );
}
