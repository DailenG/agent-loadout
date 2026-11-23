// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Toolbar, server card markup, monogram, detail container, two-click delete and empty state
// ported from DailenG/skilldock@181b52b src/app/routes/mcp.tsx; the data layer is new
// (typed commands over TanStack Query, SPEC 13.2), and so is the project scope (M24).

import { useQuery } from "@tanstack/react-query";
import { createPortal } from "react-dom";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { storeQueryKeys, useChangeNotice } from "@/app/apply/changeNotice";
import { RefreshButton } from "@/app/apply/RefreshButton";
import { HeaderToolbar } from "@/app/components/HeaderSlots";
import { ListGridViewToggle, type ListGridViewMode } from "@/app/components/ListGridViewToggle";
import { useNotifications } from "@/app/components/notifications";
import { SearchFieldIcon } from "@/app/components/SearchFieldIcon";
import { ScopeSelect, USER_SCOPE } from "@/app/components/ScopeSelect";
import { ToolbarGoInstallButton } from "@/app/components/ToolbarGoInstallButton";
import { errorMessage, t } from "@/i18n/en";
import type { ClientInfo, ServerRow, TransportKind } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import {
  ClientPill,
  DISABLE_ALL_CHOICE,
  enableAllChoice,
  useSetServerClients,
  visiblePills,
  type VisiblePill,
} from "./ClientPill";
import { AddIcon, DeleteIcon, EditIcon, McpServerMonogram, PasteIcon, PowerToggleIcon } from "./icons";
import { ProjectServersView, useProjectServers } from "./ProjectServers";
import type { ProjectDialogState } from "./ProjectServerDialog";
import { ServerEditDialog, type EditTab } from "./ServerEditDialog";

export type McpPageProps = {
  /** Opens the Pending changes drawer (drift is resolved there). */
  onOpenPending: () => void;
  /** Opens the Install page. */
  onOpenInstall: () => void;
};

type DialogState =
  | { mode: "create"; focusPaste: boolean }
  | { mode: "edit"; serverId: string; tab: EditTab };

const TRANSPORT_LABELS = {
  stdio: "mcp.transport.stdio",
  http: "mcp.transport.http",
  sse: "mcp.transport.sse",
} as const satisfies Record<TransportKind, string>;

function serverName(row: ServerRow): string {
  return row.displayName ?? row.id;
}

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

type DetailContainerProps = {
  children: ReactNode;
  isGridLayout: boolean;
  row: ServerRow;
  active: boolean;
  onClose: () => void;
  onEdit: () => void;
};

