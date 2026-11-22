// Pending changes drawer (SPEC 13.4): the pending apply plan grouped by
// scope, each file expandable to its masked unified diff, Apply all / Apply
// selected, the drift resolver for entries that need a decision, and the
// recent apply runs with "Undo this apply".

import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useState } from "react";
import { storeQueryKeys, useChangeNotice, useUndoApply } from "@/app/apply/changeNotice";
import { STATE_KEYS, diffLineClass } from "@/app/apply/labels";
import { useNotifications } from "@/app/components/notifications";
import { ResolverDialog } from "@/app/resolver/ResolverDialog";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type {
  AppliedRun,
  EntryRef,
  EntryStateKind,
  FileRef,
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

function formatTime(time: string) {
  const date = new Date(time);
  return Number.isNaN(date.getTime()) ? time : date.toLocaleString();
}

function failureText(error: unknown) {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

/** A file's identity: two projects' `<P>/.mcp.json` are different files. */
function fileKey(file: PlannedFile): string {
  return JSON.stringify([file.projectId, file.scope, file.file]);
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
  onResolve: (entry: EntryRef) => void;
};

/** States the drift resolver handles (SPEC 8.2). */
const RESOLVABLE: EntryStateKind[] = ["drifted", "conflict", "missing", "collision"];

function PlannedFileItem({ file, selected, onSelect, clientName, onResolve }: FileItemProps) {
  const [expanded, setExpanded] = useState(false);
  const detailId = useId();
  // Entries the user has to look at: a decision, a block, a problem, or a
  // state the resolver handles.
  const attention = file.entries.filter(
    (entry) =>
      entry.unresolved ||
      entry.state === "blocked" ||
      entry.problem !== null ||
      RESOLVABLE.includes(entry.state),
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
          aria-label={
            file.project
              ? t("apply.file.selectInProject", { file: file.file, project: file.project })
              : t("apply.file.select", { file: file.file })
          }
        />
        <button
          className="apply-file__toggle"
          type="button"
          aria-expanded={expanded}
          aria-controls={detailId}
          onClick={() => setExpanded((current) => !current)}
        >
          <span className="apply-file__path" title={file.project ? `${file.project}: ${file.file}` : file.file}>
            {file.project ? <span className="apply-file__project">{file.project}</span> : null}
            <span>{file.file}</span>
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
                  {entry.client && RESOLVABLE.includes(entry.state) ? (
                    <button
                      className="ghost-button apply-entry__resolve"
                      type="button"
                      aria-label={t("apply.entries.resolveAria", { name: entry.name })}
                      onClick={() =>
                        onResolve({
                          file: { projectId: file.projectId, scope: file.scope, file: file.file },
                          client: entry.client ?? "",
                          name: entry.name,
                        })
                      }
                    >
                      {t("apply.entries.resolve")}
                    </button>
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
  const [resolving, setResolving] = useState<EntryRef | null>(null);

  const apply = useMutation({
    mutationFn: async (selected: FileRef[] | null) => unwrap(await commands.runApply(selected)),
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
  // The files "Apply selected" sends: only these are written.
  const selectedRefs: FileRef[] = files
    .filter((file) => selectedFiles.has(fileKey(file)))
    .map(({ projectId, scope, file }) => ({ projectId, scope, file }));

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !resolving) {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose, resolving]);

  function setSelected(key: string, selected: boolean) {
    setSelectedFiles((current) => {
      const next = new Set(current);
      if (selected) {
        next.add(key);
      } else {
        next.delete(key);
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
                {group.files.map((file, index) => (
                  <PlannedFileItem
                    key={`${fileKey(file)}#${index}`}
                    file={file}
                    selected={selectedFiles.has(fileKey(file))}
                    onSelect={(selected) => setSelected(fileKey(file), selected)}
                    clientName={clientName}
                    onResolve={setResolving}
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
            disabled={apply.isPending || selectedRefs.length === 0}
            onClick={() => apply.mutate(selectedRefs)}
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
      {resolving ? (
        <div onClick={(event) => event.stopPropagation()} role="presentation">
          <ResolverDialog entry={resolving} clientName={clientName} onClose={() => setResolving(null)} />
        </div>
      ) : null}
    </div>
  );
}

/** The right-side drawer of pending changes; queries run only while open. */
export function ApplyDrawer({ open, onClose }: ApplyDrawerProps) {
  return open ? <DrawerBody onClose={onClose} /> : null;
}
