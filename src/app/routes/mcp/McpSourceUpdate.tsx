// The update check of a server that came from a Git repository or folder
// (M41 #5, M42), in the server's details on the MCP page: Check for update
// re-resolves the source; an available update opens a review with the
// definition fields that change, the changed files of a checkout, every new
// launch line, the setup steps, the source's trust state and the built-in
// scan. A missing or out-of-date checkout on this machine is restored through
// the same review and gates as an install (trust, then scan).

import { useState } from "react";
import { useChangeNotice } from "@/app/apply/changeNotice";
import { DangerousGate, isDangerous, ScanFindings } from "@/app/components/ScanFindings";
import { TrustSummary, useTrustPrompt } from "@/app/components/TrustDialog";
import { useNotifications } from "@/app/components/notifications";
import { SettingsDialog } from "@/app/routes/settings/SettingsDialog";
import { errorMessage, t } from "@/i18n/en";
import type { McpCheckoutReview, McpUpdateCheck } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { LaunchLines, RunsRepoCodeNote } from "../install/McpSourcePanel";
import { CHANGE_KIND_KEYS } from "../skills/labels";

/** The error's sentence; `scan_blocked` in the words of the action that was refused. */
function failureText(error: unknown, scanBlocked?: string): string {
  if (!(error instanceof IpcFailure)) {
    return t("app.error.unknown");
  }
  return error.code === "scan_blocked" && scanBlocked ? scanBlocked : errorMessage(error.code);
}

function statusText(check: McpUpdateCheck): string {
  switch (check.status) {
    case "available":
      return t("mcp.sourceUpdate.available", { commit: (check.latest ?? "").slice(0, 7) });
    case "up_to_date":
      return t("mcp.sourceUpdate.upToDate");
    case "not_tracked":
      return t("mcp.sourceUpdate.notTracked");
    default:
      return check.error ? errorMessage(check.error.code) : errorMessage("source_unreachable");
  }
}

export type McpSourceUpdateProps = {
  serverId: string;
  name: string;
};

export function McpSourceUpdate({ serverId, name }: McpSourceUpdateProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const changeNotice = useChangeNotice();
  const { resolveTrust, dialog: trustDialog } = useTrustPrompt();
  const [check, setCheck] = useState<McpUpdateCheck | null>(null);
  const [isChecking, setIsChecking] = useState(false);
  const [isRestoring, setIsRestoring] = useState(false);
  const [reviewing, setReviewing] = useState(false);
  const [restoreReview, setRestoreReview] = useState<McpCheckoutReview | null>(null);

  async function handleCheck() {
    setIsChecking(true);
    try {
      setCheck(unwrap(await commands.checkMcpSourceUpdate(serverId)));
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    } finally {
      setIsChecking(false);
    }
  }

  /** Fetches and scans what a restore would put in place; null from the backend: already up to date. */
  async function handleRestore() {
    setIsRestoring(true);
    try {
      const review = unwrap(await commands.reviewMcpCheckout(serverId));
      if (review) {
        setRestoreReview(review);
      } else {
        setCheck((current) => current && { ...current, checkoutOutOfDate: false });
        notify({ tone: "info", message: t("mcp.sourceUpdate.restoreUpToDate", { name }) });
      }
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    } finally {
      setIsRestoring(false);
    }
  }

  async function restore(review: McpCheckoutReview, allowDangerous: boolean) {
    const answer = await resolveTrust(review, () => notify({ tone: "error", message: errorMessage("source_blocked") }));
    if (!answer) {
      return;
    }
    try {
      const result = unwrap(
        await commands.restoreMcpCheckout(review.token, answer.trust, answer.movedTrust, allowDangerous),
      );
      setCheck((current) => current && { ...current, checkoutOutOfDate: false });
      changeNotice(result, t("mcp.sourceUpdate.restored", { name }));
    } catch (error) {
      notify({ tone: "error", message: failureText(error, t("scan.gate.restoreBlocked")) });
    }
    // The review is spent either way: a retry starts from Restore copy again.
    setRestoreReview(null);
  }

  function closeRestore() {
    if (restoreReview) {
      void commands.discardMcpSourceReview(restoreReview.token);
    }
    setRestoreReview(null);
  }

  return (
    <section className="mcp-source-update" aria-label={t("mcp.sourceUpdate.aria", { name })}>
      <div className="skill-card__section-header">
        <h4>{t("mcp.sourceUpdate.title")}</h4>
        <div className="tool-sync-panel__actions">
          {check?.checkoutOutOfDate ? (
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              disabled={isRestoring}
              onClick={() => void handleRestore()}
            >
              {isRestoring ? t("mcp.sourceUpdate.restoring") : t("mcp.sourceUpdate.restore")}
            </button>
          ) : null}
          {check?.status === "available" ? (
            <button className="primary-button primary-button--compact" type="button" onClick={() => setReviewing(true)}>
              {t("mcp.sourceUpdate.review")}
            </button>
          ) : null}
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            disabled={isChecking}
            onClick={() => void handleCheck()}
          >
            {isChecking ? t("mcp.sourceUpdate.checking") : t("mcp.sourceUpdate.check")}
          </button>
        </div>
      </div>
      {/* Always mounted, so a screen reader announces each check's result (a live region added with its text may not be). */}
      <p className="mcp-form-hint" role="status">
        {isChecking ? t("mcp.sourceUpdate.checking") : check ? statusText(check) : t("mcp.sourceUpdate.hint")}
        {check?.checkoutOutOfDate ? ` ${t("mcp.sourceUpdate.checkoutOutOfDate")}` : ""}
      </p>
      {restoreReview ? (
        <RestoreCheckoutDialog
          review={restoreReview}
          name={name}
          onRestore={(allowDangerous) => restore(restoreReview, allowDangerous)}
          onClose={closeRestore}
        />
      ) : null}
      {reviewing && check ? (
        <McpSourceUpdateDialog
          check={check}
          name={name}
          onClose={() => setReviewing(false)}
          onUpdate={async (allowDangerous) => {
            const answer = await resolveTrust(check, () =>
              notify({ tone: "error", message: errorMessage("source_blocked") }),
            );
            if (!answer) {
              return;
            }
            try {
              const result = unwrap(
                await commands.updateMcpSourceServer(serverId, check.latest, answer.trust, answer.movedTrust, allowDangerous),
              );
              changeNotice(result, t("mcp.sourceUpdate.updated", { name }));
              setReviewing(false);
              setCheck(null);
            } catch (error) {
              notify({ tone: "error", message: failureText(error, t("scan.gate.updateBlocked")) });
            }
          }}
        />
      ) : null}
      {trustDialog}
    </section>
  );
}

