// Project server editor (SPEC 13.2 project scope): one `[servers.<name>]` table of a project's
// manifest, with the "Write to: Project (shared) / Local (private)" choice. The dialog shell, the
// chip and key/value editors and the client grid are the store editor's (ServerEditDialog).

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useChangeNotice } from "@/app/apply/changeNotice";
import { AppSelect, type AppSelectOption } from "@/app/components/AppSelect";
import { Modal } from "@/app/components/Modal";
import { errorMessage, t } from "@/i18n/en";
import type {
  ClientInfo,
  ManifestLayerKind,
  ProjectServerDraft,
  ProjectServerKind,
  TransportKind,
} from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { ChipInput, ClientGrid, KeyValueEditor, keepRows, optional } from "./ServerEditDialog";

export type ProjectDialogState =
  | { mode: "create"; initial?: Partial<ProjectServerDraft> }
  | { mode: "edit"; name: string; layer: ManifestLayerKind };

export type ProjectServerDialogProps = {
  projectId: string;
  projectName: string;
  /** The manifests' content hash as listed. */
  hash: string;
  /** The manifest files an edit can write. */
  layers: ManifestLayerKind[];
  clients: readonly ClientInfo[];
  /** Store server ids, the choices of an override. */
  storeServers: string[];
  state: ProjectDialogState;
  onClose: () => void;
};

const LAYER_LABELS = {
  shared: "mcp.project.dialog.layer.shared",
  local: "mcp.project.dialog.layer.local",
} as const;

const KIND_OPTIONS: AppSelectOption<ProjectServerKind>[] = [
  { value: "define", label: t("mcp.project.dialog.kind.define") },
  { value: "override", label: t("mcp.project.dialog.kind.override") },
  { value: "suppress", label: t("mcp.project.dialog.kind.suppress") },
];

/** A private table over the shared entry of the same name changes that entry. */
const OVER_SHARED_KIND_OPTIONS: AppSelectOption<ProjectServerKind>[] = [
  { value: "define", label: t("mcp.project.dialog.kind.overShared") },
  { value: "override", label: t("mcp.project.dialog.kind.override") },
  { value: "suppress", label: t("mcp.project.dialog.kind.suppress") },
];

type TransportChoice = TransportKind | "inherit";

const TRANSPORTS: AppSelectOption<TransportChoice>[] = [
  { value: "stdio", label: t("mcp.dialog.transport.stdio") },
  { value: "http", label: t("mcp.dialog.transport.http") },
  { value: "sse", label: t("mcp.dialog.transport.sse") },
];

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

function emptyDraft(layer: ManifestLayerKind): ProjectServerDraft {
  return {
    name: "",
    layer,
    kind: "define",
    from: null,
    transport: "stdio",
    command: null,
    args: [],
    url: null,
    env: [],
    headers: [],
    clients: null,
    overShared: false,
  };
}

/** The draft as saved: blanks unset, fields of another transport dropped. */
export function normalizeProjectDraft(draft: ProjectServerDraft): ProjectServerDraft {
  const name = draft.name.trim();
  if (draft.kind === "suppress") {
    return { ...emptyDraft(draft.layer), name, kind: "suppress", clients: draft.clients };
  }
  const stdio = draft.transport === "stdio";
  const remote = draft.transport === "http" || draft.transport === "sse";
  return {
    ...draft,
    name,
    from: draft.kind === "override" ? optional(draft.from) : null,
    command: remote ? null : optional(draft.command),
    args: remote ? [] : draft.args,
    env: remote ? [] : keepRows(draft.env),
    url: stdio ? null : optional(draft.url),
    headers: stdio ? [] : keepRows(draft.headers),
  };
}

