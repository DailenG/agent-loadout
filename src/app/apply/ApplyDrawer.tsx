// Pending changes drawer (SPEC 13.4): the pending apply plan grouped by
// scope, each file expandable to its masked unified diff, Apply all / Apply
// selected, and the recent apply runs with "Undo this apply".

import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useState } from "react";
import { storeQueryKeys, useChangeNotice, useUndoApply } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type {
  AppliedRun,
  EntryStateKind,
  FileScope,
  PlannedFile,
} from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";

export type ApplyDrawerProps = {
  open: boolean;
  onClose: () => void;
};

const SCOPE_ORDER: FileScope[] = ["user", "project", "project_local"];

const SCOPE_KEYS = {
  user: "apply.scope.user",
  project: "apply.scope.project",
  project_local: "apply.scope.projectLocal",
} as const satisfies Record<FileScope, MessageKey>;

const STATE_KEYS = {
  synced: "apply.state.synced",
  pending_create: "apply.state.pendingCreate",
  pending_update: "apply.state.pendingUpdate",
  pending_remove: "apply.state.pendingRemove",
  drifted: "apply.state.drifted",
  missing: "apply.state.missing",
  conflict: "apply.state.conflict",
  foreign: "apply.state.foreign",
  collision: "apply.state.collision",
  blocked: "apply.state.blocked",
} as const satisfies Record<EntryStateKind, MessageKey>;

function diffLineClass(line: string) {
  if (line.startsWith("+++") || line.startsWith("---")) {
    return "apply-diff__line apply-diff__line--meta";
  }
  if (line.startsWith("@@")) {
    return "apply-diff__line apply-diff__line--hunk";
  }
  if (line.startsWith("+")) {
    return "apply-diff__line apply-diff__line--added";
  }
  if (line.startsWith("-")) {
    return "apply-diff__line apply-diff__line--removed";
  }
  return "apply-diff__line";
}

function formatTime(time: string) {
  const date = new Date(time);
  return Number.isNaN(date.getTime()) ? time : date.toLocaleString();
}

