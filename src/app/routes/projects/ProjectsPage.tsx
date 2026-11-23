// Projects page (SPEC 13.3): the projects this machine knows, Add folder and
// Init manifest (the folder is picked in the OS dialog and stays in the
// backend), the trust review, Apply, Remove from list, and the effective
// config matrix of a project.

import { useMutation } from "@tanstack/react-query";
import { useId, useState } from "react";
import { useChangeNotice } from "@/app/apply/changeNotice";
import { RefreshButton } from "@/app/apply/RefreshButton";
import { HeaderToolbar } from "@/app/components/HeaderSlots";
import { Modal } from "@/app/components/Modal";
import { useNotifications } from "@/app/components/notifications";
import { useAddFolderFlow } from "@/app/routes/projects/AddFolderFlow";
import { EffectiveMatrixView } from "@/app/routes/projects/EffectiveMatrix";
import { LAYOUT_KEYS, TRUST_KEYS, failureText } from "@/app/routes/projects/labels";
import { useActiveScope, useProjectList } from "@/app/scope/ActiveScope";
import { errorMessage, t } from "@/i18n/en";
import type { ProjectRow, ProjectTrust } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

const TRUST_TONES: Record<ProjectTrust, string> = {
  trusted: "tone-positive",
  untrusted: "tone-warning",
  changed: "tone-warning",
  missing: "tone-danger",
  invalid: "tone-danger",
};

type ProjectCardProps = {
  project: ProjectRow;
  busy: boolean;
  onOpen: () => void;
  onEffective: () => void;
  onApply: () => void;
  onRemove: () => void;
  onReview: () => void;
};

function ProjectCard({ project, busy, onOpen, onEffective, onApply, onRemove, onReview }: ProjectCardProps) {
  const needsReview = project.trust === "untrusted" || project.trust === "changed";
  const usable = project.trust !== "missing" && project.trust !== "invalid";
  return (
    <article className="panel-card project-card" aria-label={project.name}>
      <div className="project-card__head">
        <strong className="project-card__name">{project.name}</strong>
        <span className="project-card__path" title={project.path}>
          {project.path}
        </span>
      </div>
      <div className="project-card__meta">
        <span>{project.layout ? t(LAYOUT_KEYS[project.layout]) : t("projects.layout.none")}</span>
        <span>
          {t("projects.counts", {
            servers: project.servers,
            clients: project.clients,
            pending: project.pending,
          })}
        </span>
        <span className={`status-badge ${TRUST_TONES[project.trust]}`}>
          {t(TRUST_KEYS[project.trust])}
        </span>
      </div>
      {project.error ? (
        <p className="project-card__problem" role="alert">
          {errorMessage(project.error.code)}
        </p>
      ) : null}
      <div className="project-card__actions">
        {needsReview ? (
          <button className="primary-button primary-button--compact" type="button" onClick={onReview}>
            {t("projects.action.review")}
          </button>
        ) : null}
        <button className="secondary-button secondary-button--compact" type="button" onClick={onOpen} disabled={!usable}>
          {t("projects.action.open")}
        </button>
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          onClick={onEffective}
          disabled={!usable}
        >
          {t("projects.action.effective")}
        </button>
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          onClick={onApply}
          disabled={busy || project.trust !== "trusted"}
        >
          {t("projects.action.apply")}
        </button>
        <button className="ghost-button" type="button" onClick={onRemove} disabled={busy}>
          {t("projects.action.remove")}
        </button>
      </div>
    </article>
  );
}

export function ProjectsPage() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const reportChange = useChangeNotice();
  const projects = useProjectList();
  const { setScope } = useActiveScope();
  // A new project becomes the active scope, as from the sidebar.
  const flow = useAddFolderFlow({ onAdded: (change) => setScope({ kind: "project", id: change.id }) });
  const [matrixOf, setMatrixOf] = useState<string | null>(null);
  const [removing, setRemoving] = useState<ProjectRow | null>(null);
  const removeQuestionId = useId();

  const apply = useMutation({
    mutationFn: async (project: ProjectRow) => ({
      project,
      result: unwrap(await commands.applyProjectFiles(project.id)),
    }),
    onSuccess: ({ project, result }) => {
      reportChange(result, t("projects.applied", { name: project.name }));
      void projects.refetch();
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const remove = useMutation({
    mutationFn: async (project: ProjectRow) => ({
      project,
      result: unwrap(await commands.removeProject(project.id)),
    }),
    onSuccess: ({ project, result }) => {
      setRemoving(null);
      reportChange(result, t("projects.removed", { name: project.name }));
      void projects.refetch();
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  async function open(project: ProjectRow) {
    try {
      unwrap(await commands.revealProjectFolder(project.id));
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    }
  }

  if (matrixOf) {
    return <EffectiveMatrixView projectId={matrixOf} onBack={() => setMatrixOf(null)} />;
  }

  const rows = projects.data ?? [];
  return (
    <div className="projects-route">
      <HeaderToolbar>
        <RefreshButton label={t("projects.action.refresh")} />
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          disabled={flow.picking}
          onClick={() => flow.start("add")}
        >
          {t("projects.action.addFolder")}
        </button>
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          disabled={flow.picking}
          onClick={() => flow.start("init")}
        >
          {t("projects.action.initManifest")}
        </button>
      </HeaderToolbar>
      <section className="projects-section">
        {projects.isPending ? <p className="tools-note">{t("projects.loading")}</p> : null}
        {projects.isError ? (
          <div className="panel-card empty-state page-empty-state" role="alert">
            <h3>{t("app.error.loadFailed")}</h3>
            <p>{failureText(projects.error)}</p>
            <button className="secondary-button" type="button" onClick={() => void projects.refetch()}>
              {t("app.action.retry")}
            </button>
          </div>
        ) : null}
        {projects.data && rows.length === 0 ? (
          <div className="panel-card empty-state page-empty-state">
            <h3>{t("page.empty.title")}</h3>
            <p>{t("page.empty.projects")}</p>
            <button
              className="secondary-button"
              type="button"
              disabled={flow.picking}
              onClick={() => flow.start("add")}
            >
              {t("projects.action.addFolder")}
            </button>
          </div>
        ) : null}
        <div className="project-list">
          {rows.map((project) => (
            <ProjectCard
              key={project.id}
              project={project}
              busy={apply.isPending || remove.isPending}
              onOpen={() => void open(project)}
              onEffective={() => setMatrixOf(project.id)}
              onApply={() => apply.mutate(project)}
              onRemove={() => setRemoving(project)}
              onReview={() => flow.review(project.id)}
            />
          ))}
        </div>
      </section>
      {flow.dialogs}
      {removing ? (
        <Modal
          className="tool-manage-dialog project-dialog"
          role="alertdialog"
          label={t("projects.action.remove")}
          describedBy={removeQuestionId}
          onClose={() => setRemoving(null)}
        >
          <p id={removeQuestionId}>{t("projects.remove.confirm", { name: removing.name })}</p>
          <div className="tool-manage-dialog__actions">
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              onClick={() => setRemoving(null)}
              data-autofocus
            >
              {t("projects.cancel")}
            </button>
            <button
              className="primary-button primary-button--compact"
              type="button"
              disabled={remove.isPending}
              onClick={() => remove.mutate(removing)}
            >
              {t("projects.remove.confirmAction")}
            </button>
          </div>
        </Modal>
      ) : null}
    </div>
  );
}
