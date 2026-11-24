// History page (SPEC 13.5): every store commit newest first, filtered by
// scope and entity, with per-commit actions (Show diff, Undo this apply,
// Restore this version, Restore entity..., Tag, Download zip), user tags,
// the client file backups tab, and the remotes and conflicts strip. The list
// loads in pages of `HISTORY_PAGE_SIZE` with "Load more" (a cursor, `before`).

import { useInfiniteQuery, useMutation, useQuery } from "@tanstack/react-query";
import { memo, useCallback, useId, useState } from "react";
import { storeQueryKeys, useInvalidateStore, useUndoApply } from "@/app/apply/changeNotice";
import { AppSelect, type AppSelectOption } from "@/app/components/AppSelect";
import { collapsibleAction, HEADER_ACTION_LABEL, HeaderToolbar } from "@/app/components/HeaderSlots";
import { Modal } from "@/app/components/Modal";
import { useNotifications } from "@/app/components/notifications";
import { SearchFieldIcon } from "@/app/components/SearchFieldIcon";
import { useEventCallback } from "@/app/components/useEventCallback";
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

/** Commits per page: a large store has thousands. */
export const HISTORY_PAGE_SIZE = 200;

/** The scope and entity the user picked; the page adds the limit and the cursor. */
type ListFilter = Omit<HistoryFilter, "limit" | "before">;

type RowProps = {
  entry: HistoryEntry;
  selected: boolean;
  /** The diff of this row is open (only a selected row has one). */
  diffOpen: boolean;
  busy: boolean;
  /** Stable across renders and taking the entry, so the memoized row re-renders only when its own props change. */
  onSelectEntry: (entry: HistoryEntry) => void;
  onToggleDiff: () => void;
  onUndoEntry: (entry: HistoryEntry) => void;
  onDialogEntry: (kind: Dialog["kind"], entry: HistoryEntry) => void;
  onZipEntry: (entry: HistoryEntry) => void;
  onDeleteTag: (name: string) => void;
};

/** A row's content; its `li` is the virtual slot (#17). */
const HistoryRow = memo(function HistoryRow({
  entry,
  selected,
  diffOpen,
  busy,
  onSelectEntry,
  onToggleDiff,
  onUndoEntry,
  onDialogEntry,
  onZipEntry,
  onDeleteTag,
}: RowProps) {
  const onSelect = () => onSelectEntry(entry);
  const onUndo = () => onUndoEntry(entry);
  const onDialog = (kind: Dialog["kind"]) => onDialogEntry(kind, entry);
  const onZip = () => onZipEntry(entry);
  return (
    <>
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
    </>
  );
});