type McpSourceUpdateDialogProps = {
  check: McpUpdateCheck;
  name: string;
  /** `allowDangerous` only through "Update anyway". */
  onUpdate: (allowDangerous: boolean) => Promise<void>;
  onClose: () => void;
};

export function McpSourceUpdateDialog({ check, name, onUpdate, onClose }: McpSourceUpdateDialogProps) {
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const dangerous = isDangerous(check.scan);
  const blocked = check.trust?.state.kind === "blocked" || check.movedTrust?.state.kind === "blocked";

  async function update(allowDangerous: boolean) {
    setBusy(true);
    try {
      await onUpdate(allowDangerous);
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingsDialog
      wide
      title={t("mcp.sourceUpdate.dialogTitle", { name })}
      subtitle={t("mcp.sourceUpdate.commits", {
        from: (check.current ?? "").slice(0, 7),
        to: (check.latest ?? "").slice(0, 7),
      })}
      closeLabel={t("trust.close")}
      portal
      initialFocus="dialog"
      dismissible={!busy}
      onClose={onClose}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" disabled={busy} onClick={onClose}>
            {t("mcp.dialog.cancel")}
          </button>
          {dangerous ? (
            <button
              className="secondary-button secondary-button--compact danger-button"
              type="button"
              disabled={busy || blocked || !acknowledged}
              onClick={() => void update(true)}
            >
              {t("mcp.sourceUpdate.updateAnyway")}
            </button>
          ) : (
            <button
              className="primary-button primary-button--compact"
              type="button"
              disabled={busy || blocked}
              onClick={() => void update(false)}
            >
              {busy ? t("mcp.sourceUpdate.updating") : t("mcp.sourceUpdate.update")}
            </button>
          )}
        </>
      }
    >
      <div className="source-review">
        {check.trust ? <TrustSummary info={check.trust} /> : null}
        {check.movedTrust ? <TrustSummary info={check.movedTrust} label={t("trust.summary.moved")} /> : null}
        {blocked ? <p className="dialog-error" role="alert">{errorMessage("source_blocked")}</p> : null}
        <section>
          <h4>{t("mcp.sourceUpdate.changes")}</h4>
          {check.changes.length === 0 ? (
            <p className="mcp-form-hint">{t("mcp.sourceUpdate.noFieldChanges")}</p>
          ) : (
            <table className="mcp-update-diff">
              <thead>
                <tr>
                  <th scope="col">{t("mcp.sourceUpdate.field")}</th>
                  <th scope="col">{t("mcp.sourceUpdate.old")}</th>
                  <th scope="col">{t("mcp.sourceUpdate.new")}</th>
                </tr>
              </thead>
              <tbody>
                {check.changes.map((change) => (
                  <tr key={change.field}>
                    <th scope="row"><code>{change.field}</code></th>
                    <td className="mcp-update-diff__old">
                      {change.old === null ? <em>{t("mcp.sourceUpdate.unset")}</em> : <code>{change.old}</code>}
                    </td>
                    <td className="mcp-update-diff__new">
                      {change.new === null ? <em>{t("mcp.sourceUpdate.unset")}</em> : <code>{change.new}</code>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
        {check.launchLines.length > 0 || check.launch ? (
          <section>
            <h4>{check.launchLines.length > 1 ? t("mcp.sourceUpdate.launchLines") : t("mcp.sourceUpdate.launch")}</h4>
            <LaunchLines lines={check.launchLines.length > 0 ? check.launchLines : [{ os: null, line: check.launch ?? "" }]} />
          </section>
        ) : null}
        {check.runsRepoCode ? <RunsRepoCodeNote /> : null}
        {check.setup.length > 0 ? (
          <section>
            <h4>{t("mcp.sourceUpdate.setup")}</h4>
            {check.setup.map((step) => (
              <code key={step} className="mcp-draft__setup">{step}</code>
            ))}
          </section>
        ) : null}
        {check.setupAgain ? <p className="dialog-warning" role="note">{t("mcp.sourceUpdate.setupAgain")}</p> : null}
        {check.files.length > 0 ? (
          <section>
            <h4>{t("mcp.sourceUpdate.files", { count: check.files.length })}</h4>
            <ul className="source-review__changed-files">
              {check.files.map((file) => (
                <li key={file.path}>
                  <code>{file.path}</code>
                  <span className="status-badge tone-neutral">{t(CHANGE_KIND_KEYS[file.kind])}</span>
                  {file.script ? <span className="status-badge tone-warning">{t("skill.update.script")}</span> : null}
                </li>
              ))}
            </ul>
          </section>
        ) : null}
        {check.scan ? <ScanFindings scan={check.scan} /> : null}
        {dangerous ? (
          <DangerousGate
            acknowledged={acknowledged}
            onChange={setAcknowledged}
            description={t("scan.gate.updateDescription")}
          />
        ) : null}
      </div>
    </SettingsDialog>
  );
}

type RestoreCheckoutDialogProps = {
  review: McpCheckoutReview;
  name: string;
  /** `allowDangerous` only through "Restore anyway". */
  onRestore: (allowDangerous: boolean) => Promise<void>;
  onClose: () => void;
};

/** Restoring this machine's copy runs through the gates of an install: the trust question, then the scan. */
export function RestoreCheckoutDialog({ review, name, onRestore, onClose }: RestoreCheckoutDialogProps) {
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const dangerous = isDangerous(review.scan);
  const blocked = review.trust?.state.kind === "blocked" || review.movedTrust?.state.kind === "blocked";

  async function restore(allowDangerous: boolean) {
    setBusy(true);
    try {
      await onRestore(allowDangerous);
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingsDialog
      wide
      title={t("mcp.sourceUpdate.restoreTitle", { name })}
      subtitle={t("mcp.sourceUpdate.restoreCommit", { commit: review.commit.slice(0, 7) })}
      closeLabel={t("trust.close")}
      portal
      initialFocus="dialog"
      dismissible={!busy}
      onClose={onClose}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" disabled={busy} onClick={onClose}>
            {t("mcp.dialog.cancel")}
          </button>
          {dangerous ? (
            <button
              className="secondary-button secondary-button--compact danger-button"
              type="button"
              disabled={busy || blocked || !acknowledged}
              onClick={() => void restore(true)}
            >
              {t("mcp.sourceUpdate.restoreAnyway")}
            </button>
          ) : (
            <button
              className="primary-button primary-button--compact"
              type="button"
              disabled={busy || blocked}
              onClick={() => void restore(false)}
            >
              {busy ? t("mcp.sourceUpdate.restoring") : t("mcp.sourceUpdate.restoreConfirm")}
            </button>
          )}
        </>
      }
    >
      <div className="source-review">
        <p>{t("mcp.sourceUpdate.restoreExplain")}</p>
        {review.trust ? <TrustSummary info={review.trust} /> : null}
        {review.movedTrust ? <TrustSummary info={review.movedTrust} label={t("trust.summary.moved")} /> : null}
        {blocked ? <p className="dialog-error" role="alert">{errorMessage("source_blocked")}</p> : null}
        <ScanFindings scan={review.scan} />
        {dangerous ? (
          <DangerousGate
            acknowledged={acknowledged}
            onChange={setAcknowledged}
            description={t("scan.gate.restoreDescription")}
          />
        ) : null}
      </div>
    </SettingsDialog>
  );
}
