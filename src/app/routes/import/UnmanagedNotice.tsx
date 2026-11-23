// The "not managed by Loadout" notice (#9 option A): after the drift
// triggers (watcher, window focus, Refresh) the shell reads the entries not
// acknowledged yet from the shared every-scope summary (`unmanaged.ts`) and
// the skill folders of the user scope, and shows a small banner in the
// sidebar, "omp has 5 MCP servers not managed by Loadout. Review". Review
// opens the Import page; Review and Dismiss both acknowledge what was
// shown, so the same entries never come back. Settings turns it off (on by
// default), kept in the webview's storage like the theme.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { importQueryKeys, storeQueryKeys } from "@/app/apply/changeNotice";
import { SettingsItem } from "@/app/components/SettingsGroup";
import { useUnmanagedSkillCount } from "@/app/routes/import/skills/useUnmanagedSkillCount";
import { t } from "@/i18n/en";
import { BIN_NAME } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";
import { unmanagedAllQuery, useUnmanagedNoticeEnabled } from "./unmanaged";

/** Skill folders the notice has shown (`<client>:<name>:<hash>`); MCP findings are acknowledged in the store's state instead. */
export const SEEN_SKILLS_STORAGE_KEY = `${BIN_NAME}.import.seenSkillFolders`;

function readSeenSkills(): Set<string> {
  try {
    const stored: unknown = JSON.parse(window.localStorage.getItem(SEEN_SKILLS_STORAGE_KEY) ?? "[]");
    return new Set(Array.isArray(stored) ? stored.filter((key): key is string => typeof key === "string") : []);
  } catch {
    return new Set();
  }
}

/** Settings > Preferences: the notice's switch. */
export function UnmanagedNoticeSetting() {
  const [enabled, setEnabled] = useUnmanagedNoticeEnabled();
  return (
    <SettingsItem label={t("import.notice.setting.label")} description={t("import.notice.setting.description")}>
      <input
        type="checkbox"
        checked={enabled}
        aria-label={t("import.notice.setting.label")}
        onChange={(event) => setEnabled(event.target.checked)}
      />
    </SettingsItem>
  );
}

export type UnmanagedNoticeProps = {
  /** The sidebar is collapsed: a compact button stands in for the banner. */
  collapsed: boolean;
  /** Opens the Import page. */
  onReview: () => void;
};

export function UnmanagedNotice({ collapsed, onReview }: UnmanagedNoticeProps) {
  const { commands } = useTransport();
  const queryClient = useQueryClient();
  const [enabled] = useUnmanagedNoticeEnabled();
  const summary = useQuery({ ...unmanagedAllQuery(commands), enabled });
  const skills = useUnmanagedSkillCount(enabled);
  const [seenSkills, setSeenSkills] = useState(readSeenSkills);
  // Acknowledged here, so a slow or failed acknowledgment never shows them again.
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set());

  const entries = (summary.data?.groups ?? []).flatMap((group) =>
    group.entries
      .filter((entry) => entry.new && !dismissed.has(entry.id))
      .map((entry) => ({ client: group.client, id: entry.id })),
  );
  const skillKeys = (skills?.keys ?? []).filter((key) => !seenSkills.has(key));
  const clientIds = [...new Set(entries.map((entry) => entry.client))];
  const tools = useQuery({
    queryKey: storeQueryKeys.tools,
    queryFn: async () => unwrap(await commands.listTools()),
    enabled: enabled && clientIds.length === 1,
  });
  const shown = enabled && (entries.length > 0 || skillKeys.length > 0);

  const lines: string[] = [];
  if (clientIds.length === 1) {
    const client = tools.data?.find((tool) => tool.id === clientIds[0])?.name ?? clientIds[0];
    lines.push(t(entries.length === 1 ? "import.notice.mcpOne" : "import.notice.mcpClient", { client, count: entries.length }));
  } else if (clientIds.length > 1) {
    lines.push(t("import.notice.mcpClients", { count: entries.length, clients: clientIds.length }));
  }
  if (skillKeys.length > 0) {
    lines.push(t(skillKeys.length === 1 ? "import.notice.skillOne" : "import.notice.skills", { count: skillKeys.length }));
  }
  const message = lines.join(" ");

  function acknowledge() {
    const ids = entries.map((entry) => entry.id);
    setDismissed((current) => new Set([...current, ...ids]));
    const seen = new Set([...seenSkills, ...skillKeys]);
    window.localStorage.setItem(SEEN_SKILLS_STORAGE_KEY, JSON.stringify([...seen]));
    setSeenSkills(seen);
    if (ids.length > 0) {
      // The summaries refetch so the "New" badges of the lists clear too.
      void commands.acknowledgeUnmanaged(ids).finally(() => {
        for (const queryKey of [importQueryKeys.unmanagedAll, importQueryKeys.unmanaged]) {
          void queryClient.invalidateQueries({ queryKey });
        }
      });
    }
  }

  function review() {
    acknowledge();
    onReview();
  }

  return (
    <>
      {/* Mounted while the app runs, so a new message is announced. */}
      <p className="sr-only" role="status" aria-live="polite" data-testid="unmanaged-notice-live">
        {shown ? message : ""}
      </p>
      {!shown ? null : collapsed ? (
        <div className="unmanaged-notice unmanaged-notice--compact">
          <button
            className="unmanaged-notice__compact"
            type="button"
            aria-label={t("import.notice.reviewAria", { message })}
            data-tooltip={message}
            onClick={review}
          >
            <span aria-hidden="true">{entries.length + skillKeys.length}</span>
          </button>
        </div>
      ) : (
        <section className="unmanaged-notice" aria-label={t("import.notice.aria")}>
          <p className="unmanaged-notice__message">{message}</p>
          <div className="unmanaged-notice__actions">
            <button className="primary-button primary-button--compact" type="button" onClick={review}>
              {t("import.notice.review")}
            </button>
            <button className="secondary-button secondary-button--compact" type="button" onClick={acknowledge}>
              {t("import.notice.dismiss")}
            </button>
          </div>
        </section>
      )}
    </>
  );
}
