// "Restore from remote..." (SPEC 9.6 "Restore lists machines", 9.5,
// 11.9): the source is a configured folder remote or a folder picked in the
// OS dialog; the user picks the machine to restore from. Client files are
// not touched: the missing-secret checklist runs once and the apply plan is
// offered next.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { storeQueryKeys, useInvalidateStore } from "@/app/apply/changeNotice";
import { AppSelect } from "@/app/components/AppSelect";
import { useNotifications } from "@/app/components/notifications";
import { failureText } from "@/app/routes/projects/labels";
import { SettingsDialog } from "@/app/routes/settings/SettingsDialog";
import { SecretsChecklist } from "@/app/routes/settings/secrets/SecretsChecklist";
import { formatTime } from "@/app/routes/settings/versioning/labels";
import { platformLabels, t } from "@/i18n/en";
import type { Platform, RemoteMachine, RestoreFromKind, RestoreFromResult, RestoreSource } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type RestoreFromRemoteDialogProps = {
  open: boolean;
  onClose: () => void;
  onRestored?: (result: RestoreFromResult) => void;
  /** "Review and apply": opens Pending changes. */
  onOpenPending?: () => void;
};

const RESULT_KEYS = {
  created: "settings.versioning.restore.result.created",
  merged: "settings.versioning.restore.result.merged",
  restored: "settings.versioning.restore.result.restored",
} as const satisfies Record<RestoreFromKind, string>;

type Source = { source: RestoreSource; label: string };

const machineKey = (machine: RemoteMachine) => `${machine.storeId}/${machine.machineId}`;

export function RestoreFromRemoteDialog({ open, ...props }: RestoreFromRemoteDialogProps) {
  return open ? <RestoreForm {...props} /> : null;
}

