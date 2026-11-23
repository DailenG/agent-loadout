// Trust review (SPEC 13.3, 17): before a project applies, its manifests are
// shown (the content of a new file, the diff of a changed one, against what
// was last trusted); Trust accepts exactly the content shown.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useId } from "react";
import { useChangeNotice } from "@/app/apply/changeNotice";
import { diffLineClass } from "@/app/apply/labels";
import { Modal } from "@/app/components/Modal";
import { useNotifications } from "@/app/components/notifications";
import { failureText, projectRelativePath } from "@/app/routes/projects/labels";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type { ManifestChangeKind, ReviewedFile } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

const CHANGE_KEYS = {
  new: "projects.review.change.new",
  changed: "projects.review.change.changed",
  unchanged: "projects.review.change.unchanged",
  removed: "projects.review.change.removed",
} as const satisfies Record<ManifestChangeKind, MessageKey>;

export type TrustReviewDialogProps = {
  projectId: string;
  onClose: () => void;
  onTrusted: () => void;
};

function ReviewedFileView({ file }: { file: ReviewedFile }) {
  const lines = (file.diff ?? file.content ?? "").replace(/\n$/, "").split("\n");
  // The project folder is under the dialog title; the file is named relative to it.
  const path = projectRelativePath(file.path);
  const label = file.diff
    ? t("projects.review.diff", { file: path })
    : t("projects.review.content", { file: path });
  return (
    <section className="project-review__file" aria-label={path}>
      <div className="project-review__file-head">
        <strong>{path}</strong>
        <span className="status-badge tone-warning">{t(CHANGE_KEYS[file.change])}</span>
      </div>
      {file.change === "removed" || file.change === "unchanged" ? null : (
        <pre className="apply-diff" aria-label={label}>
          {lines.map((line, index) => (
            <span key={index} className={file.diff ? diffLineClass(line) : "apply-diff__line"}>
              {line}
              {"\n"}
            </span>
          ))}
        </pre>
      )}
    </section>
  );
}

export function TrustReviewDialog({ projectId, onClose, onTrusted }: TrustReviewDialogProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const reportChange = useChangeNotice();
  const titleId = useId();
  const review = useQuery({
    queryKey: ["review_project", projectId],
    queryFn: async () => unwrap(await commands.reviewProject(projectId)),
    // The review must show the files as they are now.
    staleTime: 0,
    gcTime: 0,
  });

  const trust = useMutation({
    mutationFn: async (hash: string) => unwrap(await commands.trustProject(projectId, hash)),
    onSuccess: (change) => {
      reportChange(change, t("projects.review.trusted", { name: review.data?.name ?? projectId }));
      onTrusted();
    },
    onError: (error) => {
      notify({ tone: "error", message: failureText(error) });
      // A manifest that changed after the review is shown again.
      void review.refetch();
    },
  });

  const data = review.data;
  return (
    <Modal
      className="tool-manage-dialog project-dialog project-review"
      labelledBy={titleId}
      initialFocus="dialog"
      onClose={onClose}
    >
      <div className="tool-manage-dialog__header">
        <div className="tool-manage-dialog__title">
          <h3 id={titleId}>{t("projects.review.title", { name: data?.name ?? projectId })}</h3>
          {data ? <p title={data.path}>{data.path}</p> : null}
        </div>
        <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={t("projects.cancel")}>
          ×
        </button>
      </div>
      <div className="project-dialog__body project-review__body">
        <p className="project-review__intro" role="note">
          {t("projects.review.intro")}
        </p>
        {review.isPending ? <p>{t("projects.review.loading")}</p> : null}
        {review.isError ? <p role="alert">{failureText(review.error)}</p> : null}
        {data?.files.map((file) => <ReviewedFileView key={file.file} file={file} />)}
        {data?.warnings.map((warning, index) => (
          <p key={index} className="project-dialog__hint">
            {errorMessage(warning.code)}
          </p>
        ))}
      </div>
      <div className="tool-manage-dialog__actions">
        <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
          {t("projects.cancel")}
        </button>
        <button
          className="primary-button primary-button--compact"
          type="button"
          disabled={!data || trust.isPending || review.isFetching}
          onClick={() => data && trust.mutate(data.hash)}
        >
          {t("projects.review.trust")}
        </button>
      </div>
    </Modal>
  );
}