function ChangesTab({ filter, onOpenPending }: { filter: ListFilter; onOpenPending: () => void }) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const undo = useUndoApply();
  // A new filter is a new query, so it starts again at the first page. Each page asks for one commit
  // more than it shows, to know whether "Load more" has anything to load, and the next page starts
  // after the last commit shown (`before`), so loading more never fetches the loaded pages again.
  const history = useInfiniteQuery({
    queryKey: [...storeQueryKeys.history, filter],
    queryFn: async ({ pageParam }) =>
      unwrap(await commands.listHistory({ ...filter, limit: HISTORY_PAGE_SIZE + 1, before: pageParam })),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => (last.length > HISTORY_PAGE_SIZE ? last[HISTORY_PAGE_SIZE - 1].id : undefined),
  });
  const [selectedId, setSelectedId] = useState("");
  const [diffOpen, setDiffOpen] = useState(false);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [confirming, setConfirming] = useState<HistoryEntry | null>(null);
  const [undoing, setUndoing] = useState(false);
  const undoQuestionId = useId();

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
  const pages = history.data?.pages ?? [];
  const rows = pages.flatMap((page) => page.slice(0, HISTORY_PAGE_SIZE));
  const hasMore = history.hasNextPage;
  // Rows are plain list items, loaded 200 at a time (#17): nothing mounts or unmounts while scrolling,
  // and neither virtualization nor `content-visibility` beat that (perf section 11). Memoized rows with
  // stable callbacks keep a selection or a page load from re-rendering the others.
  const handleSelect = useCallback((entry: HistoryEntry) => {
    setSelectedId((current) => (current === entry.id ? "" : entry.id));
    setDiffOpen(false);
  }, []);
  const handleToggleDiff = useCallback(() => setDiffOpen((current) => !current), []);
  const handleUndoEntry = useEventCallback((entry: HistoryEntry) => void handleUndo(entry, false));
  const handleDialog = useCallback((kind: Dialog["kind"], entry: HistoryEntry) => setDialog({ kind, entry }), []);
  const handleZip = useEventCallback((entry: HistoryEntry) => zip.mutate(entry));
  const handleDeleteTag = useEventCallback((name: string) => removeTag.mutate(name));
  const busy = undoing || removeTag.isPending || zip.isPending;
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
          {rows.map((entry) => {
            const selected = entry.id === selectedId;
            return (
              <li
                key={entry.id}
                className={`panel-card history-row${selected ? " is-selected" : ""}`}
                aria-label={entry.summary}
              >
                <HistoryRow
                  entry={entry}
                  selected={selected}
                  diffOpen={selected && diffOpen}
                  busy={busy}
                  onSelectEntry={handleSelect}
                  onToggleDiff={handleToggleDiff}
                  onUndoEntry={handleUndoEntry}
                  onDialogEntry={handleDialog}
                  onZipEntry={handleZip}
                  onDeleteTag={handleDeleteTag}
                />
              </li>
            );
          })}
        </ul>
      ) : null}
      {pages.length > 1 ? (
        <p className="sr-only" role="status">
          {t("history.shown", { count: rows.length })}
        </p>
      ) : null}
      {hasMore ? (
        <div className="history-load-more">
          {/* `aria-disabled` while loading: the button keeps the focus, so the keyboard can load more again. */}
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            aria-disabled={history.isFetchingNextPage}
            onClick={() => {
              if (!history.isFetchingNextPage) {
                void history.fetchNextPage();
              }
            }}
          >
            {history.isFetchingNextPage ? t("history.loading") : t("history.loadMore")}
          </button>
        </div>
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
        <Modal
          className="tool-manage-dialog project-dialog"
          role="alertdialog"
          label={t("apply.undo.action")}
          describedBy={undoQuestionId}
          onClose={() => setConfirming(null)}
        >
          <p id={undoQuestionId}>{t("apply.undo.confirmMessage")}</p>
          <p className="project-dialog__hint">{confirming.summary}</p>
          <div className="tool-manage-dialog__actions">
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              onClick={() => setConfirming(null)}
              data-autofocus
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
        </Modal>
      ) : null}
    </>
  );
}

function TagIcon() {
  return (
    <svg className="skills-toolbar-button__svg" viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path
        d="M3.75 4.75v4.4c0 .4.16.78.44 1.06l5.6 5.6a1.5 1.5 0 0 0 2.12 0l3.4-3.4a1.5 1.5 0 0 0 0-2.12l-5.6-5.6a1.5 1.5 0 0 0-1.06-.44h-4.4a.5.5 0 0 0-.5.5Z"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
      <circle cx="7" cy="7.5" r="1.1" fill="currentColor" />
    </svg>
  );
}

function RemotesIcon() {
  return (
    <svg className="skills-toolbar-button__svg" viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path
        d="M6 15.25h8.25a3.25 3.25 0 0 0 .4-6.48 4.75 4.75 0 0 0-9.15.9A2.8 2.8 0 0 0 6 15.25Z"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function HistoryPage({ onOpenPending, onOpenSettings }: HistoryPageProps) {
  const [scope, setScope] = useState<HistoryScope>("all");
  const [entityDraft, setEntityDraft] = useState("");
  const [entity, setEntity] = useState("");
  const [tab, setTab] = useState<Tab>("changes");
  const [taggingCurrent, setTaggingCurrent] = useState(false);
  const filter: ListFilter = { scope, entity: entity === "" ? null : entity };

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
            className="secondary-button secondary-button--compact skills-toolbar-button"
            type="button"
            onClick={() => setTaggingCurrent(true)}
            {...collapsibleAction(t("history.toolbar.tagCurrent"), "late")}
          >
            <span aria-hidden="true" className="skills-toolbar-button__icon">
              <TagIcon />
            </span>
            <span className={HEADER_ACTION_LABEL}>{t("history.toolbar.tagCurrent")}</span>
          </button>
          <button
            className="secondary-button secondary-button--compact skills-toolbar-button"
            type="button"
            onClick={onOpenSettings}
            {...collapsibleAction(t("history.toolbar.remotes"), "early")}
          >
            <span aria-hidden="true" className="skills-toolbar-button__icon">
              <RemotesIcon />
            </span>
            <span className={HEADER_ACTION_LABEL}>{t("history.toolbar.remotes")}</span>
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
