// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Ported from DailenG/skilldock@181b52b src/features/skills/components/SkillListPage.tsx: header
// toolbar (search, list/grid/grouped views, refresh, update all), the source tabs above the list,
// card list and grid rows, grouped sections with collapsible headers, and empty states. The data
// layer is new: typed commands over TanStack Query; scope-aware (SPEC 13.7: a project scope lists
// the skills enabled in that project); update checks run on request (SPEC 12.4).

import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { storeQueryKeys, useInvalidateStore } from "@/app/apply/changeNotice";
import { useActiveScope } from "@/app/scope/ActiveScope";
import { HeaderToolbar } from "@/app/components/HeaderSlots";
import { useNotifications } from "@/app/components/notifications";
import { SearchFieldIcon } from "@/app/components/SearchFieldIcon";
import { ToolbarGoInstallButton } from "@/app/components/ToolbarGoInstallButton";
import { errorMessage, t } from "@/i18n/en";
import type { SkillInfo, SkillUpdateCheck } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";
import { RefreshIcon } from "../mcp/icons";
import { GridIcon, GroupIcon, ListIcon, UpdateAllIcon } from "./icons";
import { failureText, monogram } from "./labels";
import { SkillCard } from "./SkillCard";
import { AGENTS_SOURCE, STORE_SOURCE, SkillSourceSwitcher, type SourceTab } from "./SkillSourceSwitcher";
import { AgentsCliListView, ToolSkillList } from "./SkillSourceView";
import { useSkillActions } from "./actions";
import { useRenameConfirm } from "./useRenameConfirm";

export type SkillViewMode = "list" | "grid" | "grouped";

const GRID_COLUMNS = 3;

const VIEW_MODES: { mode: SkillViewMode; label: "skills.view.list" | "skills.view.grid" | "skills.view.grouped"; icon: () => JSX.Element }[] = [
  { mode: "list", label: "skills.view.list", icon: ListIcon },
  { mode: "grid", label: "skills.view.grid", icon: GridIcon },
  { mode: "grouped", label: "skills.view.grouped", icon: GroupIcon },
];

type SkillGroup = {
  id: string;
  label: string;
  skills: SkillInfo[];
};

/** SkillDock's grouping by source: one group per repository URL, local folders together. */
function groupBySource(skills: SkillInfo[]): SkillGroup[] {
  const groups = new Map<string, SkillGroup>();
  for (const skill of skills) {
    const url = skill.source?.url;
    const id = url ?? (skill.source?.kind === "local" ? "local" : "other");
    const label = url ?? t(id === "local" ? "skills.group.local" : "skills.group.other");
    const group = groups.get(id) ?? { id, label, skills: [] };
    group.skills.push(skill);
    groups.set(id, group);
  }
  return [...groups.values()].sort((a, b) => a.label.localeCompare(b.label));
}

export type SkillsPageProps = {
  /** Opens the Install page. */
  onOpenInstall: () => void;
};

