// Projects page (SPEC 13.3): the projects this machine knows, Add folder and
// Init manifest (the folder is picked in the OS dialog and stays in the
// backend), the trust review, Apply, Remove from list, and the effective
// config matrix of a project.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { storeQueryKeys, useChangeNotice } from "@/app/apply/changeNotice";
import { HeaderToolbar } from "@/app/components/HeaderSlots";
import { useNotifications } from "@/app/components/notifications";
import { EffectiveMatrixView } from "@/app/routes/projects/EffectiveMatrix";
import { FolderDialog } from "@/app/routes/projects/FolderDialog";
import { TrustReviewDialog } from "@/app/routes/projects/TrustReviewDialog";
import { LAYOUT_KEYS, TRUST_KEYS, failureText } from "@/app/routes/projects/labels";
import { errorMessage, t } from "@/i18n/en";
import type { PickedFolder, ProjectRow, ProjectTrust } from "@/ipc/bindings";
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
  const projects = useQuery({
    queryKey: storeQueryKeys.projects,
    queryFn: async () => unwrap(await commands.listProjects()),
  });
  const [picked, setPicked] = useState<{ folder: PickedFolder; intent: "add" | "init" } | null>(null);
  const [reviewing, setReviewing] = useState<string | null>(null);
  const [matrixOf, setMatrixOf] = useState<string | null>(null);
  const [removing, setRemoving] = useState<ProjectRow | null>(null);

  const pick = useMutation({
    mutationFn: async (intent: "add" | "init") => ({
      folder: unwrap(await commands.pickProjectFolder()),
      intent,
    }),
    onSuccess: ({ folder, intent }) => {
      if (folder) {
        setPicked({ folder, intent });
      }
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

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
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          disabled={pick.isPending}
          onClick={() => pick.mutate("add")}
        >
          {t("projects.action.addFolder")}
        </button>
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          disabled={pick.isPending}
          onClick={() => pick.mutate("init")}
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
              onReview={() => setReviewing(project.id)}
            />
          ))}
        </div>
      </section>
      {picked ? (
        <FolderDialog
          folder={picked.folder}
          intent={picked.intent}
          onClose={() => setPicked(null)}
          onDone={(change, review) => {
            setPicked(null);
            void projects.refetch();
            if (review) {
              setReviewing(change.id);
            }
          }}
        />
      ) : null}
      {reviewing ? (
        <TrustReviewDialog
          projectId={reviewing}
          onClose={() => setReviewing(null)}
          onTrusted={() => {
            setReviewing(null);
            void projects.refetch();
          }}
        />
      ) : null}
      {removing ? (
        <div className="dialog-backdrop" role="presentation" onClick={() => setRemoving(null)}>
          <div
            className="tool-manage-dialog project-dialog"
            role="alertdialog"
            aria-modal="true"
            aria-label={t("projects.action.remove")}
            onClick={(event) => event.stopPropagation()}
          >
            <p>{t("projects.remove.confirm", { name: removing.name })}</p>
            <div className="tool-manage-dialog__actions">
              <button className="secondary-button secondary-button--compact" type="button" onClick={() => setRemoving(null)}>
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
          </div>
        </div>
      ) : null}
    </div>
  );
}
