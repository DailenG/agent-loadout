// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Ported from DailenG/skilldock@181b52b src/app/routes/market.tsx (category switcher in the
// header, install method tabs and their icons), src/features/install/components/RepoInstallPanel.tsx
// (Git form) and LocalInstallPanel.tsx (manual install form). SPEC 13.7: Skill: Git | Local and
// MCP: Manual form | Paste config; the marketplace and plugin tabs are hidden in v1. The data
// layer is new: a local source is picked in the OS dialog (the path stays in the backend under a
// token, SPEC 17), and a taken id installs as `<id>-2` only after confirmation (SPEC 12.1).

import { useQuery } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { HeaderToolbar } from "@/app/components/HeaderSlots";
import { useNotifications } from "@/app/components/notifications";
import { t } from "@/i18n/en";
import type { PickedSkillSource, SkillInstalled, SkillSourcePick } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { AddIcon, PasteIcon } from "../mcp/icons";
import { ServerEditDialog } from "../mcp/ServerEditDialog";
import { useSkillActions } from "../skills/actions";
import { failureText } from "../skills/labels";
import { useRenameConfirm } from "../skills/useRenameConfirm";

export type InstallCategory = "skill" | "mcp";
export type InstallTab = "git" | "local";

const CATEGORIES: { key: InstallCategory; label: "install.category.skill" | "install.category.mcp" }[] = [
  { key: "skill", label: "install.category.skill" },
  { key: "mcp", label: "install.category.mcp" },
];

const TABS: { key: InstallTab; label: "install.tab.git" | "install.tab.local" }[] = [
  { key: "git", label: "install.tab.git" },
  { key: "local", label: "install.tab.local" },
];

function CategoryIcon({ category }: { category: InstallCategory }) {
  if (category === "skill") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d="M12 4.5 14 10l5.5 2-5.5 2L12 19.5l-2-5.5L4.5 12 10 10 12 4.5Z" fill="currentColor" />
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M2.4 11.3 11.45 2.26a3.2 3.2 0 0 1 4.53 4.53l-6.84 6.83" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="1.6" />
      <path d="m9.24 13.53 6.74-6.74a3.2 3.2 0 0 1 4.52 0l.05.05a3.2 3.2 0 0 1 0 4.52l-8.19 8.19a1.07 1.07 0 0 0 0 1.51l1.68 1.68" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="1.6" />
      <path d="m13.71 4.53-6.69 6.69a3.2 3.2 0 0 0 4.53 4.53l6.69-6.7" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="1.6" />
    </svg>
  );
}

