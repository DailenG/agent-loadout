// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Ported from DailenG/skilldock@181b52b src/features/skills/components/SkillSourceView.tsx: one
// row per entry of a tool's skill folder with its management status badge (managed, unmanaged,
// conflict), import, open folder, two-click delete and expandable details. Delete moves the entry
// to the trash and never deletes (SPEC 12.5); managed entries link to their store skill instead.
// The agents CLI list (SPEC 12.4) is new. Adopt (one folder, or every name with one content)
// replaces real folders with links to the store (#3).

import { useEffect, useRef, useState } from "react";
import { t } from "@/i18n/en";
import type { AgentsCliList, SkillScope, ToolSkillDir, ToolSkillEntry } from "@/ipc/bindings";
import { IpcFailure } from "@/ipc/transport";
import { AdoptDialog } from "../import/skills/AdoptDialog";
import { DeleteIcon } from "../mcp/icons";
import { useSkillActions } from "./actions";
import { FolderIcon, ImportIcon, SkillMonogram } from "./icons";
import { ENTRY_CLASS_KEYS, ENTRY_CLASS_TONES, monogram } from "./labels";

type EntryRowProps = {
  dir: ToolSkillDir;
  entry: ToolSkillEntry;
  scope: SkillScope;
  expanded: boolean;
  onExpandedChange: (expanded: boolean) => void;
  onShowManaged: (skillId: string) => void;
  /** Opens the adopt dialog for this folder. */
  onAdopt: () => void;
  /** Asks the user before importing under a new id; resolves true to go on. */
  confirmRename: () => Promise<boolean>;
};

