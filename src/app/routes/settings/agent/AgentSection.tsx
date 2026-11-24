// Settings > Agent integration (M46, #8): the read-only Loadout skill for
// coding agents. One checkbox per detected client (omp first) turns the
// store skill `loadout` on or off there through the normal skill sync; the
// version row says whether the store copy is current, waiting for an update
// or customized (never updated). The startup hook runs the update of an
// older store copy once per app start and says what it did.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { storeQueryKeys, useInvalidateStore } from "@/app/apply/changeNotice";
import { SettingsGroup, SettingsItem } from "@/app/components/SettingsGroup";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t } from "@/i18n/en";
import type { AgentSkillClient, AgentSkillStatus } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { SettingsDialog } from "../SettingsDialog";

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

function versionText(status: AgentSkillStatus): string {
  const copy = status.copy;
  const bundled = status.bundledVersion;
  if (!copy) {
    return t("settings.agent.version.none", { bundled });
  }
  const values = { version: copy.version, bundled };
  switch (copy.state) {
    case "current":
      return t("settings.agent.version.current", values);
    case "outdated":
      return t("settings.agent.version.outdated", values);
    case "customized":
      return t("settings.agent.version.customized", values);
    case "newer":
      return t("settings.agent.version.newer", values);
  }
}

/**
 * Once per app start: brings an older store copy of the Loadout skill up to
 * date (or reports a customized one) and says so. A failure is not reported:
 * the skill keeps working as it is, and Settings shows its state.
 */
export function useAgentSkillStartup() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const ran = useRef(false);

  useEffect(() => {
    if (ran.current) {
      return;
    }
    ran.current = true;
    void commands
      .refreshAgentSkill()
      .then((result) => {
        if (result.status !== "ok" || !result.data) {
          return;
        }
        const notice = result.data;
        if (notice.kind === "updated") {
          notify({ tone: "info", message: t("settings.agent.notice.updated", { from: notice.from, to: notice.to }) });
          if (notice.needsAttention) {
            notify({ tone: "info", message: t("settings.agent.attention") });
          }
          void invalidate();
        } else {
          notify({ tone: "info", message: t("settings.agent.notice.customized", { bundled: notice.bundled }) });
        }
      })
      .catch(() => undefined);
  }, [commands, invalidate, notify]);
}

