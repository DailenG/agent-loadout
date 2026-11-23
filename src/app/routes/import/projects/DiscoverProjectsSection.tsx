// Discover projects (#3, SPEC 13.3): the folders the clients already know
// (Claude Code, Codex, omp, Pi) that this machine has no project for, as a
// checklist; "Add selected" binds them untrusted (a folder without a
// manifest gets a private one) and each added project offers its review.
// Shown on the Import page and, in a dialog, from the Projects page.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { storeQueryKeys, useInvalidateImports, useInvalidateStore } from "@/app/apply/changeNotice";
import { orderByRecommended } from "@/app/clients";
import { useNotifications } from "@/app/components/notifications";
import { reasonText } from "@/app/routes/import/skills/labels";
import { useAddFolderFlow } from "@/app/routes/projects/AddFolderFlow";
import { LAYOUT_KEYS, TRUST_KEYS, failureText } from "@/app/routes/projects/labels";
import { t } from "@/i18n/en";
import type { DiscoveredProject, DiscoveredProjectsAdded } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

/** Not a store query: discovery reads client files, so it runs again on every open. */
const DISCOVER_KEY = ["discover_projects"] as const;

function ProjectRowItem(props: {
  project: DiscoveredProject;
  checked: boolean;
  clientNames: (ids: string[]) => string;
  onChange: (checked: boolean) => void;
}) {
  const { project, checked, clientNames, onChange } = props;
  const hintId = useId();
  return (
    <li className={`discover-item${project.error ? " is-blocked" : ""}`}>
      <label className="project-dialog__choice">
        <input
          type="checkbox"
          checked={checked}
          disabled={project.error !== null}
          aria-describedby={hintId}
          onChange={(event) => onChange(event.target.checked)}
        />
        <span>
          <strong>{project.manifest?.name ?? project.name}</strong>
          <span className="adopt-path" title={project.path}>
            {project.path}
          </span>
        </span>
      </label>
      <div id={hintId} className="discover-item__meta">
        <span>{t("discover.knownBy", { clients: clientNames(project.clients) })}</span>
        {project.manifest ? (
          <span className="status-badge tone-info">
            {t(LAYOUT_KEYS[project.manifest.layout])}
          </span>
        ) : (
          <span className="status-badge tone-neutral">{t("discover.noManifest")}</span>
        )}
        {project.error ? <span className="discover-item__problem">{reasonText(project.error)}</span> : null}
      </div>
    </li>
  );
}