function EntryRow({ dir, entry, scope, expanded, onExpandedChange, onShowManaged, onAdopt, confirmRename }: EntryRowProps) {
  const actions = useSkillActions();
  const [isImporting, setIsImporting] = useState(false);
  const [isTrashConfirming, setIsTrashConfirming] = useState(false);
  const [isTrashing, setIsTrashing] = useState(false);
  const trashRef = useRef<HTMLButtonElement | null>(null);
  const detailsId = `skill-source-${dir.client}-${entry.name}`;

  useEffect(() => {
    if (!isTrashConfirming) {
      return;
    }
    function handlePointerDown(event: PointerEvent) {
      if (!trashRef.current?.contains(event.target as Node)) {
        setIsTrashConfirming(false);
      }
    }
    document.addEventListener("pointerdown", handlePointerDown);
    return () => document.removeEventListener("pointerdown", handlePointerDown);
  }, [isTrashConfirming]);

  async function handleImport() {
    setIsImporting(true);
    try {
      await actions.importEntry(dir.client, scope, entry.name, false);
    } catch (error) {
      if (error instanceof IpcFailure && error.code === "already_exists" && (await confirmRename())) {
        await actions.importEntry(dir.client, scope, entry.name, true).catch(() => undefined);
      }
    } finally {
      setIsImporting(false);
    }
  }

  async function handleTrash() {
    if (!isTrashConfirming) {
      setIsTrashConfirming(true);
      return;
    }
    setIsTrashing(true);
    try {
      await actions.trashEntry(dir.client, scope, entry.name);
    } catch {
      setIsTrashing(false);
      setIsTrashConfirming(false);
    }
  }

  const status = t(ENTRY_CLASS_KEYS[entry.class]);
  return (
    <article
      className={`skill-card skill-source-card skill-source-card--list${expanded ? " is-expanded" : ""}`}
      aria-label={entry.name}
    >
      <button
        className="skill-card__identity skill-source-card__summary-button"
        type="button"
        onClick={() => onExpandedChange(!expanded)}
        aria-expanded={expanded}
        aria-controls={detailsId}
      >
        <SkillMonogram label={monogram(entry.name)} />
        <div className="skill-card__title-stack">
          <div className="skill-card__title-row">
            <h3>{entry.name}</h3>
            <span className={`status-badge ${ENTRY_CLASS_TONES[entry.class]}`}>{status}</span>
            {entry.class !== "managed" ? (
              <span className="status-badge tone-info">
                {entry.linkTarget ? t("skills.source.entryKind.link") : t("skills.source.entryKind.directory")}
              </span>
            ) : null}
          </div>
          <p className="skill-card__summary-description">{entry.description ?? t("skills.description.empty")}</p>
        </div>
      </button>
      <div className="skill-card__list-actions">
        {entry.class === "managed" && entry.skillId ? (
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            onClick={() => onShowManaged(entry.skillId!)}
          >
            {t("skills.source.showManaged")}
          </button>
        ) : null}
        {entry.class !== "managed" && !entry.linkTarget ? (
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            onClick={onAdopt}
            aria-label={t("skills.source.adoptAria", { name: entry.name })}
            data-tooltip={t("skills.source.adoptTooltip")}
          >
            {t("skills.source.adopt")}
          </button>
        ) : null}
        {entry.class === "unmanaged" && !entry.linkTarget ? (
          <button
            className="skill-card__icon-button skill-card__icon-button--update"
            type="button"
            disabled={isImporting}
            onClick={() => void handleImport()}
            aria-label={isImporting ? t("skills.source.importing") : t("skills.source.import")}
            data-tooltip={isImporting ? t("skills.source.importing") : t("skills.source.import")}
          >
            <ImportIcon />
          </button>
        ) : null}
        <button
          className="skill-card__icon-button"
          type="button"
          onClick={() => void actions.revealTool(dir.client, scope).catch(() => undefined)}
          aria-label={t("skills.source.openToolFolder", { tool: dir.name })}
          data-tooltip={t("skills.source.openFolder")}
        >
          <FolderIcon />
        </button>
        {entry.class !== "managed" ? (
          isTrashConfirming || isTrashing ? (
            <button
              ref={trashRef}
              className="skill-card__delete-confirm-button"
              type="button"
              onClick={() => void handleTrash()}
              disabled={isTrashing}
              aria-label={t("skill.card.aria.deleteConfirm", {
                state: isTrashing ? t("skill.card.deleteLoading") : t("skill.card.deleteConfirm"),
                name: entry.name,
              })}
            >
              {isTrashing ? t("skill.card.deleteLoading") : t("skill.card.deleteConfirm")}
            </button>
          ) : (
            <button
              ref={trashRef}
              className="skill-card__icon-button skill-card__icon-button--delete"
              type="button"
              onClick={() => void handleTrash()}
              aria-label={t("skills.source.trash", { name: entry.name })}
              data-tooltip={t("skills.source.trashTooltip")}
            >
              <DeleteIcon />
            </button>
          )
        ) : null}
        <button
          className="skill-card__chevron-button"
          type="button"
          onClick={() => onExpandedChange(!expanded)}
          aria-expanded={expanded}
          aria-controls={detailsId}
          aria-label={t("skills.source.details.toggle", {
            state: expanded ? t("skill.card.collapse") : t("skill.card.expand"),
            name: entry.name,
          })}
        >
          <span className="skill-card__chevron" aria-hidden="true">{expanded ? "⌄" : "›"}</span>
        </button>
      </div>
      {expanded ? (
        <div id={detailsId} className="skill-card__details skill-source-card__details">
          <dl className="detail-grid detail-grid--source">
            <div>
              <dt>{t("skills.source.details.managementStatus")}</dt>
              <dd>{status}</dd>
            </div>
            <div>
              <dt>{t("skills.source.details.localPath")}</dt>
              <dd className="detail-grid__single-line" data-tooltip={entry.path}>{entry.path}</dd>
            </div>
            {entry.linkTarget ? (
              <div>
                <dt>{t("skills.source.details.linkTarget")}</dt>
                <dd className="detail-grid__single-line" data-tooltip={entry.linkTarget}>{entry.linkTarget}</dd>
              </div>
            ) : null}
            {entry.skillId ? (
              <div>
                <dt>{t("skills.source.details.storeSkill")}</dt>
                <dd>{entry.skillId}</dd>
              </div>
            ) : null}
          </dl>
        </div>
      ) : null}
    </article>
  );
}

type ToolSkillListProps = {
  dir: ToolSkillDir;
  scope: SkillScope;
  query: string;
  onShowManaged: (skillId: string) => void;
  confirmRename: () => Promise<boolean>;
};

