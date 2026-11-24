// The MCP page at project scope (SPEC 13.2): the servers of a known project's manifests with
// per-client pills from the project plan, the store servers the folder inherits, and edits of
// the manifest (each one commit that trusts and applies what Loadout wrote). An untrusted or
// changed project is read only until it is reviewed and trusted (SPEC 17, `loadout project
// trust`). Card, pill and toolbar markup are the store list's (McpPage, ClientPill). A manifest
// this account cannot write (M38) is shown with its reason, and the edits that would write it
// are disabled.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { storeQueryKeys, useChangeNotice } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { ReadOnlyBadge, ReadOnlyNote, readOnlyReason } from "@/app/components/ReadOnlyBadge";
import { TrustReviewDialog } from "@/app/routes/projects/TrustReviewDialog";
import { errorMessage, t } from "@/i18n/en";
import type {
  ChangeResult,
  ClientChoice,
  ClientInfo,
  InheritedServer,
  ManifestLayerKind,
  ProjectScopeInfo,
  ProjectServerKind,
  ProjectServerList,
  ProjectServerRow,
  ReadOnly,
} from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { PillButton, pillsToShow, type VisiblePill } from "./ClientPill";
import { DeleteIcon, EditIcon, McpServerMonogram, PowerToggleIcon } from "./icons";
import { ProjectServerDialog, type ProjectDialogState } from "./ProjectServerDialog";

export type ProjectServersViewProps = {
  projectId: string;
  query: string;
  /** Opens Pending changes, where drift is resolved. */
  onOpenPending: () => void;
  /** The open editor, owned by the page (its toolbar has Add). */
  dialog: ProjectDialogState | null;
  onDialog: (dialog: ProjectDialogState | null) => void;
  /** A project edit started or ended (the page locks its Add button too). */
  onBusy: (busy: boolean) => void;
};

const TRANSPORT_LABELS = {
  stdio: "mcp.transport.stdio",
  http: "mcp.transport.http",
  sse: "mcp.transport.sse",
} as const;

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

function kindLabel(row: ProjectServerRow): string {
  const kinds: Record<ProjectServerKind, string> = {
    define: t("mcp.project.kind.define"),
    override: t("mcp.project.kind.override", { from: row.from ?? "" }),
    suppress: t("mcp.project.kind.suppress"),
  };
  return kinds[row.kind];
}

/** The manifest layers an edit can write: none when the whole project is read only (M38). */
export function writableLayers(project: ProjectScopeInfo): ManifestLayerKind[] {
  if (project.readOnly) {
    return [];
  }
  return project.layers.filter((layer) => !project.readOnlyLayers.some((entry) => entry.layer === layer));
}

/** Why `layer` cannot be written, or null. */
function layerReadOnly(project: ProjectScopeInfo, layer: ManifestLayerKind): ReadOnly | null {
  if (project.readOnly) {
    return project.readOnly;
  }
  const entry = project.readOnlyLayers.find((item) => item.layer === layer);
  return entry ? { kind: entry.kind } : null;
}

/** The project scope's server list; the page and the view share this query. */
export function useProjectServers(projectId: string, enabled = true) {
  const { commands } = useTransport();
  return useQuery({
    queryKey: [...storeQueryKeys.projectServers, projectId],
    queryFn: async () => unwrap(await commands.listProjectServers(projectId)),
    enabled,
  });
}

