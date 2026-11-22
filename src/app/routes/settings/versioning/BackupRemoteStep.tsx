// The first run's optional "Add a backup remote" step (SPEC 15.2, 9.6): Git
// URL and Folder as equal choices; "Not now" leaves the store local-only,
// and Settings then shows "No backup remote".

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { failureText } from "@/app/routes/projects/labels";
import { SettingsDialog } from "@/app/routes/settings/SettingsDialog";
import { AddRemoteDialog } from "@/app/routes/settings/versioning/AddRemoteDialog";
import { KIND_KEYS } from "@/app/routes/settings/versioning/labels";
import { t } from "@/i18n/en";
import type { RemoteKindDto } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type BackupRemoteStepProps = {
  /** The step was answered: a remote added, or "Not now". */
  onDone: () => void;
};

export function BackupRemoteStep({ onDone }: BackupRemoteStepProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const queryClient = useQueryClient();
  const [choice, setChoice] = useState<RemoteKindDto | null>(null);

  const notNow = useMutation({
    mutationFn: async () => unwrap(await commands.dismissBackupRemoteOffer()),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: storeQueryKeys.remotes });
      onDone();
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  if (choice) {
    return (
      <AddRemoteDialog
        open
        firstRun
        initialKind={choice}
        onClose={() => setChoice(null)}
        onAdded={() => {
          void queryClient.invalidateQueries({ queryKey: storeQueryKeys.remotes });
          onDone();
        }}
      />
    );
  }

  const dismiss = () => {
    if (!notNow.isPending) {
      notNow.mutate();
    }
  };
  return (
    <SettingsDialog
      title={t("firstRun.backupRemote.title")}
      closeLabel={t("settings.versioning.close")}
      onClose={dismiss}
      actions={
        <>
          <button className="ghost-button" type="button" disabled={notNow.isPending} onClick={dismiss}>
            {t("firstRun.backupRemote.notNow")}
          </button>
          <button className="secondary-button secondary-button--compact" type="button" onClick={() => setChoice("git")}>
            {t(KIND_KEYS.git)}
          </button>
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            onClick={() => setChoice("folder")}
          >
            {t(KIND_KEYS.folder)}
          </button>
        </>
      }
    >
      <p>{t("firstRun.backupRemote.intro")}</p>
      <p className="project-dialog__hint">{t("firstRun.backupRemote.choices")}</p>
    </SettingsDialog>
  );
}

/**
 * Mounted once in the app shell: shows the step while the backend still
 * offers it (no remote, and not answered on this machine).
 */
export function FirstRunBackupRemote() {
  const { commands } = useTransport();
  const [done, setDone] = useState(false);
  const overview = useQuery({
    queryKey: storeQueryKeys.remotes,
    queryFn: async () => unwrap(await commands.remotesOverview()),
  });
  if (done || !overview.data?.offerBackupRemote) {
    return null;
  }
  return <BackupRemoteStep onDone={() => setDone(true)} />;
}