export function AgentSection() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);
  const status = useQuery({
    queryKey: storeQueryKeys.agentSkill,
    queryFn: async () => unwrap(await commands.agentSkillStatus()),
  });

  const toggle = useMutation({
    mutationFn: async ({ client, enabled }: { client: AgentSkillClient; enabled: boolean }) =>
      unwrap(await commands.setAgentSkillClient(client.id, enabled)),
    onSuccess: (change, { client, enabled }) => {
      notify(
        change.links.needsAttention
          ? { tone: "info", message: t("settings.agent.attention") }
          : {
              tone: "success",
              message: t(enabled ? "settings.agent.enabled" : "settings.agent.disabled", { client: client.name }),
            },
      );
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
    onSettled: () => invalidate(),
  });

  const remove = useMutation({
    mutationFn: async () => unwrap(await commands.removeAgentSkill()),
    onSuccess: () => {
      setConfirmRemove(false);
      notify({ tone: "success", message: t("settings.agent.removed") });
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
    onSettled: () => invalidate(),
  });

  const reset = useMutation({
    mutationFn: async () => unwrap(await commands.resetAgentSkill()),
    onSuccess: (change) => {
      setConfirmReset(false);
      notify(
        change.links.needsAttention
          ? { tone: "info", message: t("settings.agent.attention") }
          : { tone: "success", message: t("settings.agent.restored") },
      );
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
    onSettled: () => invalidate(),
  });

  const reveal = useMutation({
    mutationFn: async () => unwrap(await commands.revealAgentSkill()),
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const data = status.data;
  const busy = toggle.isPending || remove.isPending || reset.isPending;
  const restorable = data?.copy?.state === "outdated" || data?.copy?.state === "customized";
  // Detected clients, and any the skill is still enabled for.
  const clients = data ? data.clients.filter((client) => client.detected || client.enabled) : [];

  return (
    <SettingsGroup title={t("settings.agent.title")}>
      <SettingsItem readonly label={t("settings.agent.about.label")} description={t("settings.agent.about.description")}>
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          disabled={reveal.isPending}
          onClick={() => reveal.mutate()}
        >
          {t("settings.agent.view")}
        </button>
      </SettingsItem>
      {status.isError ? (
        <SettingsItem readonly label={t("settings.agent.version.label")} description={failureText(status.error)} />
      ) : !data ? (
        <SettingsItem readonly label={t("settings.agent.version.label")} description={t("settings.agent.loading")} />
      ) : (
        <>
          <SettingsItem readonly label={t("settings.agent.version.label")} description={versionText(data)}>
            {restorable ? (
              <button
                className="secondary-button secondary-button--compact"
                type="button"
                disabled={busy}
                onClick={() => (data.copy?.state === "customized" ? setConfirmReset(true) : reset.mutate())}
              >
                {t("settings.agent.restore")}
              </button>
            ) : null}
            {data.copy ? (
              <button
                className="secondary-button secondary-button--compact semantic-action semantic-action--disable"
                type="button"
                disabled={busy}
                onClick={() => setConfirmRemove(true)}
              >
                {t("settings.agent.remove")}
              </button>
            ) : null}
          </SettingsItem>
          {data.taken ? (
            <SettingsItem
              readonly
              label={t("settings.agent.taken.label")}
              description={t("settings.agent.taken.description")}
            />
          ) : (
            <SettingsItem
              readonly
              label={t("settings.agent.clients.label")}
              description={
                clients.length === 0 ? (
                  t("settings.agent.clients.none")
                ) : (
                  <span className="sources-meaning">
                    <span>{t("settings.agent.clients.description")}</span>
                    <ul className="agent-clients" aria-label={t("settings.agent.clients.listAria")}>
                      {clients.map((client) => (
                        <li key={client.id}>
                          <label>
                            <input
                              type="checkbox"
                              checked={client.enabled}
                              disabled={busy}
                              aria-label={t("settings.agent.clients.toggleAria", { client: client.name })}
                              onChange={(event) => toggle.mutate({ client, enabled: event.target.checked })}
                            />{" "}
                            {client.name}
                            {client.detected ? null : (
                              <span className="status-badge tone-neutral">{t("settings.agent.clients.notDetected")}</span>
                            )}
                          </label>
                        </li>
                      ))}
                    </ul>
                  </span>
                )
              }
            />
          )}
        </>
      )}
      {confirmRemove ? (
        <Confirm
          title={t("settings.agent.confirmRemove.title")}
          description={t("settings.agent.confirmRemove.description")}
          action={t("settings.agent.remove")}
          busy={remove.isPending}
          onCancel={() => setConfirmRemove(false)}
          onConfirm={() => remove.mutate()}
        />
      ) : null}
      {confirmReset ? (
        <Confirm
          title={t("settings.agent.confirmRestore.title")}
          description={t("settings.agent.confirmRestore.description")}
          action={t("settings.agent.restore")}
          busy={reset.isPending}
          onCancel={() => setConfirmReset(false)}
          onConfirm={() => reset.mutate()}
        />
      ) : null}
    </SettingsGroup>
  );
}

function Confirm(props: {
  title: string;
  description: string;
  action: string;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const describedBy = useId();
  return (
    <SettingsDialog
      title={props.title}
      closeLabel={t("trust.close")}
      role="alertdialog"
      describedBy={describedBy}
      onClose={props.onCancel}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" onClick={props.onCancel}>
            {t("settings.agent.cancel")}
          </button>
          <button
            className="secondary-button secondary-button--compact danger-button"
            type="button"
            data-autofocus
            disabled={props.busy}
            onClick={props.onConfirm}
          >
            {props.action}
          </button>
        </>
      }
    >
      <p id={describedBy}>{props.description}</p>
    </SettingsDialog>
  );
}