function AddedView(props: {
  result: DiscoveredProjectsAdded;
  candidates: DiscoveredProject[];
  onReview: (id: string) => void;
  onAgain: () => void;
}) {
  const { result, candidates, onReview, onAgain } = props;
  const nameOf = (candidate: string, path: string) => {
    const found = candidates.find((entry) => entry.id === candidate);
    return found?.manifest?.name ?? found?.name ?? path;
  };
  return (
    <div className="adopt-result">
      {result.added.length > 0 ? (
        <section className="adopt-result__group" aria-label={t("discover.result.added")}>
          <h4>{t("discover.result.added")}</h4>
          <ul className="adopt-list adopt-list--plain">
            {result.added.map((added) => (
              <li key={added.id} className="discover-added">
                <span className="discover-added__copy">
                  <strong>{nameOf(added.candidate, added.path)}</strong>
                  <span className="adopt-path" title={added.path}>
                    {added.path}
                  </span>
                  <span className="project-dialog__hint">
                    {t(TRUST_KEYS[added.trust])}
                    {added.createdManifest ? ` · ${t("discover.result.createdManifest")}` : ""}
                  </span>
                </span>
                <button
                  className="primary-button primary-button--compact"
                  type="button"
                  onClick={() => onReview(added.id)}
                  aria-label={t("discover.result.reviewAria", { name: nameOf(added.candidate, added.path) })}
                >
                  {t("discover.result.review")}
                </button>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {result.failed.length > 0 ? (
        <section className="adopt-result__group" aria-label={t("discover.result.failed")}>
          <h4>{t("discover.result.failed")}</h4>
          <ul className="adopt-list adopt-list--plain">
            {result.failed.map((failed) => (
              <li key={failed.candidate} className="adopt-result-row">
                <strong>{nameOf(failed.candidate, failed.path)}</strong>
                <span className="adopt-path" title={failed.path}>
                  {failed.path}
                </span>
                <span className="adopt-reason">{reasonText(failed.error)}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      <div className="adopt-actions">
        <button className="secondary-button secondary-button--compact" type="button" onClick={onAgain}>
          {t("discover.again")}
        </button>
      </div>
    </div>
  );
}

export type DiscoverProjectsSectionProps = {
  /** Accepted for the Import page's shared section contract; adding applies nothing. */
  onOpenPending?: () => void;
  /** In the Projects page's dialog: the dialog has the title, and Cancel closes it. */
  onCancel?: () => void;
  /** Id for the description, so the Projects page's dialog is described by it. */
  descriptionId?: string;
};

export function DiscoverProjectsSection({ onCancel, descriptionId }: DiscoverProjectsSectionProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const invalidateImports = useInvalidateImports();
  const flow = useAddFolderFlow();
  const titleId = useId();
  const resultHeading = useRef<HTMLHeadingElement | null>(null);
  const discovery = useQuery({
    queryKey: DISCOVER_KEY,
    queryFn: async () => unwrap(await commands.discoverProjects()),
    staleTime: 0,
    refetchOnMount: "always",
  });
  const tools = useQuery({
    queryKey: storeQueryKeys.tools,
    queryFn: async () => unwrap(await commands.listTools()),
  });
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [result, setResult] = useState<DiscoveredProjectsAdded | null>(null);
  const [adding, setAdding] = useState(false);

  // Add selected leaves with the list: focus goes to the result's heading, not the page.
  useEffect(() => {
    if (result) {
      resultHeading.current?.focus();
    }
  }, [result]);

  const candidates = discovery.data?.candidates ?? [];
  const addable = candidates.filter((candidate) => candidate.error === null);
  const chosen = addable.filter((candidate) => selected.has(candidate.id));
  const clientNames = (ids: string[]) => {
    const known = new Map((tools.data ?? []).map((tool) => [tool.id, tool.name]));
    return orderByRecommended(ids, (id) => id)
      .map((id) => known.get(id) ?? id)
      .join(", ");
  };

  async function add() {
    setAdding(true);
    try {
      const done = unwrap(await commands.addDiscoveredProjects(chosen.map((candidate) => candidate.id)));
      setResult(done);
      setSelected(new Set());
      notify(
        done.failed.length > 0
          ? { tone: "info", message: t("discover.result.summary", { added: done.added.length, failed: done.failed.length }) }
          : { tone: "success", message: t("discover.added", { count: done.added.length }) },
      );
      await invalidateImports();
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    } finally {
      setAdding(false);
      await invalidate();
    }
  }

  function again() {
    setResult(null);
    void discovery.refetch();
  }

  let body: JSX.Element;
  if (result) {
    body = <AddedView result={result} candidates={candidates} onReview={flow.review} onAgain={again} />;
  } else if (discovery.isPending) {
    body = <p className="tools-note">{t("discover.loading")}</p>;
  } else if (discovery.isError) {
    body = (
      <div className="dialog-error" role="alert">
        <span>{failureText(discovery.error)}</span>
        <button className="secondary-button secondary-button--compact" type="button" onClick={() => void discovery.refetch()}>
          {t("app.action.retry")}
        </button>
      </div>
    );
  } else if (candidates.length === 0) {
    body = <p className="tools-note">{t("discover.empty")}</p>;
  } else {
    body = (
      <>
        <div className="discover-toolbar">
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            disabled={addable.length === 0 || chosen.length === addable.length}
            onClick={() => setSelected(new Set(addable.map((candidate) => candidate.id)))}
          >
            {t("discover.selectAll")}
          </button>
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            disabled={chosen.length === 0}
            onClick={() => setSelected(new Set())}
          >
            {t("discover.selectNone")}
          </button>
        </div>
        <ul className="adopt-list" aria-label={t("discover.listAria")}>
          {candidates.map((candidate) => (
            <ProjectRowItem
              key={candidate.id}
              project={candidate}
              checked={selected.has(candidate.id)}
              clientNames={clientNames}
              onChange={(checked) =>
                setSelected((current) => {
                  const next = new Set(current);
                  if (checked) {
                    next.add(candidate.id);
                  } else {
                    next.delete(candidate.id);
                  }
                  return next;
                })
              }
            />
          ))}
        </ul>
      </>
    );
  }

  return (
    <section className="adopt-section discover-section" aria-labelledby={onCancel ? undefined : titleId}>
      {onCancel ? null : (
        <h2 id={titleId} className="adopt-section__title">
          {t("discover.title")}
        </h2>
      )}
      <p id={descriptionId} className="adopt-section__description">
        {t("discover.description")}
      </p>
      {(discovery.data?.warnings ?? []).map((warning, index) => (
        <p key={`${index}-${warning.code}`} className="mcp-page-warning" role="note">
          {reasonText(warning)}
        </p>
      ))}
      {result ? (
        <h3 ref={resultHeading} className="adopt-result__title" tabIndex={-1}>
          {t("discover.result.title")}
        </h3>
      ) : null}
      {/* Mounted empty before the add, so the result is announced when it fills. */}
      <p className="adopt-result__summary" role="status">
        {result ? t("discover.result.summary", { added: result.added.length, failed: result.failed.length }) : ""}
      </p>
      {body}
      {result ? null : (
        <div className="adopt-actions">
          {chosen.some((candidate) => !candidate.manifest) ? (
            <p className="project-dialog__hint">{t("discover.privateNote")}</p>
          ) : null}
          {onCancel ? (
            <button className="secondary-button secondary-button--compact" type="button" onClick={onCancel}>
              {t("projects.cancel")}
            </button>
          ) : null}
          <button
            className="primary-button primary-button--compact"
            type="button"
            disabled={adding || chosen.length === 0}
            onClick={() => void add()}
          >
            {adding ? t("discover.adding") : t("discover.add", { count: chosen.length })}
          </button>
        </div>
      )}
      {flow.dialogs}
    </section>
  );
}
