// The adopt flow of one scope (#3, SPEC 12.5): the unmanaged skill folders,
// names with one content as checkboxes, names with several as a choice (use
// one content, keep each, skip), then one adopt and its result: what was
// adopted (the folder replaced by a link, the original in the trash), what
// was kept and why, and the folders that cannot be adopted.
//
// Shared by the Import page's Skills section and the Skills page's Adopt
// actions (one folder, or every name with one content of a client).

import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { storeQueryKeys, useInvalidateImports, useInvalidateStore } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { useAddFolderFlow } from "@/app/routes/projects/AddFolderFlow";
import { displayPath } from "@/app/routes/projects/labels";
import { LINK_STATUS_KEYS, failureText, formatSize } from "@/app/routes/skills/labels";
import { useProjectList } from "@/app/scope/ActiveScope";
import { t, type MessageKey } from "@/i18n/en";
import type {
  SkillAdoptResult,
  SkillAdoptedFolder,
  SkillFolderStatus,
  SkillScope,
  UnmanagedSkill,
  UnmanagedSkillProblem,
  UnmanagedSkillVariant,
} from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";
import {
  adoptChoices,
  fixedVariant,
  initialDecision,
  renamedId,
  unmanagedSkillsQuery,
  type AdoptDecision,
  type AdoptMode,
} from "./adopt";
import { reasonText } from "./labels";

const STATUS_KEYS = {
  adopted: "adopt.status.adopted",
  differs: "adopt.status.differs",
  conflict: "adopt.status.conflict",
  skipped: "adopt.status.skipped",
  kept: "adopt.status.kept",
} as const satisfies Record<SkillFolderStatus, MessageKey>;

const MODE_KEYS = {
  pick: "adopt.mode.pick",
  both: "adopt.mode.both",
  skip: "adopt.mode.skip",
} as const satisfies Record<AdoptMode, MessageKey>;

const KEPT: SkillFolderStatus[] = ["differs", "conflict", "kept"];

type ClientNames = (id: string) => string;

/** Why a folder stayed where it was. */
function keptReason(folder: SkillAdoptedFolder): string {
  if (folder.error) {
    return reasonText(folder.error);
  }
  if (folder.status === "differs") {
    return t("adopt.reason.differs");
  }
  if (folder.status === "conflict") {
    return t("adopt.reason.conflict");
  }
  return t(STATUS_KEYS[folder.status]);
}

/** The folder that holds a trash entry: `<data>/backups/trash`. */
function parentFolder(path: string): string {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return cut > 0 ? path.slice(0, cut) : path;
}

function variantClients(variant: UnmanagedSkillVariant, names: ClientNames): string {
  if (variant.folders.length === 0) {
    return t("adopt.storeCopy");
  }
  return [...new Set(variant.folders.map((folder) => names(folder.client)))].join(", ");
}

function FolderList({ variant, names }: { variant: UnmanagedSkillVariant; names: ClientNames }) {
  if (variant.folders.length === 0) {
    return null;
  }
  return (
    <ul className="adopt-folders">
      {variant.folders.map((folder) => (
        <li key={`${folder.client}:${folder.path}`}>
          <span className="adopt-folders__client">{names(folder.client)}</span>
          <span className="adopt-path" title={displayPath(folder.path)}>
            {displayPath(folder.path)}
          </span>
        </li>
      ))}
    </ul>
  );
}

function VariantBadges({ variant }: { variant: UnmanagedSkillVariant }) {
  return (
    <>
      {variant.inStore ? <span className="status-badge tone-positive">{t("adopt.inStore")}</span> : null}
      {variant.folders.length > 1 ? (
        <span className="status-badge tone-info">{t("adopt.sameIn", { count: variant.folders.length })}</span>
      ) : null}
    </>
  );
}

type IdenticalRowProps = {
  candidate: UnmanagedSkill;
  checked: boolean;
  names: ClientNames;
  onChange: (checked: boolean) => void;
};

