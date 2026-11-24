// Settings > Sources (M39 #24, M42): every trust and block decision about a
// git source, what each one means, and the two changes the user can make:
// revoke (Loadout asks again next time) and block (nothing is fetched,
// installed or updated from it). A source that has no record yet can be
// blocked by its URL. Nothing installed is removed by either.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useId, useState } from "react";
import { storeQueryKeys, useInvalidateStore } from "@/app/apply/changeNotice";
import { SettingsGroup, SettingsItem } from "@/app/components/SettingsGroup";
import { changeText, OfficialBadge } from "@/app/components/TrustDialog";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type { SourceDecidedVia, SourceRecordInfo, SourceScope, SourceTrustInfo } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { SettingsDialog } from "../SettingsDialog";

const SCOPE_LABELS: Record<SourceScope, MessageKey> = {
  repo: "settings.sources.scope.repo",
  owner: "settings.sources.scope.owner",
  host: "settings.sources.scope.host",
};

const VIA_LABELS: Record<SourceDecidedVia, MessageKey> = {
  app: "settings.sources.via.app",
  "cli-prompt": "settings.sources.via.cliPrompt",
  "cli-flag": "settings.sources.via.cliFlag",
  "cli-command": "settings.sources.via.cliCommand",
};

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

/** "Jun 3, 2026" in the user's locale; the raw value when it does not parse. */
function formatDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleDateString(undefined, { dateStyle: "medium" });
}

function recordDescription(record: SourceRecordInfo): string {
  const decision = t(record.decision === "trust" ? "settings.sources.trusted" : "settings.sources.blocked", {
    scope: t(SCOPE_LABELS[record.scope]),
  });
  const decided = t("settings.sources.decided", { date: formatDate(record.decidedAt) });
  const via = record.via ? ` ${t(VIA_LABELS[record.via])}` : "";
  const machine = record.machine ? ` ${t("settings.sources.onMachine", { machine: record.machine })}` : "";
  return `${decision} ${decided}${via}${machine}.`;
}

type Confirm = { kind: "revoke" | "block"; record: SourceRecordInfo };

