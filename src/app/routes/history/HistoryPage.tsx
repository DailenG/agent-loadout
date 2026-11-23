// History page (SPEC 13.5): every store commit newest first, filtered by
// scope and entity, with per-commit actions (Show diff, Undo this apply,
// Restore this version, Restore entity..., Tag, Download zip), user tags,
// the client file backups tab, and the remotes and conflicts strip.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { storeQueryKeys, useInvalidateStore, useUndoApply } from "@/app/apply/changeNotice";
import { AppSelect, type AppSelectOption } from "@/app/components/AppSelect";
import { HeaderToolbar } from "@/app/components/HeaderSlots";
import { useNotifications } from "@/app/components/notifications";
import { SearchFieldIcon } from "@/app/components/SearchFieldIcon";
import { CommitDetailPanel } from "@/app/routes/history/CommitDetailPanel";
import { FileBackupsTab } from "@/app/routes/history/FileBackupsTab";
import {
  OUTCOME_KEYS,
  OUTCOME_TONES,
  SCOPE_KEYS,
  failureText,
  formatTime,
  isAutoTag,
} from "@/app/routes/history/labels";
import { RemotesStrip } from "@/app/routes/history/RemotesStrip";
import { RestoreDialog } from "@/app/routes/history/RestoreDialog";
import { TagDialog } from "@/app/routes/history/TagDialog";
import { t, type MessageKey } from "@/i18n/en";
import type { HistoryEntry, HistoryFilter, HistoryScope } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";

export type HistoryPageProps = {
  /** Opens Pending changes: a restore waits there to be applied (SPEC 9.5). */
  onOpenPending: () => void;
  /** Opens Settings > Versioning & Backup (remotes). */
  onOpenSettings: () => void;
};

type Tab = "changes" | "backups";

const TABS = [
  { id: "changes", label: "history.tabs.changes" },
  { id: "backups", label: "history.tabs.backups" },
] as const satisfies readonly { id: Tab; label: MessageKey }[];

const SCOPE_OPTIONS: AppSelectOption<HistoryScope>[] = (["all", "user", "projects"] as const).map((scope) => ({
  value: scope,
  label: t(SCOPE_KEYS[scope]),
}));

type Dialog = { kind: "tag" | "restore" | "restoreEntity"; entry: HistoryEntry };

type RowProps = {
  entry: HistoryEntry;
  selected: boolean;
  diffOpen: boolean;
  busy: boolean;
  onSelect: () => void;
  onToggleDiff: () => void;
  onUndo: () => void;
  onDialog: (kind: Dialog["kind"]) => void;
  onZip: () => void;
  onDeleteTag: (name: string) => void;
};

function HistoryRow({
  entry,
  selected,
  diffOpen,
  busy,
  onSelect,
  onToggleDiff,
  onUndo,
  onDialog,
  onZip,
  onDeleteTag,
}: RowProps) {
  return (
    <li className={`panel-card history-row${selected ? " is-selected" : ""}`} aria-label={entry.summary}>
      <div className="history-row__head">
        <button className="history-row__main" type="button" aria-pressed={selected} onClick={onSelect}>
          <span className="history-row__summary">{entry.summary}</span>
          <span className="history-row__meta">
            {t("history.row.meta", { machine: entry.machine, time: formatTime(entry.time), id: entry.shortId })}
          </span>
        </button>
        <div className="history-row__badges">
          {entry.apply ? (
            <span className={`status-badge ${OUTCOME_TONES[entry.apply.outcome]}`}>
              {t(OUTCOME_KEYS[entry.apply.outcome])}
            </span>
          ) : null}
          {entry.tags.map((tag) => (
            <span key={tag} className={`history-tag${isAutoTag(tag) ? " history-tag--auto" : ""}`}>
              <span>{tag}</span>
              {isAutoTag(tag) ? null : (
                <button
                  className="history-tag__delete"
                  type="button"
                  aria-label={t("history.tag.delete", { name: tag })}
                  disabled={busy}
                  onClick={() => onDeleteTag(tag)}
                >
                  ×
                </button>
              )}
            </span>
          ))}
        </div>
      </div>
      {selected ? (
        <div className="history-row__actions" role="toolbar" aria-label={t("history.actions.aria", { summary: entry.summary })}>
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            aria-expanded={diffOpen}
            onClick={onToggleDiff}
          >
            {t("history.action.showDiff")}
          </button>
          {entry.apply?.undoable ? (
            <button className="secondary-button secondary-button--compact" type="button" disabled={busy} onClick={onUndo}>
              {t("apply.undo.action")}
            </button>
          ) : null}
          <button className="secondary-button secondary-button--compact" type="button" onClick={() => onDialog("restore")}>
            {t("history.action.restoreVersion")}
          </button>
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            onClick={() => onDialog("restoreEntity")}
          >
            {t("history.action.restoreEntity")}
          </button>
          <button className="secondary-button secondary-button--compact" type="button" onClick={() => onDialog("tag")}>
            {t("history.action.tag")}
          </button>
          <button className="secondary-button secondary-button--compact" type="button" disabled={busy} onClick={onZip}>
            {t("history.action.zip")}
          </button>
        </div>
      ) : null}
      {selected && diffOpen ? <CommitDetailPanel commitId={entry.id} /> : null}
    </li>
  );
}

