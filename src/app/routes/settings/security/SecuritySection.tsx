// Settings > Security (M51, #30): the optional external scanner, NVIDIA
// SkillSpector. Off by default. The disclosure says what it does, that it
// runs on this computer and exactly what can leave the machine (dependency
// names and versions to OSV.dev; the one-time PyPI download on Prepare).
// Loadout never installs it: the status says whether uvx or a skillspector on
// PATH can run it, Prepare downloads the pinned uvx form once (uv's lines as
// progress, Cancel), and setup instructions show when neither is found. The
// switch stays off until it can run here; it can always be turned off.

import { useIsMutating, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef } from "react";
import { scannerStatusKey, useScannerStatus } from "@/app/components/ExternalScan";
import { SettingsGroup, SettingsItem } from "@/app/components/SettingsGroup";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t } from "@/i18n/en";
import type { ExternalScannerStatus } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

/** The version and interpreter the disclosure names before the status arrives (the core's constants). */
const FALLBACK = { testedVersion: "2.13.7", python: "3.13" };

/** Prepare's last line from uv, and uv's last line of a failed Prepare, kept in the query cache so they survive leaving Settings. */
const prepareLineKey = ["scanner_prepare_line"] as const;
const prepareFailureKey = ["scanner_prepare_failure"] as const;
const prepareMutationKey = ["prepare_scanner"] as const;
/** Whether the user canceled the Prepare in flight (cached, so a Cancel clicked after leaving Settings and coming
 * back counts). A canceled answer without it comes from a cancel the backend kept from before (it arrived after an
 * earlier Prepare ended), so Prepare is asked once more. */
const prepareCancelKey = ["scanner_prepare_cancel"] as const;

function useCachedLine(key: readonly string[]): string | null {
  return (
    useQuery<string | null>({
      queryKey: key,
      queryFn: () => null,
      enabled: false,
      staleTime: Infinity,
    }).data ?? null
  );
}

function statusBadge(status: ExternalScannerStatus): { tone: string; label: string; description: string } {
  const values = { version: status.version ?? status.testedVersion, python: status.python };
  if (status.availability === "ready") {
    return {
      tone: "tone-positive",
      label: t("settings.security.status.ready"),
      description: t(status.runtime === "path" ? "settings.security.status.path" : "settings.security.status.uvx", values),
    };
  }
  if (status.availability === "needs_prepare") {
    return {
      tone: "tone-warning",
      label: t("settings.security.status.needsPrepare"),
      description: t("settings.security.status.needsPrepareDetail", values),
    };
  }
  return {
    tone: "tone-neutral",
    label: t("settings.security.status.notAvailable"),
    description: t("settings.security.status.notAvailableDetail"),
  };
}

/** What SkillSpector does and what can leave this computer, before anything is turned on. */
function Disclosure({ version, python }: { version: string; python: string }) {
  const { commands } = useTransport();
  return (
    <div className="settings-form-item settings-form-item--readonly scanner-disclosure">
      <div className="settings-form-item__copy">
        <span className="settings-form-item__title">{t("settings.security.skillspector")}</span>
        <div className="scanner-disclosure__text">
          <p>{t("settings.security.what")}</p>
          <p>{t("settings.security.local")}</p>
          <p>{t("settings.security.download", { version, python })}</p>
          <p>{t("settings.security.noAccount")}</p>
          <p className="scanner-disclosure__links">
            {/* Fixed help topics: `open_help` opens only the URLs in its table. */}
            <button className="ghost-button" type="button" onClick={() => void commands.openHelp("skillspector")}>
              {t("settings.security.project", { version })}
            </button>
          </p>
        </div>
      </div>
    </div>
  );
}

