// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Ported from DailenG/skilldock@181b52b src/features/skills/components/ToolSyncPanel.tsx: the
// "Enable in Tools" header, Enable All / Disable All, and one pill per tool with optimistic
// state. The data layer is new: each pill is a `set_skill_clients` store change (SPEC 12.1
// `enable.clients`), Enable All selects all detected clients, and the link/copy method is a
// setting of the skill (SPEC 12.2). ProjectSkillPanel is new (SPEC 12.3). A client whose
// skill folder this account cannot write (M38) has its pill disabled with the lock and the
// reason; Enable All / Disable All send what they always did and the backend reports it blocked.

import { useEffect, useId, useState } from "react";
import { orderByRecommended } from "@/app/clients";
import { AppSelect, type AppSelectOption } from "@/app/components/AppSelect";
import { RecommendedBadge, recommendedReasonId } from "@/app/components/RecommendedBadge";
import { ReadOnlyBadge, ReadOnlyNote, readOnlyReason } from "@/app/components/ReadOnlyBadge";
import { toolLogoUrl } from "@/app/components/ToolLogo";
import { t } from "@/i18n/en";
import type { SkillClient, SkillInfo, SkillMethod, SkillProject } from "@/ipc/bindings";
import { useSkillActions } from "./actions";

const METHOD_OPTIONS: AppSelectOption<SkillMethod>[] = [
  { value: "link", label: t("skill.tools.method.link") },
  { value: "copy", label: t("skill.tools.method.copy") },
];

function PillLogo({ client }: { client: SkillClient }) {
  const [failed, setFailed] = useState(false);
  const url = toolLogoUrl(client.id);
  return (
    <span className="tool-pill__logo" aria-hidden="true">
      {url && !failed ? <img src={url} alt="" onError={() => setFailed(true)} /> : <span>{client.name.slice(0, 1)}</span>}
    </span>
  );
}

function skillFolderTooltip(client: SkillClient): string {
  return client.readOnly
    ? t("readOnly.skills.folderNoPath", { client: client.name, reason: readOnlyReason(client.readOnly.kind) })
    : "";
}

type ToolSyncPanelProps = {
  skill: SkillInfo;
  clients: SkillClient[];
  /** The card's power toggle is running. */
  isBulkUpdatingExternally?: boolean;
  onBulkUpdatingChange?: (busy: boolean) => void;
};

export function ToolSyncPanel({ skill, clients, isBulkUpdatingExternally = false, onBulkUpdatingChange }: ToolSyncPanelProps) {
  const actions = useSkillActions();
  const baseId = useId();
  const [enabled, setEnabled] = useState<string[]>(skill.clients);
  const [pending, setPending] = useState<string[]>([]);
  const [bulkAction, setBulkAction] = useState<"enable" | "disable" | null>(null);
  const isBulk = bulkAction !== null || isBulkUpdatingExternally;

  useEffect(() => {
    setEnabled(skill.clients);
  }, [skill.clients]);

  async function handleToggle(client: SkillClient) {
    if (isBulk || pending.includes(client.id)) {
      return;
    }
    const wasEnabled = enabled.includes(client.id);
    setEnabled((current) => (wasEnabled ? current.filter((id) => id !== client.id) : [...current, client.id]));
    setPending((current) => [...current, client.id]);
    try {
      await actions.setClients(skill.id, skill.name, [client.id], !wasEnabled);
    } catch {
      setEnabled(skill.clients);
    } finally {
      setPending((current) => current.filter((id) => id !== client.id));
    }
  }

  async function handleAll(on: boolean) {
    if (isBulk || pending.length > 0) {
      return;
    }
    const previous = enabled;
    setEnabled(on ? clients.filter((client) => client.detected).map((client) => client.id) : []);
    setBulkAction(on ? "enable" : "disable");
    onBulkUpdatingChange?.(true);
    try {
      await actions.setClients(skill.id, skill.name, [], on);
    } catch {
      setEnabled(previous);
    } finally {
      setBulkAction(null);
      onBulkUpdatingChange?.(false);
    }
  }

  const enabledCount = clients.filter((client) => enabled.includes(client.id)).length;

  return (
    <section className="tool-sync-panel">
      <div className="skill-card__section-header">
        <h4>{t("skill.tools.title")}</h4>
        <div className="tool-sync-panel__actions">
          <AppSelect
            className="skill-method-select"
            value={skill.method}
            options={METHOD_OPTIONS}
            ariaLabel={t("skill.tools.method")}
            selectedLabel={`${t("skill.tools.method")}: ${t(skill.method === "copy" ? "skill.tools.method.copy" : "skill.tools.method.link")}`}
            disabled={isBulk}
            onChange={(method) => {
              if (method !== skill.method) {
                void actions.setMethod(skill.id, skill.name, method).catch(() => undefined);
              }
            }}
          />
          <button
            className="secondary-button secondary-button--compact semantic-action semantic-action--enable"
            type="button"
            onClick={() => void handleAll(true)}
            disabled={isBulk || pending.length > 0 || (skill.allDetected && enabledCount > 0)}
          >
            {bulkAction === "enable" ? t("skill.tools.enableAllLoading") : t("skill.tools.enableAll")}
          </button>
          <button
            className="secondary-button secondary-button--compact semantic-action semantic-action--disable"
            type="button"
            onClick={() => void handleAll(false)}
            disabled={isBulk || pending.length > 0 || enabledCount === 0}
          >
            {bulkAction === "disable" ? t("skill.tools.disableAllLoading") : t("skill.tools.disableAll")}
          </button>
        </div>
      </div>
      <div className="tool-pill-grid">
        {orderByRecommended(clients, (client) => client.id).map((client) => {
          const on = enabled.includes(client.id);
          const tooltip = client.readOnly
            ? skillFolderTooltip(client)
            : on
              ? t("skill.tools.tooltip.enabled")
              : client.detected
                ? t("skill.tools.tooltip.disabled")
                : t("skill.tools.tooltip.notDetected");
          const reasonId = recommendedReasonId(baseId, client.id);
          const readOnlyId = client.readOnly ? `${baseId}-${client.id}-read-only` : undefined;
          const describedBy = [reasonId, readOnlyId].filter(Boolean).join(" ") || undefined;
          const pill = (
            <button
              key={client.id}
              className={`tool-pill${on ? " is-enabled" : ""}${client.detected ? "" : " is-undetected"}${client.readOnly ? " is-read-only" : ""}`}
              type="button"
              onClick={() => void handleToggle(client)}
              aria-pressed={on}
              aria-label={on ? t("skill.tools.aria.disable", { name: client.name }) : t("skill.tools.aria.enable", { name: client.name })}
              aria-describedby={describedBy}
              data-tooltip={tooltip}
              disabled={isBulk || pending.includes(client.id) || client.readOnly !== null}
            >
              <PillLogo client={client} />
              <span className="tool-pill__name">{client.name}</span>
              <RecommendedBadge clientId={client.id} variant="mark" reasonId={reasonId} />
              <ReadOnlyBadge readOnly={client.readOnly} variant="mark" reasonId={readOnlyId} tooltip={tooltip} />
              <span className="sr-only">{on ? t("skill.tools.toggle.on") : t("skill.tools.toggle.off")}</span>
            </button>
          );
          // A disabled button gets no pointer events; the wrapper shows the tooltip.
          return client.readOnly ? (
            <span key={client.id} className="mcp-pill-wrap" data-tooltip={tooltip}>
              {pill}
            </span>
          ) : pill;
        })}
      </div>
    </section>
  );
}