/** One client's skill folder (SPEC 12.5). */
export function ToolSkillList({ dir, scope, query, onShowManaged, confirmRename }: ToolSkillListProps) {
  const [expanded, setExpanded] = useState("");
  // `""`: every name with one content; a name: that folder.
  const [adopting, setAdopting] = useState<string | null>(null);
  const needle = query.trim().toLowerCase();
  const rows = dir.entries.filter(
    (entry) => !needle || [entry.name, entry.description ?? "", entry.path].join(" ").toLowerCase().includes(needle),
  );
  const hasRealFolders = dir.entries.some((entry) => entry.class !== "managed" && !entry.linkTarget);

  return (
    <>
      <div className="skills-source-folder-row">
        <p className="skills-source-folder" title={dir.dir}>
          {dir.exists
            ? t("skills.source.folder", { path: dir.dir })
            : t("skills.source.folderMissing", { tool: dir.name, path: dir.dir })}
        </p>
        {hasRealFolders ? (
          <button className="secondary-button secondary-button--compact" type="button" onClick={() => setAdopting("")}>
            {t("skills.source.adoptAll")}
          </button>
        ) : null}
      </div>
      <div className="card-list">
        {rows.length > 0 ? (
          rows.map((entry) => (
            <EntryRow
              key={entry.name}
              dir={dir}
              entry={entry}
              scope={scope}
              expanded={expanded === entry.name}
              onExpandedChange={(open) => setExpanded(open ? entry.name : "")}
              onShowManaged={onShowManaged}
              onAdopt={() => setAdopting(entry.name)}
              confirmRename={confirmRename}
            />
          ))
        ) : (
          <div className="panel-card empty-state">
            <h3>{t("skills.source.empty.title")}</h3>
            <p>{t("skills.source.empty.description", { tool: dir.name })}</p>
          </div>
        )}
      </div>
      {adopting !== null ? (
        <AdoptDialog
          title={
            adopting
              ? t("skills.source.adoptDialog.title", { name: adopting })
              : t("skills.source.adoptAllDialog.title", { tool: dir.name })
          }
          scope={scope}
          client={dir.client}
          name={adopting || undefined}
          onClose={() => setAdopting(null)}
        />
      ) : null}
    </>
  );
}

type AgentsCliListViewProps = {
  list: AgentsCliList;
  query: string;
  confirmRename: () => Promise<boolean>;
};

/** `~/.agents/skills`, read only, with "Vendor into store" (SPEC 12.4). */
export function AgentsCliListView({ list, query, confirmRename }: AgentsCliListViewProps) {
  const actions = useSkillActions();
  const [busy, setBusy] = useState("");
  const needle = query.trim().toLowerCase();
  const rows = list.skills.filter(
    (skill) => !needle || [skill.name, skill.description ?? "", skill.source ?? ""].join(" ").toLowerCase().includes(needle),
  );

  async function vendor(name: string) {
    setBusy(name);
    try {
      await actions.vendor(name, false);
    } catch (error) {
      if (error instanceof IpcFailure && error.code === "already_exists" && (await confirmRename())) {
        await actions.vendor(name, true).catch(() => undefined);
      }
    } finally {
      setBusy("");
    }
  }

  async function runCli(action: "update" | "remove", name: string) {
    setBusy(name);
    try {
      await actions.agentsCli(action, name);
    } catch {
      // Reported by the action.
    } finally {
      setBusy("");
    }
  }

  return (
    <>
      <p className="skills-source-folder" title={list.dir}>
        {t("skills.agents.description", { path: list.dir })}
      </p>
      {!list.cliFound ? <p className="mcp-page-warning" role="note">{t("skills.agents.cliMissing")}</p> : null}
      <div className="card-list">
        {rows.length > 0 ? (
          rows.map((skill) => (
            <article key={skill.name} className="skill-card skill-source-card skill-source-card--list" aria-label={skill.name}>
              <div className="skill-card__identity skill-source-card__summary-button">
                <SkillMonogram label={monogram(skill.name)} />
                <div className="skill-card__title-stack">
                  <div className="skill-card__title-row">
                    <h3>{skill.name}</h3>
                    {skill.source ? <span className="status-badge tone-neutral">{skill.source}</span> : null}
                    {skill.isLink ? <span className="status-badge tone-info">{t("skills.source.entryKind.link")}</span> : null}
                  </div>
                  <p className="skill-card__summary-description">{skill.description ?? t("skills.description.empty")}</p>
                </div>
              </div>
              <div className="skill-card__list-actions">
                <button
                  className="skill-card__icon-button skill-card__icon-button--update"
                  type="button"
                  disabled={busy === skill.name}
                  onClick={() => void vendor(skill.name)}
                  aria-label={t("skills.agents.vendorAria", { name: skill.name })}
                  data-tooltip={t("skills.agents.vendor")}
                >
                  <ImportIcon />
                </button>
                {list.cliFound ? (
                  <>
                    <button
                      className="secondary-button secondary-button--compact"
                      type="button"
                      disabled={busy === skill.name}
                      onClick={() => void runCli("update", skill.name)}
                    >
                      {t("skills.agents.update")}
                    </button>
                    <button
                      className="secondary-button secondary-button--compact"
                      type="button"
                      disabled={busy === skill.name}
                      onClick={() => void runCli("remove", skill.name)}
                    >
                      {t("skills.agents.remove")}
                    </button>
                  </>
                ) : null}
              </div>
            </article>
          ))
        ) : (
          <div className="panel-card empty-state">
            <p>{t("skills.agents.empty")}</p>
          </div>
        )}
      </div>
    </>
  );
}