export function SourcesSection() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [blockingUrl, setBlockingUrl] = useState(false);
  const sources = useQuery({
    queryKey: storeQueryKeys.sources,
    queryFn: async () => unwrap(await commands.listSources()),
  });

  const change = useMutation({
    mutationFn: async ({ kind, record }: Confirm) =>
      unwrap(await (kind === "revoke" ? commands.revokeSource(record.id) : commands.blockSource(record.id))),
    onSuccess: (_commit, { kind, record }) => {
      setConfirm(null);
      notify({
        tone: "success",
        message: t(kind === "revoke" ? "settings.sources.revoked" : "settings.sources.blockedDone", {
          source: record.display,
        }),
      });
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
    onSettled: () => invalidate(),
  });

  const records = sources.data ?? [];

  return (
    <SettingsGroup title={t("settings.sources.title")}>
      <SettingsItem readonly label={t("settings.sources.about.label")} description={t("settings.sources.about.description")}>
        <button className="secondary-button secondary-button--compact" type="button" onClick={() => setBlockingUrl(true)}>
          {t("settings.sources.blockUrl")}
        </button>
      </SettingsItem>
      <SettingsItem
        readonly
        label={t("settings.sources.meaning.label")}
        description={
          <span className="sources-meaning">
            <span>{t("settings.sources.meaning.trust")}</span>
            <span>{t("settings.sources.meaning.block")}</span>
            <span>{t("settings.sources.meaning.revoke")}</span>
          </span>
        }
      />
      {sources.isError ? (
        <SettingsItem readonly label={t("settings.sources.title")} description={failureText(sources.error)} />
      ) : sources.isPending ? (
        <SettingsItem readonly label={t("settings.sources.title")} description={t("settings.sources.loading")} />
      ) : records.length === 0 ? (
        <SettingsItem readonly label={t("settings.sources.empty.label")} description={t("settings.sources.empty.description")} />
      ) : (
        <ul className="sources-list" aria-label={t("settings.sources.listAria")}>
          {records.map((record) => (
            <li key={record.id} className="sources-list__item">
              <div className="sources-list__copy">
                <div className="sources-list__title">
                  <code>{record.display}</code>
                  <span className={`status-badge ${record.decision === "trust" ? "tone-positive" : "tone-danger"}`}>
                    {t(record.decision === "trust" ? "settings.sources.badge.trusted" : "settings.sources.badge.blocked")}
                  </span>
                  {record.official ? <OfficialBadge /> : null}
                  {record.lapse ? (
                    <span className="status-badge tone-warning">{t("settings.sources.badge.lapsed")}</span>
                  ) : null}
                </div>
                <p>{recordDescription(record)}</p>
                {record.lapse ? (
                  <p className="sources-list__lapse">
                    {t("settings.sources.lapsed", { reason: changeText(record.lapse), date: formatDate(record.lapse.at) })}
                  </p>
                ) : null}
              </div>
              <div className="sources-list__actions">
                {record.decision === "trust" ? (
                  <button
                    className="secondary-button secondary-button--compact semantic-action semantic-action--disable"
                    type="button"
                    aria-label={t("settings.sources.blockAria", { source: record.display })}
                    onClick={() => setConfirm({ kind: "block", record })}
                  >
                    {t("settings.sources.block")}
                  </button>
                ) : null}
                <button
                  className="secondary-button secondary-button--compact"
                  type="button"
                  aria-label={t(
                    record.decision === "trust" ? "settings.sources.revokeAria" : "settings.sources.unblockAria",
                    { source: record.display },
                  )}
                  onClick={() => setConfirm({ kind: "revoke", record })}
                >
                  {t(record.decision === "trust" ? "settings.sources.revoke" : "settings.sources.unblock")}
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {confirm ? (
        <ConfirmDialog
          confirm={confirm}
          busy={change.isPending}
          onCancel={() => setConfirm(null)}
          onConfirm={() => change.mutate(confirm)}
        />
      ) : null}
      {blockingUrl ? <BlockUrlDialog onClose={() => setBlockingUrl(false)} /> : null}
    </SettingsGroup>
  );
}

function ConfirmDialog(props: { confirm: Confirm; busy: boolean; onCancel: () => void; onConfirm: () => void }) {
  const describedBy = useId();
  const { kind, record } = props.confirm;
  const unblock = kind === "revoke" && record.decision === "block";
  const title = kind === "block"
    ? t("settings.sources.confirmBlock.title", { source: record.display })
    : unblock
      ? t("settings.sources.confirmUnblock.title", { source: record.display })
      : t("settings.sources.confirmRevoke.title", { source: record.display });
  const description = kind === "block"
    ? t("settings.sources.confirmBlock.description")
    : unblock
      ? t("settings.sources.confirmUnblock.description")
      : t("settings.sources.confirmRevoke.description");
  const label = kind === "block"
    ? t("settings.sources.block")
    : unblock
      ? t("settings.sources.unblock")
      : t("settings.sources.revoke");

  return (
    <SettingsDialog
      title={title}
      closeLabel={t("trust.close")}
      role="alertdialog"
      describedBy={describedBy}
      onClose={props.onCancel}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" onClick={props.onCancel}>
            {t("settings.sources.cancel")}
          </button>
          <button
            className={kind === "block" ? "secondary-button secondary-button--compact danger-button" : "primary-button primary-button--compact"}
            type="button"
            data-autofocus
            disabled={props.busy}
            onClick={props.onConfirm}
          >
            {label}
          </button>
        </>
      }
    >
      <p id={describedBy}>{description}</p>
    </SettingsDialog>
  );
}

/** Blocks a source that has no record yet: its URL, then the scope. */
function BlockUrlDialog({ onClose }: { onClose: () => void }) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const formId = useId();
  const [url, setUrl] = useState("");
  const [info, setInfo] = useState<SourceTrustInfo | null>(null);
  const [scope, setScope] = useState<SourceScope>("repo");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit() {
    setError("");
    setBusy(true);
    try {
      if (!info) {
        setInfo(unwrap(await commands.checkSourceTrust(url.trim())));
        setScope("repo");
      } else {
        unwrap(await commands.blockSourceUrl(url.trim(), scope));
        const option = info.scopes.find((entry) => entry.scope === scope);
        notify({ tone: "success", message: t("settings.sources.blockedDone", { source: option?.display ?? info.display }) });
        void invalidate();
        onClose();
      }
    } catch (failure) {
      setError(failureText(failure));
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingsDialog
      title={t("settings.sources.blockUrl.title")}
      closeLabel={t("trust.close")}
      onClose={onClose}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
            {t("settings.sources.cancel")}
          </button>
          <button
            className={info ? "secondary-button secondary-button--compact danger-button" : "primary-button primary-button--compact"}
            type="submit"
            form={formId}
            disabled={busy || !url.trim()}
          >
            {info ? t("settings.sources.block") : t("settings.sources.blockUrl.next")}
          </button>
        </>
      }
    >
      <form
        id={formId}
        className="project-dialog__field"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <label className="project-dialog__field">
          <span>{t("settings.sources.blockUrl.label")}</span>
          <input
            className="tool-manage-dialog__search"
            type="text"
            spellCheck={false}
            autoComplete="off"
            placeholder="https://github.com/acme/tools"
            value={url}
            onChange={(event) => {
              setUrl(event.target.value);
              setInfo(null);
            }}
          />
        </label>
        <span className="project-dialog__hint">{t("settings.sources.blockUrl.hint")}</span>
        {info ? (
          <fieldset className="trust-dialog__choices">
            <legend>{t("settings.sources.blockUrl.scope")}</legend>
            {info.scopes.map((option) => (
              <label key={option.scope} className={`trust-dialog__choice${scope === option.scope ? " is-selected" : ""}`}>
                <input
                  type="radio"
                  name="block-scope"
                  checked={scope === option.scope}
                  onChange={() => setScope(option.scope)}
                />
                <span className="trust-dialog__choice-copy">
                  <span>{t(SCOPE_LABELS[option.scope])}</span>
                  <code>{option.display}</code>
                </span>
              </label>
            ))}
          </fieldset>
        ) : null}
        {error ? <div className="dialog-error" role="alert">{error}</div> : null}
      </form>
    </SettingsDialog>
  );
}
