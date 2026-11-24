// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Toolbar, server card markup, monogram, detail container, two-click delete and empty state
// ported from DailenG/skilldock@181b52b src/app/routes/mcp.tsx; the data layer is new
// (typed commands over TanStack Query, SPEC 13.2), and so is the project scope (M24).

import { useQuery } from "@tanstack/react-query";
import { memo, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState, type MutableRefObject, type ReactNode } from "react";
import { storeQueryKeys, useChangeNotice } from "@/app/apply/changeNotice";
import { RefreshButton } from "@/app/apply/RefreshButton";
import { collapsibleAction, HEADER_ACTION_LABEL, HeaderToolbar } from "@/app/components/HeaderSlots";
import { ListGridViewToggle, type ListGridViewMode } from "@/app/components/ListGridViewToggle";
import { Modal } from "@/app/components/Modal";
import { useNotifications } from "@/app/components/notifications";
import { ReadOnlyBadge } from "@/app/components/ReadOnlyBadge";
import { SearchFieldIcon } from "@/app/components/SearchFieldIcon";
import { useEventCallback } from "@/app/components/useEventCallback";
import { useVirtualList, VirtualSlot } from "@/app/components/useVirtualSlots";
import type { ImportTarget } from "@/app/routes/import/model";
import { useScopeUnmanaged, UnmanagedSection } from "@/app/routes/import/UnmanagedSection";
import { useActiveScope } from "@/app/scope/ActiveScope";
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
import { ProjectServersView, useProjectServers, writableLayers } from "./ProjectServers";
import type { ProjectDialogState } from "./ProjectServerDialog";
import { ServerEditDialog, type EditTab } from "./ServerEditDialog";
import { McpSourceUpdate } from "./McpSourceUpdate";

export type McpPageProps = {
  /** Opens the Pending changes drawer (drift is resolved there). */
  onOpenPending: () => void;
  /** Opens the Install page. */
  onOpenInstall: () => void;
  /** Opens the Import page with an entry picked ("Import…" on a not-managed entry). */
  onOpenImport: (target?: ImportTarget) => void;
};

type DialogState =
  | { mode: "create"; focusPaste: boolean }
  | { mode: "edit"; serverId: string; tab: EditTab };

/** Heights of a list card and a grid card never rendered yet (#17), in pixels. */
const LIST_CARD_ESTIMATE = 68;
const GRID_CARD_ESTIMATE = 150;

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
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = previousOverflow;
    };
  }, [isGridLayout]);

  if (!isGridLayout) {
    return <div className="mcp-server-card__details">{children}</div>;
  }

  const name = serverName(row);
  return (
    <Modal
      className="skill-card-detail-modal mcp-server-card__detail-modal"
      backdropClassName="skill-card-detail-modal__backdrop"
      element="section"
      label={`${t("mcp.card.basicInfo")} ${name}`}
      initialFocus="dialog"
      portal
      onClose={onClose}
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
    </Modal>
  );
}

type McpServerCardProps = {
  row: ServerRow;
  clients: ClientInfo[];
  isGrid: boolean;
  isExpanded: boolean;
  isBulk: boolean;
  isDeleteConfirming: boolean;
  isDeleting: boolean;
  /** Some card is deleting: every delete and edit waits for it. */
  anyDeleting: boolean;
  deleteActionRef: MutableRefObject<HTMLButtonElement | null>;
  // Stable across renders (the card is memoized), so they take the row or its ID.
  /** Sets the expanded card: the row's ID, or "" to collapse. */
  onExpandedChange: (serverId: string) => void;
  onDelete: (row: ServerRow) => void;
  onToggleAll: (row: ServerRow, enable: boolean) => void;
  onEdit: (serverId: string, tab: EditTab) => void;
  onOpenPending: () => void;
};

