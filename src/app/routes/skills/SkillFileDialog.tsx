// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Dialog shell, sidebar file list and preview ported from DailenG/skilldock@181b52b
// src/features/skills/components/SkillFileDialog.tsx (files and update panels). Read only: the
// store copy is changed by installs, updates and history restores, each one commit (SPEC 12.4).
// The data layer is new: `list_skill_files`/`read_skill_file` and `skill_update_diff`.

import { useQuery } from "@tanstack/react-query";
import { lazy, Suspense, useEffect, useId, useState, type ReactNode } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { ExternalScanSection, gateDescription, isExternalDangerous, useReviewScan } from "@/app/components/ExternalScan";
import { Modal } from "@/app/components/Modal";
import { DangerousGate, isDangerous, ScanFindings } from "@/app/components/ScanFindings";
import { TrustSummary, useTrustPrompt, type TrustAnswer } from "@/app/components/TrustDialog";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t } from "@/i18n/en";
import type { SkillFileChange, SkillInfo, SkillUpdateCheck } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";
import { CHANGE_KIND_KEYS, failureText, fileTreeRows, formatSize } from "./labels";

// CodeMirror is most of the bundle and only the update diff needs it: load it on first use.
const SkillDiffView = lazy(() => import("./SkillDiffView").then((module) => ({ default: module.SkillDiffView })));

type FileItemProps = {
  path: string;
  selected: boolean;
  badge?: string;
  /** An added or modified file that looks executable (M39): marked in the update review. */
  script?: boolean;
  onSelect: () => void;
};

function FileItem({ path, selected, badge, script = false, onSelect }: FileItemProps) {
  const parts = path.split("/");
  return (
    <button
      className={`skill-file-dialog__tree-item skill-file-dialog__tree-item--file${selected ? " is-selected" : ""}`}
      style={{ paddingLeft: `${12 + (parts.length - 1) * 14}px` }}
      type="button"
      title={path}
      aria-current={selected ? "true" : undefined}
      onClick={onSelect}
    >
      <span className="skill-file-dialog__tree-label">{parts[parts.length - 1]}</span>
      {badge ? <span className="skill-file-dialog__language-badge">{badge}</span> : null}
      {script ? (
        <span className="status-badge tone-warning" title={t("skill.update.scriptHint")}>
          {t("skill.update.script")}
        </span>
      ) : null}
    </button>
  );
}

/** A folder's heading in the files tree (M28c, finding 16): not a control, its files follow indented. */
function FolderRow({ name, depth }: { name: string; depth: number }) {
  return (
    <div
      className="skill-file-dialog__tree-item skill-file-dialog__tree-item--directory skill-file-dialog__tree-folder"
      style={{ paddingLeft: `${12 + depth * 14}px` }}
    >
      <span className="skill-file-dialog__tree-label">{`${name}/`}</span>
    </div>
  );
}

/** The sidebar rows for `paths`: folder headings, then each file through `file`. */
function treeItems(paths: readonly string[], file: (path: string) => ReactNode): ReactNode[] {
  return fileTreeRows(paths).map((row) =>
    row.kind === "folder" ? <FolderRow key={`folder:${row.path}`} name={row.name} depth={row.depth} /> : file(row.path),
  );
}

type DialogShellProps = {
  title: string;
  onClose: () => void;
  sidebar: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
};

function DialogShell({ title, onClose, sidebar, children, footer }: DialogShellProps) {
  const titleId = useId();

  return (
    <Modal
      className="skill-file-dialog"
      backdropClassName="dialog-backdrop skill-file-dialog-backdrop"
      labelledBy={titleId}
      initialFocus="dialog"
      onClose={onClose}
    >
      <div className="skill-file-dialog__header">
        <div className="skill-file-dialog__title">
          <h3 id={titleId}>{title}</h3>
        </div>
        <div className="skill-file-dialog__toolbar">
          <button className="skill-file-dialog__close" type="button" onClick={onClose} aria-label={t("skill.files.close")}>
            <span aria-hidden="true">×</span>
          </button>
        </div>
      </div>
      <div className="skill-file-dialog__body">
        <aside className="skill-file-dialog__sidebar" aria-label={t("skill.files.list")}>
          {sidebar}
        </aside>
        <section className="skill-file-dialog__editor">{children}</section>
      </div>
      {footer ? <div className="skill-file-dialog__footer">{footer}</div> : null}
    </Modal>
  );
}

