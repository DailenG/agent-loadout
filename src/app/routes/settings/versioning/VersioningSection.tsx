// Settings > Versioning & Backup (SPEC 13.6 item 4, 9.6-9.9): the remotes
// in the order they were added (none is ranked) with their status, push and
// pull per remote or for all, token set/clear for `auth = token`, Add
// remote, Restore from remote..., and the automatic tag retention. Without
// a remote the store is local-only: "No backup remote".

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { storeQueryKeys, useInvalidateStore } from "@/app/apply/changeNotice";
import { SettingsGroup, SettingsItem } from "@/app/components/SettingsGroup";
import { useNotifications } from "@/app/components/notifications";
import { failureText } from "@/app/routes/projects/labels";
import { PasswordDialog, SettingsDialog } from "@/app/routes/settings/SettingsDialog";
import { AddRemoteDialog } from "@/app/routes/settings/versioning/AddRemoteDialog";
import { RestoreFromRemoteDialog } from "@/app/routes/settings/versioning/RestoreFromRemoteDialog";
import {
  AUTH_KEYS,
  KIND_KEYS,
  MODE_KEYS,
  formatTime,
  remoteStatus,
  syncToolNote,
} from "@/app/routes/settings/versioning/labels";
import { errorMessage, t } from "@/i18n/en";
import type { IpcError, PullEntry, PushEntry, RemoteEntry } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type VersioningSectionProps = {
  /** "Review and apply" after a pull or restore changed desired state. */
  onOpenPending?: () => void;
};

const failureOf = (error: IpcError | null) => (error ? errorMessage(error.code) : t("app.error.unknown"));