function ChangesTab({ filter, onOpenPending }: { filter: HistoryFilter; onOpenPending: () => void }) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const undo = useUndoApply();
  const history = useQuery({
    queryKey: [...storeQueryKeys.history, filter],
    queryFn: async () => unwrap(await commands.listHistory(filter)),
  });
  const [selectedId, setSelectedId] = useState("");
  const [diffOpen, setDiffOpen] = useState(false);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [confirming, setConfirming] = useState<HistoryEntry | null>(null);
  const [undoing, setUndoing] = useState(false);

  const removeTag = useMutation({
    mutationFn: async (name: string) => {
      unwrap(await commands.deleteTag(name));
      return name;
    },
    onSuccess: (name) => {
      void invalidate();
      notify({ tone: "success", message: t("history.tag.deleted", { name }) });
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const zip = useMutation({
    mutationFn: async (entry: HistoryEntry) => ({ entry, saved: unwrap(await commands.saveVersionZip(entry.id)) }),
    onSuccess: ({ entry, saved }) => {
      if (saved) {
        notify({ tone: "success", message: t("history.zip.saved", { id: entry.shortId }) });
      }
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  // As the drawer's undo: the newest apply never asks; an older one may
  // answer `confirmation_required`, then the user confirms (SPEC 8.7).
  async function handleUndo(entry: HistoryEntry, confirmed: boolean) {
    if (!entry.apply) {
      return;
    }
    setUndoing(true);
    try {
      await undo(entry.apply.applyId, confirmed);
      setConfirming(null);
    } catch (error) {
      if (error instanceof IpcFailure && error.code === "confirmation_required") {
        setConfirming(entry);
      }
    } finally {
      setUndoing(false);
    }
  }

  const filtered = filter.scope !== "all" || filter.entity !== null;
  const rows = history.data ?? [];
  return (
    <>
      {history.isPending ? <p className="history-note">{t("history.loading")}</p> : null}
      {history.isError ? (
        <div className="panel-card empty-state page-empty-state" role="alert">
          <h3>{t("app.error.loadFailed")}</h3>
          <p>{failureText(history.error)}</p>
          <button className="secondary-button" type="button" onClick={() => void history.refetch()}>
            {t("app.action.retry")}
          </button>
        </div>
      ) : null}
      {history.data && rows.length === 0 ? (
        <div className="panel-card empty-state page-empty-state">
          <h3>{t("page.empty.title")}</h3>
          <p>{filtered ? t("history.empty.filtered") : t("page.empty.history")}</p>
        </div>
      ) : null}
      {rows.length > 0 ? (
        <ul className="history-list" aria-label={t("history.list.aria")}>
          {rows.map((entry) => (
            <HistoryRow
              key={entry.id}
              entry={entry}
              selected={entry.id === selectedId}
              diffOpen={diffOpen}
              busy={undoing || removeTag.isPending || zip.isPending}
              onSelect={() => {
                setSelectedId(entry.id === selectedId ? "" : entry.id);
                setDiffOpen(false);
              }}
              onToggleDiff={() => setDiffOpen((current) => !current)}
              onUndo={() => void handleUndo(entry, false)}
              onDialog={(kind) => setDialog({ kind, entry })}
              onZip={() => zip.mutate(entry)}
              onDeleteTag={(name) => removeTag.mutate(name)}
            />
          ))}
        </ul>
      ) : null}
      {dialog?.kind === "tag" ? <TagDialog entry={dialog.entry} onClose={() => setDialog(null)} /> : null}
      {dialog && dialog.kind !== "tag" ? (
        <RestoreDialog
          entry={dialog.entry}
          pick={dialog.kind === "restoreEntity"}
          onClose={() => setDialog(null)}
          onRestored={() => {
            setDialog(null);
            onOpenPending();
          }}
        />
      ) : null}
      {confirming ? (
        <div className="dialog-backdrop" role="presentation" onClick={() => setConfirming(null)}>
          <div
            className="tool-manage-dialog project-dialog"
            role="alertdialog"
            aria-modal="true"
            aria-label={t("apply.undo.action")}
            onClick={(event) => event.stopPropagation()}
          >
            <p>{t("apply.undo.confirmMessage")}</p>
            <p className="project-dialog__hint">{confirming.summary}</p>
            <div className="tool-manage-dialog__actions">
              <button
                className="secondary-button secondary-button--compact"
                type="button"
                onClick={() => setConfirming(null)}
              >
                {t("history.cancel")}
              </button>
              <button
                className="primary-button primary-button--compact"
                type="button"
                disabled={undoing}
                onClick={() => void handleUndo(confirming, true)}
              >
                {t("apply.undo.confirmAction")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}

export function HistoryPage({ onOpenPending, onOpenSettings }: HistoryPageProps) {
  const [scope, setScope] = useState<HistoryScope>("all");
  const [entityDraft, setEntityDraft] = useState("");
  const [entity, setEntity] = useState("");
  const [tab, setTab] = useState<Tab>("changes");
  const [taggingCurrent, setTaggingCurrent] = useState(false);
  const filter: HistoryFilter = { scope, entity: entity === "" ? null : entity, limit: null };

  return (
    <div className="history-route">
      <HeaderToolbar>
        <section className="history-toolbar skills-header-bar__tools" aria-label={t("history.toolbar.aria")}>
          <AppSelect
            className="history-toolbar__scope"
            value={scope}
            options={SCOPE_OPTIONS}
            onChange={setScope}
            ariaLabel={t("history.toolbar.scope")}
          />
          <label className="search-field search-field--header history-toolbar__entity">
            <span className="sr-only">{t("history.toolbar.entity")}</span>
            <SearchFieldIcon />
            <input
              type="search"
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="none"
              spellCheck={false}
              aria-label={t("history.toolbar.entity")}
              placeholder={t("history.toolbar.entityPlaceholder")}
              value={entityDraft}
              onChange={(event) => setEntityDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  setEntity(entityDraft.trim());
                }
              }}
              onBlur={() => setEntity(entityDraft.trim())}
            />
          </label>
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            onClick={() => setTaggingCurrent(true)}
          >
            {t("history.toolbar.tagCurrent")}
          </button>
          <button className="secondary-button secondary-button--compact" type="button" onClick={onOpenSettings}>
            {t("history.toolbar.remotes")}
          </button>
        </section>
      </HeaderToolbar>
      <div className="skills-source-tabs history-tabs" role="tablist" aria-label={t("history.tabs.aria")}>
        {TABS.map((item) => (
          <button
            key={item.id}
            id={`history-tab-${item.id}`}
            className={`skills-source-tab${tab === item.id ? " is-selected" : ""}`}
            type="button"
            role="tab"
            aria-selected={tab === item.id}
            aria-controls="history-tab-panel"
            onClick={() => setTab(item.id)}
          >
            {t(item.label)}
          </button>
        ))}
      </div>
      <section
        id="history-tab-panel"
        className="history-section"
        role="tabpanel"
        aria-labelledby={`history-tab-${tab}`}
      >
        {tab === "changes" ? <ChangesTab filter={filter} onOpenPending={onOpenPending} /> : <FileBackupsTab />}
      </section>
      <RemotesStrip onOpenSettings={onOpenSettings} />
      {taggingCurrent ? <TagDialog entry={null} onClose={() => setTaggingCurrent(false)} /> : null}
    </div>
  );
}
