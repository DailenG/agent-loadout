// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Grid layout, card markup and icons ported from DailenG/skilldock@181b52b src/app/routes/tools.tsx;
// the refresh button from src/app/App.tsx. Data and capability summary are new (SPEC 13.7), as
// are the Recommended badge and the "Get" action for an undetected recommended client (#2), and
// the project view: at project scope the cards name that project's files (SPEC 13.1).

import { useIsFetching, useQuery, useQueryClient } from "@tanstack/react-query";
import { memo, useMemo, useState } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { useRecheckTargets } from "@/app/apply/driftTriggers";
import { recommendedFirst } from "@/app/clients";
import { HeaderToolbar } from "@/app/components/HeaderSlots";
import { useNotifications } from "@/app/components/notifications";
import { useEventCallback } from "@/app/components/useEventCallback";
import { ReadOnlyBadge } from "@/app/components/ReadOnlyBadge";
import { RecommendedBadge } from "@/app/components/RecommendedBadge";
import { ToolLogo } from "@/app/components/ToolLogo";
import type { ImportTarget } from "@/app/routes/import/model";
import { unmanagedByClient, useScopeUnmanaged } from "@/app/routes/import/UnmanagedSection";
import { projectRelativePath } from "@/app/routes/projects/labels";
import { ToolManageDialog } from "@/app/routes/tools/ToolManageDialog";
import { useActiveScope } from "@/app/scope/ActiveScope";
import { errorMessage, t } from "@/i18n/en";
import type { ProjectRow, ToolInfo } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";