/** The store copy's files, each shown as text (SkillDock's file browser). */
export function SkillFileDialog({ skill, onClose }: { skill: SkillInfo; onClose: () => void }) {
  const { commands } = useTransport();
  const files = useQuery({
    queryKey: [...storeQueryKeys.skillFiles, skill.id],
    queryFn: async () => unwrap(await commands.listSkillFiles(skill.id)),
  });
  const [selected, setSelected] = useState("");
  const paths = files.data?.map((file) => file.path) ?? [];
  const current = selected || (paths.includes("SKILL.md") ? "SKILL.md" : paths[0] ?? "");
  const file = useQuery({
    queryKey: [...storeQueryKeys.skillFile, skill.id, current],
    queryFn: async () => unwrap(await commands.readSkillFile(skill.id, current)),
    enabled: current !== "",
  });

  return (
    <DialogShell
      title={t("skill.files.title", { name: skill.name })}
      onClose={onClose}
      sidebar={treeItems(paths, (path) => (
        <FileItem key={path} path={path} selected={path === current} onSelect={() => setSelected(path)} />
      ))}
    >
      <div className="skill-file-dialog__editor-header">
        <div className="skill-file-dialog__file-identity">
          <strong>{current}</strong>
          {file.data ? (
            <span className="skill-file-dialog__language-badge">{formatSize(file.data.bytes)}</span>
          ) : null}
        </div>
      </div>
      {files.isError || file.isError ? (
        <div className="skill-file-dialog__empty" role="alert">{failureText(files.error ?? file.error)}</div>
      ) : !current ? (
        <div className="skill-file-dialog__empty">{t("skill.files.empty")}</div>
      ) : file.isPending ? (
        <div className="skill-file-dialog__empty">{t("skill.files.loading")}</div>
      ) : file.data?.text == null ? (
        <div className="skill-file-dialog__empty">{t("skill.files.binary")}</div>
      ) : (
        <div className="skill-file-dialog__preview">
          <pre className="skill-file-dialog__plain-preview" aria-label={current}>
            {file.data.text}
          </pre>
        </div>
      )}
    </DialogShell>
  );
}

type SkillUpdateDialogProps = {
  skill: SkillInfo;
  check: SkillUpdateCheck;
  isUpdating: boolean;
  /** `allowDangerous` only through "Update anyway"; `answer` holds the trust dialog's answers when the sources needed them. */
  onUpdate: (allowDangerous: boolean, answer: TrustAnswer) => void;
  onClose: () => void;
};

/**
 * The update preview (SPEC 12.4): each changed file's diff (executable-looking
 * files marked), the source's trust state, the built-in scan of the new
 * content (M42) and, when it's on, SkillSpector's (M51), then Update. An
 * untrusted or lapsed source asks first; a dangerous verdict of either scan
 * updates only through "Update anyway".
 */
