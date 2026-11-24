// Install > MCP > From Git or folder (M41 #5, M42). A Git URL (optional
// folder and branch, tag or commit) or a picked folder is resolved into
// drafts: proposed servers, nothing saved and nothing run. Each draft shows
// where it came from, its exact command, what it needs (secrets, setup
// steps) and the built-in scan; a draft that runs code from the repository
// says so plainly. The user picks drafts, edits them in the server editor,
// picks clients and saves: the source's trust question first (M39), then the
// scan gate (M40), one store commit. The review part (`McpSourceReviewView`)
// is shared with Install > Browse (M45), where a catalog entry hands off
// into exactly this review.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { storeQueryKeys, useChangeNotice } from "@/app/apply/changeNotice";
import { Modal } from "@/app/components/Modal";
import { DangerousGate, isDangerous, ScanFindings } from "@/app/components/ScanFindings";
import { TrustSummary, useTrustPrompt } from "@/app/components/TrustDialog";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, platformLabels, t, type MessageKey } from "@/i18n/en";
import type {
  ClientChoice,
  ClientInfo,
  McpDraft,
  McpDraftKind,
  McpLaunchLine,
  Platform,
  McpSourceReview,
  PickedSkillSource,
  ServerDraft,
  TrustGrantArg,
} from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { ClientGrid, normalizeDraft, ServerForm } from "../mcp/ServerEditDialog";

type SourceKind = "git" | "folder";

const KIND_LABELS: Record<McpDraftKind, MessageKey> = {
  registry: "install.mcpSource.kind.registry",
  bundle: "install.mcpSource.kind.bundle",
  client_config: "install.mcpSource.kind.clientConfig",
  package: "install.mcpSource.kind.package",
};

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

/** Empty text is "not given". */
function optional(value: string): string | null {
  return value.trim() || null;
}

/** "server.json, packages[0], registry converter". */
function provenance(draft: McpDraft): string {
  return t(draft.entry ? "install.mcpSource.fromEntry" : "install.mcpSource.from", {
    file: draft.file,
    entry: draft.entry ?? "",
    converter: draft.converter,
  });
}

/** The plain note every draft that runs the repository's code carries. */
export function RunsRepoCodeNote() {
  return (
    <p className="dialog-warning runs-repo-code" role="note">
      {t("install.mcpSource.runsRepoCode")}
    </p>
  );
}

/**
 * Every line a server launches with: the base definition, then each OS
 * override as it runs there (M41). Paths stay under their placeholders.
 */
export function LaunchLines({ lines }: { lines: McpLaunchLine[] }) {
  if (lines.length === 1 && lines[0].os === null) {
    return <code className="mcp-draft__launch">{lines[0].line}</code>;
  }
  return (
    <dl className="mcp-launch-lines">
      {lines.map((entry) => (
        <div key={entry.os ?? ""}>
          <dt>
            {entry.os === null
              ? t("mcp.launch.base")
              : t("mcp.launch.os", { os: platformLabels[entry.os as Platform] ?? entry.os })}
          </dt>
          <dd>
            <code className="mcp-draft__launch">{entry.line}</code>
          </dd>
        </div>
      ))}
    </dl>
  );
}

type Edits = Record<string, ServerDraft>;

/** A save the scan refused: which edited servers, and the findings the backend named. */
type ScanBlockedSave = { servers: string; detail: string };

/** The command line or URL a draft launches, as the row and the scan see it. */
function launchOf(draft: ServerDraft): string {
  const saved = normalizeDraft(draft);
  return saved.transport === "stdio" ? [saved.command ?? "", ...saved.args].join(" ").trim() : (saved.url ?? "");
}