export function SkillsPage({ onOpenInstall }: SkillsPageProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const actions = useSkillActions();
  const { confirmRename, dialog: renameDialog } = useRenameConfirm();
  // The sidebar's scope (SPEC 13.1); a switch closes the open card and returns to the store tab.
  const { scope } = useActiveScope();
  const scopeKey = scope.kind === "user" ? "user" : `project:${scope.id}`;
  const [shownScopeKey, setShownScopeKey] = useState(scopeKey);
  const [source, setSource] = useState(STORE_SOURCE);
  const [query, setQuery] = useState("");
  const [viewMode, setViewMode] = useState<SkillViewMode>("list");
  const [expandedId, setExpandedId] = useState("");
  if (shownScopeKey !== scopeKey) {
    setShownScopeKey(scopeKey);
    setExpandedId("");
    setSource(STORE_SOURCE);
  }
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});
  const [updates, setUpdates] = useState<Record<string, SkillUpdateCheck>>({});
  const [isChecking, setIsChecking] = useState(false);
  const [isUpdatingAll, setIsUpdatingAll] = useState(false);

  const list = useQuery({
    queryKey: [...storeQueryKeys.skills, scope],
    queryFn: async () => unwrap(await commands.listSkills(scope)),
  });
  const toolDirs = useQuery({
    queryKey: [...storeQueryKeys.skillToolDirs, scope],
    queryFn: async () => unwrap(await commands.listSkillToolDirs(scope)),
  });

  const data = list.data;
  const project = data?.project ?? null;
  const skills = data?.skills ?? [];
  const clients = data?.clients ?? [];
  const allTags = useMemo(
    () => [...new Set(skills.flatMap((skill) => skill.tags))].sort((a, b) => a.localeCompare(b)),
    [skills],
  );
  const needle = query.trim().toLowerCase();
  const filtered = skills.filter(
    (skill) =>
      !needle
      || [skill.id, skill.name, skill.description ?? "", skill.source?.url ?? "", ...skill.tags]
        .join(" ")
        .toLowerCase()
        .includes(needle),
  );
  // A project scope lists the skills enabled there first.
  const visible = project
    ? [...filtered].sort((a, b) => Number(b.projectEnabled) - Number(a.projectEnabled))
    : filtered;
  const available = Object.values(updates).filter((check) => check.status === "available");

  const toolTabs: SourceTab[] = [
    ...(toolDirs.data ?? []).map((dir) => ({ id: dir.client, name: dir.name, count: dir.entries.length })),
    ...(data?.agentsCli
      ? [{ id: AGENTS_SOURCE, name: t("skills.source.agentsCli"), count: data.agentsCli.skills.length }]
      : []),
  ];
  const activeDir = toolDirs.data?.find((dir) => dir.client === source);
  const activeSource = source === STORE_SOURCE || activeDir || (source === AGENTS_SOURCE && data?.agentsCli)
    ? source
    : STORE_SOURCE;

  async function handleCheckUpdates() {
    setIsChecking(true);
    try {
      const checks = unwrap(await commands.checkSkillUpdates([]));
      setUpdates(Object.fromEntries(checks.map((check) => [check.id, check])));
      const count = checks.filter((check) => check.status === "available").length;
      const unreachable = checks.filter((check) => check.status === "source_unreachable").length;
      const failed = checks.filter((check) => check.status === "failed").length;
      notify({
        tone: unreachable + failed > 0 ? "info" : "success",
        message: [
          count > 0 ? t("skills.checked.available", { count }) : t("skills.checked.none"),
          unreachable > 0 ? t("skills.checked.unreachable", { count: unreachable }) : "",
          failed > 0 ? t("skills.checked.failed", { count: failed }) : "",
        ].join(" ").trim(),
      });
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    } finally {
      setIsChecking(false);
    }
  }

  async function handleUpdateAll() {
    setIsUpdatingAll(true);
    const done: string[] = [];
    let failed = 0;
    for (const check of available) {
      const skill = skills.find((entry) => entry.id === check.id);
      try {
        await actions.update(check.id, skill?.name ?? check.id, check.latest ?? "");
        done.push(check.id);
      } catch {
        failed += 1;
      }
    }
    // Failed updates keep their entry, so they can be retried or reviewed.
    setUpdates((current) => Object.fromEntries(Object.entries(current).filter(([id]) => !done.includes(id))));
    const updated = done.length;
    notify({
      tone: failed > 0 ? "info" : "success",
      message: failed > 0
        ? t("skills.updateAll.partialFailed", { updated, failed })
        : t("skills.updateAll.done", { count: updated }),
    });
    setIsUpdatingAll(false);
  }

  function showManaged(skillId: string) {
    setSource(STORE_SOURCE);
    setQuery("");
    setExpandedId(skillId);
  }

  function renderCard(skill: SkillInfo, layout: "list" | "grid" = "list") {
    return (
      <SkillCard
        key={skill.id}
        skill={skill}
        clients={clients}
        project={project}
        layout={layout}
        expanded={expandedId === skill.id}
        onExpandedChange={(open) => setExpandedId(open ? skill.id : "")}
        update={updates[skill.id]}
        onUpdated={() =>
          setUpdates((current) => Object.fromEntries(Object.entries(current).filter(([id]) => id !== skill.id)))
        }
        allTags={allTags}
      />
    );
  }

  const isFetching = list.isFetching || toolDirs.isFetching;
  const toolbar = (
    <section className="mcp-toolbar skills-header-bar__tools" aria-label={t("skills.toolbar.aria")}>
      <label className="search-field search-field--header mcp-toolbar__search">
        <span className="sr-only">{t("skills.searchAria")}</span>
        <SearchFieldIcon />
        <input
          type="search"
          autoComplete="off"
          spellCheck={false}
          aria-label={t("skills.searchAria")}
          placeholder={t("skills.searchPlaceholder")}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>
      <div className="skills-view-toggle" role="group" aria-label={t("skills.view.aria")}>
        {VIEW_MODES.map(({ mode, label, icon: Icon }) => (
          <button
            key={mode}
            className={`skills-view-toggle__button${viewMode === mode ? " is-active" : ""}`}
            type="button"
            aria-pressed={viewMode === mode}
            aria-label={t(label)}
            data-tooltip={t(label)}
            onClick={() => setViewMode(mode)}
          >
            <Icon />
          </button>
        ))}
      </div>
      <button
        className={`secondary-button secondary-button--compact skills-toolbar-button skills-toolbar-button--refresh${isFetching ? " is-loading" : ""}`}
        type="button"
        onClick={() => void invalidate()}
        disabled={isFetching}
      >
        <span aria-hidden="true" className="skills-toolbar-button__icon">
          <RefreshIcon isSpinning={isFetching} />
        </span>
        <span>{t("skills.refresh")}</span>
      </button>
      {available.length > 0 ? (
        <button
          className="secondary-button secondary-button--compact skills-toolbar-button skills-toolbar-button--update-all"
          type="button"
          onClick={() => void handleUpdateAll()}
          disabled={isUpdatingAll}
        >
          <span aria-hidden="true" className="skills-toolbar-button__icon">
            <UpdateAllIcon isSpinning={isUpdatingAll} />
          </span>
          <span>{isUpdatingAll ? t("skills.updating") : t("skills.updateAllWithCount", { count: available.length })}</span>
        </button>
      ) : (
        <button
          className="secondary-button secondary-button--compact skills-toolbar-button skills-toolbar-button--update-all"
          type="button"
          onClick={() => void handleCheckUpdates()}
          disabled={isChecking || skills.length === 0}
        >
          <span aria-hidden="true" className="skills-toolbar-button__icon">
            <UpdateAllIcon isSpinning={isChecking} />
          </span>
          <span>{isChecking ? t("skills.checkingUpdates") : t("skills.checkUpdates")}</span>
        </button>
      )}
      <ToolbarGoInstallButton onClick={onOpenInstall} />
    </section>
  );

  const gridRows: SkillInfo[][] = [];
  for (let index = 0; index < visible.length; index += GRID_COLUMNS) {
    gridRows.push(visible.slice(index, index + GRID_COLUMNS));
  }

  let body: JSX.Element;
  if (activeSource === AGENTS_SOURCE && data?.agentsCli) {
    body = <AgentsCliListView list={data.agentsCli} query={query} confirmRename={confirmRename} />;
  } else if (activeDir) {
    body = (
      <ToolSkillList dir={activeDir} scope={scope} query={query} onShowManaged={showManaged} confirmRename={confirmRename} />
    );
  } else if (list.isPending) {
    body = (
      <div className="panel-card empty-state">
        <h3>{t("skills.loading")}</h3>
      </div>
    );
  } else if (visible.length === 0) {
    body = (
      <div className="panel-card empty-state">
        {skills.length === 0 ? (
          <>
            <h3>{t("skills.empty.noneTitle")}</h3>
            <p>{t("skills.empty.noneDescription")}</p>
            <div className="empty-state__actions">
              <button className="primary-button" type="button" onClick={onOpenInstall}>
                {t("skills.empty.git")}
              </button>
              <button className="secondary-button" type="button" onClick={onOpenInstall}>
                {t("skills.empty.local")}
              </button>
            </div>
          </>
        ) : (
          <>
            <h3>{t("skills.empty.noMatchTitle")}</h3>
            <p>{t("skills.empty.noMatchDescription")}</p>
          </>
        )}
      </div>
    );
  } else if (viewMode === "grouped") {
    body = (
      <>
        {groupBySource(visible).map((group) => {
          const isCollapsed = collapsedGroups[group.id] ?? false;
          const updateCount = group.skills.filter((skill) => updates[skill.id]?.status === "available").length;
          const toggle = () => setCollapsedGroups((current) => ({ ...current, [group.id]: !isCollapsed }));
          return (
            <section key={group.id} className="skill-group-section skill-group-section--default">
              <div
                className="skill-group-section__header"
                role="button"
                tabIndex={0}
                onClick={toggle}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    toggle();
                  }
                }}
                aria-expanded={!isCollapsed}
                aria-label={t(isCollapsed ? "skills.group.expand" : "skills.group.collapse", { label: group.label })}
              >
                <div className="skill-group-section__title">
                  <div className="link-badge link-badge--monogram" aria-hidden="true">
                    <span className="link-badge__label">{monogram(group.label)}</span>
                  </div>
                  <div className="skill-group-section__copy">
                    <div className="skill-group-section__name-row">
                      <h3>{group.label}</h3>
                      <span className="skill-group-section__badge" aria-hidden="true">{t("skills.group.badge")}</span>
                      <span className="skill-group-section__count">{t("skills.group.count", { count: group.skills.length })}</span>
                    </div>
                  </div>
                </div>
                <div className="skill-group-section__meta">
                  {updateCount > 0 ? (
                    <span className="skill-group-section__state skill-group-section__state--update">
                      {t("skills.group.updateCount", { count: updateCount })}
                    </span>
                  ) : null}
                  <span className="skill-group-section__toggle" aria-hidden="true">
                    <span className={`skill-group-section__chevron${isCollapsed ? " is-collapsed" : ""}`}>⌄</span>
                  </span>
                </div>
              </div>
              {!isCollapsed ? (
                <div className="skill-group-section__list">{group.skills.map((skill) => renderCard(skill))}</div>
              ) : null}
            </section>
          );
        })}
      </>
    );
  } else if (viewMode === "grid") {
    body = (
      <>
        {gridRows.map((row) => (
          <div key={row[0].id} className="skill-card-grid__row">
            {row.map((skill) => renderCard(skill, "grid"))}
          </div>
        ))}
      </>
    );
  } else {
    body = <>{visible.map((skill) => renderCard(skill))}</>;
  }

  const projectClients = project
    ? project.clients.map((id) => clients.find((client) => client.id === id)?.name ?? id).join(", ")
      || t("skills.scope.projectNoClients")
    : "";

  return (
    <div className="skills-page">
      <HeaderToolbar>{toolbar}</HeaderToolbar>
      {list.isError ? (
        <div className="dialog-error mcp-page-error" role="alert">
          <span>{failureText(list.error)}</span>
          <button className="secondary-button secondary-button--compact" type="button" onClick={() => void list.refetch()}>
            {t("app.action.retry")}
          </button>
        </div>
      ) : null}
      {data?.sizeWarning ? <p className="mcp-page-warning" role="note">{t("skills.sizeWarning")}</p> : null}
      {data?.agentsCli?.warnings.map((warning, index) => (
        <p key={`${index}-${warning.code}`} className="mcp-page-warning" role="note">
          {errorMessage(warning.code)}
        </p>
      ))}
      {project ? (
        <p className="skills-scope-note" role="note">
          {t(project.share ? "skills.scope.projectNoteShare" : "skills.scope.projectNote", {
            name: project.name,
            clients: projectClients,
          })}
          {project.trust !== "trusted" ? ` ${t("skills.scope.untrusted")}` : ""}
        </p>
      ) : null}
      <SkillSourceSwitcher active={activeSource} storeCount={skills.length} tools={toolTabs} onChange={setSource} />
      {activeSource === STORE_SOURCE ? (
        <div className={`card-list${viewMode === "grid" ? " skill-card-grid" : ""}`}>{body}</div>
      ) : (
        body
      )}
      {renameDialog}
    </div>
  );
}