/** A name with one content: in one folder or the same in several (collapsed to one row). */
function IdenticalRow({ candidate, checked, names, onChange }: IdenticalRowProps) {
  const variant = candidate.variants[0];
  return (
    <li className="adopt-item">
      <label className="project-dialog__choice adopt-item__head">
        <input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
        <span>
          <strong>{candidate.name}</strong>
          <span className="project-dialog__hint">
            {variant?.description ?? t("skills.description.empty")}
            {variant ? ` · ${formatSize(variant.bytes)}` : ""}
          </span>
        </span>
      </label>
      {variant ? (
        <div className="adopt-item__body">
          <div className="adopt-item__badges">
            <VariantBadges variant={variant} />
          </div>
          <FolderList variant={variant} names={names} />
        </div>
      ) : null}
    </li>
  );
}

type ConflictRowProps = {
  candidate: UnmanagedSkill;
  decision: AdoptDecision;
  names: ClientNames;
  onChange: (decision: AdoptDecision) => void;
};

/** A name with several contents: what to do, and which content keeps the name. */
function ConflictRow({ candidate, decision, names, onChange }: ConflictRowProps) {
  const groupId = useId();
  const fixed = fixedVariant(candidate);
  const others = candidate.variants.filter((variant) => variant.hash !== decision.variant);
  const renamed = others.map((variant) => renamedId(candidate.name, variant)).filter((id): id is string => id !== null);
  return (
    <li className="adopt-item adopt-item--conflict">
      <div className="adopt-item__title">
        <strong>{candidate.name}</strong>
        <span className="status-badge tone-warning">{t("adopt.contents", { count: candidate.variants.length })}</span>
      </div>
      <fieldset className="adopt-choice">
        <legend className="sr-only">{t("adopt.mode.legend", { name: candidate.name })}</legend>
        {(["pick", "both", "skip"] as const).map((mode) => (
          <label key={mode} className="project-dialog__choice">
            <input
              type="radio"
              name={`${groupId}-mode`}
              checked={decision.mode === mode}
              onChange={() => onChange({ ...decision, mode })}
            />
            <span>
              <span>{t(MODE_KEYS[mode])}</span>
              {mode === "pick" && decision.mode === "pick" ? (
                <span className="project-dialog__hint">{t("adopt.mode.pickHint")}</span>
              ) : null}
              {mode === "both" && decision.mode === "both" && renamed.length > 0 ? (
                <span className="project-dialog__hint">
                  {t("adopt.mode.bothHint", { ids: renamed.join(", ") })}{" "}
                  {t("adopt.renamedHint", { name: candidate.name })}
                </span>
              ) : null}
            </span>
          </label>
        ))}
      </fieldset>
      {decision.mode !== "skip" ? (
        <fieldset className="adopt-choice adopt-choice--variants">
          <legend>{t("adopt.variant.legend", { name: candidate.name })}</legend>
          {fixed ? <p className="project-dialog__hint">{t("adopt.variant.storeFixed")}</p> : null}
          {candidate.variants.map((variant) => (
            <label key={variant.hash} className="project-dialog__choice adopt-variant">
              <input
                type="radio"
                name={`${groupId}-variant`}
                checked={decision.variant === variant.hash}
                disabled={fixed !== null && fixed.hash !== variant.hash}
                onChange={() => onChange({ ...decision, variant: variant.hash })}
              />
              <span>
                <span className="adopt-variant__head">
                  <span>{variantClients(variant, names)}</span>
                  <VariantBadges variant={variant} />
                </span>
                <span className="project-dialog__hint">
                  {variant.description ?? t("skills.description.empty")} · {formatSize(variant.bytes)}
                </span>
                <FolderList variant={variant} names={names} />
              </span>
            </label>
          ))}
        </fieldset>
      ) : null}
    </li>
  );
}

function ProblemList({ problems, names }: { problems: UnmanagedSkillProblem[]; names: ClientNames }) {
  return (
    <ul className="adopt-list adopt-list--plain">
      {problems.map((problem) => (
        <li key={`${problem.client}:${problem.path}`} className="adopt-result-row">
          <strong>{problem.name}</strong>
          <span className="adopt-folders__client">{names(problem.client)}</span>
          <span className="adopt-path" title={displayPath(problem.path)}>
            {displayPath(problem.path)}
          </span>
          <span className="adopt-reason">{reasonText(problem.error)}</span>
        </li>
      ))}
    </ul>
  );
}

