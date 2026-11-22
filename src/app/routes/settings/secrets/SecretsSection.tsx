// Settings > Secrets (SPEC 13.6 item 5, 11.9-11.11): the providers of this
// machine ("1Password CLI not found" with install steps when `op` is
// missing), the service account token, the 1Password account, the alias
// list, "Check all" (an explicit action: resolving may prompt 1Password)
// with the checklist and its fixes, and "Map a 1Password Environment".

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { storeQueryKeys, useChangeNotice } from "@/app/apply/changeNotice";
import { SettingsGroup, SettingsItem } from "@/app/components/SettingsGroup";
import { useNotifications } from "@/app/components/notifications";
import { failureText } from "@/app/routes/projects/labels";
import { PasswordDialog } from "@/app/routes/settings/SettingsDialog";
import { MapEnvironmentDialog } from "@/app/routes/settings/secrets/MapEnvironmentDialog";
import { SecretsChecklist } from "@/app/routes/settings/secrets/SecretsChecklist";
import {
  FEATURE_KEYS,
  PROVIDER_KEYS,
  PROVIDER_STATE_KEYS,
  PROVIDER_STATE_TONES,
} from "@/app/routes/settings/secrets/labels";
import { t } from "@/i18n/en";
import type { ProviderEntry } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type SecretsSectionProps = {
  /** "Review and apply" once fixes from the checklist are saved. */
  onOpenPending?: () => void;
};

/** Detection only (`op --version`, keyring backend): never signs in. */
const providersKey = ["secret_providers"] as const;

function ProviderBody({ provider }: { provider: ProviderEntry }) {
  if (provider.kind === "env") {
    return <p>{t("settings.secrets.env.description")}</p>;
  }
  if (provider.kind === "keyring") {
    return (
      <p>
        {provider.state === "available"
          ? (provider.backend ?? t("settings.secrets.keyring.available"))
          : t("settings.secrets.keyring.unavailable")}
      </p>
    );
  }
  if (provider.state === "unavailable") {
    return (
      <>
        <p>{t("settings.secrets.op.notFound")}</p>
        <ul className="settings-provider__list" aria-label={t("settings.secrets.op.installSteps")}>
          <li>{t("settings.secrets.op.install.windows")}</li>
          <li>{t("settings.secrets.op.install.arch")}</li>
          <li>{t("settings.secrets.op.install.linux")}</li>
          <li>{t("settings.secrets.op.install.macos")}</li>
          <li>{t("settings.secrets.op.install.then")}</li>
        </ul>
      </>
    );
  }
  if (provider.state === "error") {
    return <p>{t("settings.secrets.op.versionUnknown")}</p>;
  }
  return (
    <>
      <p>
        {provider.version
          ? t("settings.secrets.op.version", { version: provider.version })
          : t("settings.secrets.op.versionUnknown")}
      </p>
      <ul className="settings-provider__list">
        {provider.features.map((feature) => (
          <li key={feature.feature}>
            {t("settings.secrets.op.feature", {
              feature: t(FEATURE_KEYS[feature.feature]),
              available: t(feature.available ? "settings.secrets.yes" : "settings.secrets.no"),
            })}
          </li>
        ))}
      </ul>
    </>
  );
}

function AccountForm({ current }: { current: string | null }) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const reportChange = useChangeNotice();
  const [account, setAccount] = useState(current ?? "");
  const save = useMutation({
    mutationFn: async () => unwrap(await commands.setOpAccount(account.trim() || null)),
    onSuccess: (result) => reportChange(result, t("settings.secrets.account.saved")),
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });
  return (
    <form
      className="settings-actions"
      onSubmit={(event) => {
        event.preventDefault();
        save.mutate();
      }}
    >
      <input
        className="tool-manage-dialog__search settings-inline-input"
        type="text"
        spellCheck={false}
        aria-label={t("settings.secrets.account.label")}
        placeholder={t("settings.secrets.account.placeholder")}
        value={account}
        onChange={(event) => setAccount(event.target.value)}
      />
      <button
        className="secondary-button secondary-button--compact"
        type="submit"
        disabled={save.isPending || account.trim() === (current ?? "")}
      >
        {t("settings.secrets.save")}
      </button>
    </form>
  );
}

