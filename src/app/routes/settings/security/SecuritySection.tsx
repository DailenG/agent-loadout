// Settings > Security (M49, #27): the optional external scanner, Snyk Agent
// Scan. Off by default. The disclosure says what it does, that skills go to
// Snyk's cloud and exactly what is sent; Install downloads the pinned build
// (progress, Cancel) and verifies it; the token is a secret reference only
// (an alias from the Secrets page or a provider reference), never a value;
// the switch stays off until it is installed and a token is set.

import { useIsMutating, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { AppSelect } from "@/app/components/AppSelect";
import { EXTERNAL_SCANNER_AVAILABLE, notScannedReason, scannerStatusKey, useScannerStatus } from "@/app/components/ExternalScan";
import { SettingsGroup, SettingsItem } from "@/app/components/SettingsGroup";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type { ExternalScannerStatus, ScannerInstallProgress, ScannerTokenKind } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { SettingsDialog } from "../SettingsDialog";

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

/** Bytes as megabytes with one decimal ("25.5 MB"). */
function megabytes(bytes: number): string {
  return t("settings.security.snyk.megabytes", { size: (bytes / 1_000_000).toFixed(1) });
}

const TOKEN_KIND_KEYS: Record<ScannerTokenKind, MessageKey> = {
  alias: "settings.security.snyk.token.kind.alias",
  env: "settings.security.snyk.token.kind.env",
  keyring: "settings.security.snyk.token.kind.keyring",
  one_password: "settings.security.snyk.token.kind.onePassword",
};

function statusLine(status: ExternalScannerStatus): { tone: string; label: string; description: string } {
  if (!status.supported) {
    return { tone: "tone-neutral", label: t("settings.security.snyk.status.unsupported"), description: t("settings.security.snyk.status.unsupportedDetail") };
  }
  if (!status.installed) {
    return { tone: "tone-neutral", label: t("settings.security.snyk.status.notInstalled"), description: t("settings.security.snyk.status.notInstalledDetail") };
  }
  if (!status.tokenRef) {
    return { tone: "tone-warning", label: t("settings.security.snyk.status.noToken"), description: t("settings.security.snyk.status.noTokenDetail") };
  }
  if (!status.enabled) {
    return { tone: "tone-neutral", label: t("settings.security.snyk.status.off"), description: t("settings.security.snyk.status.offDetail") };
  }
  return { tone: "tone-positive", label: t("settings.security.snyk.status.on"), description: t("settings.security.snyk.status.onDetail") };
}

/** The install's last download step, kept in the query cache so it survives leaving Settings (review F7). */
const installProgressKey = ["scanner_install_progress"] as const;
const installMutationKey = ["install_scanner"] as const;

/** What the scanner does and what goes to Snyk, before anything is turned on. */
function Disclosure() {
  const { commands } = useTransport();
  const listId = useId();
  return (
    <div className="settings-form-item settings-form-item--readonly scanner-disclosure">
      <div className="settings-form-item__copy">
        <span className="settings-form-item__title">{t("settings.security.snyk.title")}</span>
        <div className="scanner-disclosure__text">
          <p>{t("settings.security.snyk.what")}</p>
          <p>{t("settings.security.snyk.cloud")}</p>
          <p className="scanner-disclosure__links">
            {/* Fixed help topics: `open_help` opens only the URLs in its table. */}
            <button className="ghost-button" type="button" onClick={() => void commands.openHelp("snyk_terms")}>
              {t("settings.security.snyk.terms")}
            </button>
            <button className="ghost-button" type="button" onClick={() => void commands.openHelp("snyk_privacy")}>
              {t("settings.security.snyk.privacy")}
            </button>
          </p>
          <p id={listId}>{t("settings.security.snyk.sentIntro")}</p>
          <ul aria-labelledby={listId}>
            <li>{t("settings.security.snyk.sent.files")}</li>
            <li>{t("settings.security.snyk.sent.name")}</li>
            <li>{t("settings.security.snyk.sent.path")}</li>
            <li>{t("settings.security.snyk.sent.user")}</li>
            <li>{t("settings.security.snyk.sent.host")}</li>
            <li>{t("settings.security.snyk.sent.version")}</li>
          </ul>
          <p>{t("settings.security.snyk.notSent")}</p>
          <p>{t("settings.security.snyk.token")}</p>
          <p>{t("settings.security.snyk.never")}</p>
        </div>
      </div>
    </div>
  );
}

/** Nothing while the external scanner is disabled in this build (0.7.1: Snyk dropped). */
export function SecuritySection() {
  return EXTERNAL_SCANNER_AVAILABLE ? <SnykSection /> : null;
}

function SnykSection() {
  const { commands, events } = useTransport();
  const { notify } = useNotifications();
  const queryClient = useQueryClient();
  const status = useScannerStatus();
  // An install in flight outlives this page (the mutation and its progress live in the app's caches), so
  // coming back shows it with Cancel instead of offering a second Install.
  const installing = useIsMutating({ mutationKey: installMutationKey }) > 0;
  const progress = useQuery<ScannerInstallProgress | null>({
    queryKey: installProgressKey,
    queryFn: () => null,
    enabled: false,
    staleTime: Infinity,
  }).data ?? null;
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [editingToken, setEditingToken] = useState(false);
  const [tokenCheck, setTokenCheck] = useState<string | null>(null);
  const store = (next: ExternalScannerStatus) => queryClient.setQueryData(scannerStatusKey, next);
  const setProgress = (step: ScannerInstallProgress | null) => queryClient.setQueryData(installProgressKey, step);

  const install = useMutation({
    mutationKey: installMutationKey,
    // Listens before the download starts, so no step is missed; stops when it ends either way.
    mutationFn: async () => {
      setProgress({ received: 0, total: queryClient.getQueryData<ExternalScannerStatus>(scannerStatusKey)?.sizeBytes ?? 0 });
      const stop = await events.onScannerInstallProgress(setProgress);
      try {
        return unwrap(await commands.installScanner());
      } finally {
        stop();
      }
    },
    onSuccess: (next) => {
      store(next);
      notify({
        tone: "success",
        message: t(next.enabled ? "settings.security.snyk.installedOn" : "settings.security.snyk.installed", {
          version: next.version,
        }),
      });
    },
    onError: (error) =>
      notify(
        error instanceof IpcFailure && error.code === "canceled"
          ? { tone: "info", message: t("settings.security.snyk.installCanceled") }
          : { tone: "error", message: failureText(error) },
      ),
    onSettled: () => setProgress(null),
  });

  // Remove turns the switch off too (review F8): nothing can run without the scanner, and a reinstall
  // shouldn't start sending skills to Snyk again on its own.
  const remove = useMutation({
    mutationFn: async () => {
      const removed = unwrap(await commands.removeScanner());
      return removed.enabled ? unwrap(await commands.setScannerEnabled(false)) : removed;
    },
    onSuccess: (next) => {
      store(next);
      setConfirmRemove(false);
      notify({ tone: "success", message: t("settings.security.snyk.removed") });
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const setEnabled = useMutation({
    mutationFn: async (enabled: boolean) => unwrap(await commands.setScannerEnabled(enabled)),
    onSuccess: (next) => store(next),
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const clearToken = useMutation({
    mutationFn: async () => unwrap(await commands.setScannerToken(null)),
    onSuccess: (next) => {
      store(next);
      setTokenCheck(null);
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const checkToken = useMutation({
    mutationFn: async () => unwrap(await commands.checkScannerToken()),
    onSuccess: (check) =>
      setTokenCheck(
        check.ok
          ? t("settings.security.snyk.token.checkOk")
          : t("settings.security.snyk.token.checkFailed", {
              reason: check.reason ? notScannedReason(check.reason) : t("external.reason.failed"),
            }),
      ),
    onError: (error) => setTokenCheck(failureText(error)),
  });

  const data = status.data;
  if (!data) {
    return (
      <SettingsGroup title={t("settings.security.title")}>
        <Disclosure />
        <SettingsItem
          readonly
          label={t("settings.security.snyk.status.label")}
          description={status.isError ? failureText(status.error) : t("settings.security.loading")}
        />
      </SettingsGroup>
    );
  }

  const line = statusLine(data);
  const busy = installing || remove.isPending || setEnabled.isPending || clearToken.isPending;
  const canEnable = data.installed && data.tokenRef !== null;
  const size = data.sizeBytes === null ? null : megabytes(data.sizeBytes);
  const percent = progress && progress.total > 0 ? Math.round((progress.received / progress.total) * 100) : 0;
  const progressText =
    progress && progress.total > 0
      ? t("settings.security.snyk.progressText", { received: megabytes(progress.received), total: megabytes(progress.total) })
      : t("settings.security.snyk.starting");

  return (
    <SettingsGroup title={t("settings.security.title")}>
      <Disclosure />
      <p className="sr-only" role="status">
        {installing ? t("settings.security.snyk.liveInstalling") : ""}
      </p>
      <SettingsItem readonly label={t("settings.security.snyk.status.label")} description={line.description}>
        <span className={`status-badge ${line.tone}`}>{line.label}</span>
      </SettingsItem>
      <SettingsItem
        readonly
        label={t("settings.security.snyk.scanner.label")}
        description={
          !data.supported
            ? t("settings.security.snyk.status.unsupportedDetail")
            : installing
              ? progress && progress.total > 0
                ? t("settings.security.snyk.downloading", { received: megabytes(progress.received), total: megabytes(progress.total) })
                : t("settings.security.snyk.starting")
              : data.installed
                ? t("settings.security.snyk.verified", { version: data.version })
                : t("settings.security.snyk.notInstalled", { version: data.version, size: size ?? "" })
        }
      >
        {installing ? (
          <div className="scanner-install">
            <progress
              className="scanner-install__bar"
              max={100}
              value={percent}
              aria-label={t("settings.security.snyk.progressAria")}
              aria-valuetext={progressText}
            />
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              onClick={() => void commands.cancelExternalScan(null)}
            >
              {t("settings.security.snyk.cancelInstall")}
            </button>
          </div>
        ) : data.installed ? (
          <button
            className="secondary-button secondary-button--compact semantic-action semantic-action--disable"
            type="button"
            disabled={busy}
            onClick={() => setConfirmRemove(true)}
          >
            {t("settings.security.snyk.remove")}
          </button>
        ) : data.supported ? (
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            disabled={busy}
            onClick={() => install.mutate()}
          >
            {t("settings.security.snyk.install")}
          </button>
        ) : null}
      </SettingsItem>
      <SettingsItem
        readonly
        label={t("settings.security.snyk.token.label")}
        description={
          <span className="sources-meaning">
            {data.tokenRef ? (
              <span>
                {t(TOKEN_KIND_KEYS[data.tokenKind ?? "alias"])}: <code>{data.tokenRef}</code>
              </span>
            ) : (
              <span>{t("settings.security.snyk.token.none")}</span>
            )}
            <span>{t("settings.security.snyk.token.hint")}</span>
            {/* Always mounted, so the check's answer is announced. */}
            <span role="status">{tokenCheck ?? ""}</span>
          </span>
        }
      >
        <div className="settings-actions">
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            disabled={busy}
            onClick={() => setEditingToken(true)}
          >
            {data.tokenRef ? t("settings.security.snyk.token.change") : t("settings.security.snyk.token.set")}
          </button>
          {data.tokenRef ? (
            <>
              <button
                className="secondary-button secondary-button--compact"
                type="button"
                disabled={busy || checkToken.isPending}
                onClick={() => checkToken.mutate()}
              >
                {t("settings.security.snyk.token.check")}
              </button>
              <button
                className="ghost-button"
                type="button"
                disabled={busy}
                onClick={() => clearToken.mutate()}
              >
                {t("settings.security.snyk.token.clear")}
              </button>
            </>
          ) : null}
        </div>
      </SettingsItem>
      <SettingsItem
        readonly
        label={t("settings.security.snyk.enable.label")}
        description={canEnable ? t("settings.security.snyk.enable.description") : t("settings.security.snyk.enable.needs")}
      >
        <input
          type="checkbox"
          checked={data.enabled}
          disabled={busy || (!canEnable && !data.enabled)}
          aria-label={t("settings.security.snyk.enable.label")}
          onChange={(event) => setEnabled.mutate(event.target.checked)}
        />
      </SettingsItem>
      {confirmRemove ? (
        <ConfirmRemove busy={remove.isPending} onCancel={() => setConfirmRemove(false)} onConfirm={() => remove.mutate()} />
      ) : null}
      {editingToken ? (
        <TokenDialog
          status={data}
          onSaved={(next) => {
            store(next);
            setTokenCheck(null);
            setEditingToken(false);
          }}
          onClose={() => setEditingToken(false)}
        />
      ) : null}
    </SettingsGroup>
  );
}

function ConfirmRemove(props: { busy: boolean; onCancel: () => void; onConfirm: () => void }) {
  const describedBy = useId();
  return (
    <SettingsDialog
      title={t("settings.security.snyk.confirmRemove.title")}
      closeLabel={t("trust.close")}
      role="alertdialog"
      describedBy={describedBy}
      onClose={props.onCancel}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" data-autofocus onClick={props.onCancel}>
            {t("settings.security.cancel")}
          </button>
          <button
            className="secondary-button secondary-button--compact danger-button"
            type="button"
            disabled={props.busy}
            onClick={props.onConfirm}
          >
            {t("settings.security.snyk.remove")}
          </button>
        </>
      }
    >
      <p id={describedBy}>{t("settings.security.snyk.confirmRemove.description")}</p>
    </SettingsDialog>
  );
}

const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** The reference text for `kind` and what the user typed or picked; null while it isn't one. */
function referenceFor(kind: ScannerTokenKind, text: string): string | null {
  const value = text.trim();
  switch (kind) {
    case "alias":
      return ENV_NAME.test(value) ? value : null;
    case "env":
      return ENV_NAME.test(value) ? `env:${value}` : null;
    case "keyring":
      return /^keyring:\/\/[^/\s]+\/\S+$/.test(value) ? value : null;
    case "one_password":
      return /^op:\/\/[^/\s]+\/[^/\s]+\/\S+$/.test(value) ? value : null;
  }
}

/** The editable part of the current reference for its kind (`env:NAME` edits as `NAME`). */
function initialText(status: ExternalScannerStatus): string {
  const reference = status.tokenRef ?? "";
  return status.tokenKind === "env" ? reference.slice("env:".length) : reference;
}

const KIND_ORDER: ScannerTokenKind[] = ["alias", "env", "one_password", "keyring"];

const KIND_INPUT_KEYS: Record<Exclude<ScannerTokenKind, "alias">, MessageKey> = {
  env: "settings.security.snyk.token.input.env",
  keyring: "settings.security.snyk.token.input.keyring",
  one_password: "settings.security.snyk.token.input.onePassword",
};

const KIND_PLACEHOLDERS: Record<Exclude<ScannerTokenKind, "alias">, string> = {
  env: "SNYK_TOKEN",
  keyring: "keyring://snyk/agent-scan",
  one_password: "op://Private/Snyk/token",
};

const KIND_INVALID_KEYS: Record<ScannerTokenKind, MessageKey> = {
  alias: "settings.security.snyk.token.invalid.alias",
  env: "settings.security.snyk.token.invalid.env",
  keyring: "settings.security.snyk.token.invalid.keyring",
  one_password: "settings.security.snyk.token.invalid.onePassword",
};

/** A 1Password Environment value is only ever injected by `op run`: Loadout can't hand it to the scanner. */
const OP_ENVIRONMENT = "op-env://";

/**
 * Sets the token reference: a secret from the Secrets page, an environment
 * variable, a 1Password reference or an OS keyring entry. Only the
 * reference is stored and sent; the token itself is never typed here.
 */
function TokenDialog(props: {
  status: ExternalScannerStatus;
  onSaved: (next: ExternalScannerStatus) => void;
  onClose: () => void;
}) {
  const { commands } = useTransport();
  const formId = useId();
  const hintId = useId();
  const [kind, setKind] = useState<ScannerTokenKind>(props.status.tokenKind ?? "alias");
  const [text, setText] = useState(initialText(props.status));
  const [error, setError] = useState<string | null>(null);
  // The overview carries each alias's reference, so aliases the scanner can't use are left out (review F9).
  const secrets = useQuery({
    queryKey: ["secrets_overview", "scanner_token"],
    queryFn: async () => unwrap(await commands.secretsOverview()),
  });
  const reference = referenceFor(kind, text);

  const save = useMutation({
    mutationFn: async (value: string) => unwrap(await commands.setScannerToken(value)),
    onSuccess: props.onSaved,
    onError: (failure) => setError(failureText(failure)),
  });

  const usable = (secrets.data?.aliases ?? []).filter((entry) => !entry.reference.startsWith(OP_ENVIRONMENT));
  const hidden = (secrets.data?.aliases.length ?? 0) - usable.length;
  const aliasOptions = usable.map((entry) => ({ value: entry.alias, label: entry.alias }));
  // Why Save is off: shown once there is something to fix (or nothing picked yet).
  const invalid = reference === null && (kind === "alias" ? aliasOptions.length > 0 : text.trim() !== "");

  return (
    <SettingsDialog
      title={t("settings.security.snyk.token.dialogTitle")}
      closeLabel={t("settings.security.cancel")}
      onClose={props.onClose}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" onClick={props.onClose}>
            {t("settings.security.cancel")}
          </button>
          <button
            className="primary-button primary-button--compact"
            type="submit"
            form={formId}
            disabled={reference === null || save.isPending}
          >
            {t("settings.security.snyk.token.save")}
          </button>
        </>
      }
    >
      <form
        id={formId}
        className="scanner-token"
        onSubmit={(event) => {
          event.preventDefault();
          if (reference !== null && !save.isPending) {
            setError(null);
            save.mutate(reference);
          }
        }}
      >
        <p className="project-dialog__hint">{t("settings.security.snyk.token.dialogHint")}</p>
        <fieldset className="scanner-token__kinds">
          <legend>{t("settings.security.snyk.token.kindLegend")}</legend>
          {KIND_ORDER.map((option) => (
            <label key={option} className="scan-gate__ack">
              <input
                type="radio"
                name={`${formId}-kind`}
                checked={kind === option}
                onChange={() => {
                  setKind(option);
                  setText(option === props.status.tokenKind ? initialText(props.status) : "");
                  setError(null);
                }}
              />
              <span>{t(TOKEN_KIND_KEYS[option])}</span>
            </label>
          ))}
        </fieldset>
        {kind === "alias" ? (
          aliasOptions.length === 0 ? (
            <p className="project-dialog__hint">{t("settings.security.snyk.token.noAliases")}</p>
          ) : (
            <div className="project-dialog__field">
              <span>{t("settings.security.snyk.token.input.alias")}</span>
              <AppSelect
                value={text}
                options={aliasOptions}
                selectedLabel={text || t("settings.security.snyk.token.pickAlias")}
                ariaLabel={t("settings.security.snyk.token.input.alias")}
                onChange={setText}
              />
            </div>
          )
        ) : (
          <label className="project-dialog__field">
            <span>{t(KIND_INPUT_KEYS[kind])}</span>
            <input
              className="tool-manage-dialog__search"
              type="text"
              autoComplete="off"
              spellCheck={false}
              placeholder={KIND_PLACEHOLDERS[kind]}
              value={text}
              aria-invalid={invalid || undefined}
              aria-describedby={invalid ? hintId : undefined}
              onChange={(event) => setText(event.target.value)}
            />
          </label>
        )}
        {kind === "alias" && hidden > 0 ? <p className="project-dialog__hint">{t("settings.security.snyk.token.opEnvHidden")}</p> : null}
        {invalid ? (
          <p id={hintId} className="project-dialog__hint">
            {t(KIND_INVALID_KEYS[kind])}
          </p>
        ) : null}
        {error ? (
          <p className="dialog-error" role="alert">
            {error}
          </p>
        ) : null}
      </form>
    </SettingsDialog>
  );
}