export function VersioningSection({ onOpenPending }: VersioningSectionProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const queryClient = useQueryClient();
  const invalidateStore = useInvalidateStore();
  const overview = useQuery({
    queryKey: storeQueryKeys.remotes,
    queryFn: async () => unwrap(await commands.remotesOverview()),
  });
  const [adding, setAdding] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [removing, setRemoving] = useState<RemoteEntry | null>(null);
  const removeQuestionId = useId();
  const [tokenFor, setTokenFor] = useState<string | null>(null);
  const [pulled, setPulled] = useState<{ servers: number; skills: number } | null>(null);

  const refreshRemotes = () => queryClient.invalidateQueries({ queryKey: storeQueryKeys.remotes });
  const onError = (error: unknown) => {
    notify({ tone: "error", message: failureText(error) });
    void refreshRemotes();
  };

  const push = useMutation({
    mutationFn: async (name: string | null) => unwrap(await commands.pushRemotes(name)),
    onSuccess: (entries: PushEntry[]) => {
      for (const entry of entries.filter((item) => !item.ok)) {
        notify({
          tone: "error",
          message: t("settings.versioning.push.failed", { remote: entry.remote, error: failureOf(entry.error) }),
        });
      }
      const pushed = entries.filter((item) => item.ok).map((item) => item.remote);
      if (pushed.length > 0) {
        notify({ tone: "success", message: t("settings.versioning.push.done", { remotes: pushed.join(", ") }) });
      }
      // A rejected git push pulled first, which changes the store.
      void (entries.some((item) => item.pulled) ? invalidateStore() : refreshRemotes());
    },
    onError,
  });

  const pull = useMutation({
    mutationFn: async (name: string | null) => unwrap(await commands.pullRemotes(name)),
    onSuccess: (entries: PullEntry[]) => {
      void invalidateStore();
      let servers = 0;
      let skills = 0;
      let conflicts = 0;
      for (const entry of entries) {
        servers += entry.changedServers;
        skills += entry.changedSkills;
        conflicts += entry.conflicts;
        if (!entry.ok) {
          notify({
            tone: "error",
            message: t("settings.versioning.pull.failed", { remote: entry.remote, error: failureOf(entry.error) }),
          });
        }
        for (const head of entry.heads.filter((item) => item.error)) {
          notify({
            tone: "error",
            message: t("settings.versioning.pull.headFailed", {
              remote: entry.remote,
              machine: head.label,
              error: failureOf(head.error),
            }),
          });
        }
        if (entry.conflicts > 0) {
          notify({
            tone: "error",
            message: t("settings.versioning.pull.conflicts", { remote: entry.remote, count: entry.conflicts }),
          });
        }
      }
      if (servers + skills > 0) {
        setPulled({ servers, skills });
        return;
      }
      const ok = entries.filter((item) => item.ok).map((item) => item.remote);
      if (ok.length > 0 && conflicts === 0) {
        notify({ tone: "success", message: t("settings.versioning.pull.done", { remotes: ok.join(", ") }) });
      }
    },
    onError,
  });

  const remove = useMutation({
    mutationFn: async (name: string) => unwrap(await commands.removeRemote(name)),
    onSuccess: (removed) => {
      setRemoving(null);
      notify({
        tone: "info",
        message: t(removed.tokenDeleted ? "settings.versioning.remove.doneToken" : "settings.versioning.remove.done", {
          name: removed.name,
        }),
      });
      void refreshRemotes();
    },
    onError,
  });

  const setToken = useMutation({
    mutationFn: async ({ name, token }: { name: string; token: string }) =>
      unwrap(await commands.setRemoteToken(name, token)),
    onSuccess: (_data, { name }) => {
      setTokenFor(null);
      notify({ tone: "success", message: t("settings.versioning.token.saved", { name }) });
      void refreshRemotes();
    },
    onError,
  });

  const clearToken = useMutation({
    mutationFn: async (name: string) => ({ name, cleared: unwrap(await commands.clearRemoteToken(name)) }),
    onSuccess: ({ name, cleared }) => {
      notify({
        tone: "info",
        message: t(cleared ? "settings.versioning.token.cleared" : "settings.versioning.token.none", { name }),
      });
      void refreshRemotes();
    },
    onError,
  });

  const data = overview.data;
  const remotes = data?.remotes ?? [];
  const syncing = push.isPending || pull.isPending;

  return (
    <SettingsGroup title={t("settings.versioning.title")}>
      {overview.isPending ? (
        <SettingsItem readonly label={t("settings.versioning.remotes")} description={t("settings.versioning.loading")} />
      ) : null}
      {overview.isError ? (
        <SettingsItem label={t("settings.versioning.remotes")} description={failureText(overview.error)}>
          <button className="secondary-button secondary-button--compact" type="button" onClick={() => void overview.refetch()}>
            {t("app.action.retry")}
          </button>
        </SettingsItem>
      ) : null}
      {data ? (
        <SettingsItem
          label={t("settings.versioning.remotes")}
          description={remotes.length === 0 ? t("settings.versioning.noRemote") : t("settings.versioning.remotesHint")}
        >
          <div className="settings-actions">
            {remotes.length > 0 ? (
              <>
                <button
                  className="secondary-button secondary-button--compact"
                  type="button"
                  disabled={syncing}
                  onClick={() => push.mutate(null)}
                >
                  {t("settings.versioning.pushAll")}
                </button>
                <button
                  className="secondary-button secondary-button--compact"
                  type="button"
                  disabled={syncing}
                  onClick={() => pull.mutate(null)}
                >
                  {t("settings.versioning.pullAll")}
                </button>
              </>
            ) : null}
            <button className="secondary-button secondary-button--compact" type="button" onClick={() => setAdding(true)}>
              {t("settings.versioning.addRemote")}
            </button>
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              onClick={() => setRestoring(true)}
            >
              {t("settings.versioning.restoreFrom")}
            </button>
          </div>
        </SettingsItem>
      ) : null}

      {pulled ? (
        <div className="settings-form-item settings-notice" role="status">
          <p>{t("settings.versioning.pull.changed", { servers: pulled.servers, skills: pulled.skills })}</p>
          <div className="settings-actions">
            {onOpenPending ? (
              <button
                className="primary-button primary-button--compact"
                type="button"
                onClick={() => {
                  setPulled(null);
                  onOpenPending();
                }}
              >
                {t("settings.versioning.reviewApply")}
              </button>
            ) : null}
            <button className="ghost-button" type="button" onClick={() => setPulled(null)}>
              {t("settings.versioning.dismiss")}
            </button>
          </div>
        </div>
      ) : null}

      {remotes.map((remote) => (
        <article key={remote.name} className="settings-form-item settings-remote" aria-label={remote.name}>
          <div className="settings-form-item__copy">
            <span className="settings-form-item__title settings-remote__title">
              <strong>{remote.name}</strong>
              <span className="status-badge tone-neutral">{t(KIND_KEYS[remote.kind])}</span>
              {remote.mode ? <span>{t(MODE_KEYS[remote.mode])}</span> : null}
              {remote.auth ? <span>{t(AUTH_KEYS[remote.auth])}</span> : null}
            </span>
            <p className="settings-path" title={remote.location}>
              {remote.location}
            </p>
            {remote.username ? <p>{t("settings.versioning.remote.username", { username: remote.username })}</p> : null}
            {remote.syncTool ? <p>{syncToolNote(remote.syncTool)}</p> : null}
            {remote.auth === "token" ? (
              <p>{t(remote.tokenStored ? "settings.versioning.token.stored" : "settings.versioning.token.missing")}</p>
            ) : null}
            <div className="settings-remote__status">
              {remoteStatus(remote.status).map((part) => (
                <span key={part.text} className={`status-badge ${part.tone}`}>
                  {part.text}
                </span>
              ))}
              {remote.status.pushedAt ? (
                <span className="project-dialog__hint">
                  {t("settings.versioning.remote.pushedAt", { time: formatTime(remote.status.pushedAt) })}
                </span>
              ) : null}
            </div>
          </div>
          <div className="settings-actions">
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              disabled={syncing}
              onClick={() => push.mutate(remote.name)}
            >
              {t("settings.versioning.push")}
            </button>
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              disabled={syncing}
              onClick={() => pull.mutate(remote.name)}
            >
              {t("settings.versioning.pull")}
            </button>
            {remote.auth === "token" ? (
              <>
                <button
                  className="secondary-button secondary-button--compact"
                  type="button"
                  onClick={() => setTokenFor(remote.name)}
                >
                  {t("settings.versioning.token.set")}
                </button>
                <button
                  className="secondary-button secondary-button--compact"
                  type="button"
                  disabled={!remote.tokenStored || clearToken.isPending}
                  onClick={() => clearToken.mutate(remote.name)}
                >
                  {t("settings.versioning.token.clear")}
                </button>
              </>
            ) : null}
            <button className="ghost-button" type="button" onClick={() => setRemoving(remote)}>
              {t("settings.versioning.remove")}
            </button>
          </div>
        </article>
      ))}

      {data ? (
        <SettingsItem
          readonly
          label={t("settings.versioning.retention.label")}
          description={t("settings.versioning.retention.days", { days: data.autoTagRetentionDays })}
        />
      ) : null}

      <AddRemoteDialog open={adding} firstRun={false} onClose={() => setAdding(false)} onAdded={() => setAdding(false)} />
      <RestoreFromRemoteDialog open={restoring} onClose={() => setRestoring(false)} onOpenPending={onOpenPending} />
      {tokenFor ? (
        <PasswordDialog
          title={t("settings.versioning.token.title", { name: tokenFor })}
          label={t("settings.versioning.token.label")}
          hint={t("settings.versioning.token.hint")}
          submitLabel={t("settings.versioning.token.save")}
          cancelLabel={t("settings.versioning.cancel")}
          onSubmit={(token) => setToken.mutateAsync({ name: tokenFor, token })}
          onClose={() => setTokenFor(null)}
        />
      ) : null}
      {removing ? (
        <SettingsDialog
          role="alertdialog"
          describedBy={removeQuestionId}
          title={t("settings.versioning.remove.title", { name: removing.name })}
          closeLabel={t("settings.versioning.cancel")}
          onClose={() => setRemoving(null)}
          actions={
            <>
              <button
                className="secondary-button secondary-button--compact"
                type="button"
                onClick={() => setRemoving(null)}
                data-autofocus
              >
                {t("settings.versioning.cancel")}
              </button>
              <button
                className="primary-button primary-button--compact"
                type="button"
                disabled={remove.isPending}
                onClick={() => remove.mutate(removing.name)}
              >
                {t("settings.versioning.remove.confirm")}
              </button>
            </>
          }
        >
          <p id={removeQuestionId}>{t("settings.versioning.remove.body", { name: removing.name })}</p>
        </SettingsDialog>
      ) : null}
    </SettingsGroup>
  );
}