export function ProjectServersView({ projectId, query, onOpenPending, dialog, onDialog, onBusy }: ProjectServersViewProps) {
  const { commands } = useTransport();
  const queryClient = useQueryClient();
  const changeNotice = useChangeNotice();
  const { notify } = useNotifications();
  const list = useProjectServers(projectId);
  const [reviewing, setReviewing] = useState(false);
  const [expanded, setExpanded] = useState("");
  const [busy, setBusy] = useState("");
  const [deleteConfirming, setDeleteConfirming] = useState("");

  const data = list.data;
  const project = data?.project;
  const untrusted = !project || project.trust !== "trusted";
  // The manifests an edit can write (M38); none: every edit is disabled and the note says why.
  const writable = project ? writableLayers(project) : [];
  const noWrite = writable.length === 0;
  const hideLayer: ManifestLayerKind | undefined = writable.includes("shared") ? "shared" : writable[0];
  const needle = query.trim().toLowerCase();
  const rows = (data?.servers ?? []).filter(
    (row) => !needle || [row.name, row.from ?? "", row.launch].some((field) => field.toLowerCase().includes(needle)),
  );

  // One project edit at a time: each sends the hash it was listed with, so
  // every action waits until the edit is done and the list is fresh again.
  const locked = Boolean(busy) || list.isFetching;

  async function run(key: string, change: () => Promise<ChangeResult>, message: string) {
    if (locked) {
      return;
    }
    setBusy(key);
    onBusy(true);
    try {
      changeNotice(await change(), message);
      await queryClient.invalidateQueries({ queryKey: [...storeQueryKeys.projectServers, projectId] });
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    } finally {
      setBusy("");
      onBusy(false);
    }
  }

  function setClients(row: ProjectServerRow, choice: ClientChoice, message: string) {
    if (!project) {
      return;
    }
    void run(
      row.name,
      async () => unwrap(await commands.setProjectServerClients(projectId, project.hash, row.name, row.editLayer, choice)),
      message,
    );
  }

  function handleDelete(row: ProjectServerRow) {
    if (!project) {
      return;
    }
    if (deleteConfirming !== row.name) {
      setDeleteConfirming(row.name);
      return;
    }
    setDeleteConfirming("");
    void run(
      row.name,
      async () => unwrap(await commands.removeProjectServer(projectId, project.hash, row.name, row.editLayer)),
      t("mcp.project.deleted", { name: row.name, project: project.name }),
    );
  }

  function handleHide(server: InheritedServer) {
    if (!project || !hideLayer) {
      return;
    }
    const draft = {
      name: server.id,
      layer: hideLayer,
      kind: "suppress" as const,
      from: null,
      transport: null,
      command: null,
      args: [],
      url: null,
      env: [],
      headers: [],
      clients: null,
    };
    void run(
      server.id,
      async () => unwrap(await commands.saveProjectServer(projectId, project.hash, draft, true)),
      t("mcp.project.hidden", { name: server.id, project: project.name }),
    );
  }

  if (list.isError) {
    return (
      <div className="dialog-error mcp-page-error" role="alert">
        <span>{failureText(list.error)}</span>
        <button className="secondary-button secondary-button--compact" type="button" onClick={() => void list.refetch()}>
          {t("app.action.retry")}
        </button>
      </div>
    );
  }
  if (!data || !project) {
    return <p className="mcp-page-loading">{t("mcp.project.loading")}</p>;
  }

  return (
    <>
      <p className="skills-scope-note" role="note">
        {t("mcp.project.note", { name: project.name, path: project.path })}
      </p>
      {untrusted ? (
        <div className="mcp-page-warning mcp-project-trust" role="alert">
          <span>{t(project.trust === "changed" ? "mcp.project.changed" : "mcp.project.untrusted")}</span>
          <button className="secondary-button secondary-button--compact" type="button" onClick={() => setReviewing(true)}>
            {t("mcp.project.review")}
          </button>
        </div>
      ) : null}
      {project.readOnly ? (
        <ReadOnlyNote>
          {t("readOnly.project.whole", { name: project.name, reason: readOnlyReason(project.readOnly.kind) })}
        </ReadOnlyNote>
      ) : (
        project.readOnlyLayers.map((entry) => (
          <ReadOnlyNote key={entry.layer}>
            {t(entry.layer === "shared" ? "readOnly.layer.shared" : "readOnly.layer.local", {
              reason: readOnlyReason(entry.kind),
            })}
          </ReadOnlyNote>
        ))
      )}
      {data.warnings.map((warning, index) => (
        <p key={`${index}-${warning.code}`} className="mcp-page-warning" role="note">
          {errorMessage(warning.code)}
        </p>
      ))}

      <section className="mcp-server-list card-list" aria-label={project.name}>
        {rows.map((row) => (
          <ProjectServerCard
            key={row.name}
            row={row}
            clients={data.clients}
            untrusted={untrusted}
            editLayerReadOnly={layerReadOnly(project, row.editLayer)}
            busy={busy === row.name}
            locked={locked}
            expanded={expanded === row.name}
            deleteConfirming={deleteConfirming === row.name}
            onToggleExpanded={() => setExpanded(expanded === row.name ? "" : row.name)}
            onEdit={() => onDialog({ mode: "edit", name: row.name, layer: row.editLayer })}
            onDelete={() => handleDelete(row)}
            onSetClients={(choice, message) => setClients(row, choice, message)}
            onOpenPending={onOpenPending}
            projectName={project.name}
          />
        ))}
        {data.servers.length === 0 ? (
          <div className="panel-card empty-state">
            <h3>{t("mcp.project.empty.title")}</h3>
            <p>{t("mcp.project.empty.description")}</p>
          </div>
        ) : null}
      </section>

      {data.inherited.length > 0 ? (
        <section className="apply-drawer__section mcp-project-inherited" aria-labelledby="mcp-project-inherited">
          <h4 id="mcp-project-inherited" className="apply-drawer__section-title">
            {t("mcp.project.inherited.title")}
          </h4>
          <p className="apply-drawer__note">{t("mcp.project.inherited.description")}</p>
          <ul className="apply-runs">
            {data.inherited.map((server) => {
              const name = server.displayName ?? server.id;
              return (
                <li key={server.id} className="apply-run">
                  <div className="apply-run__copy">
                    <span className="apply-run__summary">{name}</span>
                    <span className="apply-run__time">{server.launch}</span>
                  </div>
                  <button
                    className="secondary-button secondary-button--compact"
                    type="button"
                    disabled={untrusted || noWrite || locked}
                    aria-label={t("mcp.project.inherited.overrideAria", { name })}
                    onClick={() =>
                      onDialog({
                        mode: "create",
                        initial: { name: server.id, kind: "override", from: server.id, transport: null },
                      })}
                  >
                    {t("mcp.project.inherited.override")}
                  </button>
                  <button
                    className="secondary-button secondary-button--compact semantic-action semantic-action--disable"
                    type="button"
                    disabled={untrusted || noWrite || locked}
                    aria-label={t("mcp.project.inherited.hideAria", { name })}
                    onClick={() => handleHide(server)}
                  >
                    {t("mcp.project.inherited.hide")}
                  </button>
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}

      {dialog && !untrusted && !noWrite ? (
        <ProjectServerDialog
          projectId={projectId}
          projectName={project.name}
          hash={project.hash}
          layers={writable}
          clients={data.clients}
          storeServers={data.storeServers}
          state={dialog}
          onClose={() => onDialog(null)}
        />
      ) : null}
      {reviewing ? (
        <TrustReviewDialog
          projectId={projectId}
          onClose={() => setReviewing(false)}
          onTrusted={() => {
            setReviewing(false);
            void list.refetch();
          }}
        />
      ) : null}
    </>
  );
}

type ProjectServerCardProps = {
  row: ProjectServerRow;
  clients: ClientInfo[];
  projectName: string;
  /** The project is not trusted: nothing is edited until it is reviewed. */
  untrusted: boolean;
  /** The manifest this row's edits write is read only for this account (M38). */
  editLayerReadOnly: ReadOnly | null;
  /** This card's change is running (spinner). */
  busy: boolean;
  /** A project edit is running or the list is refetching: hashes may be stale. */
  locked: boolean;
  expanded: boolean;
  deleteConfirming: boolean;
  onToggleExpanded: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onSetClients: (choice: ClientChoice, message: string) => void;
  onOpenPending: () => void;
};

function ProjectServerCard(props: ProjectServerCardProps) {
  const { row, clients, projectName, untrusted, editLayerReadOnly, busy, locked, expanded, deleteConfirming } = props;
  const pills = pillsToShow(row.pills, clients);
  const selected = row.pills.filter((pill) => pill.selected).map((pill) => pill.client);
  // What a toggle starts from: the manifest's explicit list as written (it
  // may name clients this machine opts out of, which stay), or for
  // `all-detected` the clients the pills show as selected here.
  const written = row.clients.allDetected ? selected : row.clients.clients;
  // The pills Enable All turns on: detected clients with a project config.
  const ready = pills.filter(
    (entry) => entry.client.detected && entry.client.supported && entry.pill.state !== "not_supported",
  );
  const allReady = ready.length > 0 && ready.every((entry) => entry.pill.selected);
  const suppress = row.kind === "suppress";
  const subtitle = row.invalid ? errorMessage(row.invalid.code) : row.launch || kindLabel(row);
  const powerLabel = allReady
    ? t("mcp.card.disableAllApps", { name: row.name })
    : t("mcp.card.enableAllApps", { name: row.name });
  // A row whose manifest is read only (M38) can be viewed, not changed.
  const disabled = untrusted || locked || editLayerReadOnly !== null;

  function enableAll() {
    const missing = ready.map((entry) => entry.client.id).filter((id) => !written.includes(id));
    props.onSetClients(
      { allDetected: false, clients: [...written, ...missing] },
      t("mcp.toggledAll.on", { name: row.name }),
    );
  }

  function disableAll() {
    props.onSetClients({ allDetected: false, clients: [] }, t("mcp.toggledAll.off", { name: row.name }));
  }

  function toggle(entry: VisiblePill) {
    if (entry.pill.state === "drift") {
      props.onOpenPending();
      return;
    }
    const rest = written.filter((id) => id !== entry.client.id);
    props.onSetClients(
      { allDetected: false, clients: entry.pill.selected ? rest : [...rest, entry.client.id] },
      t("mcp.project.toggled", { client: entry.client.name, name: row.name, project: projectName }),
    );
  }

  return (
    <article
      className={`mcp-server-card${expanded ? " is-expanded" : ""}${row.invalid ? " is-invalid" : ""}`}
      aria-label={row.name}
    >
      <div className="mcp-server-card__header">
        <div
          className="mcp-server-card__summary-button"
          role="button"
          tabIndex={0}
          aria-expanded={expanded}
          aria-label={t(expanded ? "mcp.card.collapseAria" : "mcp.card.expandAria", { name: row.name })}
          onClick={props.onToggleExpanded}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              props.onToggleExpanded();
            }
          }}
        >
          <div className="mcp-server-card__main">
            <div className="mcp-server-card__identity">
              <McpServerMonogram name={row.name} active={selected.length > 0 && !suppress} />
              <div className="mcp-server-card__title-stack">
                <div className="mcp-server-card__title-row">
                  <strong>{row.name}</strong>
                  {row.invalid ? <span className="status-badge tone-danger">{t("mcp.card.invalid")}</span> : null}
                  {row.transport ? (
                    <span className="status-badge tone-neutral mcp-server-card__tool-summary-badge">
                      {t(TRANSPORT_LABELS[row.transport])}
                    </span>
                  ) : null}
                  <span className={`status-badge ${suppress ? "tone-warning" : "tone-info"}`}>{kindLabel(row)}</span>
                  {row.layers.map((layer) => (
                    <span key={layer} className="status-badge tone-neutral mcp-layer-chip">
                      {t(layer === "shared" ? "mcp.project.layer.shared" : "mcp.project.layer.local")}
                    </span>
                  ))}
                  <ReadOnlyBadge
                    readOnly={editLayerReadOnly}
                    focusable={false}
                    tooltip={editLayerReadOnly ? t("readOnly.layer.editDisabled", { reason: readOnlyReason(editLayerReadOnly.kind) }) : undefined}
                  />
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
          <button
            className={`skill-card__icon-button plugins-page__toggle-icon-button ${allReady ? "is-enabled" : selected.length > 0 ? "is-partial" : "is-disabled"}`}
            type="button"
            onClick={() => (allReady ? disableAll() : enableAll())}
            aria-label={powerLabel}
            data-tooltip={powerLabel}
            disabled={disabled || (ready.length === 0 && selected.length === 0)}
          >
            <PowerToggleIcon isSpinning={busy} />
          </button>
          <button
            className="skill-card__icon-button"
            type="button"
            onClick={props.onEdit}
            aria-label={t("mcp.card.edit", { name: row.name })}
            data-tooltip={t("mcp.card.editTooltip")}
            disabled={disabled}
          >
            <EditIcon />
          </button>
          {deleteConfirming ? (
            <button
              className="skill-card__delete-confirm-button"
              type="button"
              onClick={props.onDelete}
              aria-label={t("mcp.card.deleteConfirmAria", { state: t("mcp.card.deleteConfirm"), name: row.name })}
              data-tooltip={t("mcp.card.deleteConfirmTooltip")}
              disabled={locked}
            >
              {t("mcp.card.deleteConfirm")}
            </button>
          ) : (
            <button
              className="skill-card__icon-button skill-card__icon-button--delete"
              type="button"
              onClick={props.onDelete}
              aria-label={t("mcp.card.delete", { name: row.name })}
              data-tooltip={t("mcp.card.deleteTooltip")}
              disabled={disabled}
            >
              <DeleteIcon />
            </button>
          )}
          <span className="skill-card__chevron mcp-server-card__chevron" aria-hidden="true">
            {expanded ? "⌄" : "›"}
          </span>
        </div>
      </div>
      {expanded ? (
        <div className="mcp-server-card__details">
          {editLayerReadOnly ? (
            <ReadOnlyNote>{t("readOnly.layer.editDisabled", { reason: readOnlyReason(editLayerReadOnly.kind) })}</ReadOnlyNote>
          ) : null}
          <section>
            <div className="skill-card__section-header">
              <h4>{t(suppress ? "mcp.project.hiddenFor" : "mcp.card.enableApps")}</h4>
              {ready.length > 0 || selected.length > 0 ? (
                <div className="tool-sync-panel__actions">
                  <button
                    className="secondary-button secondary-button--compact semantic-action semantic-action--enable"
                    type="button"
                    onClick={enableAll}
                    disabled={disabled || allReady}
                    aria-label={t("mcp.card.enableAllApps", { name: row.name })}
                  >
                    {t("mcp.card.enableAll")}
                  </button>
                  <button
                    className="secondary-button secondary-button--compact semantic-action semantic-action--disable"
                    type="button"
                    onClick={disableAll}
                    disabled={disabled || selected.length === 0}
                    aria-label={t("mcp.card.disableAllApps", { name: row.name })}
                  >
                    {t("mcp.card.disableAll")}
                  </button>
                </div>
              ) : null}
            </div>
            {row.usesDefaults ? <p className="mcp-form-hint">{t("mcp.project.clientsDefault")}</p> : null}
            {pills.length > 0 ? (
              <div className="tool-pill-grid">
                {pills.map((entry) => (
                  <PillButton
                    key={entry.client.id}
                    entry={entry}
                    busy={disabled}
                    onClick={() => toggle(entry)}
                  />
                ))}
              </div>
            ) : (
              <p className="mcp-server-card__tool-empty">{t("mcp.card.noClients")}</p>
            )}
          </section>
        </div>
      ) : null}
    </article>
  );
}