function FolderOpenIcon() {
  return (
    <svg viewBox="0 0 18 18" fill="none" aria-hidden="true">
      <path
        d="M2.75 5.25A1.5 1.5 0 0 1 4.25 3.75h3.1l1.2 1.35h4.95A1.5 1.5 0 0 1 15 6.6v1.15"
        stroke="currentColor"
        strokeWidth="1.35"
        strokeLinecap="round"
      />
      <path
        d="M3.15 7.75h11.7l-.7 4.7a1.5 1.5 0 0 1-1.48 1.28H4.98a1.5 1.5 0 0 1-1.48-1.28l-.35-2.35"
        stroke="currentColor"
        strokeWidth="1.35"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function RefreshIcon({ spinning }: { spinning: boolean }) {
  return (
    <span aria-hidden="true" className="skills-toolbar-button__icon">
      <svg
        className={spinning ? "skills-toolbar-button__svg is-spinning" : "skills-toolbar-button__svg"}
        viewBox="0 0 20 20"
        fill="none"
      >
        {[
          "M16.2 9.1a6.2 6.2 0 0 0-10.7-3.6",
          "M3.7 3.9v3.7h3.7",
          "M3.8 10.9a6.2 6.2 0 0 0 10.7 3.6",
          "M16.3 16.1v-3.7h-3.7",
        ].map((d) => (
          <path
            key={d}
            d={d}
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        ))}
      </svg>
    </span>
  );
}

function transportSummary(tool: ToolInfo) {
  const names = [
    tool.transports.stdio ? "stdio" : null,
    tool.transports.http ? "HTTP" : null,
    tool.transports.sse ? "SSE" : null,
  ].filter((name) => name !== null);
  return names.length > 0 ? names.join(" / ") : t("tools.capability.none");
}

type ToolCardProps = {
  tool: ToolInfo;
  /** The active project: its files are named under its placeholder path. */
  project: ProjectRow | null;
  isOpening: boolean;
  /** Entries of this client's files in the sidebar's scope not managed by Loadout. */
  unmanagedCount: number;
  openDisabled: boolean;
  /** Stable across renders (the card is memoized), so these take the tool's ID. */
  onOpenFolder: (toolId: string) => void;
  onManage: (toolId: string) => void;
  onGet: (toolId: string) => void;
};

/** Reads the fetching state itself, so a refetch re-renders this button and not every card. */
function ToolsRefreshButton() {
  const queryClient = useQueryClient();
  const recheckTargets = useRecheckTargets();
  const isFetching = useIsFetching({ queryKey: storeQueryKeys.tools }) > 0;
  return (
    <button
      className={`ghost-button tools-refresh-button${isFetching ? " is-loading" : ""}`}
      type="button"
      onClick={() => void recheckTargets().then(() => queryClient.refetchQueries({ queryKey: storeQueryKeys.tools }))}
      disabled={isFetching}
      aria-label={t("tools.refresh")}
    >
      <RefreshIcon spinning={isFetching} />
    </button>
  );
}

const ToolCard = memo(function ToolCard({ tool, project, unmanagedCount, isOpening, openDisabled, onOpenFolder, onManage, onGet }: ToolCardProps) {
  const yesNo = (value: boolean) => (value ? t("tools.capability.yes") : t("tools.capability.no"));

  return (
    <article className="tool-card tool-card--simple" aria-label={tool.name}>
      <div className="tool-card__header">
        <div className="tool-card__header-copy">
          <div className="tool-card__title-row">
            <ToolLogo clientId={tool.id} name={tool.name} />
            <strong>{tool.name}</strong>
            <RecommendedBadge clientId={tool.id} />
            <div className="tool-card__status-badges">
              <span className="status-badge tone-neutral">
                {t("tools.tier", { tier: tool.tier.toUpperCase() })}
              </span>
              {tool.detected ? (
                <span className="status-badge tone-positive">{t("tools.detected")}</span>
              ) : (
                <span className="status-badge tone-neutral">{t("tools.notDetected")}</span>
              )}
              {tool.readOnly ? (
                <span className="status-badge tone-warning">{t("tools.readOnly")}</span>
              ) : null}
              {!tool.supported ? (
                <span className="status-badge tone-warning">{t("tools.unsupportedOs")}</span>
              ) : null}
              {unmanagedCount > 0 ? (
                <span className="status-badge tone-warning">
                  {t("import.unmanaged.toolBadge", { count: unmanagedCount })}
                </span>
              ) : null}
            </div>
          </div>
          {tool.homepage && !tool.detected ? (
            <p className="tool-card__get-hint">{t("tools.getHint", { name: tool.name })}</p>
          ) : null}
        </div>
        <div className="tool-card__header-actions">
          {tool.homepage && !tool.detected ? (
            <button
              className="secondary-button secondary-button--compact tool-card__get-button"
              type="button"
              onClick={() => onGet(tool.id)}
              aria-label={t("tools.getAria", { name: tool.name })}
            >
              {t("tools.get", { name: tool.name })}
            </button>
          ) : null}
          <button
            className="primary-button primary-button--compact tool-card__manage-button"
            type="button"
            onClick={() => onManage(tool.id)}
            aria-label={t("tools.manageAria", { name: tool.name })}
          >
            {t("tools.manage")}
          </button>
        </div>
      </div>
      <div className="tool-card__simple-copy">
        <div className="tool-card__path-row tools-path-row">
          <span className="tool-card__path-label">{t("tools.userFiles")}</span>
          <ul className="tools-path-list" aria-label={t("tools.userFilesAria", { name: tool.name })}>
            {tool.userFiles.length === 0 ? (
              <li className="tool-card__path-value">{t("tools.noUserFiles")}</li>
            ) : (
              tool.userFiles.map((file) => (
                <li
                  key={file.path}
                  className={`tool-card__path-value${file.exists ? "" : " is-missing"}${file.readOnly ? " has-read-only" : ""}`}
                  title={file.path}
                >
                  <span className="tools-path-text">
                    {file.path}
                    {file.exists ? null : (
                      <span className="tools-path-missing">{` (${t("tools.fileMissing")})`}</span>
                    )}
                  </span>
                  <ReadOnlyBadge readOnly={file.readOnly} path={file.path} />
                </li>
              ))
            )}
          </ul>
          <button
            className="tool-card__path-button"
            type="button"
            onClick={() => onOpenFolder(tool.id)}
            aria-label={t("tools.openFolderAria", { name: tool.name })}
            data-tooltip={t("tools.openFolder")}
            disabled={openDisabled || tool.userFiles.length === 0}
          >
            <FolderOpenIcon />
            <span className="sr-only">{isOpening ? t("tools.opening") : t("tools.openFolder")}</span>
          </button>
        </div>
        <div className="tool-card__path-row tools-path-row">
          <span className="tool-card__path-label">
            {project ? t("tools.projectFilesIn", { name: project.name }) : t("tools.projectFiles")}
          </span>
          <ul
            className="tools-path-list"
            aria-label={t("tools.projectFilesAria", { name: tool.name })}
          >
            {tool.projectFiles.length === 0 ? (
              <li className="tool-card__path-value">{t("tools.noProjectFiles")}</li>
            ) : (
              tool.projectFiles.map((path) => {
                // `<P>/...` is relative to the project folder: under the active project's folder, or
                // bare under "In each project" at user scope. Other paths are user files keyed by project.
                const shown = projectRelativePath(path, project?.path);
                // The active project's read-only files (M38) use the same `<P>/...` form.
                const readOnly = project?.readOnlyFiles.find((entry) => entry.file === path) ?? null;
                return (
                  <li key={path} className={`tool-card__path-value${readOnly ? " has-read-only" : ""}`} title={shown}>
                    <span className="tools-path-text">{shown}</span>
                    <ReadOnlyBadge readOnly={readOnly} path={shown} />
                  </li>
                );
              })
            )}
          </ul>
        </div>
        <dl className="tools-capabilities">
          <dt>{t("tools.capability.transports")}</dt>
          <dd>{transportSummary(tool)}</dd>
          <dt>{t("tools.capability.envRefs")}</dt>
          <dd>{yesNo(tool.envRefs)}</dd>
          <dt>{t("tools.capability.project")}</dt>
          <dd>{yesNo(tool.projectFiles.length > 0)}</dd>
          <dt>{t("tools.capability.restart")}</dt>
          <dd>{yesNo(tool.restartRequired)}</dd>
        </dl>
        {tool.warnings.map((warning, index) => (
          <span key={index} className="tools-warning">
            {errorMessage(warning.code)}
          </span>
        ))}
      </div>
    </article>
  );
});

export type ToolsPageProps = {
  /** Opens the MCP page, where inherited servers are changed. */
  onOpenMcp: () => void;
  /** Opens the Import page with an entry picked. */
  onOpenImport: (target?: ImportTarget) => void;
};

export function ToolsPage({ onOpenMcp, onOpenImport }: ToolsPageProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const { project } = useActiveScope();
  const tools = useQuery({
    queryKey: storeQueryKeys.tools,
    queryFn: async () => unwrap(await commands.listTools()),
  });
  const unmanaged = useScopeUnmanaged();
  const unmanagedCounts = useMemo(() => unmanagedByClient(unmanaged.data), [unmanaged.data]);
  // Detected first, then the recommended client, then table order
  // (Array.prototype.sort is stable).
  const sortedTools = useMemo(() => {
    const byRecommended = recommendedFirst<ToolInfo>((tool) => tool.id);
    return [...(tools.data ?? [])].sort(
      (left, right) => Number(right.detected) - Number(left.detected) || byRecommended(left, right),
    );
  }, [tools.data]);
  const [managingToolId, setManagingToolId] = useState("");
  const [openingToolId, setOpeningToolId] = useState("");
  const managingTool = sortedTools.find((tool) => tool.id === managingToolId) ?? null;

  const handleOpenFolder = useEventCallback(async (toolId: string) => {
    if (openingToolId) {
      return;
    }
    setOpeningToolId(toolId);
    try {
      unwrap(await commands.revealToolFolder(toolId));
    } catch (error) {
      notify({
        tone: "error",
        message: error instanceof IpcFailure ? errorMessage(error.code) : t("tools.error.openFolder"),
      });
    } finally {
      setOpeningToolId("");
    }
  });

  const handleGet = useEventCallback(async (toolId: string) => {
    try {
      unwrap(await commands.openToolHomepage(toolId));
    } catch (error) {
      notify({
        tone: "error",
        message: error instanceof IpcFailure ? errorMessage(error.code) : t("tools.error.openHomepage"),
      });
    }
  });

  return (
    <div className="tools-route">
      <HeaderToolbar>
        <ToolsRefreshButton />
      </HeaderToolbar>
      <section className="tools-section">
        {tools.isPending ? <p className="tools-note">{t("tools.loading")}</p> : null}
        {tools.isError ? (
          <div className="panel-card empty-state page-empty-state" role="alert">
            <h3>{t("app.error.loadFailed")}</h3>
            <p>
              {tools.error instanceof IpcFailure
                ? errorMessage(tools.error.code)
                : t("app.error.unknown")}
            </p>
            <button className="secondary-button" type="button" onClick={() => void tools.refetch()}>
              {t("app.action.retry")}
            </button>
          </div>
        ) : null}
        {tools.data && sortedTools.length === 0 ? (
          <div className="panel-card empty-state page-empty-state">
            <h3>{t("page.empty.title")}</h3>
            <p>{t("page.empty.tools")}</p>
          </div>
        ) : null}
        <div className="tool-card-grid">
          {sortedTools.map((tool) => (
            <ToolCard
              key={tool.id}
              tool={tool}
              project={project}
              unmanagedCount={unmanagedCounts.get(tool.id) ?? 0}
              isOpening={openingToolId === tool.id}
              openDisabled={Boolean(openingToolId)}
              onOpenFolder={handleOpenFolder}
              onManage={setManagingToolId}
              onGet={handleGet}
            />
          ))}
        </div>
      </section>
      {managingTool ? (
        <ToolManageDialog
          isOpen
          tool={managingTool}
          onClose={() => setManagingToolId("")}
          unmanaged={unmanaged.data}
          onOpenMcp={() => {
            setManagingToolId("");
            onOpenMcp();
          }}
          onOpenImport={(target) => {
            setManagingToolId("");
            onOpenImport(target);
          }}
        />
      ) : null}
    </div>
  );
}