function RestoreForm({ onClose, onRestored, onOpenPending }: Omit<RestoreFromRemoteDialogProps, "open">) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidateStore = useInvalidateStore();
  const overview = useQuery({
    queryKey: storeQueryKeys.remotes,
    queryFn: async () => unwrap(await commands.remotesOverview()),
  });
  const folderRemotes = (overview.data?.remotes ?? []).filter((remote) => remote.kind === "folder");
  const [source, setSource] = useState<Source | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const onError = (error: unknown) => notify({ tone: "error", message: failureText(error) });

  const list = useMutation({
    mutationFn: async (next: RestoreSource) => unwrap(await commands.listRestoreMachines(next)),
    onError,
  });

  function choose(next: Source) {
    setSource(next);
    setPicked(null);
    list.mutate(next.source);
  }

  const pick = useMutation({
    mutationFn: async () => unwrap(await commands.pickRemoteFolder()),
    onSuccess: (folder) => {
      if (folder) {
        choose({ source: { kind: "folder", token: folder.token }, label: folder.path });
      }
    },
    onError,
  });

  // May prompt for 1Password approval: run once, right after the restore
  // the user asked for, and again only after a fix.
  const check = useMutation({
    mutationFn: async () => unwrap(await commands.checkSecrets([])),
    onError,
  });

  const restore = useMutation({
    mutationFn: async ({ from, machineId }: { from: RestoreSource; machineId: string | null }) =>
      unwrap(await commands.restoreFromRemote(from, machineId)),
    onSuccess: (result) => {
      void invalidateStore();
      onRestored?.(result);
      check.mutate();
    },
    onError,
  });

  if (restore.data) {
    const result = restore.data;
    return (
      <SettingsDialog
        title={t("settings.versioning.restore.doneTitle")}
        closeLabel={t("settings.versioning.close")}
        onClose={onClose}
        wide
        actions={
          <>
            <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
              {t("settings.versioning.done")}
            </button>
            {onOpenPending ? (
              <button
                className="primary-button primary-button--compact"
                type="button"
                onClick={() => {
                  onOpenPending();
                  onClose();
                }}
              >
                {t("settings.versioning.reviewApply")}
              </button>
            ) : null}
          </>
        }
      >
        <p role="status">{t(RESULT_KEYS[result.kind], { from: result.from })}</p>
        {result.conflicts > 0 ? (
          <p className="settings-warning">{t("settings.versioning.restore.conflicts", { count: result.conflicts })}</p>
        ) : null}
        {result.tag ? (
          <p className="project-dialog__hint">{t("settings.versioning.restore.tag", { tag: result.tag })}</p>
        ) : null}
        <h4 className="settings-subheading">{t("settings.secrets.checklist.title")}</h4>
        <p className="project-dialog__hint">{t("settings.secrets.checklist.intro")}</p>
        {check.isPending ? <p className="project-dialog__hint">{t("settings.secrets.checklist.checking")}</p> : null}
        {check.data ? <SecretsChecklist checks={check.data} onChanged={() => check.mutate()} /> : null}
      </SettingsDialog>
    );
  }

  const listing = list.data;
  const machines = listing?.machines ?? [];
  const chosen = machines.find((machine) => machineKey(machine) === picked) ?? null;
  const canRestore = source !== null && listing !== undefined && (chosen !== null || (listing.direct && machines.length === 0));

  return (
    <SettingsDialog
      title={t("settings.versioning.restore.title")}
      subtitle={source?.label}
      closeLabel={t("settings.versioning.close")}
      onClose={onClose}
      wide
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
            {t("settings.versioning.cancel")}
          </button>
          <button
            className="primary-button primary-button--compact"
            type="button"
            disabled={!canRestore || restore.isPending}
            onClick={() => {
              if (source) {
                restore.mutate({ from: source.source, machineId: chosen?.machineId ?? null });
              }
            }}
          >
            {t("settings.versioning.restore.submit")}
          </button>
        </>
      }
    >
      <p className="project-dialog__hint">{t("settings.versioning.restore.intro")}</p>
      <div className="settings-actions">
        {folderRemotes.length > 0 ? (
          <AppSelect
            value={source?.source.kind === "remote" ? source.source.name : ""}
            options={folderRemotes.map((remote) => ({ value: remote.name, label: remote.name }))}
            onChange={(name) => choose({ source: { kind: "remote", name }, label: name })}
            ariaLabel={t("settings.versioning.restore.remote")}
            selectedLabel={source?.source.kind === "remote" ? undefined : t("settings.versioning.restore.remotePlaceholder")}
          />
        ) : null}
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          disabled={pick.isPending}
          onClick={() => pick.mutate()}
        >
          {t("settings.versioning.chooseFolder")}
        </button>
      </div>

      {list.isPending ? <p className="project-dialog__hint">{t("settings.versioning.restore.listing")}</p> : null}
      {listing && listing.artifactsIgnored > 0 ? (
        <p className="project-dialog__hint">
          {t("settings.versioning.restore.artifacts", { count: listing.artifactsIgnored })}
        </p>
      ) : null}
      {listing && machines.length === 0 ? (
        <p>{t(listing.direct ? "settings.versioning.restore.direct" : "settings.versioning.restore.noMachines")}</p>
      ) : null}
      {machines.length > 0 ? (
        <table className="settings-machines">
          <caption className="sr-only">{t("settings.versioning.restore.machines")}</caption>
          <thead>
            <tr>
              <th scope="col">
                <span className="sr-only">{t("settings.versioning.restore.column.pick")}</span>
              </th>
              <th scope="col">{t("settings.versioning.restore.column.machine")}</th>
              <th scope="col">{t("settings.versioning.restore.column.os")}</th>
              <th scope="col">{t("settings.versioning.restore.column.pushed")}</th>
              <th scope="col">{t("settings.versioning.restore.column.head")}</th>
            </tr>
          </thead>
          <tbody>
            {machines.map((machine) => {
              const key = machineKey(machine);
              return (
                <tr key={key} className={picked === key ? "is-selected" : undefined}>
                  <td>
                    <input
                      type="radio"
                      name="restore-machine"
                      aria-label={machine.label}
                      checked={picked === key}
                      onChange={() => setPicked(key)}
                    />
                  </td>
                  <td>
                    <span className="settings-machines__label">{machine.label}</span>
                    {machine.thisMachine ? (
                      <span className="status-badge tone-info">{t("settings.versioning.restore.thisMachine")}</span>
                    ) : null}
                    {!machine.sameStore ? (
                      <span className="status-badge tone-warning">{t("settings.versioning.restore.otherStore")}</span>
                    ) : null}
                  </td>
                  <td>{platformLabels[machine.os as Platform] ?? machine.os}</td>
                  <td>{formatTime(machine.pushedAt)}</td>
                  <td>{machine.subject ?? t("settings.versioning.restore.noSubject")}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      ) : null}
    </SettingsDialog>
  );
}
