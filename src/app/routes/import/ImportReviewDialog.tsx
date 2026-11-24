// Review and result of an MCP import (#3): what will be imported, enabled for
// which clients and where each secret goes; then the one `apply_import` and
// what it did (imported, skipped with reasons, secret aliases, the apply).
// A definition found only in project files offers "Add to project manifest"
// (`save_project_server` with `from`), after a trust review when the project
// is not trusted. The backups the apply made of files that held an extracted
// secret keep its old value (M35): the result says so and offers "Remove
// those backups now" (`purge_import_backups`) after a confirmation.

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { storeQueryKeys, useChangeNotice, useInvalidateImports } from "@/app/apply/changeNotice";
import { Modal } from "@/app/components/Modal";
import { useNotifications } from "@/app/components/notifications";
import { ReadOnlyBadge, ReadOnlyNote, readOnlyReason } from "@/app/components/ReadOnlyBadge";
import { manifestReadOnly } from "@/app/routes/projects/labels";
import { TrustReviewDialog } from "@/app/routes/projects/TrustReviewDialog";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type {
  BackupPurgeFailure,
  ImportApplied,
  ManifestLayerKind,
  ProjectServerDraft,
  SecretDestinationKind,
} from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import {
  enabledClients,
  IMPORT_SCAN_KEY,
  manifestPlacement,
  projectsOf,
  type ImportNames,
  type ImportPlan,
  type ManifestPlacement,
  type SecretAnswer,
} from "./model";

const DESTINATION_KEYS = {
  keyring: "import.destination.keyring",
  onepassword: "import.destination.onepassword",
  env: "import.destination.env",
} as const satisfies Record<SecretDestinationKind, MessageKey>;

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

function answerText(answer: SecretAnswer): string {
  switch (answer.choice) {
    case "keyring":
      return t("import.destination.keyring");
    case "onepassword":
      return t("import.destination.onepasswordRef", { reference: answer.text.trim() });
    case "env":
      return t("import.destination.envName", { name: answer.text.trim() });
    case "skip":
      return t("import.secret.skip");
  }
}

/** A definition-only server and a project it was found in. */
type ManifestOffer = {
  id: string;
  name: string;
  projectId: string;
  projectName: string;
  key: string;
  placement: ManifestPlacement;
};

export type ImportReviewDialogProps = {
  plan: ImportPlan;
  names: ImportNames;
  onClose: () => void;
  /** The import committed: the section clears its choices. */
  onImported: () => void;
  onOpenPending: () => void;
};

