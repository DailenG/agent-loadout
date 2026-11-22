// Settings > Updates (SPEC 16.4): the off switch and a manual check. The
// updater exists only in release builds for Windows (NSIS) and the AppImage;
// every other build says where updates come from instead. With the switch
// off, nothing here or at startup contacts the network.

import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { SettingsGroup, SettingsItem } from "@/app/components/SettingsGroup";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t } from "@/i18n/en";
import type { UpdateOffer, UpdaterStatus } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";

export const updaterQueryKey = ["updater_status"] as const;

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

function useUpdaterStatus() {
  const { commands } = useTransport();
  return useQuery({
    queryKey: updaterQueryKey,
    queryFn: async () => unwrap(await commands.updaterStatus()),
  });
}

/**
 * One quiet check per app start when the build has the updater and the
 * switch is on. A failure (offline, feed down) is not reported: the app
 * works without updates, and Settings has "Check now".
 */
export function useStartupUpdateCheck() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const status = useUpdaterStatus();
  const checked = useRef(false);
  const ready = status.data?.available === true && status.data.enabled;

  useEffect(() => {
    if (!ready || checked.current) {
      return;
    }
    checked.current = true;
    void commands
      .checkForUpdate()
      .then((result) => {
        if (result.status === "ok" && result.data) {
          notify({ tone: "info", message: t("settings.updates.startupOffer", { version: result.data.version }) });
        }
      })
      .catch(() => undefined);
  }, [commands, notify, ready]);
}

type CheckResult = { kind: "offer"; offer: UpdateOffer } | { kind: "current" };

export function UpdatesSection() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const queryClient = useQueryClient();
  const status = useUpdaterStatus();
  const [result, setResult] = useState<CheckResult | null>(null);

  const toggle = useMutation({
    mutationFn: async (enabled: boolean) => unwrap(await commands.setCheckForUpdates(enabled)),
    onSuccess: (next: UpdaterStatus) => {
      queryClient.setQueryData(updaterQueryKey, next);
      if (!next.enabled) {
        setResult(null);
      }
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const check = useMutation({
    mutationFn: async () => unwrap(await commands.checkForUpdate()),
    onSuccess: (offer) => setResult(offer ? { kind: "offer", offer } : { kind: "current" }),
    onError: (error) => {
      setResult(null);
      notify({ tone: "error", message: failureText(error) });
    },
  });

  const install = useMutation({
    mutationFn: async () => unwrap(await commands.installUpdate()),
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const data = status.data;
  if (!data) {
    return null;
  }

  if (!data.available) {
    return (
      <SettingsGroup title={t("settings.updates.title")}>
        <SettingsItem
          readonly
          label={t("settings.updates.unavailable.label")}
          description={t("settings.updates.unavailable.description")}
        />
      </SettingsGroup>
    );
  }

  const checkDescription =
    result?.kind === "offer"
      ? t("settings.updates.offer", { version: result.offer.version, current: result.offer.currentVersion })
      : result?.kind === "current"
        ? t("settings.updates.current")
        : data.enabled
          ? t("settings.updates.check.description")
          : t("settings.updates.check.off");

  return (
    <SettingsGroup title={t("settings.updates.title")}>
      <SettingsItem label={t("settings.updates.switch.label")} description={t("settings.updates.switch.description")}>
        <input
          type="checkbox"
          checked={data.enabled}
          disabled={toggle.isPending}
          aria-label={t("settings.updates.switch.label")}
          onChange={(event) => toggle.mutate(event.target.checked)}
        />
      </SettingsItem>
      <SettingsItem label={t("settings.updates.check.label")} description={checkDescription}>
        {result?.kind === "offer" ? (
          <button
            className="primary-button primary-button--compact"
            type="button"
            disabled={install.isPending}
            onClick={() => install.mutate()}
          >
            {install.isPending ? t("settings.updates.installing") : t("settings.updates.install")}
          </button>
        ) : (
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            disabled={!data.enabled || check.isPending}
            onClick={() => check.mutate()}
          >
            {check.isPending ? t("settings.updates.checking") : t("settings.updates.check.action")}
          </button>
        )}
      </SettingsItem>
    </SettingsGroup>
  );
}
