// The missing-secret checklist (SPEC 11.9): one row per alias with its
// status and the servers and clients that need it, and the quick fixes:
// a keyring value, a 1Password item, an environment variable, or "Skip for
// now". A fix for a provider this machine lacks defaults to this machine's
// overlay ("Use keyring on this machine", SPEC 11.11).

import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { useChangeNotice } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { failureText } from "@/app/routes/projects/labels";
import { STATUS_TONES, neededByText, statusLabel } from "@/app/routes/settings/secrets/labels";
import { t } from "@/i18n/en";
import type { AliasCheck, SecretScope } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type SecretsChecklistProps = {
  checks: AliasCheck[];
  /** A fix was saved: re-run the check. */
  onChanged: () => void;
};

export function SecretsChecklist({ checks, onChanged }: SecretsChecklistProps) {
  if (checks.length === 0) {
    return <p className="project-dialog__hint">{t("settings.secrets.checklist.empty")}</p>;
  }
  return (
    <div className="secrets-checklist">
      {checks.map((check) => (
        <CheckRow key={check.alias} check={check} onChanged={onChanged} />
      ))}
    </div>
  );
}

type FixKind = "keyring" | "op" | "env";

const FIX_KEYS = {
  keyring: "settings.secrets.fix.keyring",
  op: "settings.secrets.fix.op",
  env: "settings.secrets.fix.env",
} as const satisfies Record<FixKind, string>;

const FIX_INPUT_KEYS = {
  keyring: "settings.secrets.fix.keyringInput",
  op: "settings.secrets.fix.opInput",
  env: "settings.secrets.fix.envInput",
} as const satisfies Record<FixKind, string>;

const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

function CheckRow({ check, onChanged }: { check: AliasCheck; onChanged: () => void }) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const reportChange = useChangeNotice();
  const [skipped, setSkipped] = useState(false);
  const [fix, setFix] = useState<FixKind | null>(null);
  const [text, setText] = useState("");
  // A provider missing here is fixed on this machine only (SPEC 11.11).
  const [thisMachine, setThisMachine] = useState(
    check.status === "provider_unavailable" || check.status === "provider_too_old",
  );
  const needsFix = check.status !== "resolved" && check.status !== "unverifiable";
  const scope: SecretScope = thisMachine ? "this_machine" : "store";

  const save = useMutation({
    mutationFn: async ({ kind, value }: { kind: FixKind; value: string }) =>
      unwrap(
        kind === "keyring"
          ? await commands.setSecretKeyringValue(check.alias, value, scope)
          : await commands.setSecretReference(check.alias, kind === "env" ? `env:${value}` : value, scope, null),
      ),
    onSuccess: (result) => {
      reportChange(result, t("settings.secrets.fix.saved", { alias: check.alias }));
      setFix(null);
      onChanged();
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const value = fix === "keyring" ? text : text.trim();
  const valid =
    fix === "keyring" ? value !== "" : fix === "op" ? value.startsWith("op://") : fix === "env" && ENV_NAME.test(value);

  return (
    <article className="secrets-check" aria-label={check.alias}>
      <div className="secrets-check__head">
        <strong>{check.alias}</strong>
        <span className={`status-badge ${STATUS_TONES[check.status]}`}>{statusLabel(check)}</span>
        {check.blocking && needsFix ? (
          <span className="status-badge tone-danger">{t("settings.secrets.checklist.blocking")}</span>
        ) : null}
      </div>
      {check.reference ? <code className="secrets-check__ref">{check.reference}</code> : null}
      <p className="secrets-check__needed">{neededByText(check.neededBy)}</p>
      {needsFix && skipped ? <p className="project-dialog__hint">{t("settings.secrets.fix.skipped")}</p> : null}
      {needsFix && !skipped ? (
        <div className="secrets-check__fixes">
          <div className="settings-actions">
            {(["keyring", "op", "env"] as const).map((kind) => (
              <button
                key={kind}
                className={`secondary-button secondary-button--compact${fix === kind ? " is-selected" : ""}`}
                type="button"
                aria-pressed={fix === kind}
                onClick={() => {
                  setFix(kind);
                  setText("");
                }}
              >
                {t(FIX_KEYS[kind])}
              </button>
            ))}
            <button className="ghost-button" type="button" onClick={() => setSkipped(true)}>
              {t("settings.secrets.fix.skip")}
            </button>
          </div>
          {fix ? (
            <form
              className="secrets-check__form"
              onSubmit={(event) => {
                event.preventDefault();
                if (!valid || save.isPending) {
                  return;
                }
                setText("");
                save.mutate({ kind: fix, value });
              }}
            >
              <label className="project-dialog__field">
                <span>{t(FIX_INPUT_KEYS[fix])}</span>
                <input
                  className="tool-manage-dialog__search"
                  type={fix === "keyring" ? "password" : "text"}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder={fix === "op" ? t("settings.secrets.fix.opPlaceholder") : undefined}
                  value={text}
                  onChange={(event) => setText(event.target.value)}
                />
              </label>
              <label className="secrets-check__scope">
                <input type="checkbox" checked={thisMachine} onChange={(event) => setThisMachine(event.target.checked)} />
                <span>{t("settings.secrets.fix.thisMachine")}</span>
              </label>
              <div className="settings-actions">
                <button className="ghost-button" type="button" onClick={() => setFix(null)}>
                  {t("settings.secrets.cancel")}
                </button>
                <button
                  className="primary-button primary-button--compact"
                  type="submit"
                  disabled={!valid || save.isPending}
                >
                  {t("settings.secrets.fix.save")}
                </button>
              </div>
            </form>
          ) : null}
        </div>
      ) : null}
    </article>
  );
}