export function SkillUpdateDialog({ skill, check, isUpdating, onUpdate, onClose }: SkillUpdateDialogProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const { resolveTrust, dialog: trustDialog } = useTrustPrompt();
  const [selected, setSelected] = useState<SkillFileChange | null>(check.changes[0] ?? null);
  const latest = check.latest ?? "";
  // The backend remembers this run for `update_skill(id, latest, ...)`: the gate is the scan shown. The scan
  // starts as soon as the review opens.
  const external = useReviewScan((runId) => commands.externalScanUpdate(skill.id, latest, runId));
  const shown = external.shown;
  const [acknowledged, setAcknowledged] = useState(false);
  useEffect(() => setAcknowledged(false), [shown]);
  const builtinDangerous = isDangerous(check.scan);
  const externalDangerous = isExternalDangerous(shown);
  const dangerous = builtinDangerous || externalDangerous;
  const actionNote = external.needsScan
    ? t("external.live.needsScan", { action: t(dangerous ? "skill.update.anyway" : "skill.update.confirm") })
    : dangerous
      ? t("external.live.confirm", { action: t("skill.update.anyway") })
      : t("external.live.available", { action: t("skill.update.confirm") });
  const blocked = check.trust?.state.kind === "blocked" || check.movedTrust?.state.kind === "blocked";
  const diff = useQuery({
    queryKey: ["skill_update_diff", skill.id, check.latest, selected?.path],
    queryFn: async () => unwrap(await commands.skillUpdateDiff(skill.id, selected!.path)),
    enabled: selected !== null,
  });

  async function update(allowDangerous: boolean) {
    const answer = await resolveTrust(check, () =>
      notify({ tone: "error", message: errorMessage("source_blocked") }),
    );
    if (answer) {
      onUpdate(allowDangerous, answer);
    }
  }

  return (
    <>
      <DialogShell
        title={t("skill.update.title", { name: skill.name })}
        onClose={onClose}
        sidebar={treeItems(
          check.changes.map((change) => change.path),
          (path) => {
            const change = check.changes.find((item) => item.path === path)!;
            return (
              <FileItem
                key={change.path}
                path={change.path}
                badge={t(CHANGE_KIND_KEYS[change.kind])}
                script={change.script}
                selected={change.path === selected?.path}
                onSelect={() => setSelected(change)}
              />
            );
          },
        )}
        footer={
          <>
            <button className="secondary-button" type="button" onClick={onClose}>
              {t("skill.update.cancel")}
            </button>
            {dangerous ? (
              <button
                className="secondary-button danger-button"
                type="button"
                disabled={isUpdating || blocked || !acknowledged || external.busy || external.needsScan}
                onClick={() => void update(true)}
              >
                {isUpdating ? t("skills.updating") : t("skill.update.anyway")}
              </button>
            ) : (
              <button
                className="primary-button"
                type="button"
                disabled={isUpdating || blocked || external.busy || external.needsScan}
                onClick={() => void update(false)}
              >
                {isUpdating ? t("skills.updating") : t("skill.update.confirm")}
              </button>
            )}
          </>
        }
      >
        <div className="skill-file-dialog__editor-header">
          <div className="skill-file-dialog__file-identity">
            <span>{t("skill.update.description", { commit: (check.latest ?? "").slice(0, 7) })}</span>
          </div>
        </div>
        <div className="skill-update-review">
          {check.trust ? <TrustSummary info={check.trust} /> : null}
          {check.movedTrust ? <TrustSummary info={check.movedTrust} label={t("trust.summary.moved")} /> : null}
          {blocked ? <p className="dialog-error" role="alert">{errorMessage("source_blocked")}</p> : null}
          {check.scan ? <ScanFindings scan={check.scan} /> : null}
          {external.phase !== "off" ? <ExternalScanSection review={external} actionNote={actionNote} /> : null}
          {dangerous ? (
            <DangerousGate
              acknowledged={acknowledged}
              onChange={setAcknowledged}
              description={gateDescription(builtinDangerous, externalDangerous, "update")}
            />
          ) : null}
        </div>
        {check.changes.length === 0 ? (
          <div className="skill-file-dialog__empty">{t("skill.update.none")}</div>
        ) : diff.isError ? (
          <div className="skill-file-dialog__empty" role="alert">{failureText(diff.error)}</div>
        ) : diff.isPending ? (
          <div className="skill-file-dialog__empty">{t("skill.update.loading")}</div>
        ) : diff.data.binary ? (
          <div className="skill-file-dialog__empty">{t("skill.update.binary")}</div>
        ) : (
          <Suspense fallback={<div className="skill-file-dialog__empty">{t("skill.update.loading")}</div>}>
            <SkillDiffView
              before={diff.data.before ?? ""}
              after={diff.data.after ?? ""}
              ariaLabel={t("skill.diff.aria", { path: diff.data.path })}
            />
          </Suspense>
        )}
      </DialogShell>
      {trustDialog}
    </>
  );
}