type ProjectSkillPanelProps = {
  skill: SkillInfo;
  project: SkillProject;
  clients: SkillClient[];
};

/** Project scope (SPEC 12.3): on or off in the project's manifest, for the project's clients. */
export function ProjectSkillPanel({ skill, project, clients }: ProjectSkillPanelProps) {
  const actions = useSkillActions();
  const [busy, setBusy] = useState(false);
  const projectClients = orderByRecommended(project.clients, (id) => id);
  const names = projectClients.map((id) => clients.find((client) => client.id === id)?.name ?? id);
  const locked = skill.projectEnabled && skill.projectShared;

  async function toggle() {
    setBusy(true);
    try {
      await actions.setProject(project.id, skill.id, skill.name, !skill.projectEnabled, project.hash);
    } catch {
      // Reported by the action.
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="tool-sync-panel">
      <div className="skill-card__section-header">
        <h4>{t("skill.project.title")}</h4>
        <div className="tool-sync-panel__actions">
          <button
            className={`secondary-button secondary-button--compact semantic-action ${skill.projectEnabled ? "semantic-action--disable" : "semantic-action--enable"}`}
            type="button"
            disabled={busy || locked || project.trust !== "trusted" || project.readOnly !== null}
            onClick={() => void toggle()}
          >
            {skill.projectEnabled ? t("skill.project.off") : t("skill.project.on")}
          </button>
        </div>
      </div>
      <p className="skill-project-panel__note">
        {t("skill.project.clients", {
          clients: names.length > 0 ? names.join(", ") : t("skills.scope.projectNoClients"),
        })}
      </p>
      {locked ? <p className="skill-project-panel__note" role="note">{t("skill.project.shared")}</p> : null}
      {project.readOnly ? (
        <ReadOnlyNote>
          {t("readOnly.project.whole", { name: project.name, reason: readOnlyReason(project.readOnly.kind) })}
        </ReadOnlyNote>
      ) : null}
      <div className="tool-pill-grid">
        {projectClients.map((id) => {
          const client = clients.find((entry) => entry.id === id) ?? { id, name: id, detected: false, readOnly: null };
          return (
            <span key={id} className={`tool-pill${skill.projectEnabled ? " is-enabled" : ""}${client.readOnly ? " is-read-only" : ""}`}>
              <PillLogo client={client} />
              <span className="tool-pill__name">{client.name}</span>
              <ReadOnlyBadge readOnly={client.readOnly} variant="mark" tooltip={skillFolderTooltip(client)} />
            </span>
          );
        })}
      </div>
    </section>
  );
}