function TabIcon({ tab }: { tab: InstallTab }) {
  if (tab === "local") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M4 7.5A2.5 2.5 0 0 1 6.5 5h3.3c.5 0 1 .2 1.4.6l1.3 1.3c.2.2.5.4.9.4H17.5A2.5 2.5 0 0 1 20 9.8v6.7A2.5 2.5 0 0 1 17.5 19h-11A2.5 2.5 0 0 1 4 16.5v-9Z"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinejoin="round"
        />
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M10 7.5h-2A3.5 3.5 0 1 0 8 14h2m4-6.5h2a3.5 3.5 0 1 1 0 7h-2m-5 0 6-5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** Empty text is "not given". */
function optional(value: string): string | null {
  return value.trim() || null;
}

type InstallRunner = (renameIfTaken: boolean) => Promise<SkillInstalled>;

/**
 * Runs an install; on `already_exists` asks, then installs as `<id>-2`
 * (SPEC 12.1). Resolves to the result, or null when the user declined or
 * it failed (the action reported why).
 */
function useInstallFlow() {
  const { confirmRename, dialog } = useRenameConfirm();

  async function runInstall(install: InstallRunner): Promise<SkillInstalled | null> {
    try {
      return await install(false);
    } catch (error) {
      if (!(error instanceof IpcFailure) || error.code !== "already_exists" || !(await confirmRename())) {
        return null;
      }
    }
    return install(true).catch(() => null);
  }

  return { runInstall, dialog };
}

type PanelProps = {
  /** "View in Skills" on the success toast. */
  onView: () => void;
};

function GitInstallPanel({ onView }: PanelProps) {
  const actions = useSkillActions();
  const { runInstall, dialog } = useInstallFlow();
  const [url, setUrl] = useState("");
  const [path, setPath] = useState("");
  const [gitRef, setGitRef] = useState("");
  const [id, setId] = useState("");
  const [isInstalling, setIsInstalling] = useState(false);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setIsInstalling(true);
    const done = await runInstall((renameIfTaken) =>
      actions.installGit(url.trim(), optional(path), optional(gitRef), optional(id), renameIfTaken, onView),
    );
    if (done) {
      setUrl("");
      setPath("");
      setGitRef("");
      setId("");
    }
    setIsInstalling(false);
  }

  return (
    <section className="panel-card market-panel">
      <form className="repo-form" onSubmit={(event) => void handleSubmit(event)}>
        <div className="repo-form__section">
          <div className="repo-form__source-row">
            <label className="repo-form__field">
              <span className="repo-form__label">{t("install.repo.url")}</span>
              <input
                type="text"
                placeholder={t("install.repo.urlPlaceholder")}
                value={url}
                onChange={(event) => setUrl(event.target.value)}
              />
            </label>
            <label className="repo-form__field repo-form__field--branch">
              <span className="repo-form__label">{t("install.repo.ref")}</span>
              <input
                type="text"
                placeholder={t("install.repo.refPlaceholder")}
                value={gitRef}
                onChange={(event) => setGitRef(event.target.value)}
              />
            </label>
          </div>
          <div className="repo-form__source-row">
            <label className="repo-form__field">
              <span className="repo-form__label">{t("install.repo.path")}</span>
              <input
                type="text"
                placeholder={t("install.repo.pathPlaceholder")}
                value={path}
                onChange={(event) => setPath(event.target.value)}
              />
            </label>
            <label className="repo-form__field repo-form__field--branch">
              <span className="repo-form__label">{t("install.repo.id")}</span>
              <input
                type="text"
                placeholder={t("install.repo.idPlaceholder")}
                value={id}
                onChange={(event) => setId(event.target.value)}
              />
            </label>
          </div>
          <div className="repo-form__hint-block">
            <p className="repo-form__hint-title">{t("install.repo.supported")}</p>
            <ul className="repo-form__hint-list">
              <li>https://github.com/acme/skills</li>
              <li>https://github.com/acme/skills/tree/main/skills/repo-conventions</li>
              <li>git@github.com:acme/skills.git</li>
            </ul>
            <p className="repo-form__hint-title">{t("install.repo.hint")}</p>
          </div>
        </div>
        <div className="repo-form__actions">
          <button
            className={`primary-button repo-form__submit-button${isInstalling ? " is-loading" : ""}`}
            type="submit"
            disabled={!url.trim() || isInstalling}
          >
            {isInstalling ? t("install.repo.installing") : t("install.repo.install")}
          </button>
        </div>
      </form>
      {dialog}
    </section>
  );
}

function LocalInstallPanel({ onView }: PanelProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const actions = useSkillActions();
  const { runInstall, dialog } = useInstallFlow();
  const [picked, setPicked] = useState<PickedSkillSource | null>(null);
  const [id, setId] = useState("");
  const [isInstalling, setIsInstalling] = useState(false);

  async function pick(kind: SkillSourcePick) {
    try {
      const result = unwrap(await commands.pickSkillSource(kind));
      if (result) {
        setPicked(result);
      }
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    }
  }

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    if (!picked) {
      return;
    }
    setIsInstalling(true);
    const done = await runInstall((renameIfTaken) =>
      actions.installLocal(picked.token, optional(id), renameIfTaken, onView),
    );
    if (done) {
      setPicked(null);
      setId("");
    }
    setIsInstalling(false);
  }

  return (
    <section className="panel-card market-panel local-install-panel">
      <div className="panel-header">
        <h2>{t("install.tab.local")}</h2>
        <p>{t("install.local.manualDescription")}</p>
      </div>
      <form className="local-install-form" onSubmit={(event) => void handleSubmit(event)}>
        <button
          className={`local-install-dropzone${picked ? " is-selected" : ""}`}
          type="button"
          onClick={() => void pick("folder")}
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 4v10m0-10 4 4m-4-4-4 4M5 18h14" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          {picked ? (
            <strong className="local-install-dropzone__selected">
              <span>{t("install.local.selected")}</span>
              <span>{picked.path}</span>
            </strong>
          ) : (
            <strong>{t("install.local.pickTitle")}</strong>
          )}
          <span>{t("install.local.pickHint")}</span>
        </button>
        <div className="local-install-form__section">
          <div className="local-install-form__path-row">
            <button
              className="secondary-button local-install-form__icon-button"
              type="button"
              aria-label={t("install.local.chooseFolder")}
              title={t("install.local.chooseFolder")}
              onClick={() => void pick("folder")}
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M4 7.5A2.5 2.5 0 0 1 6.5 5h3.2l1.5 1.5h6.3A2.5 2.5 0 0 1 20 9v7.5A2.5 2.5 0 0 1 17.5 19h-11A2.5 2.5 0 0 1 4 16.5v-9Z"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.8"
                  strokeLinejoin="round"
                />
              </svg>
            </button>
            <button
              className="secondary-button local-install-form__icon-button"
              type="button"
              aria-label={t("install.local.chooseArchive")}
              title={t("install.local.chooseArchive")}
              onClick={() => void pick("archive")}
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d="M8 4h6l4 4v12H8V4Zm6 0v4h4M5 7v13" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
                <path d="M11 11h2m-2 3h2m-2 3h2" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
              </svg>
            </button>
          </div>
        </div>
        <label className="repo-form__field">
          <span className="repo-form__label">{t("install.repo.id")}</span>
          <input
            type="text"
            placeholder={t("install.repo.idPlaceholder")}
            value={id}
            onChange={(event) => setId(event.target.value)}
          />
        </label>
        <div className="repo-form__actions">
          <button className="primary-button" type="submit" disabled={!picked || isInstalling}>
            {isInstalling ? t("install.local.installing") : t("install.local.install")}
          </button>
        </div>
      </form>
      {dialog}
    </section>
  );
}