export function McpSourcePanel() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const servers = useQuery({
    queryKey: storeQueryKeys.servers,
    queryFn: async () => unwrap(await commands.listServers()),
  });
  const [kind, setKind] = useState<SourceKind>("git");
  const [url, setUrl] = useState("");
  const [path, setPath] = useState("");
  const [gitRef, setGitRef] = useState("");
  const [folder, setFolder] = useState<PickedSkillSource | null>(null);
  const [review, setReview] = useState<McpSourceReview | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const kindGroup = useId();
  const tokenRef = useDiscardOnLeave(review);

  function discard() {
    if (review) {
      void commands.discardMcpSourceReview(review.token);
    }
    setReview(null);
  }

  async function pickFolder() {
    try {
      const result = unwrap(await commands.pickSkillSource("folder"));
      if (result) {
        setFolder(result);
      }
    } catch (failure) {
      notify({ tone: "error", message: failureText(failure) });
    }
  }

  async function handleReview(event: FormEvent) {
    event.preventDefault();
    discard();
    setIsLoading(true);
    try {
      setReview(
        unwrap(
          kind === "git"
            ? await commands.reviewMcpGitSource(url.trim(), optional(path), optional(gitRef))
            : await commands.reviewMcpLocalSource(folder?.token ?? ""),
        ),
      );
    } catch (failure) {
      notify({ tone: "error", message: failureText(failure) });
    } finally {
      setIsLoading(false);
    }
  }

  const canReview = kind === "git" ? Boolean(url.trim()) : folder !== null;

  return (
    <section className="panel-card market-panel mcp-source-panel">
      <div className="panel-header">
        <h2>{t("install.mcpSource.title")}</h2>
        <p>{t("install.mcpSource.description")}</p>
      </div>
      <form className="repo-form" onSubmit={(event) => void handleReview(event)}>
        <div className="repo-form__section">
          <div className="mcp-source-kind" role="radiogroup" aria-labelledby={kindGroup}>
            <span id={kindGroup} className="repo-form__label">{t("install.mcpSource.kind")}</span>
            <label className={`mcp-source-kind__option${kind === "git" ? " is-selected" : ""}`}>
              <input type="radio" name="mcp-source-kind" checked={kind === "git"} onChange={() => setKind("git")} />
              <span>{t("install.mcpSource.kind.git")}</span>
            </label>
            <label className={`mcp-source-kind__option${kind === "folder" ? " is-selected" : ""}`}>
              <input type="radio" name="mcp-source-kind" checked={kind === "folder"} onChange={() => setKind("folder")} />
              <span>{t("install.mcpSource.kind.folder")}</span>
            </label>
          </div>
          {kind === "git" ? (
            <>
              <div className="repo-form__source-row">
                <label className="repo-form__field">
                  <span className="repo-form__label">{t("install.repo.url")}</span>
                  <input
                    type="text"
                    placeholder={t("install.mcpSource.urlPlaceholder")}
                    spellCheck={false}
                    value={url}
                    onChange={(event) => setUrl(event.target.value)}
                  />
                </label>
                <label className="repo-form__field repo-form__field--branch">
                  <span className="repo-form__label">{t("install.mcpSource.ref")}</span>
                  <input
                    type="text"
                    placeholder={t("install.repo.refPlaceholder")}
                    spellCheck={false}
                    value={gitRef}
                    onChange={(event) => setGitRef(event.target.value)}
                  />
                </label>
              </div>
              <label className="repo-form__field">
                <span className="repo-form__label">{t("install.mcpSource.path")}</span>
                <input
                  type="text"
                  placeholder={t("install.mcpSource.pathPlaceholder")}
                  spellCheck={false}
                  value={path}
                  onChange={(event) => setPath(event.target.value)}
                />
              </label>
            </>
          ) : (
            <div className="mcp-source-folder">
              <button className="secondary-button" type="button" onClick={() => void pickFolder()}>
                {t("install.mcpSource.chooseFolder")}
              </button>
              <span className="mcp-source-folder__path">
                {folder ? <code>{folder.path}</code> : t("install.mcpSource.noFolder")}
              </span>
            </div>
          )}
          <p className="repo-form__hint-title">{t("install.mcpSource.hint")}</p>
        </div>
        <div className="repo-form__actions">
          <button className="primary-button" type="submit" disabled={!canReview || isLoading || isSaving}>
            {isLoading ? t("install.mcpSource.reviewing") : t("install.mcpSource.review")}
          </button>
        </div>
      </form>
      {review ? (
        <McpSourceReviewView
          key={review.token}
          review={review}
          clients={servers.data?.clients ?? []}
          onDiscard={discard}
          onSavingChange={setIsSaving}
          onSaved={() => {
            setIsSaving(false);
            // Saved: the backend consumed the review.
            tokenRef.current = null;
            setReview(null);
            setUrl("");
            setPath("");
            setGitRef("");
            setFolder(null);
          }}
        />
      ) : null}
    </section>
  );
}