export function SecretsSection({ onOpenPending }: SecretsSectionProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const queryClient = useQueryClient();
  const providers = useQuery({
    queryKey: providersKey,
    queryFn: async () => unwrap(await commands.secretProviders()),
  });
  const overview = useQuery({
    queryKey: storeQueryKeys.secrets,
    queryFn: async () => unwrap(await commands.secretsOverview()),
  });
  const [settingToken, setSettingToken] = useState(false);
  const [mapping, setMapping] = useState(false);
  const onError = (error: unknown) => notify({ tone: "error", message: failureText(error) });
  const refreshSecrets = () => queryClient.invalidateQueries({ queryKey: storeQueryKeys.secrets });

  // A mutation, never a query: resolving may prompt 1Password, so it runs
  // only when the user asks.
  const check = useMutation({
    mutationFn: async () => unwrap(await commands.checkSecrets([])),
    onSuccess: () => void refreshSecrets(),
    onError,
  });

  const setServiceAccount = useMutation({
    mutationFn: async (token: string) => unwrap(await commands.setOpServiceAccount(token)),
    onSuccess: () => {
      setSettingToken(false);
      notify({ tone: "success", message: t("settings.secrets.serviceAccount.saved") });
      void refreshSecrets();
    },
    onError,
  });

  const clearServiceAccount = useMutation({
    mutationFn: async () => unwrap(await commands.clearOpServiceAccount()),
    onSuccess: (cleared) => {
      notify({
        tone: "info",
        message: t(cleared ? "settings.secrets.serviceAccount.cleared" : "settings.secrets.serviceAccount.none"),
      });
      void refreshSecrets();
    },
    onError,
  });

  const data = overview.data;
  return (
    <SettingsGroup title={t("settings.secrets.title")}>
      {providers.isPending ? (
        <SettingsItem readonly label={t("settings.secrets.providers")} description={t("settings.secrets.loading")} />
      ) : null}
      {providers.isError ? (
        <SettingsItem label={t("settings.secrets.providers")} description={failureText(providers.error)}>
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            onClick={() => void providers.refetch()}
          >
            {t("app.action.retry")}
          </button>
        </SettingsItem>
      ) : null}
      {(providers.data ?? []).map((provider) => (
        <section key={provider.kind} className="settings-form-item settings-provider" aria-label={t(PROVIDER_KEYS[provider.kind])}>
          <div className="settings-form-item__copy">
            <span className="settings-form-item__title">{t(PROVIDER_KEYS[provider.kind])}</span>
            <ProviderBody provider={provider} />
          </div>
          <span className={`status-badge ${PROVIDER_STATE_TONES[provider.state]}`}>
            {t(PROVIDER_STATE_KEYS[provider.state])}
          </span>
        </section>
      ))}

      {overview.isError ? (
        <SettingsItem label={t("settings.secrets.aliases.label")} description={failureText(overview.error)}>
          <button className="secondary-button secondary-button--compact" type="button" onClick={() => void overview.refetch()}>
            {t("app.action.retry")}
          </button>
        </SettingsItem>
      ) : null}

      {data ? (
        <>
          <div className="settings-form-item">
            <div className="settings-form-item__copy">
              <span className="settings-form-item__title">{t("settings.secrets.serviceAccount.label")}</span>
              <p>{t("settings.secrets.serviceAccount.tradeoff")}</p>
              <p>
                {t(data.serviceAccountSet ? "settings.secrets.serviceAccount.set" : "settings.secrets.serviceAccount.unset")}
              </p>
            </div>
            <div className="settings-actions">
              <button
                className="secondary-button secondary-button--compact"
                type="button"
                onClick={() => setSettingToken(true)}
              >
                {t("settings.secrets.serviceAccount.setAction")}
              </button>
              <button
                className="secondary-button secondary-button--compact"
                type="button"
                disabled={!data.serviceAccountSet || clearServiceAccount.isPending}
                onClick={() => clearServiceAccount.mutate()}
              >
                {t("settings.secrets.serviceAccount.clearAction")}
              </button>
            </div>
          </div>

          <SettingsItem label={t("settings.secrets.account.label")} description={t("settings.secrets.account.hint")}>
            <AccountForm key={data.opAccount ?? ""} current={data.opAccount} />
          </SettingsItem>

          <SettingsItem
            label={t("settings.secrets.aliases.label")}
            description={
              data.environment
                ? t("settings.secrets.aliases.environment", { id: data.environment })
                : t("settings.secrets.aliases.hint")
            }
          >
            <div className="settings-actions">
              <button
                className="secondary-button secondary-button--compact"
                type="button"
                disabled={check.isPending}
                onClick={() => check.mutate()}
              >
                {t("settings.secrets.checkAll")}
              </button>
              <button className="secondary-button secondary-button--compact" type="button" onClick={() => setMapping(true)}>
                {t("settings.secrets.mapEnvironment")}
              </button>
            </div>
          </SettingsItem>

          <div className="settings-secrets-block">
            {data.aliases.length === 0 ? (
              <p className="project-dialog__hint">{t("settings.secrets.aliases.empty")}</p>
            ) : (
              <table className="settings-secrets-table">
                <caption className="sr-only">{t("settings.secrets.aliases.label")}</caption>
                <thead>
                  <tr>
                    <th scope="col">{t("settings.secrets.aliases.column.alias")}</th>
                    <th scope="col">{t("settings.secrets.aliases.column.reference")}</th>
                    <th scope="col">{t("settings.secrets.aliases.column.servers")}</th>
                  </tr>
                </thead>
                <tbody>
                  {data.aliases.map((alias) => (
                    <tr key={alias.alias}>
                      <th scope="row">
                        <span>{alias.alias}</span>
                        {alias.source === "machine" ? (
                          <span
                            className="status-badge tone-info"
                            title={
                              alias.storeReference
                                ? t("settings.secrets.aliases.overrides", { reference: alias.storeReference })
                                : undefined
                            }
                          >
                            {t("settings.secrets.thisMachine")}
                          </span>
                        ) : null}
                      </th>
                      <td>
                        <code>{alias.reference}</code>
                      </td>
                      <td>{alias.servers.length > 0 ? alias.servers.join(", ") : t("settings.secrets.aliases.unused")}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {data.undefined.length > 0 ? (
              <div className="settings-secrets-undefined">
                <h4 className="settings-subheading">{t("settings.secrets.aliases.undefined")}</h4>
                <ul aria-label={t("settings.secrets.aliases.undefined")}>
                  {data.undefined.map((alias) => (
                    <li key={alias}>{alias}</li>
                  ))}
                </ul>
              </div>
            ) : null}
          </div>
        </>
      ) : null}

      {check.isPending ? (
        <div className="settings-secrets-block">
          <p className="project-dialog__hint">{t("settings.secrets.checklist.checking")}</p>
        </div>
      ) : null}
      {check.data ? (
        <div className="settings-secrets-block" aria-label={t("settings.secrets.checklist.title")} role="region">
          <h4 className="settings-subheading">{t("settings.secrets.checklist.title")}</h4>
          <SecretsChecklist checks={check.data} onChanged={() => check.mutate()} />
          {onOpenPending ? (
            <div className="settings-actions">
              <button className="secondary-button secondary-button--compact" type="button" onClick={onOpenPending}>
                {t("settings.secrets.reviewApply")}
              </button>
            </div>
          ) : null}
        </div>
      ) : null}

      {settingToken ? (
        <PasswordDialog
          title={t("settings.secrets.serviceAccount.label")}
          label={t("settings.secrets.serviceAccount.input")}
          hint={t("settings.secrets.serviceAccount.tradeoff")}
          submitLabel={t("settings.secrets.save")}
          cancelLabel={t("settings.secrets.cancel")}
          onSubmit={(token) => setServiceAccount.mutateAsync(token)}
          onClose={() => setSettingToken(false)}
        />
      ) : null}
      <MapEnvironmentDialog open={mapping} account={data?.opAccount ?? null} onClose={() => setMapping(false)} />
    </SettingsGroup>
  );
}