type AdoptResultViewProps = {
  /** The result limited to the names the user was offered. */
  result: SkillAdoptResult;
  names: ClientNames;
  onAgain: () => void;
};

/** Adopted, kept and skipped folders of a result. */
function resultCounts(folders: readonly SkillAdoptedFolder[]) {
  return {
    adopted: folders.filter((folder) => folder.status === "adopted").length,
    kept: folders.filter((folder) => KEPT.includes(folder.status)).length,
    skipped: folders.filter((folder) => folder.status === "skipped").length,
  };
}

/** What an adopt did: adopted (with the trash), kept with reasons, problems, link notes. */
function AdoptResultView({ result, names, onAgain }: AdoptResultViewProps) {
  const adopted = result.folders.filter((folder) => folder.status === "adopted");
  const kept = result.folders.filter((folder) => KEPT.includes(folder.status));
  const trash = adopted.find((folder) => folder.trash)?.trash ?? null;
  const notes = result.links.notes.filter((note) => note.status !== "created" && note.status !== "adopted");
  return (
    <div className="adopt-result">
      {adopted.length > 0 ? (
        <section className="adopt-result__group" aria-label={t("adopt.result.adopted")}>
          <h4>{t("adopt.result.adopted")}</h4>
          <ul className="adopt-list adopt-list--plain">
            {adopted.map((folder) => (
              <li key={`${folder.client}:${folder.path}`} className="adopt-result-row">
                <strong>{folder.skillId ?? folder.name}</strong>
                <span className="adopt-folders__client">{names(folder.client)}</span>
                <span className="adopt-path" title={displayPath(folder.path)}>
                  {displayPath(folder.path)}
                </span>
                {folder.skillId && folder.skillId !== folder.name ? (
                  <span className="adopt-reason">
                    {t("adopt.result.renamed", { id: folder.skillId, name: folder.name })}{" "}
                    {t("adopt.renamedHint", { name: folder.name })}
                  </span>
                ) : null}
              </li>
            ))}
          </ul>
          {trash ? (
            <p className="adopt-result__trash">
              {t("adopt.result.trash")}{" "}
              <span className="adopt-path" title={parentFolder(trash)}>
                {parentFolder(trash)}
              </span>
            </p>
          ) : null}
        </section>
      ) : null}
      {kept.length > 0 ? (
        <section className="adopt-result__group" aria-label={t("adopt.result.kept")}>
          <h4>{t("adopt.result.kept")}</h4>
          <ul className="adopt-list adopt-list--plain">
            {kept.map((folder) => (
              <li key={`${folder.client}:${folder.path}`} className="adopt-result-row">
                <strong>{folder.name}</strong>
                <span className="adopt-folders__client">{names(folder.client)}</span>
                <span className="adopt-path" title={displayPath(folder.path)}>
                  {displayPath(folder.path)}
                </span>
                <span className="adopt-reason">{keptReason(folder)}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {result.problems.length > 0 ? (
        <section className="adopt-result__group" aria-label={t("adopt.result.problems")}>
          <h4>{t("adopt.result.problems")}</h4>
          <ProblemList problems={result.problems} names={names} />
        </section>
      ) : null}
      {notes.length > 0 || result.links.warnings.length > 0 ? (
        <section className="adopt-result__group" aria-label={t("adopt.result.links")}>
          <h4>{t("adopt.result.links")}</h4>
          <ul className="adopt-list adopt-list--plain">
            {notes.map((note) => (
              <li key={`${note.client}:${note.path}`} className="adopt-result-row">
                <strong>{note.skillId}</strong>
                <span className="adopt-folders__client">{names(note.client)}</span>
                <span className="adopt-path" title={note.path}>
                  {note.path}
                </span>
                <span className="adopt-reason">{t(LINK_STATUS_KEYS[note.status])}</span>
              </li>
            ))}
            {result.links.warnings.map((warning, index) => (
              <li key={`warning-${index}`} className="adopt-result-row">
                <span className="adopt-reason">{reasonText(warning)}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      <div className="adopt-actions">
        <button className="secondary-button secondary-button--compact" type="button" onClick={onAgain}>
          {t("adopt.result.again")}
        </button>
      </div>
    </div>
  );
}

export type AdoptPanelProps = {
  scope: SkillScope;
  /** One client's skill folder, or every client (`null`). */
  client: string | null;
  /** Only these names (the Skills page's Adopt on one folder). */
  names?: readonly string[];
  /** Only names with one content (the Skills page's "Adopt all identical"). */
  identicalOnly?: boolean;
  /** Renders a Cancel button before Adopt (the dialog's). */
  onCancel?: () => void;
};

export function AdoptPanel({ scope, client, names: only, identicalOnly = false, onCancel }: AdoptPanelProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const invalidateImports = useInvalidateImports();
  const flow = useAddFolderFlow();
  const scan = useQuery(unmanagedSkillsQuery(commands, scope, client));
  const skillList = useQuery({
    queryKey: [...storeQueryKeys.skills, scope],
    queryFn: async () => unwrap(await commands.listSkills(scope)),
  });
  const projects = useProjectList(scope.kind === "project");
  const project = scope.kind === "project" ? (projects.data?.find((row) => row.id === scope.id) ?? null) : null;
  const untrusted = project !== null && project.trust !== "trusted";

  const [decisions, setDecisions] = useState<Record<string, AdoptDecision>>({});
  const [result, setResult] = useState<SkillAdoptResult | null>(null);
  const [adopting, setAdopting] = useState(false);
  const resultHeading = useRef<HTMLHeadingElement | null>(null);

  // Adopt leaves with the list: focus goes to the result's heading, not the page.
  useEffect(() => {
    if (result) {
      resultHeading.current?.focus();
    }
  }, [result]);

  const clientNames = useMemo(() => {
    const known = new Map((skillList.data?.clients ?? []).map((entry) => [entry.id, entry.name]));
    return (id: string) => known.get(id) ?? id;
  }, [skillList.data]);

  const candidates = useMemo(
    () =>
      (scan.data?.candidates ?? []).filter(
        (candidate) => (!only || only.includes(candidate.name)) && (!identicalOnly || !candidate.conflict),
      ),
    [identicalOnly, only, scan.data],
  );
  const problems = (scan.data?.problems ?? []).filter((problem) => !only || only.includes(problem.name));
  const identical = candidates.filter((candidate) => !candidate.conflict);
  const conflicts = candidates.filter((candidate) => candidate.conflict);
  const decisionOf = (candidate: UnmanagedSkill) => decisions[candidate.name] ?? initialDecision(candidate);
  const chosen = candidates.filter((candidate) => decisionOf(candidate).mode !== "skip").length;

  async function adopt() {
    setAdopting(true);
    try {
      const offered = new Set(candidates.map((candidate) => candidate.name));
      const choices = adoptChoices(scan.data?.candidates ?? [], offered, decisions);
      const done = unwrap(await commands.adoptSkills(scope, client, choices));
      // The result covers what the user was offered; the names skipped for them are no news.
      const shown: SkillAdoptResult = {
        ...done,
        folders: done.folders.filter((folder) => offered.has(folder.name)),
        problems: done.problems.filter((problem) => !only || only.includes(problem.name)),
      };
      setResult(shown);
      setDecisions({});
      const { adopted, kept } = resultCounts(shown.folders);
      notify(
        kept > 0 || done.links.needsAttention
          ? { tone: "info", message: t("adopt.doneAttention", { adopted, kept }) }
          : { tone: "success", message: t("adopt.done", { count: adopted }) },
      );
      await invalidateImports();
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    } finally {
      setAdopting(false);
      await invalidate();
    }
  }

  const counts = result ? resultCounts(result.folders) : null;
  let summary = "";
  if (counts) {
    summary = counts.adopted + counts.kept + counts.skipped === 0
      ? t("adopt.result.nothing")
      : t("adopt.result.summary", counts);
  }

  let body: JSX.Element;
  if (result) {
    body = <AdoptResultView result={result} names={clientNames} onAgain={() => setResult(null)} />;
  } else if (scan.isPending) {
    body = <p className="tools-note">{t("adopt.loading")}</p>;
  } else if (scan.isError) {
    body = (
      <div className="dialog-error" role="alert">
        <span>{failureText(scan.error)}</span>
        <button className="secondary-button secondary-button--compact" type="button" onClick={() => void scan.refetch()}>
          {t("app.action.retry")}
        </button>
      </div>
    );
  } else {
    body = (
      <>
        {candidates.length === 0 && problems.length === 0 ? <p className="tools-note">{t("adopt.empty")}</p> : null}
        {identical.length > 0 ? (
          <section className="adopt-group" aria-label={t("adopt.identical.title")}>
            {only ? null : (
              <>
                <h4>{t("adopt.identical.title")}</h4>
                <p className="project-dialog__hint">{t("adopt.identical.hint")}</p>
              </>
            )}
            <ul className="adopt-list">
              {identical.map((candidate) => (
                <IdenticalRow
                  key={candidate.name}
                  candidate={candidate}
                  names={clientNames}
                  checked={decisionOf(candidate).mode !== "skip"}
                  onChange={(checked) =>
                    setDecisions((current) => ({
                      ...current,
                      [candidate.name]: { ...decisionOf(candidate), mode: checked ? "pick" : "skip" },
                    }))
                  }
                />
              ))}
            </ul>
          </section>
        ) : null}
        {conflicts.length > 0 ? (
          <section className="adopt-group" aria-label={t("adopt.conflicts.title")}>
            {only ? null : (
              <>
                <h4>{t("adopt.conflicts.title")}</h4>
                <p className="project-dialog__hint">{t("adopt.conflicts.hint")}</p>
              </>
            )}
            <ul className="adopt-list">
              {conflicts.map((candidate) => (
                <ConflictRow
                  key={candidate.name}
                  candidate={candidate}
                  names={clientNames}
                  decision={decisionOf(candidate)}
                  onChange={(decision) => setDecisions((current) => ({ ...current, [candidate.name]: decision }))}
                />
              ))}
            </ul>
          </section>
        ) : null}
        {problems.length > 0 ? (
          <details className="adopt-group adopt-problems">
            <summary>{t("adopt.problems.title", { count: problems.length })}</summary>
            <ProblemList problems={problems} names={clientNames} />
          </details>
        ) : null}
      </>
    );
  }

  return (
    <div className="adopt-panel">
      {untrusted && project ? (
        <div className="adopt-untrusted" role="note">
          <span>{t("adopt.untrusted", { name: project.name })}</span>
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            onClick={() => flow.review(project.id)}
          >
            {t("adopt.review")}
          </button>
        </div>
      ) : null}
      {result ? (
        <h3 ref={resultHeading} className="adopt-result__title" tabIndex={-1}>
          {t("adopt.result.title")}
        </h3>
      ) : null}
      {/* Mounted empty before the adopt, so the result is announced when it fills. */}
      <p className="adopt-result__summary" role="status">
        {summary}
      </p>
      {body}
      {result ? null : (
        <div className="adopt-actions">
          {candidates.length > 0 ? <p className="project-dialog__hint">{t("adopt.trashNote")}</p> : null}
          {onCancel ? (
            <button className="secondary-button secondary-button--compact" type="button" onClick={onCancel}>
              {t("projects.cancel")}
            </button>
          ) : null}
          <button
            className="primary-button primary-button--compact"
            type="button"
            disabled={adopting || chosen === 0 || untrusted || !scan.data}
            onClick={() => void adopt()}
          >
            {adopting ? t("adopt.adopting") : t("adopt.action", { count: chosen })}
          </button>
        </div>
      )}
      {flow.dialogs}
    </div>
  );
}