export function ProjectServerDialog(props: ProjectServerDialogProps) {
  const { projectId, state, onClose } = props;
  const { commands } = useTransport();
  const editing = state.mode === "edit" ? state : null;
  const detail = useQuery({
    queryKey: ["get_project_server", projectId, editing?.name, editing?.layer],
    queryFn: async () => unwrap(await commands.getProjectServer(projectId, editing?.name ?? "", editing?.layer ?? "local")),
    enabled: Boolean(editing),
    gcTime: 0,
  });

  const title = editing ? t("mcp.project.dialog.editTitle") : t("mcp.project.dialog.createTitle");
  const fallbackLayer = props.layers.includes("shared") ? "shared" : "local";
  let body;
  if (state.mode === "create") {
    body = <ProjectServerEditor {...props} create initialDraft={{ ...emptyDraft(fallbackLayer), ...state.initial }} />;
  } else if (detail.data) {
    body = <ProjectServerEditor {...props} create={false} initialDraft={detail.data} />;
  } else {
    body = (
      <>
        <div className="mcp-edit-dialog__body">
          {detail.isError ? (
            <div className="dialog-error" role="alert">{failureText(detail.error)}</div>
          ) : (
            <p className="mcp-edit-dialog__note">{t("mcp.dialog.loading")}</p>
          )}
        </div>
        <div className="mcp-edit-dialog__footer">
          <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
            {t("mcp.dialog.cancel")}
          </button>
        </div>
      </>
    );
  }

  return (
    <Modal className="mcp-edit-dialog" label={title} onClose={onClose}>
      <div className="mcp-edit-dialog__header">
        <h3>{title}</h3>
        <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={t("mcp.dialog.close")}>
          ×
        </button>
      </div>
      {body}
    </Modal>
  );
}

type EditorProps = ProjectServerDialogProps & {
  create: boolean;
  initialDraft: ProjectServerDraft;
};