/**
 * A review left open when the page goes away is dropped with its temporary
 * clone. The returned ref holds the open review's token; set it to null once
 * the backend consumed the review (saved).
 */
export function useDiscardOnLeave(review: McpSourceReview | null) {
  const { commands } = useTransport();
  const tokenRef = useRef<string | null>(null);
  tokenRef.current = review?.token ?? null;
  useEffect(
    () => () => {
      if (tokenRef.current) {
        void commands.discardMcpSourceReview(tokenRef.current);
      }
    },
    [commands],
  );
  return tokenRef;
}

export type McpSourceReviewViewProps = {
  review: McpSourceReview;
  clients: readonly ClientInfo[];
  /** Where the review came from, above the source facts (a catalog entry, M45). */
  origin?: ReactNode;
  /** Discard: the parent drops the review (and its token). */
  onDiscard: () => void;
  /** Saved: the backend consumed the review. */
  onSaved: () => void;
  /** Saving started or ended: the parent keeps its own actions off meanwhile. */
  onSavingChange?: (saving: boolean) => void;
};

/**
 * The drafts of one review: pick, edit, choose clients and save through the
 * trust question (M39) and the scan gate (M40). Give it `key={review.token}`
 * so a new review starts with fresh picks.
 */
export function McpSourceReviewView({
  review,
  clients: clientList,
  origin,
  onDiscard,
  onSaved,
  onSavingChange,
}: McpSourceReviewViewProps) {
  const { commands } = useTransport();
  const changeNotice = useChangeNotice();
  const { resolveTrust, dialog: trustDialog } = useTrustPrompt();
  const [edits, setEdits] = useState<Edits>(() => Object.fromEntries(review.drafts.map((draft) => [draft.key, draft.draft])));
  // One draft is the obvious pick; several are the user's choice (as `--pick` in the CLI).
  const [picked, setPicked] = useState<string[]>(() => (review.drafts.length === 1 ? [review.drafts[0].key] : []));
  const [clients, setClients] = useState<ClientChoice>({ allDetected: false, clients: [] });
  const [editing, setEditing] = useState<McpDraft | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [scanBlocked, setScanBlocked] = useState<ScanBlockedSave | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  useEffect(() => onSavingChange?.(isSaving), [isSaving, onSavingChange]);
  const [error, setError] = useState("");

  const pickedDrafts = review.drafts.filter((draft) => picked.includes(draft.key));
  const isEdited = (draft: McpDraft) => launchOf(edits[draft.key]) !== launchOf(draft.draft);
  const dangerous = scanBlocked !== null || isDangerous(review.scan, ...pickedDrafts.map((draft) => draft.scan));
  // A registry entry's drafts each run from their own source (a package, a remote host): a picked one that is
  // blocked refuses the save too.
  const blocked =
    review.trust?.state.kind === "blocked" ||
    review.movedTrust?.state.kind === "blocked" ||
    pickedDrafts.some((draft) => draft.trust?.state.kind === "blocked");

  /**
   * Any change to what would be saved: the "Save anyway" a refused save
   * offered, and the acknowledgement, were for the content before it.
   */
  function clearGate() {
    setScanBlocked(null);
    setAcknowledged(false);
    setError("");
  }

  function chooseClients(choice: ClientChoice) {
    clearGate();
    setClients(choice);
    setEdits((current) =>
      Object.fromEntries(Object.entries(current).map(([key, draft]) => [key, { ...draft, clients: choice }])),
    );
  }

  function togglePick(key: string, on: boolean) {
    clearGate();
    setPicked((current) => (on ? [...current, key] : current.filter((entry) => entry !== key)));
  }

  async function handleSave() {
    if (pickedDrafts.length === 0) {
      return;
    }
    setError("");
    const onBlocked = () => setError(errorMessage("source_blocked"));
    const answer = await resolveTrust(review, onBlocked);
    if (!answer) {
      return;
    }
    // Then what each picked draft runs from, once per source.
    const ownGrants = new Map<string, TrustGrantArg | null>();
    for (const draft of pickedDrafts) {
      if (!draft.trust || ownGrants.has(draft.trust.display)) {
        continue;
      }
      const own = await resolveTrust({ trust: draft.trust }, onBlocked);
      if (!own) {
        return;
      }
      ownGrants.set(draft.trust.display, own.trust);
    }
    setIsSaving(true);
    try {
      const picks = pickedDrafts.map((draft) => ({
        key: draft.key,
        draft: normalizeDraft(edits[draft.key]),
        trust: draft.trust ? (ownGrants.get(draft.trust.display) ?? null) : null,
      }));
      const saved = unwrap(
        await commands.saveMcpSourceDrafts(review.token, picks, answer.trust, answer.movedTrust, dangerous),
      );
      changeNotice(saved.change, t("install.mcpSource.saved", { ids: saved.ids.join(", ") }));
      onSaved();
    } catch (failure) {
      if (failure instanceof IpcFailure && failure.code === "scan_blocked") {
        // The edited command lines scan dangerous. No re-review exists for an edited draft, so the
        // refusal names the edited servers and shows the findings the backend reported; then the gate.
        const edited = pickedDrafts.filter(isEdited);
        const named = (edited.length > 0 ? edited : pickedDrafts).map((draft) => edits[draft.key].id).join(", ");
        setScanBlocked({ servers: named, detail: failure.message });
        setAcknowledged(false);
        setError("");
      } else {
        setError(failureText(failure));
      }
    } finally {
      setIsSaving(false);
    }
  }

  return (
    <>
      <div className="mcp-source-review" aria-label={t("install.mcpSource.reviewAria")} role="region">
        {origin}
        <SourceFacts review={review} />
          {review.trust ? <TrustSummary info={review.trust} /> : null}
          {review.movedTrust ? <TrustSummary info={review.movedTrust} label={t("trust.summary.moved")} /> : null}
          <ScanFindings scan={review.scan} title={t("install.mcpSource.filesScan")} />
          {review.skippedFolders.length > 0 ? (
            <p className="mcp-form-hint" role="note">
              {t("install.mcpSource.skippedFolders", { folders: review.skippedFolders.join(", ") })}
            </p>
          ) : null}
          <h3 className="mcp-source-review__heading">
            {review.drafts.length === 1
              ? t("install.mcpSource.drafts.one")
              : t("install.mcpSource.drafts.many", { count: review.drafts.length })}
          </h3>
          {review.drafts.length === 0 ? <p>{t("install.mcpSource.drafts.none")}</p> : null}
          <ul className="mcp-draft-list">
            {review.drafts.map((draft) => (
              <DraftRow
                key={draft.key}
                draft={draft}
                edited={edits[draft.key]}
                isEdited={isEdited(draft)}
                picked={picked.includes(draft.key)}
                onPick={(on) => togglePick(draft.key, on)}
                onEdit={() => setEditing(draft)}
              />
            ))}
          </ul>
          {review.skipped.length > 0 ? (
            <section className="mcp-source-skipped">
              <h4>{t("install.mcpSource.skipped")}</h4>
              <ul>
                {review.skipped.map((skip, index) => (
                  <li key={`${skip.file}-${skip.entry ?? ""}-${index}`}>
                    <code>{skip.entry ? `${skip.file} (${skip.entry})` : skip.file}</code>
                    <span>{errorMessage(skip.problem.code)}</span>
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
          {review.drafts.length > 0 ? (
            <section className="mcp-source-clients">
              <h4>{t("install.mcpSource.clients")}</h4>
              <p className="mcp-form-hint">{t("install.mcpSource.clientsHint")}</p>
              <ClientGrid clients={clientList} choice={clients} onChange={chooseClients} />
            </section>
          ) : null}
          {blocked ? <p className="dialog-error" role="alert">{errorMessage("source_blocked")}</p> : null}
          {scanBlocked ? (
            <div className="dialog-error" role="alert">
              <p>{t("scan.gate.saveBlocked", { servers: scanBlocked.servers })}</p>
              <p>{t("scan.gate.saveBlockedDetail", { detail: scanBlocked.detail })}</p>
            </div>
          ) : null}
          {dangerous && pickedDrafts.length > 0 ? (
            <DangerousGate
              acknowledged={acknowledged}
              onChange={setAcknowledged}
              description={t("scan.gate.saveDescription")}
            />
          ) : null}
          {error ? <div className="dialog-error" role="alert">{error}</div> : null}
          <div className="repo-form__actions mcp-source-review__actions">
            <button className="secondary-button" type="button" disabled={isSaving} onClick={onDiscard}>
              {t("install.mcpSource.discard")}
            </button>
            {dangerous ? (
              <button
                className="secondary-button danger-button"
                type="button"
                disabled={pickedDrafts.length === 0 || !acknowledged || isSaving || blocked}
                onClick={() => void handleSave()}
              >
                {isSaving ? t("install.mcpSource.saving") : t("install.mcpSource.saveAnyway")}
              </button>
            ) : (
              <button
                className="primary-button"
                type="button"
                disabled={pickedDrafts.length === 0 || isSaving || blocked}
                onClick={() => void handleSave()}
              >
                {isSaving
                  ? t("install.mcpSource.saving")
                  : pickedDrafts.length > 1
                    ? t("install.mcpSource.saveMany", { count: pickedDrafts.length })
                    : t("install.mcpSource.save")}
              </button>
            )}
          </div>
      </div>
      {editing ? (
        <DraftEditDialog
          draft={editing}
          value={edits[editing.key]}
          clients={clientList}
          onDone={(value) => {
            clearGate();
            setEdits((current) => ({ ...current, [editing.key]: value }));
            if (!picked.includes(editing.key)) {
              togglePick(editing.key, true);
            }
            setEditing(null);
          }}
          onClose={() => setEditing(null)}
        />
      ) : null}
      {trustDialog}
    </>
  );
}

function SourceFacts({ review }: { review: McpSourceReview }) {
  const source = review.source;
  return (
    <dl className="source-review__facts">
      <div>
        <dt>
          {t(
            source.kind === "git"
              ? "install.mcpSource.kind.git"
              : source.kind === "registry"
                ? "install.mcpSource.kind.registrySource"
                : "install.mcpSource.kind.folder",
          )}
        </dt>
        <dd><code>{source.url ?? source.folder}</code></dd>
      </div>
      {source.catalog ? (
        <div>
          <dt>{t("install.review.catalog")}</dt>
          <dd><code>{source.catalog}</code></dd>
        </div>
      ) : null}
      {source.version ? (
        <div>
          <dt>{t("install.review.version")}</dt>
          <dd><code>{source.version}</code></dd>
        </div>
      ) : null}
      {source.path ? (
        <div>
          <dt>{t("install.review.path")}</dt>
          <dd><code>{source.path}</code></dd>
        </div>
      ) : null}
      {source.gitRef ? (
        <div>
          <dt>{t("install.review.ref")}</dt>
          <dd><code>{source.gitRef}</code></dd>
        </div>
      ) : null}
      {source.commit ? (
        <div>
          <dt>{t("install.review.commit")}</dt>
          <dd><code>{source.commit.slice(0, 7)}</code></dd>
        </div>
      ) : null}
    </dl>
  );
}

type DraftRowProps = {
  draft: McpDraft;
  edited: ServerDraft;
  /** The launch line differs from the reviewed one: its scan on screen no longer applies. */
  isEdited: boolean;
  picked: boolean;
  onPick: (picked: boolean) => void;
  onEdit: () => void;
};

function DraftRow({ draft, edited, isEdited, picked, onPick, onEdit }: DraftRowProps) {
  const name = edited.displayName ?? edited.id;
  const titleId = useId();
  return (
    <li className={`mcp-draft${picked ? " is-picked" : ""}`} aria-labelledby={titleId}>
      <div className="mcp-draft__head">
        <label className="mcp-draft__pick">
          <input
            type="checkbox"
            checked={picked}
            aria-label={t("install.mcpSource.pick", { name })}
            onChange={(event) => onPick(event.target.checked)}
          />
          <strong id={titleId}>{name}</strong>
        </label>
        <span className="status-badge tone-neutral">{t(KIND_LABELS[draft.kind])}</span>
        <button className="secondary-button secondary-button--compact" type="button" onClick={onEdit}>
          {t("install.mcpSource.edit", { name })}
        </button>
      </div>
      <p className="mcp-draft__provenance">{provenance(draft)}</p>
      {draft.trust ? <TrustSummary info={draft.trust} label={t("trust.summary.runsFrom")} /> : null}
      <LaunchLines
        lines={
          isEdited
            ? [{ os: null, line: launchOf(edited) }, ...draft.launchLines.filter((entry) => entry.os !== null)]
            : draft.launchLines
        }
      />
      {draft.pinned ? null : <p className="dialog-warning" role="note">{t("install.mcpSource.notPinned")}</p>}
      {isEdited ? <p className="dialog-warning">{t("install.mcpSource.edited")}</p> : null}
      {draft.runsRepoCode ? <RunsRepoCodeNote /> : null}
      <DraftNeeds draft={draft} />
      <ScanFindings scan={draft.scan} title={t("install.mcpSource.draftScan")} />
    </li>
  );
}

/** Secrets to set, values not kept, setup steps and duplicates of a draft. */
function DraftNeeds({ draft }: { draft: McpDraft }) {
  const notes = draft.inputs.filter((input) => input.secret || input.valueDropped || input.required);
  if (notes.length === 0 && draft.setup.length === 0 && draft.alsoIn.length === 0 && !draft.checkout) {
    return null;
  }
  return (
    <ul className="mcp-draft__needs">
      {notes.map((input) => (
        <li key={input.field}>
          <code>{input.field}</code>{" "}
          {input.secret
            ? t("install.mcpSource.input.secret", { alias: input.alias ?? input.field })
            : t("install.mcpSource.input.required")}
          {input.valueDropped ? ` ${t("install.mcpSource.input.valueDropped")}` : ""}
          {input.description ? ` ${input.description}` : ""}
        </li>
      ))}
      {draft.checkout ? <li>{t("install.mcpSource.checkout")}</li> : null}
      {draft.setup.length > 0 ? (
        <li>
          {t("install.mcpSource.setup")}
          {draft.setup.map((step) => (
            <code key={step} className="mcp-draft__setup">{step}</code>
          ))}
        </li>
      ) : null}
      {draft.alsoIn.length > 0 ? <li>{t("install.mcpSource.alsoIn", { files: draft.alsoIn.join(", ") })}</li> : null}
    </ul>
  );
}

type DraftEditDialogProps = {
  draft: McpDraft;
  value: ServerDraft;
  clients: readonly ClientInfo[];
  onDone: (value: ServerDraft) => void;
  onClose: () => void;
};

/** One draft in the existing server editor's form, with where it came from. */
function DraftEditDialog({ draft, value, clients, onDone, onClose }: DraftEditDialogProps) {
  const [current, setCurrent] = useState(value);
  const title = t("install.mcpSource.editTitle", { name: value.displayName ?? value.id });
  return (
    <Modal className="mcp-edit-dialog" label={title} onClose={onClose}>
      <div className="mcp-edit-dialog__header">
        <h3>{title}</h3>
        <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={t("mcp.dialog.close")}>
          ×
        </button>
      </div>
      <div className="mcp-edit-dialog__body">
        <div className="mcp-draft__origin">
          <p className="mcp-draft__provenance">{provenance(draft)}</p>
          <LaunchLines lines={draft.launchLines} />
          {draft.runsRepoCode ? <RunsRepoCodeNote /> : null}
        </div>
        <ServerForm clients={clients} draft={current} mode="draft" focusPaste={false} onChange={setCurrent} />
      </div>
      <div className="mcp-edit-dialog__footer">
        <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
          {t("mcp.dialog.cancel")}
        </button>
        <button className="primary-button primary-button--compact" type="button" onClick={() => onDone(current)}>
          {t("install.mcpSource.editDone")}
        </button>
      </div>
    </Modal>
  );
}