export function ImportReviewDialog({ plan, names, onClose, onImported, onOpenPending }: ImportReviewDialogProps) {
  const { commands } = useTransport();
  const queryClient = useQueryClient();
  const invalidateImports = useInvalidateImports();
  const changeNotice = useChangeNotice();
  const { notify } = useNotifications();
  const titleId = useId();
  const descriptionId = useId();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ImportApplied | null>(null);
  // The plan as reviewed: the section's choices clear once it is imported.
  const [reviewed] = useState(plan);
  const [added, setAdded] = useState<string[]>([]);
  const [adding, setAdding] = useState("");
  const [trustFor, setTrustFor] = useState<ManifestOffer | null>(null);
  // The manifest each offer writes to, when the user chose one (`placement.offersChoice`).
  const [layers, setLayers] = useState<Record<string, ManifestLayerKind>>({});
  const resultHeadingRef = useRef<HTMLHeadingElement | null>(null);
  // "Remove those backups now": the confirmation, the call, and what it removed.
  const purgeTitleId = useId();
  const purgeQuestionId = useId();
  const [confirmingPurge, setConfirmingPurge] = useState(false);
  const [purging, setPurging] = useState(false);
  const [purged, setPurged] = useState<{
    removed: string[];
    notDeleted: BackupPurgeFailure[];
    failed: BackupPurgeFailure[];
  } | null>(null);
  const [purgeError, setPurgeError] = useState<string | null>(null);
  const oldBackups = result?.backupsWithOldValues ?? [];
  // Files whose backup still holds an old value: all listed, or those a removal failed for.
  const keptBackups = purged
    ? purged.failed.map((failure) => failure.file)
    : [...new Set(oldBackups.map((backup) => backup.file))];

  // The Import button is gone with the review: focus moves to the result's title, never the page.
  useEffect(() => {
    if (result) {
      resultHeadingRef.current?.focus();
    }
  }, [result]);

  async function runImport() {
    setBusy(true);
    setError(null);
    try {
      const applied = unwrap(await commands.applyImport(reviewed.request));
      setResult(applied);
      onImported();
      void invalidateImports();
      changeNotice(applied.change, t("import.result.toast", { count: applied.imported.length + applied.definitionOnly.length }));
    } catch (failure) {
      // Nothing was committed: the choices stay for another try.
      setError(failureText(failure));
    } finally {
      setBusy(false);
      await queryClient.invalidateQueries({ queryKey: IMPORT_SCAN_KEY });
    }
  }

  async function purgeBackups() {
    setPurging(true);
    setPurgeError(null);
    try {
      const removed = [...(purged?.removed ?? [])];
      const notDeleted = [...(purged?.notDeleted ?? [])];
      const failed: BackupPurgeFailure[] = [];
      for (const applyId of new Set(oldBackups.map((backup) => backup.applyId))) {
        const purge = unwrap(await commands.purgeImportBackups(applyId));
        removed.push(...purge.removed);
        notDeleted.push(...purge.notDeleted);
        failed.push(...purge.failed);
      }
      setPurged({ removed, notDeleted, failed });
    } catch (failure) {
      setPurgeError(failureText(failure));
    } finally {
      setPurging(false);
      setConfirmingPurge(false);
      await queryClient.invalidateQueries({ queryKey: storeQueryKeys.fileBackups });
    }
  }

  async function addToManifest(offer: ManifestOffer) {
    setAdding(offer.key);
    try {
      const list = unwrap(await commands.listProjectServers(offer.projectId));
      if (list.project.trust !== "trusted") {
        setTrustFor(offer);
        return;
      }
      const layer = layers[offer.key] ?? offer.placement.layer;
      // `from` the store server under the native name, enabled for the clients
      // whose files hold it, so the planner adopts that very entry.
      const draft: ProjectServerDraft = {
        name: offer.name,
        layer,
        kind: "override",
        from: offer.id,
        transport: null,
        command: null,
        args: [],
        url: null,
        env: [],
        headers: [],
        clients: { allDetected: false, clients: offer.placement.clients },
      };
      changeNotice(
        unwrap(await commands.saveProjectServer(offer.projectId, list.project.hash, draft, true)),
        t(layer === "shared" ? "import.manifest.addedShared" : "import.manifest.added", {
          name: offer.name,
          project: offer.projectName,
        }),
      );
      setAdded((current) => [...current, offer.key]);
    } catch (failure) {
      notify({ tone: "error", message: failureText(failure) });
    } finally {
      setAdding("");
    }
  }

  const offers: ManifestOffer[] = (result?.definitionOnly ?? []).flatMap((id) => {
    const pick = reviewed.picks.find((item) => item.id === id);
    if (!pick) {
      return [];
    }
    return projectsOf(pick.variant).map((source) => {
      const projectId = source.projectId ?? "";
      const project = names.project(projectId);
      return {
        id,
        name: pick.candidate.name,
        projectId,
        projectName: project?.name ?? projectId,
        key: `${id}\u0000${projectId}`,
        placement: manifestPlacement(pick.variant, projectId, project, names.privateTarget),
      };
    });
  });

  let body: JSX.Element;
  let actions: JSX.Element;
  if (!result) {
    body = (
      <>
        <p id={descriptionId} className="migrate-note">
          {t("import.review.intro", { count: reviewed.picks.length })}
        </p>
        <ul className="migrate-items" aria-label={t("import.review.listAria")}>
          {reviewed.picks.map((pick) => {
            const clients = enabledClients(pick.variant).map(names.client);
            return (
              <li key={`${pick.candidate.name}-${pick.variant.key}`} className="migrate-item">
                <div className="migrate-item__head">
                  <strong>{pick.id}</strong>
                  {pick.id !== pick.candidate.name ? (
                    <span className="status-badge tone-neutral">{t("import.review.from", { name: pick.candidate.name })}</span>
                  ) : null}
                </div>
                <p className="migrate-item__detail">
                  {clients.length > 0
                    ? t("import.review.enables", { clients: clients.join(", ") })
                    : t("import.review.definitionOnly")}
                </p>
                {pick.secrets.length > 0 ? (
                  <ul className="migrate-item__offers">
                    {pick.secrets.map((secret) => (
                      <li key={secret.field}>
                        {t("import.review.secret", { field: secret.field, destination: answerText(secret.answer) })}
                      </li>
                    ))}
                  </ul>
                ) : null}
              </li>
            );
          })}
        </ul>
        {reviewed.skippedForSecrets.length > 0 ? (
          <p className="migrate-note">{t("import.review.skippedSecrets", { names: reviewed.skippedForSecrets.join(", ") })}</p>
        ) : null}
        <p className="migrate-note">{t("import.review.oneCommit")}</p>
        {error ? (
          <p className="migrate-note" role="alert">
            {t("import.review.failed", { error })}
          </p>
        ) : null}
      </>
    );
    actions = (
      <>
        <button className="secondary-button" type="button" disabled={busy} onClick={onClose}>
          {t("import.review.cancel")}
        </button>
        <button className="primary-button" type="button" disabled={busy} onClick={() => void runImport()}>
          {busy ? t("import.review.importing") : t("import.review.confirm", { count: reviewed.picks.length })}
        </button>
      </>
    );
  } else {
    const run = result.change.apply;
    const needsPending = !run || run.status !== "done";
    body = (
      <>
        <p id={descriptionId} className="migrate-note">
          {t("import.result.summary", {
            imported: result.imported.length,
            definitions: result.definitionOnly.length,
            skipped: result.skipped.length,
          })}
        </p>
        <ul className="migrate-items" aria-label={t("import.result.aria")}>
          {result.imported.map((id) => (
            <li key={id} className="migrate-item migrate-item--row">
              <strong>{id}</strong>
              <span className="status-badge tone-positive">{t("import.result.imported")}</span>
            </li>
          ))}
          {result.definitionOnly.map((id) => (
            <li key={id} className="migrate-item">
              <div className="migrate-item__head">
                <strong>{id}</strong>
                <span className="status-badge tone-warning">{t("import.variant.definitionOnly")}</span>
              </div>
              <p className="migrate-item__detail">{t("import.result.definitionOnlyHint")}</p>
              {offers
                .filter((offer) => offer.id === id)
                .map((offer) => {
                  const done = added.includes(offer.key);
                  const layer = layers[offer.key] ?? offer.placement.layer;
                  // A manifest this account cannot write (M38): adding is disabled and says why.
                  const readOnly = manifestReadOnly(names.project(offer.projectId), layer);
                  return (
                    <div key={offer.key} className="import-result__offer">
                      <div className="migrate-item--row">
                        <span>{offer.projectName}</span>
                        <ReadOnlyBadge readOnly={readOnly} />
                        <button
                          className="secondary-button secondary-button--compact"
                          type="button"
                          disabled={done || adding !== "" || readOnly !== null}
                          aria-label={t("import.manifest.addAria", { id, project: offer.projectName })}
                          onClick={() => void addToManifest(offer)}
                        >
                          {done ? t("import.manifest.done") : t("import.manifest.add")}
                        </button>
                      </div>
                      {readOnly && !done ? (
                        <ReadOnlyNote>
                          {t("readOnly.import.manifest", { project: offer.projectName, reason: readOnlyReason(readOnly.kind) })}
                        </ReadOnlyNote>
                      ) : null}
                      {offer.placement.offersChoice && !done ? (
                        <fieldset className="import-candidate__choice import-result__layers">
                          <legend>{t("import.manifest.layerLegend", { project: offer.projectName })}</legend>
                          {(["shared", "local"] as const).map((option) => (
                            <label key={option}>
                              <input
                                type="radio"
                                name={`import-layer-${offer.key}`}
                                checked={layer === option}
                                disabled={option === "shared" && !offer.placement.sharedAvailable}
                                onChange={() => setLayers((current) => ({ ...current, [offer.key]: option }))}
                              />
                              {t(option === "shared" ? "import.manifest.shared" : "import.manifest.private")}
                            </label>
                          ))}
                          {offer.placement.sharedAvailable ? null : (
                            <span className="import-candidate__note">{t("import.manifest.noShared")}</span>
                          )}
                        </fieldset>
                      ) : null}
                    </div>
                  );
                })}
            </li>
          ))}
          {result.skipped.map((skipped) => (
            <li key={`skipped-${skipped.name}`} className="migrate-item">
              <div className="migrate-item__head">
                <strong>{skipped.name}</strong>
                <span className="status-badge tone-neutral">{t("import.result.skipped")}</span>
              </div>
              <p className="migrate-item__detail">{errorMessage(skipped.error.code)}</p>
            </li>
          ))}
        </ul>
        {result.secrets.length > 0 ? (
          <ul className="migrate-item__offers" aria-label={t("import.result.secretsAria")}>
            {result.secrets.map((secret) => (
              <li key={secret.alias}>
                {t("import.result.secret", {
                  alias: secret.alias,
                  server: secret.server,
                  field: secret.field,
                  destination: t(DESTINATION_KEYS[secret.destination]),
                })}
              </li>
            ))}
          </ul>
        ) : null}
        <p className="migrate-note">
          {!run
            ? t("import.result.reviewMode")
            : run.status === "done"
              ? t("import.result.applied")
              : t("import.result.attention")}
        </p>
        {oldBackups.length > 0 ? (
          <div className="import-result__backups">
            {purged && purged.removed.length > 0 ? (
              <p className="migrate-note">{t("import.result.backupsRemoved", { files: purged.removed.join(", ") })}</p>
            ) : null}
            {purged?.failed.map((failure) => (
              <p key={failure.file} className="migrate-note" role="alert">
                {t("import.result.backupsRemoveFailed", { file: failure.file, error: errorMessage(failure.error.code) })}
              </p>
            ))}
            {purged?.notDeleted.map((failure) => (
              <p key={`zeroed-${failure.file}`} className="migrate-note" role="alert">
                {t("import.result.backupsNotDeleted", { file: failure.file, error: errorMessage(failure.error.code) })}
              </p>
            ))}
            {purgeError ? (
              <p className="migrate-note" role="alert">
                {t("import.result.backupsRemoveError", { error: purgeError })}
              </p>
            ) : null}
            {keptBackups.length > 0 ? (
              <>
                {purged ? null : <p className="migrate-note">{t("import.result.backupsNote")}</p>}
                <p className="migrate-item__detail">{t("import.result.backupsFiles", { files: keptBackups.join(", ") })}</p>
                <button
                  className="secondary-button secondary-button--compact"
                  type="button"
                  disabled={purging}
                  onClick={() => setConfirmingPurge(true)}
                >
                  {t("import.result.removeBackups")}
                </button>
              </>
            ) : null}
          </div>
        ) : null}
        {result.oldValuesInFiles.length > 0 ? (
          <p className="migrate-note">
            {t("import.result.valuesStillInFiles", { files: result.oldValuesInFiles.join(", ") })}
          </p>
        ) : null}
      </>
    );
    actions = (
      <>
        <button className="secondary-button" type="button" onClick={onClose}>
          {t("import.result.close")}
        </button>
        {needsPending ? (
          <button
            className="primary-button"
            type="button"
            onClick={() => {
              onClose();
              onOpenPending();
            }}
          >
            {t("import.result.openPending")}
          </button>
        ) : null}
      </>
    );
  }

  return (
    <>
      <Modal
        className="tool-manage-dialog project-dialog settings-dialog settings-dialog--wide import-review"
        labelledBy={titleId}
        describedBy={descriptionId}
        dismissible={!busy}
        initialFocus="dialog"
        onClose={onClose}
      >
        <div className="tool-manage-dialog__header">
          <div className="tool-manage-dialog__title">
            <h3 id={titleId} ref={resultHeadingRef} tabIndex={-1}>
              {result ? t("import.result.title") : t("import.review.title")}
            </h3>
          </div>
          <button
            className="tool-manage-dialog__close"
            type="button"
            disabled={busy}
            onClick={onClose}
            aria-label={t("import.result.close")}
          >
            ×
          </button>
        </div>
        <div className="project-dialog__body settings-dialog__body">
          {/* Mounted with the dialog, so the result is announced when it arrives. */}
          <p className="sr-only" role="status" aria-live="polite">
            {!result
              ? ""
              : purged && purged.removed.length > 0
                ? t("import.result.backupsRemoved", { files: purged.removed.join(", ") })
                : t("import.result.summary", {
                    imported: result.imported.length,
                    definitions: result.definitionOnly.length,
                    skipped: result.skipped.length,
                  })}
          </p>
          {body}
        </div>
        <div className="tool-manage-dialog__actions">{actions}</div>
      </Modal>
      {trustFor ? (
        <TrustReviewDialog
          projectId={trustFor.projectId}
          onClose={() => setTrustFor(null)}
          onTrusted={() => {
            const offer = trustFor;
            setTrustFor(null);
            void addToManifest(offer);
          }}
        />
      ) : null}
      {confirmingPurge ? (
        <Modal
          className="tool-manage-dialog project-dialog"
          role="alertdialog"
          labelledBy={purgeTitleId}
          describedBy={purgeQuestionId}
          dismissible={!purging}
          onClose={() => setConfirmingPurge(false)}
        >
          <h3 id={purgeTitleId}>{t("import.result.removeBackupsTitle")}</h3>
          <p id={purgeQuestionId}>{t("import.result.removeBackupsQuestion", { files: keptBackups.join(", ") })}</p>
          <div className="tool-manage-dialog__actions">
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              disabled={purging}
              onClick={() => setConfirmingPurge(false)}
              data-autofocus
            >
              {t("import.result.removeBackupsCancel")}
            </button>
            <button
              className="primary-button primary-button--compact"
              type="button"
              disabled={purging}
              onClick={() => void purgeBackups()}
            >
              {purging ? t("import.result.removingBackups") : t("import.result.removeBackupsConfirm")}
            </button>
          </div>
        </Modal>
      ) : null}
    </>
  );
}