function failureText(error: unknown) {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

function useClientNames() {
  const { commands } = useTransport();
  const servers = useQuery({
    queryKey: storeQueryKeys.servers,
    queryFn: async () => unwrap(await commands.listServers()),
  });
  return useMemo(() => {
    const names = new Map<string, string>();
    servers.data?.clients.forEach((client) => names.set(client.id, client.name));
    return (id: string) => names.get(id) ?? id;
  }, [servers.data]);
}

type FileItemProps = {
  file: PlannedFile;
  selected: boolean;
  onSelect: (selected: boolean) => void;
  clientName: (id: string) => string;
};

function PlannedFileItem({ file, selected, onSelect, clientName }: FileItemProps) {
  const [expanded, setExpanded] = useState(false);
  const detailId = useId();
  // Entries the user has to look at: a decision, a block or a problem.
  const attention = file.entries.filter(
    (entry) => entry.unresolved || entry.state === "blocked" || entry.problem !== null,
  );
  const { counts } = file;
  const diffLines = file.diff ? file.diff.replace(/\n$/, "").split("\n") : [];

  return (
    <li className={`apply-file${expanded ? " is-expanded" : ""}`}>
      <div className="apply-file__row">
        <input
          className="apply-file__select"
          type="checkbox"
          checked={selected}
          onChange={(event) => onSelect(event.target.checked)}
          aria-label={t("apply.file.select", { file: file.file })}
        />
        <button
          className="apply-file__toggle"
          type="button"
          aria-expanded={expanded}
          aria-controls={detailId}
          onClick={() => setExpanded((current) => !current)}
        >
          <span className="apply-file__path" title={file.file}>
            {file.file}
          </span>
          <span className="apply-file__counts" aria-label={t("apply.counts.aria", { ...counts })}>
            <span className="apply-count apply-count--create">+{counts.create}</span>
            <span className="apply-count apply-count--update">~{counts.update}</span>
            <span className="apply-count apply-count--remove">-{counts.remove}</span>
            <span className="apply-count apply-count--disable">!{counts.disable}</span>
          </span>
        </button>
      </div>
      <div className="apply-file__clients">
        {file.clients.map(clientName).join(", ")}
      </div>
      {file.problem ? (
        <p className="apply-file__problem" role="alert">
          {errorMessage(file.problem.code)}
        </p>
      ) : null}
      {expanded ? (
        <div className="apply-file__detail" id={detailId}>
          {diffLines.length > 0 ? (
            <pre className="apply-diff" aria-label={t("apply.file.diff", { file: file.file })}>
              {diffLines.map((line, index) => (
                <span key={index} className={diffLineClass(line)}>
                  {line}
                  {"\n"}
                </span>
              ))}
            </pre>
          ) : (
            <p className="apply-file__note">{t("apply.file.noDiff")}</p>
          )}
          {attention.length > 0 ? (
            <ul className="apply-entries" aria-label={t("apply.entries.aria")}>
              {attention.map((entry, index) => (
                <li key={`${entry.name}-${entry.client ?? ""}-${index}`} className="apply-entry">
                  <strong className="apply-entry__name">{entry.name}</strong>
                  {entry.client ? (
                    <span className="apply-entry__client">{clientName(entry.client)}</span>
                  ) : null}
                  <span
                    className={`status-badge ${
                      entry.state === "blocked" ? "tone-danger" : "tone-warning"
                    }`}
                  >
                    {entry.unresolved && entry.state !== "blocked"
                      ? t("apply.entries.needsDecision", { state: t(STATE_KEYS[entry.state]) })
                      : t(STATE_KEYS[entry.state])}
                  </span>
                  {entry.problem ? (
                    <span className="apply-entry__problem">{errorMessage(entry.problem.code)}</span>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : null}
          {file.warnings.map((warning, index) => (
            <p key={index} className="apply-file__note">
              {errorMessage(warning.code)}
            </p>
          ))}
        </div>
      ) : null}
    </li>
  );
}

function RecentApplies() {
  const { commands } = useTransport();
  const undo = useUndoApply();
  const runs = useQuery({
    queryKey: storeQueryKeys.appliedRuns,
    queryFn: async () => unwrap(await commands.listAppliedRuns()),
  });
  const [busyId, setBusyId] = useState("");
  const [confirmingId, setConfirmingId] = useState("");

  async function handleUndo(run: AppliedRun, confirmed: boolean) {
    setBusyId(run.applyId);
    try {
      await undo(run.applyId, confirmed);
      setConfirmingId("");
    } catch (error) {
      if (error instanceof IpcFailure && error.code === "confirmation_required") {
        setConfirmingId(run.applyId);
      }
    } finally {
      setBusyId("");
    }
  }

  return (
    <section className="apply-drawer__section" aria-labelledby="apply-recent-title">
      <h4 id="apply-recent-title" className="apply-drawer__section-title">
        {t("apply.recent.title")}
      </h4>
      {runs.isPending ? <p className="apply-drawer__note">{t("apply.loading")}</p> : null}
      {runs.isError ? <p className="apply-drawer__note">{failureText(runs.error)}</p> : null}
      {runs.data && runs.data.length === 0 ? (
        <p className="apply-drawer__note">{t("apply.recent.empty")}</p>
      ) : null}
      {runs.data && runs.data.length > 0 ? (
        <ul className="apply-runs">
          {runs.data.map((run) => (
            <li key={run.applyId} className="apply-run">
              <div className="apply-run__copy">
                <span className="apply-run__summary">{run.summary}</span>
                <span className="apply-run__time">
                  {formatTime(run.time)}
                  {run.latest ? ` · ${t("apply.recent.latest")}` : ""}
                </span>
              </div>
              <button
                className="secondary-button secondary-button--compact"
                type="button"
                disabled={Boolean(busyId)}
                onClick={() => void handleUndo(run, false)}
              >
                {t("apply.undo.action")}
              </button>
              {confirmingId === run.applyId ? (
                <div className="apply-run__confirm" role="alert">
                  <span>{t("apply.undo.confirmMessage")}</span>
                  <button
                    className="secondary-button secondary-button--compact"
                    type="button"
                    disabled={Boolean(busyId)}
                    onClick={() => void handleUndo(run, true)}
                  >
                    {t("apply.undo.confirmAction")}
                  </button>
                  <button
                    className="ghost-button"
                    type="button"
                    onClick={() => setConfirmingId("")}
                  >
                    {t("apply.cancel")}
                  </button>
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}

function DrawerBody({ onClose }: { onClose: () => void }) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const reportChange = useChangeNotice();
  const clientName = useClientNames();
  const titleId = useId();
  const plan = useQuery({
    queryKey: storeQueryKeys.pendingPlan,
    queryFn: async () => unwrap(await commands.pendingPlan()),
  });
  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());

  const apply = useMutation({
    mutationFn: async (clients: string[] | null) => unwrap(await commands.runApply(clients)),
    onSuccess: (result) => {
      setSelectedFiles(new Set());
      reportChange(result, t("apply.applied"));
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const files = plan.data?.files ?? [];
  const groups = SCOPE_ORDER.map((scope) => ({
    scope,
    files: files.filter((file) => file.scope === scope),
  })).filter((group) => group.files.length > 0);
  const selectedClients = Array.from(
    new Set(files.filter((file) => selectedFiles.has(file.file)).flatMap((file) => file.clients)),
  );

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  function setSelected(file: string, selected: boolean) {
    setSelectedFiles((current) => {
      const next = new Set(current);
      if (selected) {
        next.add(file);
      } else {
        next.delete(file);
      }
      return next;
    });
  }

  return (
    <div className="dialog-backdrop apply-drawer-backdrop" role="presentation" onClick={onClose}>
      <aside
        className="apply-drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="apply-drawer__header">
          <div className="apply-drawer__title">
            <h3 id={titleId}>{t("apply.drawer.title")}</h3>
            {plan.data ? (
              <p>
                {t("apply.drawer.summary", {
                  pending: plan.data.pending,
                  writes: plan.data.writes,
                  unresolved: plan.data.unresolved,
                })}
              </p>
            ) : null}
          </div>
          <button
            className="tool-manage-dialog__close"
            type="button"
            onClick={onClose}
            aria-label={t("apply.drawer.close")}
          >
            ×
          </button>
        </div>

        <div className="apply-drawer__body">
          {plan.data?.applyMode === "review" ? (
            <p className="apply-drawer__hint">{t("apply.drawer.reviewHint")}</p>
          ) : null}
          {plan.isPending ? <p className="apply-drawer__note">{t("apply.loading")}</p> : null}
          {plan.isError ? (
            <div className="apply-drawer__note" role="alert">
              <span>{failureText(plan.error)}</span>
              <button className="ghost-button" type="button" onClick={() => void plan.refetch()}>
                {t("app.action.retry")}
              </button>
            </div>
          ) : null}
          {plan.data && files.length === 0 ? (
            <div className="apply-drawer__empty">
              <strong>{t("apply.empty.title")}</strong>
              <p>{t("apply.empty.description")}</p>
            </div>
          ) : null}
          {groups.map((group) => (
            <section
              key={group.scope}
              className="apply-drawer__section"
              aria-label={t(SCOPE_KEYS[group.scope])}
            >
              <h4 className="apply-drawer__section-title">{t(SCOPE_KEYS[group.scope])}</h4>
              <ul className="apply-files">
                {group.files.map((file) => (
                  <PlannedFileItem
                    key={file.file}
                    file={file}
                    selected={selectedFiles.has(file.file)}
                    onSelect={(selected) => setSelected(file.file, selected)}
                    clientName={clientName}
                  />
                ))}
              </ul>
            </section>
          ))}
          {plan.data?.warnings.map((warning, index) => (
            <p key={index} className="apply-drawer__note">
              {errorMessage(warning.code)}
            </p>
          ))}
          <RecentApplies />
        </div>

        <div className="apply-drawer__actions">
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            disabled={apply.isPending || selectedClients.length === 0}
            onClick={() => apply.mutate(selectedClients)}
          >
            {t("apply.action.selected")}
          </button>
          <button
            className="primary-button primary-button--compact"
            type="button"
            disabled={apply.isPending || files.length === 0}
            onClick={() => apply.mutate(null)}
          >
            {apply.isPending ? t("apply.action.applying") : t("apply.action.all")}
          </button>
        </div>
      </aside>
    </div>
  );
}

/** The right-side drawer of pending changes; queries run only while open. */
export function ApplyDrawer({ open, onClose }: ApplyDrawerProps) {
  return open ? <DrawerBody onClose={onClose} /> : null;
}
