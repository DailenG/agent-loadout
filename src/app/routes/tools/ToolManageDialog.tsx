// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Dialog shell (header, tabs, search, enabled-only switch, rows, Done) ported from
// DailenG/skilldock@181b52b src/features/skills/components/ToolManageDialog.tsx.
// Only the MCP tab is shown: Skills arrive with the skills milestone, Plugins are hidden (SPEC 13.7).
// It follows the sidebar's scope (SPEC 13.1): at project scope it toggles the client in that
// project's manifest entries with the MCP page's rules (M24: the shown hash, one edit at a time,
// read only until the project is trusted) and lists the inherited store servers read only. A
// row whose file or manifest this account cannot write (M38) shows the lock and its switch is
// disabled with the reason.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useState } from "react";
import { storeQueryKeys, useChangeNotice } from "@/app/apply/changeNotice";
import { Modal } from "@/app/components/Modal";
import { ReadOnlyBadge, ReadOnlyNote, readOnlyReason, readOnlyTooltip } from "@/app/components/ReadOnlyBadge";
import { RecommendedBadge } from "@/app/components/RecommendedBadge";
import { SearchFieldIcon } from "@/app/components/SearchFieldIcon";
import { useNotifications } from "@/app/components/notifications";
import type { ImportTarget } from "@/app/routes/import/model";
import { unmanagedByClient, UnmanagedSection } from "@/app/routes/import/UnmanagedSection";
import { useProjectServers, writableLayers } from "@/app/routes/mcp/ProjectServers";
import { TrustReviewDialog } from "@/app/routes/projects/TrustReviewDialog";
import { useActiveScope } from "@/app/scope/ActiveScope";
import { errorMessage, t } from "@/i18n/en";
import type {
  ClientChoice,
  PillState,
  ProjectServerRow,
  ReadOnly,
  ServerRow,
  ToolInfo,
  UnmanagedSummary,
} from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";

export type ToolManageDialogProps = {
  isOpen: boolean;
  onClose: () => void;
  tool: ToolInfo;
  /** Opens the MCP page (inherited servers are changed there). */
  onOpenMcp: () => void;
  /** Not-managed entries of the sidebar's scope (this client's are listed). */
  unmanaged?: UnmanagedSummary;
  /** Opens the Import page with an entry picked. */
  onOpenImport: (target: ImportTarget) => void;
};

type CapabilityRow = {
  id: string;
  name: string;
  isEnabled: boolean;
  state: PillState;
  /** The client file, or at project scope the entry's manifest, is read only (M38). */
  readOnly: ReadOnly | null;
};

/** The explicit client list of `server`'s pills, with `client` turned on or off. */
function nextChoice(server: ServerRow, client: string, enabled: boolean): ClientChoice {
  const clients = server.pills
    .filter((pill) => (pill.client === client ? enabled : pill.selected))
    .map((pill) => pill.client);
  return { allDetected: false, clients };
}

/**
 * The manifest entry's client list with `client` turned on or off. It starts from the list as
 * written (it may name clients this machine opts out of, which stay), or for `all-detected` from
 * the clients the pills show as selected here, as the MCP page's project cards do.
 */
function nextProjectChoice(row: ProjectServerRow, client: string, enabled: boolean): ClientChoice {
  const written = row.clients.allDetected
    ? row.pills.filter((pill) => pill.selected).map((pill) => pill.client)
    : row.clients.clients;
  const rest = written.filter((id) => id !== client);
  return { allDetected: false, clients: enabled ? [...rest, client] : rest };
}

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

