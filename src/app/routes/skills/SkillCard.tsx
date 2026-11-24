// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Ported from DailenG/skilldock@181b52b src/features/skills/components/SkillCard.tsx: card
// markup for list and grid layouts, monogram, enabled-tools badge and summary icons, tag
// editor popover, power toggle, two-click delete, the expanded details and the grid detail
// modal. The data layer is new (typed commands over TanStack Query); details show every
// frontmatter field, the source, owned links and problems (SPEC 12.1, 13.7), and a project
// scope shows the project toggle instead of the tool pills (SPEC 12.3).

import { memo, useEffect, useRef, useState, type FormEvent, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { orderByRecommended } from "@/app/clients";
import { Modal } from "@/app/components/Modal";
import { toolLogoUrl } from "@/app/components/ToolLogo";
import { errorMessage, t } from "@/i18n/en";
import type { SkillClient, SkillInfo, SkillProject, SkillUpdateCheck } from "@/ipc/bindings";
import type { TrustAnswer } from "@/app/components/TrustDialog";
import { DeleteIcon, PowerToggleIcon, RefreshIcon } from "../mcp/icons";
import { useSkillActions } from "./actions";
import { OpenFolderIcon, SkillMonogram, ViewFileIcon } from "./icons";
import { formatSize, LINK_METHOD_KEYS, ORIGIN_KEYS, monogram } from "./labels";
import { SkillFileDialog, SkillUpdateDialog } from "./SkillFileDialog";
import { ProjectSkillPanel, ToolSyncPanel } from "./ToolSyncPanel";

const GRID_SUMMARY_TOOL_LIMIT = 6;
const MAX_TAG_LENGTH = 20;

function SummaryToolIcon({ client }: { client: SkillClient }) {
  const [failed, setFailed] = useState(false);
  const url = toolLogoUrl(client.id);
  return (
    <span className="skill-card__tool-icon" title={client.name} aria-hidden="true">
      {url && !failed ? <img src={url} alt="" onError={() => setFailed(true)} /> : monogram(client.name)}
    </span>
  );
}

export type SkillCardProps = {
  skill: SkillInfo;
  clients: SkillClient[];
  /** The project scope, when the page shows one. */
  project: SkillProject | null;
  layout?: "list" | "grid";
  expanded: boolean;
  /** Stable across renders (the card is memoized), so it takes the skill's ID. */
  onExpandedChange: (skillId: string, expanded: boolean) => void;
  /** The last update check of this skill, if any. */
  update?: SkillUpdateCheck;
  /** The update of `update` went through: its entry is stale now. Stable, like `onExpandedChange`. */
  onUpdated: (skillId: string) => void;
  /** Every tag in the store, for the tag editor's suggestions. */
  allTags: string[];
  /**
   * Called with `true` while the card has a dialog or the tag editor open, or a toggle, update or delete
   * under way, and `false` after: the list keeps such a card rendered (#17). Stable, like `onExpandedChange`.
   */
  onActiveChange?: (skillId: string, active: boolean) => void;
};

// Memoized: a refetch or a search keystroke re-renders only the cards whose props changed.
export const SkillCard = memo(function SkillCard(props: SkillCardProps) {
  const { skill, clients, project, layout = "list", expanded, update, allTags } = props;
  const onExpandedChange = (open: boolean) => props.onExpandedChange(skill.id, open);
  const actions = useSkillActions();
  const isGrid = layout === "grid";
  const [isBulkUpdating, setIsBulkUpdating] = useState(false);
  const [isDeleteConfirming, setIsDeleteConfirming] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);
  const [showEnabledTools, setShowEnabledTools] = useState(false);
  const [dialog, setDialog] = useState<"files" | "update" | null>(null);
  const [showTagEditor, setShowTagEditor] = useState(false);
  const [tagDraft, setTagDraft] = useState("");
  const [tagPosition, setTagPosition] = useState<{ top: number; left: number }>({ top: 0, left: 0 });
  const deleteActionRef = useRef<HTMLButtonElement | null>(null);
  const tagActionRef = useRef<HTMLButtonElement | null>(null);
  const tagEditorRef = useRef<HTMLDivElement | null>(null);
  const active = isBulkUpdating || isDeleteConfirming || isDeleting || isUpdating || dialog !== null || showTagEditor;
  const { onActiveChange } = props;
  useEffect(() => {
    if (!active || !onActiveChange) {
      return;
    }
    onActiveChange(skill.id, true);
    return () => onActiveChange(skill.id, false);
  }, [active, onActiveChange, skill.id]);

  const enabledClients = orderByRecommended(
    project
      ? skill.projectEnabled
        ? clients.filter((client) => project.clients.includes(client.id))
        : []
      : clients.filter((client) => skill.clients.includes(client.id)),
    (client) => client.id,
  );
  const detected = clients.filter((client) => client.detected);
  const allEnabled = project
    ? skill.projectEnabled
    : detected.length > 0 && detected.every((client) => skill.clients.includes(client.id));
  const partial = !project && enabledClients.length > 0 && !allEnabled;
  const updateAvailable = update?.status === "available";
  const unreachable = update?.status === "source_unreachable" || update?.status === "failed";
  const description = skill.description ?? t("skills.description.empty");
  const originLabel = skill.source ? t(ORIGIN_KEYS[skill.source.kind]) : t("skill.card.sourceMethod.none");
  const enabledLabel = project
    ? skill.projectEnabled
      ? t("skill.card.enabledInProject")
      : t("skill.card.disabled")
    : enabledClients.length > 0
      ? t("skill.card.enabledCount", { count: enabledClients.length })
      : t("skill.card.disabled");
  const summaryToolsLabel = enabledClients.length > 0
    ? t("skill.card.enabledTools", { tools: enabledClients.map((client) => client.name).join(", ") })
    : t("skill.card.enabledToolsNone");
  const extraFields = skill.fields.filter((field) => field.key !== "name" && field.key !== "description");

  useEffect(() => {
    if (!isDeleteConfirming) {
      return;
    }
    function handlePointerDown(event: PointerEvent) {
      if (!deleteActionRef.current?.contains(event.target as Node)) {
        setIsDeleteConfirming(false);
      }
    }
    document.addEventListener("pointerdown", handlePointerDown);
    return () => document.removeEventListener("pointerdown", handlePointerDown);
  }, [isDeleteConfirming]);

  useEffect(() => {
    if (!showTagEditor) {
      return;
    }
    function handlePointerDown(event: PointerEvent) {
      const target = event.target as Node;
      if (!tagEditorRef.current?.contains(target) && !tagActionRef.current?.contains(target)) {
        setShowTagEditor(false);
      }
    }
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setShowTagEditor(false);
        // The editor (a popover, not a modal) took focus: give it back to its button.
        tagActionRef.current?.focus();
      }
    }
    document.addEventListener("pointerdown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [showTagEditor]);

  async function handleDelete() {
    if (isDeleting) {
      return;
    }
    if (!isDeleteConfirming) {
      setIsDeleteConfirming(true);
      return;
    }
    setIsDeleting(true);
    try {
      await actions.remove(skill.id, skill.name);
    } catch {
      setIsDeleting(false);
      setIsDeleteConfirming(false);
    }
  }

  async function handlePower() {
    if (isBulkUpdating) {
      return;
    }
    setIsBulkUpdating(true);
    try {
      if (project) {
        await actions.setProject(project.id, skill.id, skill.name, !skill.projectEnabled, project.hash);
      } else {
        await actions.setClients(skill.id, skill.name, [], !allEnabled);
      }
    } catch {
      // Reported by the action.
    } finally {
      setIsBulkUpdating(false);
    }
  }

  async function handleUpdate(allowDangerous: boolean, answer: TrustAnswer) {
    setIsUpdating(true);
    try {
      await actions.update(skill.id, skill.name, update?.latest ?? "", allowDangerous, answer);
      setDialog(null);
      props.onUpdated(skill.id);
    } catch {
      // Reported by the action.
    } finally {
      setIsUpdating(false);
    }
  }

  function openTagEditor() {
    const rect = tagActionRef.current?.getBoundingClientRect();
    if (rect) {
      setTagPosition({ top: rect.bottom + 6, left: Math.max(8, rect.left) });
    }
    setTagDraft("");
    setShowTagEditor((open) => !open);
  }

  async function saveTags(tags: string[]) {
    try {
      await actions.setTags(skill.id, skill.name, tags);
      setTagDraft("");
    } catch {
      // Reported by the action.
    }
  }

  function handleTagSubmit(event: FormEvent) {
    event.preventDefault();
    const tag = tagDraft.trim();
    if (tag && !skill.tags.some((known) => known.toLowerCase() === tag.toLowerCase())) {
      void saveTags([...skill.tags, tag]);
    }
  }

  function handleSummaryKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
      event.preventDefault();
      onExpandedChange(!expanded);
    }
  }

  const powerLabel = isBulkUpdating
    ? t("skill.card.bulkToggle.updating", { name: skill.name })
    : project
      ? t(skill.projectEnabled ? "skill.card.bulkToggle.projectOn" : "skill.card.bulkToggle.projectOff", { name: skill.name })
      : allEnabled
        ? t("skill.card.bulkToggle.disable", { name: skill.name })
        : partial
          ? t("skill.card.bulkToggle.partial", { enabled: enabledClients.length, total: detected.length })
          : t("skill.card.bulkToggle.enable", { name: skill.name });
  const powerState = allEnabled ? "is-enabled" : partial ? "is-partial" : "is-disabled";
  const powerDisabled = isBulkUpdating || isDeleting || !skill.hasContent
    || (project !== null
      && (project.trust !== "trusted" || project.readOnly !== null || (skill.projectEnabled && skill.projectShared)));

  const tagText = skill.tags.join(", ");
  const tagAction = (
    <button
      ref={tagActionRef}
      className={`skill-card__tag${tagText ? " has-tag tag-tone-neutral" : ""}${showTagEditor ? " is-editor-open" : ""}`}
      type="button"
      aria-label={tagText ? t("skill.card.tag.edit", { tag: tagText }) : t("skill.card.tag.add")}
      aria-expanded={showTagEditor}
      onClick={(event) => {
        event.stopPropagation();
        openTagEditor();
      }}
    >
      {tagText || t("skill.card.tag.addShort")}
    </button>
  );

  const updateBadge = updateAvailable ? (
    <span className="status-badge tone-positive">{t("skill.card.update.available")}</span>
  ) : update?.status === "source_blocked" ? (
    <span className="status-badge tone-danger">{t("skill.card.update.blocked")}</span>
  ) : unreachable ? (
    <span className="status-badge tone-warning">{t("skill.card.update.unreachable")}</span>
  ) : null;

  const deleteAction = isDeleteConfirming || isDeleting ? (
    <button
      ref={deleteActionRef}
      className="skill-card__delete-confirm-button"
      type="button"
      onClick={() => void handleDelete()}
      aria-label={t("skill.card.aria.deleteConfirm", {
        state: isDeleting ? t("skill.card.deleteLoading") : t("skill.card.deleteConfirm"),
        name: skill.name,
      })}
      data-tooltip={isDeleting ? t("skill.card.tooltip.deleting") : t("skill.card.tooltip.deleteConfirm")}
      disabled={isDeleting}
    >
      {isDeleting ? t("skill.card.deleteLoading") : t("skill.card.deleteConfirm")}
    </button>
  ) : (
    <button
      ref={deleteActionRef}
      className="skill-card__icon-button skill-card__icon-button--delete"
      type="button"
      onClick={() => void handleDelete()}
      aria-label={t("skill.card.aria.delete", { name: skill.name })}
      data-tooltip={t("skill.card.tooltip.delete")}
    >
      <DeleteIcon />
    </button>
  );

  const source = skill.source;
  const details = (
    <>
      <section>
        <div className="skill-card__section-header">
          <h4>{t("skill.card.basicInfo")}</h4>
        </div>
        <dl className="detail-grid detail-grid--single">
          <div>
            <dt>{t("skill.card.description")}</dt>
            <dd>{description}</dd>
          </div>
        </dl>
        <dl className="detail-grid detail-grid--source">
          <div>
            <dt>{t("skill.card.sourceType")}</dt>
            <dd>{originLabel}</dd>
          </div>
          {source?.url ? (
            <div>
              <dt>{t("skill.card.sourceAddress")}</dt>
              <dd className="detail-grid__source-value">
                <span
                  className={`detail-grid__single-line${/^https?:\/\//i.test(source.url) ? " detail-grid__source-link" : ""}`}
                  data-tooltip={source.url}
                >
                  {source.url}
                </span>
              </dd>
            </div>
          ) : null}
          {source?.path ? (
            <div>
              <dt>{t("skill.card.sourcePath")}</dt>
              <dd className="detail-grid__single-line">{source.path}</dd>
            </div>
          ) : null}
          {source?.gitRef ? (
            <div>
              <dt>{t("skill.card.sourceRef")}</dt>
              <dd>{source.gitRef}</dd>
            </div>
          ) : null}
          {source?.commit ? (
            <div>
              <dt>{t("skill.card.sourceCommit")}</dt>
              <dd>{source.commit.slice(0, 7)}</dd>
            </div>
          ) : null}
          <div>
            <dt>{t("skill.card.size")}</dt>
            <dd>{formatSize(skill.bytes)}</dd>
          </div>
        </dl>
        {extraFields.length > 0 ? (
          <dl className="tool-list-row__detail-grid skill-card__frontmatter" aria-label={t("skill.card.frontmatter")}>
            {extraFields.map((field) => (
              <div key={field.key}>
                <dt>{field.key}</dt>
                <dd className="detail-grid__single-line" data-tooltip={field.value}>{field.value}</dd>
              </div>
            ))}
          </dl>
        ) : null}
        {skill.links.length > 0 ? (
          <dl className="tool-list-row__detail-grid skill-card__links" aria-label={t("skill.card.links")}>
            {skill.links.map((link) => (
              <div key={`${link.client}-${link.path}`}>
                <dt>{`${clients.find((client) => client.id === link.client)?.name ?? link.client} · ${t(LINK_METHOD_KEYS[link.method])}`}</dt>
                <dd className="detail-grid__single-line" data-tooltip={link.path}>{link.path}</dd>
              </div>
            ))}
          </dl>
        ) : null}
        {skill.problems.length > 0 ? (
          <ul className="skill-card__problems" aria-label={t("skill.card.problems")}>
            {skill.problems.map((problem, index) => (
              <li key={`${index}-${problem.code}`} className="mcp-page-warning">{errorMessage(problem.code)}</li>
            ))}
          </ul>
        ) : null}
      </section>
      {project ? (
        <ProjectSkillPanel skill={skill} project={project} clients={clients} />
      ) : (
        <ToolSyncPanel
          skill={skill}
          clients={clients}
          isBulkUpdatingExternally={isBulkUpdating}
          onBulkUpdatingChange={setIsBulkUpdating}
        />
      )}
    </>
  );

  const suggestions = allTags.filter(
    (tag) =>
      !skill.tags.some((known) => known.toLowerCase() === tag.toLowerCase())
      && tag.toLowerCase().includes(tagDraft.trim().toLowerCase()),
  );

  return (
    <>
      <article
        className={`skill-card skill-card--${layout}${expanded ? " is-expanded" : ""}`}
        aria-label={skill.name}
      >
        <div className="skill-card__header">
          <div
            className="skill-card__summary-button"
            role="button"
            tabIndex={0}
            aria-expanded={expanded}
            onClick={() => onExpandedChange(!expanded)}
            onKeyDown={handleSummaryKeyDown}
          >
            <div className="skill-card__summary-content">
              <div className="skill-card__summary-top">
                {isGrid ? <div className="skill-card__grid-status">{updateBadge}</div> : null}
                <div className="skill-card__identity">
                  <SkillMonogram label={monogram(skill.name)} />
                  <div className="skill-card__title-stack">
                    <div className="skill-card__title-row">
                      <h3>{skill.name}</h3>
                      {!isGrid ? (
                        <>
                          <span className="status-badge tone-neutral skill-card__owner-badge">{originLabel}</span>
                          <button
                            className={`status-badge tone-info skill-card__enabled-toggle${enabledClients.length > 0 ? "" : " is-empty"}`}
                            type="button"
                            onClick={(event) => {
                              event.stopPropagation();
                              setShowEnabledTools((open) => !open);
                            }}
                            aria-expanded={showEnabledTools}
                            aria-label={summaryToolsLabel}
                            disabled={enabledClients.length === 0}
                          >
                            {enabledLabel}
                          </button>
                          {tagAction}
                          {showEnabledTools && enabledClients.length > 0 ? (
                            <div className="skill-card__summary-tools" aria-label={summaryToolsLabel}>
                              {enabledClients.map((client) => (
                                <SummaryToolIcon key={client.id} client={client} />
                              ))}
                            </div>
                          ) : null}
                        </>
                      ) : null}
                    </div>
                    <p className="skill-card__summary-description">{description}</p>
                    {isGrid ? <div className="skill-card__tag-slot">{tagAction}</div> : null}
                    {isGrid ? (
                      <div className="skill-card__grid-meta">
                        <span className={`status-badge skill-card__grid-enabled-badge ${enabledClients.length > 0 ? "tone-info" : "tone-neutral"}`}>
                          {enabledLabel}
                        </span>
                        <div className="skill-card__summary-tools" aria-label={summaryToolsLabel}>
                          {enabledClients.slice(0, GRID_SUMMARY_TOOL_LIMIT).map((client) => (
                            <SummaryToolIcon key={client.id} client={client} />
                          ))}
                          {enabledClients.length > GRID_SUMMARY_TOOL_LIMIT ? (
                            <span className="skill-card__tool-tag skill-card__tool-tag--extra">
                              +{enabledClients.length - GRID_SUMMARY_TOOL_LIMIT}
                            </span>
                          ) : null}
                        </div>
                      </div>
                    ) : null}
                  </div>
                </div>
              </div>
            </div>
          </div>
          <div className="skill-card__list-actions">
            {isGrid ? (
              <span className="skill-card__grid-source-label">
                <span className="skill-card__grid-source-text">{originLabel}</span>
              </span>
            ) : (
              updateBadge
            )}
            {updateAvailable ? (
              <button
                className="skill-card__icon-button skill-card__icon-button--update"
                type="button"
                onClick={() => setDialog("update")}
                aria-label={t("skill.card.aria.update", { name: skill.name })}
                data-tooltip={isUpdating ? t("skill.card.tooltip.updating") : t("skill.card.tooltip.update")}
                disabled={isUpdating}
              >
                <RefreshIcon isSpinning={isUpdating} />
              </button>
            ) : null}
            <button
              className={`skill-card__icon-button plugins-page__toggle-icon-button ${powerState}`}
              type="button"
              onClick={() => void handlePower()}
              aria-label={powerLabel}
              data-tooltip={powerLabel}
              disabled={powerDisabled}
            >
              <PowerToggleIcon isSpinning={isBulkUpdating} />
            </button>
            <button
              className="skill-card__icon-button"
              type="button"
              onClick={() => setDialog("files")}
              aria-label={t("skill.card.aria.viewFiles", { name: skill.name })}
              data-tooltip={t("skill.card.tooltip.viewFiles")}
            >
              <ViewFileIcon />
            </button>
            <button
              className="skill-card__icon-button"
              type="button"
              onClick={() => void actions.revealSkill(skill.id).catch(() => undefined)}
              aria-label={t("skill.card.aria.openFolder", { name: skill.name })}
              data-tooltip={t("skill.card.tooltip.openFolder")}
            >
              <OpenFolderIcon />
            </button>
            {deleteAction}
            {!isGrid ? (
              <button
                className="skill-card__chevron-button"
                type="button"
                onClick={() => onExpandedChange(!expanded)}
                aria-expanded={expanded}
                aria-label={t("skill.card.aria.expand", {
                  state: expanded ? t("skill.card.collapse") : t("skill.card.expand"),
                  name: skill.name,
                })}
              >
                <span className="skill-card__chevron" aria-hidden="true">
                  {expanded ? "⌄" : "›"}
                </span>
              </button>
            ) : null}
          </div>
        </div>
        {expanded && !isGrid ? <div className="skill-card__details">{details}</div> : null}
      </article>
      {showTagEditor
        ? createPortal(
            <div
              ref={tagEditorRef}
              className="skill-tag-editor"
              role="dialog"
              aria-label={t("skill.card.tag.dialog", { name: skill.name })}
              style={{ position: "fixed", top: tagPosition.top, left: tagPosition.left }}
            >
              <form className="skill-tag-editor__form" onSubmit={handleTagSubmit}>
                <input
                  autoFocus
                  type="text"
                  maxLength={MAX_TAG_LENGTH}
                  value={tagDraft}
                  placeholder={t("skill.card.tag.placeholder")}
                  aria-label={t("skill.card.tag.placeholder")}
                  onChange={(event) => setTagDraft(event.target.value)}
                />
              </form>
              {tagDraft.trim() && !allTags.some((tag) => tag.toLowerCase() === tagDraft.trim().toLowerCase()) ? (
                <button className="skill-tag-editor__create" type="button" onClick={() => void saveTags([...skill.tags, tagDraft.trim()])}>
                  <span aria-hidden="true">+</span>
                  <span>{t("skill.card.tag.create", { tag: tagDraft.trim() })}</span>
                </button>
              ) : null}
              {suggestions.length > 0 ? (
                <div className="skill-tag-editor__suggestions">
                  <div className="skill-tag-editor__options">
                    {suggestions.map((tag) => (
                      <button key={tag} className="tag-tone-neutral" type="button" onClick={() => void saveTags([...skill.tags, tag])}>
                        {tag}
                      </button>
                    ))}
                  </div>
                </div>
              ) : null}
              {skill.tags.length > 0 ? (
                <div className="skill-tag-editor__suggestions">
                  <div className="skill-tag-editor__options">
                    {skill.tags.map((tag) => (
                      <button
                        key={tag}
                        className="tag-tone-neutral is-selected"
                        type="button"
                        aria-label={`${t("skill.card.tag.clear")}: ${tag}`}
                        onClick={() => void saveTags(skill.tags.filter((known) => known !== tag))}
                      >
                        {tag} ×
                      </button>
                    ))}
                  </div>
                </div>
              ) : null}
              {skill.tags.length > 0 ? (
                <button className="skill-tag-editor__clear" type="button" onClick={() => void saveTags([])}>
                  {t("skill.card.tag.clear")}
                </button>
              ) : null}
            </div>,
            document.body,
          )
        : null}
      {expanded && isGrid && dialog === null ? (
        <Modal
          className="skill-card-detail-modal"
          backdropClassName="skill-card-detail-modal__backdrop"
          element="section"
          label={t("skill.card.modal.aria", { name: skill.name })}
          initialFocus="dialog"
          portal
          onClose={() => onExpandedChange(false)}
        >
          <header className="skill-card-detail-modal__header">
            <div className="skill-card-detail-modal__identity">
              <SkillMonogram label={monogram(skill.name)} />
              <div className="skill-card-detail-modal__copy">
                <div className="skill-card-detail-modal__title">
                  <h3>{skill.name}</h3>
                  {updateBadge}
                </div>
              </div>
            </div>
            <div className="skill-card-detail-modal__actions">
              {updateAvailable ? (
                <button
                  className="secondary-button secondary-button--compact skill-card-detail-modal__action is-primary"
                  type="button"
                  onClick={() => setDialog("update")}
                >
                  <RefreshIcon isSpinning={isUpdating} />
                  <span>{t("skill.card.action.update")}</span>
                </button>
              ) : null}
              <button
                className="secondary-button secondary-button--compact skill-card-detail-modal__action"
                type="button"
                onClick={() => setDialog("files")}
              >
                <ViewFileIcon />
                <span>{t("skill.card.action.viewFiles")}</span>
              </button>
              <button
                className="secondary-button secondary-button--compact skill-card-detail-modal__action"
                type="button"
                onClick={() => void actions.revealSkill(skill.id).catch(() => undefined)}
              >
                <OpenFolderIcon />
                <span>{t("skill.card.action.openFolder")}</span>
              </button>
              <button
                className="skill-card-detail-modal__close"
                type="button"
                onClick={() => onExpandedChange(false)}
                aria-label={t("skill.card.modal.close", { name: skill.name })}
              >
                <span aria-hidden="true">×</span>
              </button>
            </div>
          </header>
          <div className="skill-card__details skill-card-detail-modal__body">{details}</div>
        </Modal>
      ) : null}
      {dialog === "files" ? <SkillFileDialog skill={skill} onClose={() => setDialog(null)} /> : null}
      {dialog === "update" && update ? (
        <SkillUpdateDialog
          skill={skill}
          check={update}
          isUpdating={isUpdating}
          onUpdate={(allowDangerous, answer) => void handleUpdate(allowDangerous, answer)}
          onClose={() => setDialog(null)}
        />
      ) : null}
    </>
  );
});