function McpInstallPanel() {
  const { commands } = useTransport();
  const [dialog, setDialog] = useState<"form" | "paste" | null>(null);
  const servers = useQuery({
    queryKey: storeQueryKeys.servers,
    queryFn: async () => unwrap(await commands.listServers()),
  });

  return (
    <section className="panel-card market-panel">
      <div className="panel-header">
        <h2>{t("install.mcp.title")}</h2>
        <p>{t("install.mcp.description")}</p>
      </div>
      <div className="install-mcp-options">
        <button className="install-card" type="button" onClick={() => setDialog("form")}>
          <span className="install-card__title-row">
            <AddIcon />
            <strong>{t("install.mcp.form")}</strong>
          </span>
          <span className="install-card__lead">{t("install.mcp.formDescription")}</span>
        </button>
        <button className="install-card" type="button" onClick={() => setDialog("paste")}>
          <span className="install-card__title-row">
            <PasteIcon />
            <strong>{t("install.mcp.paste")}</strong>
          </span>
          <span className="install-card__lead">{t("install.mcp.pasteDescription")}</span>
        </button>
      </div>
      {dialog ? (
        <ServerEditDialog
          mode="create"
          initialTab="form"
          focusPaste={dialog === "paste"}
          clients={servers.data?.clients ?? []}
          onClose={() => setDialog(null)}
        />
      ) : null}
    </section>
  );
}

export type InstallPageProps = {
  /** Opens the Skills page after an install. */
  onOpenSkills: () => void;
};

export function InstallPage({ onOpenSkills }: InstallPageProps) {
  const [category, setCategory] = useState<InstallCategory>("skill");
  const [tab, setTab] = useState<InstallTab>("git");

  return (
    <div className="market-route">
      <HeaderToolbar>
        <div className="page-tabs-row page-tabs-center install-category-tabs-row">
          <div className="page-tabs filter-tabs install-category-row" role="tablist" aria-label={t("install.category.aria")}>
            {CATEGORIES.map((entry) => {
              const selected = entry.key === category;
              return (
                <button
                  key={entry.key}
                  className={`filter-tab install-category-tab${selected ? " active is-selected" : ""}`}
                  type="button"
                  role="tab"
                  aria-selected={selected}
                  onClick={() => setCategory(entry.key)}
                >
                  <span className="tab-icon install-category-tab__icon">
                    <CategoryIcon category={entry.key} />
                  </span>
                  <span>{t(entry.label)}</span>
                </button>
              );
            })}
          </div>
        </div>
      </HeaderToolbar>
      <section className="market-shell">
        {category === "skill" ? (
          <>
            <div className="market-tab-row" role="tablist" aria-label={t("install.tabs.aria")}>
              {TABS.map((entry) => {
                const selected = entry.key === tab;
                return (
                  <button
                    key={entry.key}
                    className={`market-tab${selected ? " is-selected" : ""}`}
                    type="button"
                    role="tab"
                    aria-selected={selected}
                    onClick={() => setTab(entry.key)}
                  >
                    <span className="market-tab__icon">
                      <TabIcon tab={entry.key} />
                    </span>
                    <span>{t(entry.label)}</span>
                  </button>
                );
              })}
            </div>
            {tab === "git" ? <GitInstallPanel onView={onOpenSkills} /> : <LocalInstallPanel onView={onOpenSkills} />}
          </>
        ) : (
          <McpInstallPanel />
        )}
      </section>
    </div>
  );
}