export function ToolManageDialog({ isOpen, onClose, tool, onOpenMcp, unmanaged, onOpenImport }: ToolManageDialogProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const reportChange = useChangeNotice();
  const queryClient = useQueryClient();
  const { scope } = useActiveScope();
  const projectId = scope.kind === "project" ? scope.id : "";
  const inProject = scope.kind === "project";
  const dialogTitleId = useId();
  const [query, setQuery] = useState("");
  const [showEnabledOnly, setShowEnabledOnly] = useState(false);
  const [pendingServerIds, setPendingServerIds] = useState<Set<string>>(new Set());
  // A project edit runs: its key (the entry name), so the row shows it.
  const [projectBusy, setProjectBusy] = useState("");
  const [reviewing, setReviewing] = useState(false);
  const normalizedQuery = query.trim().toLowerCase();
  const servers = useQuery({
    queryKey: storeQueryKeys.servers,
    queryFn: async () => unwrap(await commands.listServers()),
    enabled: isOpen && !inProject,
  });
  const projectServers = useProjectServers(projectId, isOpen && inProject);
  const projectData = inProject ? projectServers.data : undefined;
  const project = projectData?.project ?? null;
  const readOnly = inProject && project?.trust !== "trusted";
  // One project edit at a time: each sends the hash it was listed with, so
  // every toggle waits until the edit is done and the list is fresh again.
  const projectLocked = Boolean(projectBusy) || projectServers.isFetching;
  const source = inProject ? projectServers : servers;

  useEffect(() => {
    if (!isOpen) {
      return;
    }
    setQuery("");
    setShowEnabledOnly(false);
    setPendingServerIds(new Set());
    setReviewing(false);
  }, [isOpen, tool.id, projectId]);

  const listsClient = inProject
    ? (projectData?.clients.some((client) => client.id === tool.id && client.supported) ?? false)
    : (servers.data?.clients.some((client) => client.id === tool.id) ?? false);

  const allRows = useMemo<CapabilityRow[]>(() => {
    if (inProject) {
      const writable = projectData ? writableLayers(projectData.project) : [];
      const manifestReadOnly = (row: ProjectServerRow): ReadOnly | null => {
        if (writable.includes(row.editLayer) || !projectData) {
          return null;
        }
        const layer = projectData.project.readOnlyLayers.find((entry) => entry.layer === row.editLayer);
        return projectData.project.readOnly ?? (layer ? { kind: layer.kind } : null);
      };
      // Suppressions hide a store server; they have no switch for one client here.
      return (projectData?.servers ?? [])
        .filter((row) => row.invalid === null && row.kind !== "suppress")
        .flatMap((row) => {
          const pill = row.pills.find((item) => item.client === tool.id);
          return pill
            ? [{
                id: row.name,
                name: row.name,
                isEnabled: pill.selected,
                state: pill.state,
                readOnly: pill.readOnly ?? manifestReadOnly(row),
              }]
            : [];
        })
        .sort((left, right) => left.name.localeCompare(right.name));
    }
    if (!servers.data) {
      return [];
    }
    return servers.data.servers
      .filter((server) => server.invalid === null)
      .flatMap((server) => {
        const pill = server.pills.find((item) => item.client === tool.id);
        return pill
          ? [{
              id: server.id,
              name: server.displayName ?? server.id,
              isEnabled: pill.selected,
              state: pill.state,
              readOnly: pill.readOnly,
            }]
          : [];
      })
      .sort((left, right) => left.name.localeCompare(right.name));
  }, [inProject, projectData, servers.data, tool.id]);

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
      notify({ tone: "error", message: failureText(error) });
    } finally {
      setPendingServerIds((current) => {
        const next = new Set(current);
        next.delete(serverId);
        return next;
      });
    }
  }

  async function handleToggleProject(name: string, enabled: boolean) {
    const row = projectData?.servers.find((item) => item.name === name);
    if (!row || !project || readOnly || projectLocked) {
      return;
    }
    setProjectBusy(name);
    try {
      const result = unwrap(
        await commands.setProjectServerClients(
          projectId,
          project.hash,
          row.name,
          row.editLayer,
          nextProjectChoice(row, tool.id, enabled),
        ),
      );
      reportChange(
        result,
        t(enabled ? "tools.dialog.enabled" : "tools.dialog.disabled", { server: row.name, name: tool.name }),
      );
      await queryClient.invalidateQueries({ queryKey: [...storeQueryKeys.projectServers, projectId] });
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    } finally {
      setProjectBusy("");
    }
  }

  function rowDisabled(row: CapabilityRow) {
    if (row.state === "not_supported" || row.readOnly) {
      return true;
    }
    return inProject ? readOnly || projectLocked : pendingServerIds.has(row.id);
  }

  if (!isOpen) {
    return null;
  }

  const showRows = source.isSuccess && listsClient;
  const inherited = projectData?.inherited ?? [];

  return (
    <>
      <Modal className="tool-manage-dialog tool-manage-dialog--tools" labelledBy={dialogTitleId} onClose={onClose}>
        <div className="tool-manage-dialog__header">
          <div className="tool-manage-dialog__title">
            <div className="tool-manage-dialog__title-row">
              <h3 id={dialogTitleId}>{t("tools.dialog.title", { name: tool.name })}</h3>
              <RecommendedBadge clientId={tool.id} />
            </div>
            <p>{t("tools.dialog.summary", { enabled: enabledCount, total: allRows.length })}</p>
            {unmanaged ? (
              <p>
                {t("import.counts", {
                  managed: enabledCount,
                  unmanaged: unmanagedByClient(unmanaged).get(tool.id) ?? 0,
                })}
              </p>
            ) : null}
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

        {project ? (
          <p className="skills-scope-note tool-manage-dialog__scope-note" role="note">
            {t("tools.dialog.projectNote", { name: project.name })}
          </p>
        ) : null}
        {inProject && project?.readOnly ? (
          <ReadOnlyNote>
            {t("readOnly.project.whole", { name: project.name, reason: readOnlyReason(project.readOnly.kind) })}
          </ReadOnlyNote>
        ) : null}
        {!inProject
          ? tool.userFiles
              .filter((file) => file.readOnly)
              .map((file) => (
                <ReadOnlyNote key={file.path}>{file.readOnly ? readOnlyTooltip(file.readOnly, file.path) : ""}</ReadOnlyNote>
              ))
          : null}
        {project && readOnly ? (
          <div className="mcp-page-warning mcp-project-trust tool-manage-dialog__trust" role="alert">
            <span>{t(project.trust === "changed" ? "mcp.project.changed" : "mcp.project.untrusted")}</span>
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              onClick={() => setReviewing(true)}
            >
              {t("mcp.project.review")}
            </button>
          </div>
        ) : null}

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
          {source.isPending ? (
            <div className="tool-manage-dialog__empty">
              {t(inProject ? "tools.dialog.loadingProject" : "tools.dialog.loadingMcp")}
            </div>
          ) : null}
          {source.isError ? (
            <div className="tool-manage-dialog__empty">
              {source.error instanceof IpcFailure
                ? errorMessage(source.error.code)
                : t("tools.dialog.loadMcpFailed")}
            </div>
          ) : null}
          {source.isSuccess && !listsClient ? (
            <div className="tool-manage-dialog__empty">
              {t(inProject ? "tools.dialog.noProjectMcp" : "tools.dialog.noMcpSupport", { name: tool.name })}
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
                  <ReadOnlyBadge readOnly={item.readOnly} />
                  <button
                    className={`switch-button${item.isEnabled ? " is-enabled" : ""}${projectBusy === item.id ? " is-loading" : ""}`}
                    type="button"
                    onClick={() =>
                      void (inProject
                        ? handleToggleProject(item.id, !item.isEnabled)
                        : handleToggleMcp(item.id, !item.isEnabled))}
                    aria-pressed={item.isEnabled}
                    aria-label={
                      item.isEnabled
                        ? t("tools.dialog.item.disable", { name: item.name })
                        : t("tools.dialog.item.enable", { name: item.name })
                    }
                    disabled={rowDisabled(item)}
                  >
                    <span className="switch-button__thumb" />
                  </button>
                </div>
              ))
            : null}
          {showRows && activeRows.length === 0 ? (
            <div className="tool-manage-dialog__empty">{t("tools.dialog.emptyMcp")}</div>
          ) : null}
          {showRows && inherited.length > 0 ? (
            <section className="tool-manage-dialog__inherited" aria-labelledby={`${dialogTitleId}-inherited`}>
              <h4 id={`${dialogTitleId}-inherited`} className="apply-drawer__section-title">
                {t("tools.dialog.inherited.title")}
              </h4>
              <p className="apply-drawer__note">{t("tools.dialog.inherited.description")}</p>
              {inherited.map((server) => {
                const name = server.displayName ?? server.id;
                return (
                  <div key={server.id} className="tool-manage-dialog__item is-inherited">
                    <span className="tool-manage-dialog__item-name" title={server.launch}>
                      {name}
                    </span>
                    <button
                      className="secondary-button secondary-button--compact"
                      type="button"
                      aria-label={t("tools.dialog.inherited.openAria", { name })}
                      onClick={onOpenMcp}
                    >
                      {t("tools.dialog.inherited.open")}
                    </button>
                  </div>
                );
              })}
            </section>
          ) : null}
          <UnmanagedSection
            summary={unmanaged}
            clientNames={new Map([[tool.id, tool.name]])}
            client={tool.id}
            onImport={onOpenImport}
          />
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
      </Modal>
      {reviewing && project ? (
        <TrustReviewDialog
          projectId={projectId}
          onClose={() => setReviewing(false)}
          onTrusted={() => {
            setReviewing(false);
            void projectServers.refetch();
          }}
        />
      ) : null}
    </>
  );
}