/** Neither uv nor SkillSpector found: how to get either, in plain words. */
function SetupInstructions({ version, python }: { version: string; python: string }) {
  const { commands } = useTransport();
  return (
    <div className="settings-form-item settings-form-item--readonly scanner-disclosure">
      <div className="settings-form-item__copy">
        <span className="settings-form-item__title">{t("settings.security.setup.title")}</span>
        <div className="scanner-disclosure__text">
          <p>{t("settings.security.setup.intro")}</p>
          <ul>
            <li>
              {t("settings.security.setup.uv")}{" "}
              <button className="ghost-button" type="button" onClick={() => void commands.openHelp("uv_install")}>
                {t("settings.security.setup.uvLink")}
              </button>
            </li>
            <li>
              {t("settings.security.setup.pipx", { version, python })}{" "}
              <code>{`pipx install skillspector==${version} --python ${python}`}</code>
            </li>
          </ul>
          <p>{t("settings.security.setup.python", { python })}</p>
        </div>
      </div>
    </div>
  );
}

export function SecuritySection() {
  const { commands, events } = useTransport();
  const { notify } = useNotifications();
  const queryClient = useQueryClient();
  const status = useScannerStatus();
  // A Prepare in flight outlives this page (the mutation and its last line live in the app's caches), so
  // coming back shows it with Cancel instead of offering a second Prepare.
  const preparing = useIsMutating({ mutationKey: prepareMutationKey }) > 0;
  const line = useCachedLine(prepareLineKey);
  const failure = useCachedLine(prepareFailureKey);
  const store = (next: ExternalScannerStatus) => queryClient.setQueryData(scannerStatusKey, next);
  // One button switches between Prepare and Cancel Prepare, so focus stays on it; the Status badge
  // takes focus once Prepare succeeds (the button goes away with the Prepare row).
  const prepareButton = useRef<HTMLButtonElement>(null);
  const statusBadgeRef = useRef<HTMLSpanElement>(null);
  const prepare = useMutation({
    mutationKey: prepareMutationKey,
    // Listens before uv starts, so no line is missed; stops when it ends either way.
    mutationFn: async () => {
      queryClient.setQueryData(prepareCancelKey, false);
      queryClient.setQueryData(prepareLineKey, null);
      queryClient.setQueryData(prepareFailureKey, null);
      const stop = await events.onScannerPrepareProgress((progress) => queryClient.setQueryData(prepareLineKey, progress.line));
      try {
        // A Cancel clicked before this call still reaches it: the backend keeps it for the next Prepare.
        let result = await commands.prepareScanner();
        if (
          result.status === "error" &&
          result.error.code === "canceled" &&
          queryClient.getQueryData<boolean>(prepareCancelKey) !== true
        ) {
          result = await commands.prepareScanner();
        }
        return unwrap(result);
      } finally {
        stop();
      }
    },
    onSuccess: (next) => {
      store(next);
      const active = document.activeElement;
      if (active === null || active === document.body || active === prepareButton.current) {
        statusBadgeRef.current?.focus();
      }
      notify({
        tone: "success",
        message: t(next.enabled ? "settings.security.prepare.doneOn" : "settings.security.prepare.done", {
          version: next.version ?? next.testedVersion,
        }),
      });
    },
    onError: (error) => {
      if (error instanceof IpcFailure && error.code === "canceled") {
        notify({ tone: "info", message: t("settings.security.prepare.canceled") });
        return;
      }
      if (error instanceof IpcFailure && error.code === "scanner_download_failed" && error.message) {
        queryClient.setQueryData(prepareFailureKey, error.message);
      }
      notify({ tone: "error", message: failureText(error) });
      // Detection may have changed (uv removed, or SkillSpector found meanwhile).
      void queryClient.invalidateQueries({ queryKey: scannerStatusKey });
    },
    onSettled: () => queryClient.setQueryData(prepareLineKey, null),
  });

  const setEnabled = useMutation({
    mutationFn: async (enabled: boolean) => unwrap(await commands.setScannerEnabled(enabled)),
    onSuccess: (next) => store(next),
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const data = status.data;
  const version = data?.testedVersion ?? FALLBACK.testedVersion;
  const python = data?.python ?? FALLBACK.python;
  const recheck = (
    <button
      className="ghost-button"
      type="button"
      disabled={status.isFetching || preparing}
      onClick={() => void status.refetch()}
    >
      {t("settings.security.recheck")}
    </button>
  );

  if (!data) {
    return (
      <SettingsGroup title={t("settings.security.title")}>
        <Disclosure version={version} python={python} />
        <SettingsItem
          readonly
          label={t("settings.security.status.label")}
          description={status.isError ? failureText(status.error) : t("settings.security.checking")}
        >
          {status.isError ? recheck : null}
        </SettingsItem>
      </SettingsGroup>
    );
  }

  const badge = statusBadge(data);
  const ready = data.availability === "ready";
  const busy = preparing || setEnabled.isPending;

  return (
    <SettingsGroup title={t("settings.security.title")}>
      <Disclosure version={version} python={python} />
      <p className="sr-only" role="status">
        {preparing ? t("settings.security.prepare.live") : ""}
      </p>
      <SettingsItem
        readonly
        label={t("settings.security.status.label")}
        description={
          <span className="sources-meaning">
            <span>{status.isFetching && !preparing ? t("settings.security.checking") : badge.description}</span>
            {ready && !data.versionTested ? (
              <span>{t("settings.security.status.untested", { version: data.version ?? "", tested: data.testedVersion })}</span>
            ) : null}
            {data.enabled && !ready ? <span>{t("settings.security.status.onNotReady")}</span> : null}
          </span>
        }
      >
        <div className="settings-actions">
          <span ref={statusBadgeRef} tabIndex={-1} className={`status-badge ${badge.tone}`}>
            {badge.label}
          </span>
          {recheck}
        </div>
      </SettingsItem>
      {data.availability === "not_available" ? <SetupInstructions version={version} python={python} /> : null}
      {data.availability === "needs_prepare" || preparing ? (
        <SettingsItem
          readonly
          label={t("settings.security.prepare.label")}
          description={
            <span className="sources-meaning">
              <span>{t("settings.security.prepare.description", { version, python })}</span>
              {preparing ? <span className="scanner-prepare__line">{line ?? t("settings.security.prepare.starting")}</span> : null}
              {!preparing && failure ? (
                <span className="scanner-prepare__line">{t("settings.security.prepare.failed", { line: failure })}</span>
              ) : null}
            </span>
          }
        >
          <div className="scanner-install">
            {preparing ? (
              <progress
                className="scanner-install__bar"
                aria-label={t("settings.security.prepare.progressAria")}
                aria-valuetext={line ?? t("settings.security.prepare.starting")}
              />
            ) : null}
            <button
              ref={prepareButton}
              className="secondary-button secondary-button--compact"
              type="button"
              disabled={!preparing && busy}
              onClick={() => {
                if (preparing) {
                  queryClient.setQueryData(prepareCancelKey, true);
                  void commands.cancelExternalScan(null);
                } else {
                  prepare.mutate();
                }
              }}
            >
              {preparing ? t("settings.security.prepare.cancel") : t("settings.security.prepare.action")}
            </button>
          </div>
        </SettingsItem>
      ) : null}
      <SettingsItem
        readonly
        label={t("settings.security.timeout.label")}
        description={t("settings.security.timeout.description", { seconds: data.timeoutSecs })}
      >
        <span className="status-badge tone-neutral">{t("settings.security.timeout.value", { seconds: data.timeoutSecs })}</span>
      </SettingsItem>
      <SettingsItem
        readonly
        label={t("settings.security.enable.label")}
        description={ready ? t("settings.security.enable.description") : t("settings.security.enable.needs")}
      >
        <input
          type="checkbox"
          checked={data.enabled}
          disabled={setEnabled.isPending || (!data.enabled && (!ready || preparing))}
          aria-label={t("settings.security.enable.label")}
          onChange={(event) => setEnabled.mutate(event.target.checked)}
        />
      </SettingsItem>
      {data.snykInstalled ? (
        <SettingsItem
          readonly
          label={t("settings.security.snyk.label")}
          description={
            <span className="sources-meaning">
              <span>{t("settings.security.snyk.description")}</span>
              <code>loadout scanner remove</code>
            </span>
          }
        />
      ) : null}
    </SettingsGroup>
  );
}