/** One store server; memoized, so expanding one card or a search keystroke re-renders only the cards that changed. */
const McpServerCard = memo(function McpServerCard({
  row,
  clients,
  isGrid,
  isExpanded,
  isBulk,
  isDeleteConfirming,
  isDeleting,
  anyDeleting,
  deleteActionRef,
  onExpandedChange,
  onDelete,
  onToggleAll,
  onEdit,
  onOpenPending,
}: McpServerCardProps) {
  const name = serverName(row);
  const pills = visiblePills(row, clients);
  const enabledPills = pills.filter((entry) => entry.pill.selected);
  const readyPills = pills.filter(
    (entry: VisiblePill) => entry.client.detected && entry.client.supported && entry.pill.state !== "not_supported",
  );
  // A selected client whose file is read only (M38): the row says so; Enable
  // all / Disable all still send the same choice and the plan reports it blocked.
  const readOnlyPills = enabledPills.filter((entry) => entry.pill.readOnly);
  const allReadyEnabled = readyPills.length > 0 && readyPills.every((entry) => entry.pill.selected);
  const partial = enabledPills.length > 0 && !allReadyEnabled;
  const powerTooltip = allReadyEnabled
    ? t("mcp.card.disableAllApps", { name })
    : partial
      ? t("mcp.card.bulkToggle.partial", { enabled: enabledPills.length, total: readyPills.length })
      : t("mcp.card.enableAllApps", { name });
  const enabledSummary = enabledPills.length > 0
    ? t("mcp.summary.enabledApps", { apps: enabledPills.map((entry) => entry.client.name).join(", ") })
    : t("mcp.summary.disabled");
  const subtitle = row.invalid
    ? errorMessage(row.invalid.code)
    : row.description ?? row.launch;
  const handleClose = useCallback(() => onExpandedChange(""), [onExpandedChange]);
  // A drifted pill opens Pending changes; the grid's detail modal closes first
  // so the drawer is the only dialog (focus returns to the card's trigger).
  const openPendingFromDetail = () => {
    if (isGrid) {
      onExpandedChange("");
    }
    onOpenPending();
  };

  const deleteButton = isDeleteConfirming || isDeleting ? (
    <button
      ref={deleteActionRef}
      className="skill-card__delete-confirm-button"
      type="button"
      onClick={() => onDelete(row)}
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
      onClick={() => onDelete(row)}
      aria-label={t("mcp.card.delete", { name })}
      data-tooltip={t("mcp.card.deleteTooltip")}
      disabled={anyDeleting}
    >
      <DeleteIcon />
    </button>
  );

  const editButton = (
    <button
      className="skill-card__icon-button"
      type="button"
      onClick={() => onEdit(row.id, row.invalid ? "toml" : "form")}
      aria-label={t("mcp.card.edit", { name })}
      data-tooltip={t("mcp.card.editTooltip")}
      disabled={anyDeleting}
    >
      <EditIcon />
    </button>
  );

  return (
    <article
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
              onExpandedChange(isExpanded ? "" : row.id);
            }
          }}
          onKeyDown={(event) => {
            if (!row.invalid && (event.key === "Enter" || event.key === " ")) {
              event.preventDefault();
              onExpandedChange(isExpanded ? "" : row.id);
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
                  {readOnlyPills.length > 0 ? (
                    <ReadOnlyBadge
                      readOnly={readOnlyPills[0].pill.readOnly}
                      focusable={false}
                      tooltip={t("readOnly.tooltipClients", {
                        clients: readOnlyPills.map((entry) => entry.client.name).join(", "),
                      })}
                    />
                  ) : null}
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
              onClick={() => onToggleAll(row, !allReadyEnabled)}
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
          onClose={handleClose}
          onEdit={() => {
            onExpandedChange("");
            onEdit(row.id, "form");
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
          {row.origin?.startsWith("source:") ? <McpSourceUpdate serverId={row.id} name={name} /> : null}
          <section>
            <div className="skill-card__section-header">
              <h4>{t("mcp.card.enableApps")}</h4>
              {readyPills.length > 0 || enabledPills.length > 0 ? (
                <div className="tool-sync-panel__actions">
                  <button
                    className="secondary-button secondary-button--compact semantic-action semantic-action--enable"
                    type="button"
                    onClick={() => onToggleAll(row, true)}
                    disabled={isBulk || allReadyEnabled}
                    aria-label={t("mcp.card.enableAllApps", { name })}
                  >
                    {t("mcp.card.enableAll")}
                  </button>
                  <button
                    className="secondary-button secondary-button--compact semantic-action semantic-action--disable"
                    type="button"
                    onClick={() => onToggleAll(row, false)}
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
                    onOpenPending={openPendingFromDetail}
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
});

/**
 * The MCP page (SPEC 13.2): one card per store server with its client pills, or at project
 * scope the servers of that project's manifests.
 */
export function McpPage({ onOpenPending, onOpenInstall, onOpenImport }: McpPageProps) {
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
  // The sidebar's scope (SPEC 13.1); a switch closes the open card and any dialog.
  const { scope } = useActiveScope();
  const projectId = scope.kind === "project" ? scope.id : "";
  const inProject = scope.kind === "project";
  const [shownProjectId, setShownProjectId] = useState(projectId);
  const [projectDialog, setProjectDialog] = useState<ProjectDialogState | null>(null);
  const projectServers = useProjectServers(projectId, inProject);
  const [projectBusy, setProjectBusy] = useState(false);
  if (shownProjectId !== projectId) {
    setShownProjectId(projectId);
    setExpandedId("");
    setDialog(null);
    setProjectDialog(null);
  }
  const deleteActionRef = useRef<HTMLButtonElement | null>(null);
  const unmanaged = useScopeUnmanaged();
  const managedCount = inProject ? projectServers.data?.servers.length : servers.data?.servers.length;
  const scopeClients = (inProject ? projectServers.data?.clients : servers.data?.clients) ?? [];
  const clientNames = useMemo(() => new Map(scopeClients.map((client) => [client.id, client.name])), [scopeClients]);
  const counts =
    managedCount !== undefined && unmanaged.data ? (
      <p className="import-counts" role="note">
        {t("import.counts", { managed: managedCount, unmanaged: unmanaged.data.total })}
      </p>
    ) : null;
  const unmanagedSection = (
    <UnmanagedSection summary={unmanaged.data} clientNames={clientNames} onImport={onOpenImport} />
  );

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
  // Typing stays responsive: the list filters on the deferred query in a background render.
  const deferredQuery = useDeferredValue(query);
  const filtered = useMemo(() => {
    const needle = deferredQuery.trim().toLowerCase();
    const rows = list?.servers ?? [];
    if (!needle) {
      return rows;
    }
    return rows.filter((row) =>
      [row.id, row.displayName ?? "", row.launch].some((field) => field.toLowerCase().includes(needle)),
    );
  }, [list, deferredQuery]);
  // Only the cards near the viewport render (#17); an open or busy card always does. A view switch
  // starts the remembered heights afresh.
  const slots = useVirtualList(LIST_CARD_ESTIMATE, [expandedId, deleteConfirmingId, deletingId, bulkId], viewMode);

  const handleDelete = useEventCallback(async (row: ServerRow) => {
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
  });

  const handleToggleAll = useEventCallback(async (row: ServerRow, enable: boolean) => {
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
  });

  const handleEdit = useCallback(
    (serverId: string, tab: EditTab) => setDialog({ mode: "edit", serverId, tab }),
    [],
  );

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
      {inProject ? null : <ListGridViewToggle value={viewMode} onChange={setViewMode} />}
      <RefreshButton label={t("mcp.toolbar.refresh")} />
      {inProject ? null : (
        <button
          className="secondary-button secondary-button--compact skills-toolbar-button"
          type="button"
          onClick={() => setDialog({ mode: "create", focusPaste: true })}
          {...collapsibleAction(t("mcp.toolbar.paste"), "late")}
        >
          <span aria-hidden="true" className="skills-toolbar-button__icon">
            <PasteIcon />
          </span>
          <span className={HEADER_ACTION_LABEL}>{t("mcp.toolbar.paste")}</span>
        </button>
      )}
      <button
        className="secondary-button secondary-button--compact skills-toolbar-button"
        type="button"
        onClick={() =>
          inProject ? setProjectDialog({ mode: "create" }) : setDialog({ mode: "create", focusPaste: false })}
        // An untrusted or changed project is read only until it is trusted;
        // a running edit or refetch may make the shown hash stale; a project
        // whose manifests are read only for this account (M38) has nowhere
        // to write the new entry (the project view says why).
        disabled={
          inProject
          && (projectServers.data?.project.trust !== "trusted"
            || writableLayers(projectServers.data.project).length === 0
            || projectServers.isFetching
            || projectBusy)
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
        {counts}
        <ProjectServersView
          projectId={projectId}
          query={deferredQuery}
          onOpenPending={onOpenPending}
          dialog={projectDialog}
          onDialog={setProjectDialog}
          onBusy={setProjectBusy}
        />
        {unmanagedSection}
      </div>
    );
  }

  return (
    <div className="mcp-route">
      <HeaderToolbar>{toolbar}</HeaderToolbar>
      {counts}

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
        {filtered.map((row, index) => (
          <VirtualSlot
            key={row.id}
            list={slots}
            id={row.id}
            index={index}
            label={serverName(row)}
            className="virtual-slot"
            estimate={viewMode === "grid" ? GRID_CARD_ESTIMATE : LIST_CARD_ESTIMATE}
          >
            {() => (
              <McpServerCard
                row={row}
                clients={clients}
                isGrid={viewMode === "grid"}
                isExpanded={expandedId === row.id && !row.invalid}
                isBulk={bulkId === row.id}
                isDeleteConfirming={deleteConfirmingId === row.id}
                isDeleting={deletingId === row.id}
                anyDeleting={Boolean(deletingId)}
                deleteActionRef={deleteActionRef}
                onExpandedChange={setExpandedId}
                onDelete={handleDelete}
                onToggleAll={handleToggleAll}
                onEdit={handleEdit}
                onOpenPending={onOpenPending}
              />
            )}
          </VirtualSlot>
        ))}
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
      {unmanagedSection}

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
