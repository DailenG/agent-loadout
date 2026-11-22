// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Dialog shell (header, tabs, search, enabled-only switch, rows, Done) ported from
// DailenG/skilldock@181b52b src/features/skills/components/ToolManageDialog.tsx.
// Only the MCP tab is shown: Skills arrive with the skills milestone, Plugins are hidden (SPEC 13.7).

import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useState } from "react";
import { storeQueryKeys, useChangeNotice } from "@/app/apply/changeNotice";
import { SearchFieldIcon } from "@/app/components/SearchFieldIcon";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t } from "@/i18n/en";
import type { ClientChoice, PillState, ServerRow, ToolInfo } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";

export type ToolManageDialogProps = {
  isOpen: boolean;
  onClose: () => void;
  tool: ToolInfo;
};

type CapabilityRow = {
  id: string;
  name: string;
  isEnabled: boolean;
  state: PillState;
};

/** The explicit client list of `server`'s pills, with `client` turned on or off. */
function nextChoice(server: ServerRow, client: string, enabled: boolean): ClientChoice {
  const clients = server.pills
    .filter((pill) => (pill.client === client ? enabled : pill.selected))
    .map((pill) => pill.client);
  return { allDetected: false, clients };
}

export function ToolManageDialog({ isOpen, onClose, tool }: ToolManageDialogProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const reportChange = useChangeNotice();
  const dialogTitleId = useId();
  const [query, setQuery] = useState("");
  const [showEnabledOnly, setShowEnabledOnly] = useState(false);
  const [pendingServerIds, setPendingServerIds] = useState<Set<string>>(new Set());
  const normalizedQuery = query.trim().toLowerCase();
  const servers = useQuery({
    queryKey: storeQueryKeys.servers,
    queryFn: async () => unwrap(await commands.listServers()),
    enabled: isOpen,
  });

  useEffect(() => {
    if (!isOpen) {
      return;
    }
    setQuery("");
    setShowEnabledOnly(false);
    setPendingServerIds(new Set());
  }, [isOpen, tool.id]);

  useEffect(() => {
    if (!isOpen) {
      return;
    }
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, onClose]);

  const listsClient = servers.data?.clients.some((client) => client.id === tool.id) ?? false;

  const allRows = useMemo<CapabilityRow[]>(() => {
    if (!servers.data) {
      return [];
    }
    return servers.data.servers
      .filter((server) => server.invalid === null)
      .flatMap((server) => {
        const pill = server.pills.find((item) => item.client === tool.id);
        return pill
          ? [{ id: server.id, name: server.displayName ?? server.id, isEnabled: pill.selected, state: pill.state }]
          : [];
      })
      .sort((left, right) => left.name.localeCompare(right.name));
  }, [servers.data, tool.id]);

  const activeRows = allRows
    .filter((row) => row.name.toLowerCase().includes(normalizedQuery) || row.id.includes(normalizedQuery))
    .filter((row) => (showEnabledOnly ? row.isEnabled : true));
  const enabledCount = allRows.filter((row) => row.isEnabled).length;

  async function handleToggleMcp(serverId: string, enabled: boolean) {
    const server = servers.data?.servers.find((item) => item.id === serverId);
    if (!server || pendingServerIds.has(serverId)) {
      return;
    }
    setPendingServerIds((current) => new Set(current).add(serverId));
    try {
      const result = unwrap(
        await commands.setServerClients(serverId, nextChoice(server, tool.id, enabled)),
      );
      reportChange(
        result,
        t(enabled ? "tools.dialog.enabled" : "tools.dialog.disabled", {
          server: server.displayName ?? server.id,
          name: tool.name,
        }),
      );
    } catch (error) {
      notify({
        tone: "error",
        message: error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown"),
      });
    } finally {
      setPendingServerIds((current) => {
        const next = new Set(current);
        next.delete(serverId);
        return next;
      });
    }
  }

  if (!isOpen) {
    return null;
  }

  const showRows = servers.isSuccess && listsClient;

  return (
    <div className="dialog-backdrop" role="presentation" onClick={onClose}>
      <div
        className="tool-manage-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={dialogTitleId}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="tool-manage-dialog__header">
          <div className="tool-manage-dialog__title">
            <h3 id={dialogTitleId}>{t("tools.dialog.title", { name: tool.name })}</h3>
            <p>{t("tools.dialog.summary", { enabled: enabledCount, total: allRows.length })}</p>
          </div>
          <button
            className="tool-manage-dialog__close"
            type="button"
            onClick={onClose}
            aria-label={t("tools.dialog.close")}
          >
            ×
          </button>
        </div>

        <div className="tool-manage-dialog__tabs" role="tablist" aria-label={t("tools.dialog.tabs")}>
          <button className="tool-manage-dialog__tab is-selected" type="button" role="tab" aria-selected>
            MCP
          </button>
        </div>

        <div className="tool-manage-dialog__toolbar">
          <div className="tool-manage-dialog__search-wrap">
            <SearchFieldIcon />
            <input
              className="tool-manage-dialog__search"
              type="search"
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="none"
              spellCheck={false}
              placeholder={t("tools.dialog.searchMcp")}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
          <label className="tool-manage-dialog__toggle">
            <button
              className={`switch-button${showEnabledOnly ? " is-enabled" : ""}`}
              type="button"
              onClick={() => setShowEnabledOnly((current) => !current)}
              aria-pressed={showEnabledOnly}
              aria-label={t("tools.dialog.enabledOnly")}
            >
              <span className="switch-button__thumb" />
            </button>
            <span>{t("tools.dialog.enabledOnly")}</span>
          </label>
        </div>

        <div className="tool-manage-dialog__list">
          {servers.isPending ? (
            <div className="tool-manage-dialog__empty">{t("tools.dialog.loadingMcp")}</div>
          ) : null}
          {servers.isError ? (
            <div className="tool-manage-dialog__empty">
              {servers.error instanceof IpcFailure
                ? errorMessage(servers.error.code)
                : t("tools.dialog.loadMcpFailed")}
            </div>
          ) : null}
          {servers.isSuccess && !listsClient ? (
            <div className="tool-manage-dialog__empty">
              {t("tools.dialog.noMcpSupport", { name: tool.name })}
            </div>
          ) : null}
          {showRows
            ? activeRows.map((item) => (
                <div
                  key={item.id}
                  className={`tool-manage-dialog__item${item.isEnabled ? " is-enabled" : ""}`}
                >
                  <span className="tool-manage-dialog__item-name" title={item.name}>
                    {item.name}
                  </span>
                  <button
                    className={`switch-button${item.isEnabled ? " is-enabled" : ""}`}
                    type="button"
                    onClick={() => void handleToggleMcp(item.id, !item.isEnabled)}
                    aria-pressed={item.isEnabled}
                    aria-label={
                      item.isEnabled
                        ? t("tools.dialog.item.disable", { name: item.name })
                        : t("tools.dialog.item.enable", { name: item.name })
                    }
                    disabled={pendingServerIds.has(item.id) || item.state === "not_supported"}
                  >
                    <span className="switch-button__thumb" />
                  </button>
                </div>
              ))
            : null}
          {showRows && activeRows.length === 0 ? (
            <div className="tool-manage-dialog__empty">{t("tools.dialog.emptyMcp")}</div>
          ) : null}
        </div>

        <div className="tool-manage-dialog__actions">
          <button
            className="primary-button primary-button--compact tool-manage-dialog__done-button"
            type="button"
            onClick={onClose}
          >
            {t("tools.dialog.done")}
          </button>
        </div>
      </div>
    </div>
  );
}