function ProjectServerEditor({
  clients,
  create,
  hash,
  initialDraft,
  layers,
  onClose,
  projectId,
  projectName,
  storeServers,
}: EditorProps) {
  const { commands } = useTransport();
  const changeNotice = useChangeNotice();
  const aliases = useQuery({
    queryKey: ["list_secret_aliases"],
    queryFn: async () => unwrap(await commands.listSecretAliases()),
  });
  const [draft, setDraft] = useState(initialDraft);
  const [error, setError] = useState("");
  const [isSaving, setIsSaving] = useState(false);

  function set<K extends keyof ProjectServerDraft>(key: K, value: ProjectServerDraft[K]) {
    setDraft((current) => ({ ...current, [key]: value }));
  }

  const override = draft.kind === "override";
  const suppress = draft.kind === "suppress";
  const inheritLabel = override ? t("mcp.project.dialog.transportInherit") : t("mcp.project.dialog.transportShared");
  const transportOptions: AppSelectOption<TransportChoice>[] = override || draft.overShared
    ? [{ value: "inherit", label: inheritLabel }, ...TRANSPORTS]
    : TRANSPORTS;
  const stdioFields = draft.transport !== "http" && draft.transport !== "sse";
  const remoteFields = draft.transport !== "stdio";
  const layerOptions: AppSelectOption<ManifestLayerKind>[] = (create ? layers : [draft.layer]).map((layer) => ({
    value: layer,
    label: t(LAYER_LABELS[layer]),
  }));
  const fromOptions: AppSelectOption<string>[] = storeServers.map((id) => ({ value: id, label: id }));

  async function handleSave() {
    setError("");
    setIsSaving(true);
    try {
      const saved = normalizeProjectDraft(draft);
      const result = unwrap(await commands.saveProjectServer(projectId, hash, saved, create));
      const key = saved.kind === "suppress" ? "mcp.project.hidden" : "mcp.project.saved";
      changeNotice(result, t(key, { name: saved.name, project: projectName }));
      onClose();
    } catch (failure) {
      setError(failureText(failure));
      setIsSaving(false);
    }
  }

  return (
    <>
      <div className="mcp-edit-dialog__body">
        <div className="mcp-form">
          <label className="mcp-form-field">
            <span>{t("mcp.project.dialog.name")}</span>
            <input
              value={draft.name}
              readOnly={!create}
              disabled={!create}
              spellCheck={false}
              autoComplete="off"
              onChange={(event) => set("name", event.target.value)}
            />
          </label>
          <div className="mcp-form-field">
            <span>{t("mcp.project.dialog.layer")}</span>
            <AppSelect
              value={draft.layer}
              options={layerOptions}
              ariaLabel={t("mcp.project.dialog.layer")}
              onChange={(value) => set("layer", value)}
            />
            <p className="mcp-form-hint">{t("mcp.project.dialog.layerHint")}</p>
          </div>
          <div className="mcp-form-field">
            <span>{t("mcp.project.dialog.kind")}</span>
            <AppSelect
              value={draft.kind}
              options={draft.overShared ? OVER_SHARED_KIND_OPTIONS : KIND_OPTIONS}
              ariaLabel={t("mcp.project.dialog.kind")}
              onChange={(value) =>
                setDraft((current) => ({
                  ...current,
                  kind: value,
                  // A new definition needs a transport; an override, or a
                  // private table over the shared entry, may keep its own.
                  transport: value === "define" && !current.overShared ? current.transport ?? "stdio" : current.transport,
                  from: value === "override" ? current.from ?? (storeServers.includes(current.name) ? current.name : null) : null,
                }))}
            />
            {draft.overShared ? <p className="mcp-form-hint">{t("mcp.project.dialog.overSharedHint")}</p> : null}
          </div>
          {override ? (
            <div className="mcp-form-field">
              <span>{t("mcp.project.dialog.from")}</span>
              <AppSelect
                value={draft.from ?? ""}
                options={fromOptions}
                ariaLabel={t("mcp.project.dialog.from")}
                selectedLabel={draft.from ?? ""}
                onChange={(value) => set("from", value)}
              />
              <p className="mcp-form-hint">{t("mcp.project.dialog.overrideHint")}</p>
            </div>
          ) : null}
          {suppress ? (
            <p className="mcp-form-hint">{t("mcp.project.dialog.suppressHint")}</p>
          ) : (
            <>
              <div className="mcp-form-field">
                <span>{t("mcp.dialog.transport")}</span>
                <AppSelect
                  value={draft.transport ?? "inherit"}
                  options={transportOptions}
                  ariaLabel={t("mcp.dialog.transport")}
                  onChange={(value) => set("transport", value === "inherit" ? null : value)}
                />
              </div>
              {stdioFields ? (
                <>
                  <label className="mcp-form-field">
                    <span>{t("mcp.dialog.command")}</span>
                    <input
                      value={draft.command ?? ""}
                      spellCheck={false}
                      autoComplete="off"
                      onChange={(event) => set("command", event.target.value)}
                    />
                  </label>
                  <ChipInput
                    label={t("mcp.dialog.args")}
                    placeholder={t("mcp.dialog.argsPlaceholder")}
                    values={draft.args}
                    onChange={(values) => set("args", values)}
                  />
                  <KeyValueEditor
                    kind="env"
                    rows={draft.env}
                    aliases={aliases.data ?? []}
                    onChange={(rows) => set("env", rows)}
                  />
                </>
              ) : null}
              {remoteFields ? (
                <>
                  <label className="mcp-form-field">
                    <span>{t("mcp.dialog.url")}</span>
                    <input
                      value={draft.url ?? ""}
                      spellCheck={false}
                      autoComplete="off"
                      onChange={(event) => set("url", event.target.value)}
                    />
                  </label>
                  <KeyValueEditor
                    kind="headers"
                    rows={draft.headers}
                    aliases={aliases.data ?? []}
                    onChange={(rows) => set("headers", rows)}
                  />
                </>
              ) : null}
            </>
          )}
          <label className="mcp-form-app mcp-form-app--all">
            <input
              type="checkbox"
              checked={draft.clients === null}
              onChange={(event) =>
                set("clients", event.target.checked ? null : { allDetected: false, clients: [] })}
            />
            <span>{t("mcp.project.dialog.useDefaults")}</span>
          </label>
          {draft.clients ? (
            <ClientGrid clients={clients} choice={draft.clients} onChange={(choice) => set("clients", choice)} />
          ) : null}
          {create ? null : <p className="mcp-form-hint">{t("mcp.project.dialog.keptHint")}</p>}
        </div>
        {error ? <div className="dialog-error" role="alert">{error}</div> : null}
      </div>
      <div className="mcp-edit-dialog__footer">
        <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
          {t("mcp.dialog.cancel")}
        </button>
        <button
          className="primary-button primary-button--compact"
          type="button"
          onClick={() => void handleSave()}
          disabled={isSaving || !draft.name.trim()}
        >
          {isSaving ? t("mcp.dialog.saving") : t("mcp.dialog.save")}
        </button>
      </div>
    </>
  );
}