/** Inline details in list view; a modal in grid view. */
function McpServerDetailContainer({ children, isGridLayout, row, active, onClose, onEdit }: DetailContainerProps) {
  useEffect(() => {
    if (!isGridLayout) {
      return;
    }
    const previousOverflow = document.body.style.overflow;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    document.body.style.overflow = "hidden";
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      document.body.style.overflow = previousOverflow;
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [isGridLayout, onClose]);

  if (!isGridLayout) {
    return <div className="mcp-server-card__details">{children}</div>;
  }

  const name = serverName(row);
  return createPortal(
    <div className="skill-card-detail-modal__backdrop" role="presentation" onClick={onClose}>
      <section
        className="skill-card-detail-modal mcp-server-card__detail-modal"
        role="dialog"
        aria-modal="true"
        aria-label={`${t("mcp.card.basicInfo")} ${name}`}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="skill-card-detail-modal__header">
          <div className="skill-card-detail-modal__identity">
            <McpServerMonogram name={serverName(row)} active={active} />
            <div className="skill-card-detail-modal__copy">
              <div className="skill-card-detail-modal__title">
                <h3>{name}</h3>
              </div>
            </div>
          </div>
          <div className="skill-card-detail-modal__actions">
            <button
              className="secondary-button secondary-button--compact skill-card-detail-modal__action is-primary"
              type="button"
              onClick={onEdit}
              aria-label={t("mcp.card.edit", { name })}
            >
              <EditIcon />
              <span>{t("mcp.card.editTooltip")}</span>
            </button>
            <button
              className="skill-card-detail-modal__close"
              type="button"
              onClick={onClose}
              aria-label={t("mcp.card.collapseAria", { name })}
            >
              <span aria-hidden="true">×</span>
            </button>
          </div>
        </header>
        <div className="mcp-server-card__details skill-card-detail-modal__body">{children}</div>
      </section>
    </div>,
    document.body,
  );
}

/**
 * The MCP page (SPEC 13.2): one card per store server with its client pills, or at project
 * scope the servers of that project's manifests.
 */
export function McpPage({ onOpenPending, onOpenInstall }: McpPageProps) {
  const { commands } = useTransport();
  const changeNotice = useChangeNotice();
  const setServerClients = useSetServerClients();
  const { notify } = useNotifications();
  const servers = useQuery({
    queryKey: storeQueryKeys.servers,
    queryFn: async () => unwrap(await commands.listServers()),
  });
  const [query, setQuery] = useState("");
  const [viewMode, setViewMode] = useState<ListGridViewMode>("list");
  const [expandedId, setExpandedId] = useState("");
  const [deleteConfirmingId, setDeleteConfirmingId] = useState("");
  const [deletingId, setDeletingId] = useState("");
  const [bulkId, setBulkId] = useState("");
  const [dialog, setDialog] = useState<DialogState | null>(null);
  const [scopeKey, setScopeKey] = useState(USER_SCOPE);
  const inProject = scopeKey !== USER_SCOPE;
  const [projectDialog, setProjectDialog] = useState<ProjectDialogState | null>(null);
  const projectServers = useProjectServers(scopeKey, inProject);
  const [projectBusy, setProjectBusy] = useState(false);
  const deleteActionRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (!deleteConfirmingId) {
      return;
    }
    function handlePointerDown(event: PointerEvent) {
      if (!deleteActionRef.current?.contains(event.target as Node)) {
        setDeleteConfirmingId("");
      }
    }
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setDeleteConfirmingId("");
      }
    }
    document.addEventListener("pointerdown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [deleteConfirmingId]);

  const list = servers.data;
  const clients: ClientInfo[] = list?.clients ?? [];
  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const rows = list?.servers ?? [];
    if (!needle) {
      return rows;
    }
    return rows.filter((row) =>
      [row.id, row.displayName ?? "", row.launch].some((field) => field.toLowerCase().includes(needle)),
    );
  }, [list, query]);

  async function handleDelete(row: ServerRow) {
    if (deletingId) {
      return;
    }
    if (deleteConfirmingId !== row.id) {
      setDeleteConfirmingId(row.id);
      return;
    }
    setDeleteConfirmingId("");
    setDeletingId(row.id);
    try {
      changeNotice(unwrap(await commands.removeServers([row.id])), t("mcp.deleted", { name: serverName(row) }));
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    } finally {
      setDeletingId("");
    }
  }

  async function handleToggleAll(row: ServerRow, enable: boolean) {
    setBulkId(row.id);
    try {
      await setServerClients(
        row.id,
        enable ? enableAllChoice(row, clients) : DISABLE_ALL_CHOICE,
        t(enable ? "mcp.toggledAll.on" : "mcp.toggledAll.off", { name: serverName(row) }),
      );
    } finally {
      setBulkId("");
    }
  }

  const toolbar = (
    <section className="mcp-toolbar skills-header-bar__tools" aria-label={t("mcp.toolbar.aria")}>
      <label className="search-field search-field--header mcp-toolbar__search">
        <span className="sr-only">{t("mcp.toolbar.search")}</span>
        <SearchFieldIcon />
        <input
          type="search"
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="none"
          spellCheck={false}
          aria-label={t("mcp.toolbar.search")}
          placeholder={t("mcp.toolbar.searchPlaceholder")}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>
      <ScopeSelect
        value={scopeKey}
        ariaLabel={t("mcp.scope.aria")}
        userLabel={t("mcp.scope.user")}
        projectLabel={(name) => t("mcp.scope.project", { name })}
        onChange={(value) => {
          setScopeKey(value);
          setExpandedId("");
          setDialog(null);
          setProjectDialog(null);
        }}
      />
      {inProject ? null : <ListGridViewToggle value={viewMode} onChange={setViewMode} />}
      <RefreshButton label={t("mcp.toolbar.refresh")} />
      {inProject ? null : (
        <button
          className="secondary-button secondary-button--compact skills-toolbar-button"
          type="button"
          onClick={() => setDialog({ mode: "create", focusPaste: true })}
        >
          <span aria-hidden="true" className="skills-toolbar-button__icon">
            <PasteIcon />
          </span>
          <span>{t("mcp.toolbar.paste")}</span>
        </button>
      )}
      <button
        className="secondary-button secondary-button--compact skills-toolbar-button"
        type="button"
        onClick={() =>
          inProject ? setProjectDialog({ mode: "create" }) : setDialog({ mode: "create", focusPaste: false })}
        // An untrusted or changed project is read only until it is trusted;
        // a running edit or refetch may make the shown hash stale.
        disabled={
          inProject
          && (projectServers.data?.project.trust !== "trusted" || projectServers.isFetching || projectBusy)
        }
      >
        <span aria-hidden="true" className="skills-toolbar-button__icon">
          <AddIcon />
        </span>
        <span>{t("mcp.toolbar.add")}</span>
      </button>
      <ToolbarGoInstallButton onClick={onOpenInstall} />
    </section>
  );

  if (inProject) {
    return (
      <div className="mcp-route">
        <HeaderToolbar>{toolbar}</HeaderToolbar>
        <ProjectServersView
          projectId={scopeKey}
          query={query}
          onOpenPending={onOpenPending}
          dialog={projectDialog}
          onDialog={setProjectDialog}
          onBusy={setProjectBusy}
        />
      </div>
    );
  }

  return (
    <div className="mcp-route">
      <HeaderToolbar>{toolbar}</HeaderToolbar>

      {servers.isError ? (
        <div className="dialog-error mcp-page-error" role="alert">
          <span>{failureText(servers.error)}</span>
          <button className="secondary-button secondary-button--compact" type="button" onClick={() => void servers.refetch()}>
            {t("app.action.retry")}
          </button>
        </div>
      ) : null}
      {list?.warnings.map((warning, index) => (
        <p key={`${index}-${warning.code}`} className="mcp-page-warning" role="note">
          {errorMessage(warning.code)}
        </p>
      ))}
      {servers.isPending ? <p className="mcp-page-loading">{t("mcp.loading")}</p> : null}

      <section className={`mcp-server-list card-list${viewMode === "grid" ? " tool-card-grid" : ""}`}>
        {filtered.map((row) => {
          const name = serverName(row);
          const isGrid = viewMode === "grid";
          const isExpanded = expandedId === row.id && !row.invalid;
          const pills = visiblePills(row, clients);
          const enabledPills = pills.filter((entry) => entry.pill.selected);
          const readyPills = pills.filter(
            (entry: VisiblePill) => entry.client.detected && entry.client.supported && entry.pill.state !== "not_supported",
          );
          const allReadyEnabled = readyPills.length > 0 && readyPills.every((entry) => entry.pill.selected);
          const partial = enabledPills.length > 0 && !allReadyEnabled;
          const powerTooltip = allReadyEnabled
            ? t("mcp.card.disableAllApps", { name })
            : partial
              ? t("mcp.card.bulkToggle.partial", { enabled: enabledPills.length, total: readyPills.length })
              : t("mcp.card.enableAllApps", { name });
          const isBulk = bulkId === row.id;
          const isDeleteConfirming = deleteConfirmingId === row.id;
          const isDeleting = deletingId === row.id;
          const enabledSummary = enabledPills.length > 0
            ? t("mcp.summary.enabledApps", { apps: enabledPills.map((entry) => entry.client.name).join(", ") })
            : t("mcp.summary.disabled");
          const subtitle = row.invalid
            ? errorMessage(row.invalid.code)
            : row.description ?? row.launch;

          const deleteButton = isDeleteConfirming || isDeleting ? (
            <button
              ref={deleteActionRef}
              className="skill-card__delete-confirm-button"
              type="button"
              onClick={() => void handleDelete(row)}
              aria-label={t("mcp.card.deleteConfirmAria", {
                state: isDeleting ? t("mcp.card.deleting") : t("mcp.card.deleteConfirm"),
                name,
              })}
              data-tooltip={isDeleting ? t("mcp.card.deleting") : t("mcp.card.deleteConfirmTooltip")}
              disabled={isDeleting}
            >
              {isDeleting ? t("mcp.card.deleting") : t("mcp.card.deleteConfirm")}
            </button>
          ) : (
            <button
              className="skill-card__icon-button skill-card__icon-button--delete"
              type="button"
              onClick={() => void handleDelete(row)}
              aria-label={t("mcp.card.delete", { name })}
              data-tooltip={t("mcp.card.deleteTooltip")}
              disabled={Boolean(deletingId)}
            >
              <DeleteIcon />
            </button>
          );

          const editButton = (
            <button
              className="skill-card__icon-button"
              type="button"
              onClick={() => setDialog({ mode: "edit", serverId: row.id, tab: row.invalid ? "toml" : "form" })}
              aria-label={t("mcp.card.edit", { name })}
              data-tooltip={t("mcp.card.editTooltip")}
              disabled={Boolean(deletingId)}
            >
              <EditIcon />
            </button>
          );

          return (
            <article
              key={row.id}
              className={`mcp-server-card${isGrid ? " mcp-server-card--grid" : ""}${isExpanded ? " is-expanded" : ""}${row.invalid ? " is-invalid" : ""}`}
              aria-label={name}
            >
              <div className="mcp-server-card__header">
                <div
                  className="mcp-server-card__summary-button"
                  role={row.invalid ? undefined : "button"}
                  tabIndex={row.invalid ? undefined : 0}
                  onClick={() => {
                    if (!row.invalid) {
                      setExpandedId(isExpanded ? "" : row.id);
                    }
                  }}
                  onKeyDown={(event) => {
                    if (!row.invalid && (event.key === "Enter" || event.key === " ")) {
                      event.preventDefault();
                      setExpandedId(isExpanded ? "" : row.id);
                    }
                  }}
                  aria-expanded={row.invalid ? undefined : isExpanded}
                  aria-label={row.invalid
                    ? undefined
                    : t(isExpanded ? "mcp.card.collapseAria" : "mcp.card.expandAria", { name })}
                >
                  <div className="mcp-server-card__main">
                    <div className="mcp-server-card__identity">
                      <McpServerMonogram name={name} active={enabledPills.length > 0} />
                      <div className="mcp-server-card__title-stack">
                        <div className="mcp-server-card__title-row">
                          <strong>{name}</strong>
                          {row.invalid ? (
                            <span className="status-badge tone-danger">{t("mcp.card.invalid")}</span>
                          ) : (
                            <>
                              {row.transport ? (
                                <span className="status-badge tone-neutral mcp-server-card__tool-summary-badge">
                                  {t(TRANSPORT_LABELS[row.transport])}
                                </span>
                              ) : null}
                              <span
                                className={`status-badge ${enabledPills.length > 0 ? "tone-info" : "tone-neutral"} skill-card__enabled-toggle${enabledPills.length > 0 ? "" : " is-empty"}`}
                                aria-label={enabledSummary}
                                data-tooltip={enabledSummary}
                              >
                                {enabledPills.length > 0
                                  ? t("mcp.summary.enabledCount", { count: enabledPills.length })
                                  : t("mcp.summary.disabled")}
                              </span>
                            </>
                          )}
                          <span className="status-badge tone-neutral mcp-layer-chip">{t("scope.user")}</span>
                        </div>
                        <div className="mcp-server-card__subtitle">{subtitle}</div>
                        {row.problems.map((problem, index) => (
                          <div key={`${index}-${problem.code}`} className="mcp-server-card__problem" role="note">
                            <span aria-hidden="true">!</span> {errorMessage(problem.code)}
                          </div>
                        ))}
                      </div>
                    </div>
                  </div>
                </div>
                <div className="skill-card__list-actions mcp-server-card__actions">
                  {isGrid && row.transport ? (
                    <span className="skill-card__grid-source-label">{t(TRANSPORT_LABELS[row.transport])}</span>
                  ) : null}
                  {row.invalid ? null : (
                    <button
                      className={`skill-card__icon-button plugins-page__toggle-icon-button ${allReadyEnabled ? "is-enabled" : partial ? "is-partial" : "is-disabled"}`}
                      type="button"
                      onClick={() => void handleToggleAll(row, !allReadyEnabled)}
                      aria-label={powerTooltip}
                      data-tooltip={powerTooltip}
                      disabled={isBulk || isDeleting || (readyPills.length === 0 && enabledPills.length === 0)}
                    >
                      <PowerToggleIcon isSpinning={isBulk} />
                    </button>
                  )}
                  {editButton}
                  {deleteButton}
                  {isGrid || row.invalid ? null : (
                    <span className="skill-card__chevron mcp-server-card__chevron" aria-hidden="true">
                      {isExpanded ? "⌄" : "›"}
                    </span>
                  )}
                </div>
              </div>
              {isExpanded ? (
                <McpServerDetailContainer
                  isGridLayout={isGrid}
                  row={row}
                  active={enabledPills.length > 0}
                  onClose={() => setExpandedId("")}
                  onEdit={() => {
                    setExpandedId("");
                    setDialog({ mode: "edit", serverId: row.id, tab: "form" });
                  }}
                >
                  <section>
                    <div className="skill-card__section-header">
                      <h4>{t("mcp.card.basicInfo")}</h4>
                    </div>
                    <dl className="detail-grid detail-grid--single">
                      <div>
                        <dt>{t("mcp.card.description")}</dt>
                        <dd>{row.description ?? t("mcp.card.noDescription")}</dd>
                      </div>
                    </dl>
                    <dl className="detail-grid detail-grid--single">
                      <div>
                        <dt>{t("mcp.card.command")}</dt>
                        <dd className="detail-grid__single-line" data-tooltip={row.launch}>{row.launch}</dd>
                      </div>
                    </dl>
                    <dl className="detail-grid detail-grid--mcp-meta">
                      <div>
                        <dt>{t("mcp.card.origin")}</dt>
                        <dd className="detail-grid__single-line">{row.origin ?? t("mcp.card.originNone")}</dd>
                      </div>
                      <div>
                        <dt>{t("mcp.card.scope")}</dt>
                        <dd className="detail-grid__single-line">{t("scope.user")}</dd>
                      </div>
                    </dl>
                  </section>
                  <section>
                    <div className="skill-card__section-header">
                      <h4>{t("mcp.card.enableApps")}</h4>
                      {readyPills.length > 0 || enabledPills.length > 0 ? (
                        <div className="tool-sync-panel__actions">
                          <button
                            className="secondary-button secondary-button--compact semantic-action semantic-action--enable"
                            type="button"
                            onClick={() => void handleToggleAll(row, true)}
                            disabled={isBulk || allReadyEnabled}
                            aria-label={t("mcp.card.enableAllApps", { name })}
                          >
                            {t("mcp.card.enableAll")}
                          </button>
                          <button
                            className="secondary-button secondary-button--compact semantic-action semantic-action--disable"
                            type="button"
                            onClick={() => void handleToggleAll(row, false)}
                            disabled={isBulk || enabledPills.length === 0}
                            aria-label={t("mcp.card.disableAllApps", { name })}
                          >
                            {t("mcp.card.disableAll")}
                          </button>
                        </div>
                      ) : null}
                    </div>
                    {pills.length > 0 ? (
                      <div className="tool-pill-grid">
                        {pills.map((entry) => (
                          <ClientPill
                            key={entry.client.id}
                            row={row}
                            entry={entry}
                            busy={isBulk}
                            onOpenPending={onOpenPending}
                          />
                        ))}
                      </div>
                    ) : (
                      <p className="mcp-server-card__tool-empty">{t("mcp.card.noClients")}</p>
                    )}
                  </section>
                </McpServerDetailContainer>
              ) : null}
            </article>
          );
        })}
        {list && filtered.length === 0 ? (
          <div className="panel-card empty-state">
            {list.servers.length === 0 ? (
              <>
                <h3>{t("mcp.empty.title")}</h3>
                <p>{t("mcp.empty.description")}</p>
                <div className="empty-state__actions">
                  <button
                    className="primary-button"
                    type="button"
                    onClick={() => setDialog({ mode: "create", focusPaste: false })}
                  >
                    {t("mcp.empty.add")}
                  </button>
                  <button
                    className="secondary-button"
                    type="button"
                    onClick={() => setDialog({ mode: "create", focusPaste: true })}
                  >
                    {t("mcp.empty.paste")}
                  </button>
                </div>
              </>
            ) : (
              <>
                <h3>{t("mcp.empty.noMatchTitle")}</h3>
                <p>{t("mcp.empty.noMatchDescription")}</p>
              </>
            )}
          </div>
        ) : null}
      </section>

      {dialog ? (
        <ServerEditDialog
          mode={dialog.mode}
          serverId={dialog.mode === "edit" ? dialog.serverId : undefined}
          initialTab={dialog.mode === "edit" ? dialog.tab : "form"}
          focusPaste={dialog.mode === "create" && dialog.focusPaste}
          clients={clients}
          onClose={() => setDialog(null)}
        />
      ) : null}
    </div>
  );
}
