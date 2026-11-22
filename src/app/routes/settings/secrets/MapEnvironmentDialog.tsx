// "Map a 1Password Environment" (SPEC 11.10): the Environment's variable
// names (listed by the CLI from 2.42.0, typed by the user below it) become
// aliases `op-env://<id>/<NAME>` in one commit. Values are never read.

import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { useChangeNotice } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { failureText } from "@/app/routes/projects/labels";
import { SettingsDialog } from "@/app/routes/settings/SettingsDialog";
import { t } from "@/i18n/en";
import type { MapResult, OpEnvironmentNames } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type MapEnvironmentDialogProps = {
  open: boolean;
  /** `[onepassword] account`, prefilled. */
  account?: string | null;
  onClose: () => void;
};

export function MapEnvironmentDialog({ open, ...props }: MapEnvironmentDialogProps) {
  return open ? <MapForm {...props} /> : null;
}

function MapForm({ account: savedAccount, onClose }: Omit<MapEnvironmentDialogProps, "open">) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const reportChange = useChangeNotice();
  const [envId, setEnvId] = useState("");
  const [listed, setListed] = useState<OpEnvironmentNames | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [typed, setTyped] = useState("");
  const [account, setAccount] = useState(savedAccount ?? "");
  const [result, setResult] = useState<MapResult | null>(null);
  const onError = (error: unknown) => notify({ tone: "error", message: failureText(error) });

  const list = useMutation({
    mutationFn: async (id: string) => unwrap(await commands.listOpEnvironment(id)),
    onSuccess: (names) => {
      setListed(names);
      setSelected(new Set(names.names ?? []));
    },
    onError,
  });

  const names = listed?.names
    ? listed.names.filter((name) => selected.has(name))
    : [...new Set(typed.split(/[\s,]+/).filter((name) => name !== ""))];

  const map = useMutation({
    mutationFn: async () => unwrap(await commands.mapOpEnvironment(envId.trim(), names, account.trim() || null)),
    onSuccess: (mapped) => {
      reportChange(mapped.change, t("settings.secrets.map.done", { count: mapped.mapped.length }));
      setResult(mapped);
    },
    onError,
  });

  if (result) {
    return (
      <SettingsDialog
        title={t("settings.secrets.map.title")}
        subtitle={envId.trim()}
        closeLabel={t("settings.secrets.close")}
        onClose={onClose}
        actions={
          <button className="primary-button primary-button--compact" type="button" onClick={onClose}>
            {t("settings.secrets.done")}
          </button>
        }
      >
        <p role="status">
          {result.mapped.length > 0
            ? t("settings.secrets.map.mapped", { aliases: result.mapped.join(", ") })
            : t("settings.secrets.map.mappedNone")}
        </p>
        {result.kept.length > 0 ? <p>{t("settings.secrets.map.kept", { aliases: result.kept.join(", ") })}</p> : null}
        {result.replaced ? (
          <p className="project-dialog__hint">{t("settings.secrets.map.replaced", { id: result.replaced })}</p>
        ) : null}
      </SettingsDialog>
    );
  }

  return (
    <SettingsDialog
      title={t("settings.secrets.map.title")}
      closeLabel={t("settings.secrets.close")}
      onClose={onClose}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
            {t("settings.secrets.cancel")}
          </button>
          <button
            className="primary-button primary-button--compact"
            type="button"
            disabled={listed === null || names.length === 0 || map.isPending}
            onClick={() => map.mutate()}
          >
            {t("settings.secrets.map.submit")}
          </button>
        </>
      }
    >
      <p className="project-dialog__hint">{t("settings.secrets.map.intro")}</p>
      <form
        className="secrets-map__id"
        onSubmit={(event) => {
          event.preventDefault();
          if (envId.trim() !== "") {
            list.mutate(envId.trim());
          }
        }}
      >
        <label className="project-dialog__field">
          <span>{t("settings.secrets.map.id")}</span>
          <input
            className="tool-manage-dialog__search"
            type="text"
            spellCheck={false}
            value={envId}
            onChange={(event) => {
              setEnvId(event.target.value);
              setListed(null);
            }}
          />
        </label>
        <button
          className="secondary-button secondary-button--compact"
          type="submit"
          disabled={envId.trim() === "" || list.isPending}
        >
          {t("settings.secrets.map.load")}
        </button>
      </form>

      {listed && listed.names === null ? (
        <div className="project-dialog__field">
          <label className="project-dialog__field">
            <span>{t("settings.secrets.map.typeNames")}</span>
            <textarea
              className="tool-manage-dialog__search secrets-map__names"
              spellCheck={false}
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
            />
          </label>
          <span className="project-dialog__hint">{t("settings.secrets.map.typeNamesHint")}</span>
        </div>
      ) : null}

      {listed?.names ? (
        listed.names.length === 0 ? (
          <p>{t("settings.secrets.map.noNames")}</p>
        ) : (
          <fieldset className="project-dialog__body settings-fieldset">
            <legend>{t("settings.secrets.map.pickNames")}</legend>
            {listed.names.map((name) => (
              <label key={name} className="project-dialog__choice">
                <input
                  type="checkbox"
                  checked={selected.has(name)}
                  onChange={(event) => {
                    const next = new Set(selected);
                    if (event.target.checked) {
                      next.add(name);
                    } else {
                      next.delete(name);
                    }
                    setSelected(next);
                  }}
                />
                <span>{name}</span>
              </label>
            ))}
          </fieldset>
        )
      ) : null}

      {listed ? (
        <div className="project-dialog__field">
          <label className="project-dialog__field">
            <span>{t("settings.secrets.map.account")}</span>
            <input
              className="tool-manage-dialog__search"
              type="text"
              spellCheck={false}
              value={account}
              onChange={(event) => setAccount(event.target.value)}
            />
          </label>
          <span className="project-dialog__hint">{t("settings.secrets.account.hint")}</span>
        </div>
      ) : null}
    </SettingsDialog>
  );
}
