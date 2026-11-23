// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Dialog shell, header, footer and the client checkbox grid ported from McpEditDialog in
// DailenG/skilldock@181b52b src/app/routes/mcp.tsx; the Form | TOML | Preview tabs, the
// reference picker and paste conversion (SPEC 13.2) are new.

import { useQuery } from "@tanstack/react-query";
import {
  useEffect,
  useId,
  useRef,
  useState,
  type ClipboardEvent as ReactClipboardEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useChangeNotice } from "@/app/apply/changeNotice";
import { AppSelect, type AppSelectOption } from "@/app/components/AppSelect";
import { ToolLogo } from "@/app/components/ToolLogo";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import {
  BIN_NAME,
  type ClientChoice,
  type ClientInfo,
  type ConvertedServer,
  type DeliveryKind,
  type IpcError,
  type KeyValue,
  type ServerDraft,
  type TransportKind,
} from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { TomlEditor } from "./TomlEditor";

export type EditTab = "form" | "toml" | "preview";

export type ServerEditDialogProps = {
  /** Edit mode loads `serverId` with `getServer`. */
  mode: "create" | "edit";
  serverId?: string;
  /** Clients the server can be enabled for (`ServerList.clients`). */
  clients: readonly ClientInfo[];
  initialTab?: EditTab;
  /** Create mode: focus the paste box first ("Paste config"). */
  focusPaste?: boolean;
  onClose: () => void;
};

const TABS: { id: EditTab; label: MessageKey }[] = [
  { id: "form", label: "mcp.dialog.tab.form" },
  { id: "toml", label: "mcp.dialog.tab.toml" },
  { id: "preview", label: "mcp.dialog.tab.preview" },
];

const TRANSPORT_OPTIONS: AppSelectOption<TransportKind>[] = [
  { value: "stdio", label: t("mcp.dialog.transport.stdio") },
  { value: "http", label: t("mcp.dialog.transport.http") },
  { value: "sse", label: t("mcp.dialog.transport.sse") },
];

type DeliveryChoice = DeliveryKind | "default";

const DELIVERY_OPTIONS: AppSelectOption<DeliveryChoice>[] = [
  { value: "default", label: t("mcp.dialog.delivery.default") },
  { value: "auto", label: t("mcp.dialog.delivery.auto") },
  { value: "env", label: t("mcp.dialog.delivery.env") },
  { value: "launch", label: t("mcp.dialog.delivery.launch") },
  { value: "command", label: t("mcp.dialog.delivery.command") },
  { value: "inline", label: t("mcp.dialog.delivery.inline") },
];

const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

function emptyDraft(): ServerDraft {
  return {
    id: "",
    displayName: null,
    description: null,
    transport: "stdio",
    command: null,
    args: [],
    url: null,
    cwd: null,
    env: [],
    headers: [],
    // No client until the user picks one, as `loadout server add` without
    // `--client`: a new server never reaches a client file by default.
    clients: { allDetected: false, clients: [] },
    delivery: null,
    toolsAllow: [],
    toolsDeny: [],
  };
}

/** A trimmed text field, or null when blank. */
export function optional(value: string | null): string | null {
  const trimmed = value?.trim() ?? "";
  return trimmed ? trimmed : null;
}

/** Key/value rows as saved: keys trimmed, blank rows dropped. */
export function keepRows(rows: KeyValue[]): KeyValue[] {
  return rows
    .map((row) => ({ key: row.key.trim(), value: row.value }))
    .filter((row) => row.key || row.value);
}

/** The draft as saved: blanks are unset, fields of the other transport dropped. */
export function normalizeDraft(draft: ServerDraft): ServerDraft {
  const stdio = draft.transport === "stdio";
  return {
    ...draft,
    id: draft.id.trim(),
    displayName: optional(draft.displayName),
    description: optional(draft.description),
    command: stdio ? optional(draft.command) : null,
    args: stdio ? draft.args : [],
    cwd: stdio ? optional(draft.cwd) : null,
    env: stdio ? keepRows(draft.env) : [],
    url: stdio ? null : optional(draft.url),
    headers: stdio ? [] : keepRows(draft.headers),
  };
}

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

function clientName(clients: readonly ClientInfo[], id: string): string {
  return clients.find((client) => client.id === id)?.name ?? id;
}

/** Server editor (SPEC 13.2): Form, TOML and Preview tabs. */
export function ServerEditDialog(props: ServerEditDialogProps) {
  const { mode, serverId, onClose } = props;
  const { commands } = useTransport();
  const detail = useQuery({
    queryKey: ["get_server", serverId],
    queryFn: async () => unwrap(await commands.getServer(serverId ?? "")),
    enabled: mode === "edit" && Boolean(serverId),
    gcTime: 0,
  });

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !event.defaultPrevented) {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const title = mode === "edit" ? t("mcp.dialog.editTitle") : t("mcp.dialog.createTitle");

  let body;
  if (mode === "create") {
    body = <ServerEditor {...props} initialDraft={emptyDraft()} initialToml={null} />;
  } else if (detail.data) {
    body = (
      <ServerEditor
        {...props}
        initialDraft={detail.data.draft ?? { ...emptyDraft(), id: serverId ?? "" }}
        initialToml={detail.data.toml}
        invalid={detail.data.invalid}
        initialTab={detail.data.draft ? props.initialTab : "toml"}
      />
    );
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
    <div className="dialog-backdrop" role="presentation" onClick={onClose}>
      <div
        className="mcp-edit-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="mcp-edit-dialog__header">
          <h3>{title}</h3>
          <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={t("mcp.dialog.close")}>
            ×
          </button>
        </div>
        {body}
      </div>
    </div>
  );
}

type ServerEditorProps = ServerEditDialogProps & {
  initialDraft: ServerDraft;
  /** The server file; null in create mode. */
  initialToml: string | null;
  /** The file does not load: only the TOML tab can fix it. */
  invalid?: IpcError | null;
};

/** Which side holds edits the other has not seen yet. */
type Edited = "form" | "toml" | null;

function ServerEditor(props: ServerEditorProps) {
  const { clients, focusPaste = false, initialDraft, initialToml, invalid, mode, onClose, serverId } = props;
  const { commands } = useTransport();
  const changeNotice = useChangeNotice();
  const [tab, setTab] = useState<EditTab>(props.initialTab ?? "form");
  const [draft, setDraft] = useState(initialDraft);
  const [tomlText, setTomlText] = useState(initialToml ?? "");
  // Form edits are written into the TOML text when the TOML tab opens;
  // TOML edits are read into the Form when it is left (a text that does
  // not parse keeps the TOML tab open). Save then always has every edit.
  const [edited, setEdited] = useState<Edited>(null);
  const [isSwitching, setIsSwitching] = useState(false);
  const [error, setError] = useState("");
  const [isSaving, setIsSaving] = useState(false);
  const panelId = useId();
  const savesToml = tab === "toml";
  const canSave = !(savesToml && initialToml === null);

  function editDraft(update: (current: ServerDraft) => ServerDraft) {
    setDraft(update);
    setEdited("form");
  }

  function editToml(text: string) {
    setTomlText(text);
    setEdited("toml");
  }

  async function selectTab(next: EditTab) {
    if (next === tab || isSwitching) {
      return;
    }
    setError("");
    setIsSwitching(true);
    try {
      if (initialToml !== null && serverId) {
        if (next === "toml" && edited === "form") {
          setTomlText(unwrap(await commands.serverDraftToToml(normalizeDraft(draft), tomlText)));
          setEdited(null);
        } else if (tab === "toml" && edited === "toml") {
          setDraft(unwrap(await commands.serverTomlToDraft(serverId, tomlText)));
          setEdited(null);
        }
      }
      setTab(next);
    } catch (failure) {
      setError(failureText(failure));
    } finally {
      setIsSwitching(false);
    }
  }

  async function handleSave() {
    setError("");
    setIsSaving(true);
    try {
      if (savesToml && serverId) {
        const result = unwrap(await commands.saveServerToml(serverId, tomlText));
        changeNotice(result, t("mcp.saved", { name: serverId }));
      } else {
        const saved = normalizeDraft(draft);
        const result = unwrap(await commands.saveServer(saved, mode === "create"));
        changeNotice(result, t("mcp.saved", { name: saved.displayName ?? saved.id }));
      }
      onClose();
    } catch (failure) {
      setError(failureText(failure));
      setIsSaving(false);
    }
  }

  return (
    <>
      <div className="mcp-edit-dialog__body">
        <div className="skills-source-tabs mcp-edit-tabs" role="tablist" aria-label={t("mcp.dialog.tabs.aria")}>
          {TABS.map((item) => (
            <button
              key={item.id}
              id={`${panelId}-${item.id}`}
              className={`skills-source-tab${tab === item.id ? " is-selected" : ""}`}
              type="button"
              role="tab"
              aria-selected={tab === item.id}
              aria-controls={`${panelId}-panel`}
              disabled={Boolean(invalid) && item.id !== "toml"}
              onClick={() => void selectTab(item.id)}
            >
              {t(item.label)}
            </button>
          ))}
        </div>
        <div id={`${panelId}-panel`} role="tabpanel" aria-labelledby={`${panelId}-${tab}`} className="mcp-edit-panel">
          {tab === "form" ? (
            <ServerForm clients={clients} draft={draft} mode={mode} focusPaste={focusPaste} onChange={editDraft} />
          ) : null}
          {tab === "toml" ? (
            initialToml === null ? (
              <p className="mcp-edit-dialog__note">{t("mcp.dialog.toml.createNote")}</p>
            ) : (
              <div className="mcp-form-field">
                {invalid ? (
                  <div className="dialog-error" role="alert">
                    {errorMessage(invalid.code)}
                  </div>
                ) : null}
                <span>{t("mcp.dialog.toml.label")}</span>
                <TomlEditor value={tomlText} onChange={editToml} ariaLabel={t("mcp.dialog.toml.label")} />
                <p className="mcp-form-hint">{t("mcp.dialog.toml.hint")}</p>
              </div>
            )
          ) : null}
          {tab === "preview" ? <PreviewPanel clients={clients} draft={normalizeDraft(draft)} /> : null}
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
          disabled={isSaving || isSwitching || !canSave}
        >
          {isSaving ? t("mcp.dialog.saving") : t("mcp.dialog.save")}
        </button>
      </div>
    </>
  );
}

type ServerFormProps = {
  clients: readonly ClientInfo[];
  draft: ServerDraft;
  mode: "create" | "edit";
  focusPaste: boolean;
  onChange: (update: (draft: ServerDraft) => ServerDraft) => void;
};

function ServerForm({ clients, draft, mode, focusPaste, onChange }: ServerFormProps) {
  const { commands } = useTransport();
  const aliases = useQuery({
    queryKey: ["list_secret_aliases"],
    queryFn: async () => unwrap(await commands.listSecretAliases()),
  });
  const stdio = draft.transport === "stdio";

  function set<K extends keyof ServerDraft>(key: K, value: ServerDraft[K]) {
    onChange((current) => ({ ...current, [key]: value }));
  }

  return (
    <div className="mcp-form">
      {mode === "create" ? (
        <PasteBox
          clients={clients}
          autoFocus={focusPaste}
          onApply={(converted) => onChange((current) => ({ ...converted, clients: current.clients }))}
        />
      ) : null}
      <label className="mcp-form-field">
        <span>{t("mcp.dialog.id")}</span>
        <input
          value={draft.id}
          readOnly={mode === "edit"}
          disabled={mode === "edit"}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => set("id", event.target.value)}
        />
      </label>
      <label className="mcp-form-field">
        <span>{t("mcp.dialog.displayName")}</span>
        <input value={draft.displayName ?? ""} onChange={(event) => set("displayName", event.target.value)} />
      </label>
      <label className="mcp-form-field">
        <span>{t("mcp.dialog.description")}</span>
        <input value={draft.description ?? ""} onChange={(event) => set("description", event.target.value)} />
      </label>
      <div className="mcp-form-field">
        <span>{t("mcp.dialog.transport")}</span>
        <AppSelect
          value={draft.transport}
          options={TRANSPORT_OPTIONS}
          ariaLabel={t("mcp.dialog.transport")}
          onChange={(value) => set("transport", value)}
        />
      </div>
      {stdio ? (
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
          <label className="mcp-form-field">
            <span>{t("mcp.dialog.cwd")}</span>
            <input
              value={draft.cwd ?? ""}
              spellCheck={false}
              autoComplete="off"
              onChange={(event) => set("cwd", event.target.value)}
            />
          </label>
        </>
      ) : (
        <label className="mcp-form-field">
          <span>{t("mcp.dialog.url")}</span>
          <input
            value={draft.url ?? ""}
            spellCheck={false}
            autoComplete="off"
            onChange={(event) => set("url", event.target.value)}
          />
        </label>
      )}
      <KeyValueEditor
        kind={stdio ? "env" : "headers"}
        rows={stdio ? draft.env : draft.headers}
        aliases={aliases.data ?? []}
        onChange={(rows) => set(stdio ? "env" : "headers", rows)}
      />
      <ClientGrid clients={clients} choice={draft.clients} onChange={(choice) => set("clients", choice)} />
      <div className="mcp-form-field">
        <span>{t("mcp.dialog.delivery")}</span>
        <AppSelect
          value={draft.delivery ?? "default"}
          options={DELIVERY_OPTIONS}
          ariaLabel={t("mcp.dialog.delivery")}
          onChange={(value) => set("delivery", value === "default" ? null : value)}
        />
      </div>
      <ChipInput
        label={t("mcp.dialog.toolsAllow")}
        placeholder={t("mcp.dialog.toolsPlaceholder")}
        values={draft.toolsAllow}
        onChange={(values) => set("toolsAllow", values)}
      />
      <ChipInput
        label={t("mcp.dialog.toolsDeny")}
        placeholder={t("mcp.dialog.toolsPlaceholder")}
        values={draft.toolsDeny}
        onChange={(values) => set("toolsDeny", values)}
      />
    </div>
  );
}

export type ChipInputProps = {
  label: string;
  placeholder: string;
  values: string[];
  onChange: (values: string[]) => void;
};

/** Values as removable chips; Enter adds the typed value. */
export function ChipInput({ label, placeholder, values, onChange }: ChipInputProps) {
  const [text, setText] = useState("");

  function commit() {
    if (text.trim()) {
      onChange([...values, text.trim()]);
      setText("");
    }
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter") {
      event.preventDefault();
      commit();
    } else if (event.key === "Backspace" && !text && values.length > 0) {
      onChange(values.slice(0, -1));
    }
  }

  return (
    <div className="mcp-form-field">
      <span>{label}</span>
      <div className="mcp-chip-input">
        {values.map((value, index) => (
          <span key={`${index}-${value}`} className="mcp-chip">
            <span className="mcp-chip__text">{value}</span>
            <button
              className="mcp-chip__remove"
              type="button"
              aria-label={t("mcp.dialog.chip.remove", { value })}
              onClick={() => onChange(values.filter((_, item) => item !== index))}
            >
              ×
            </button>
          </span>
        ))}
        <input
          aria-label={label}
          value={text}
          placeholder={placeholder}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => setText(event.target.value)}
          onKeyDown={handleKeyDown}
          onBlur={commit}
        />
      </div>
    </div>
  );
}

export type KeyValueEditorProps = {
  kind: "env" | "headers";
  rows: KeyValue[];
  aliases: string[];
  onChange: (rows: KeyValue[]) => void;
};

/** Environment variables or headers; values are literals or references. */
export function KeyValueEditor({ kind, rows, aliases, onChange }: KeyValueEditorProps) {
  const label = kind === "env" ? t("mcp.dialog.env") : t("mcp.dialog.headers");

  function update(index: number, row: KeyValue) {
    onChange(rows.map((item, position) => (position === index ? row : item)));
  }

  return (
    <div className="mcp-form-field">
      <span>{label}</span>
      <div className="mcp-kv-list">
        {rows.map((row, index) => (
          <KeyValueRow
            key={index}
            index={index}
            row={row}
            aliases={aliases}
            onChange={(next) => update(index, next)}
            onRemove={() => onChange(rows.filter((_, position) => position !== index))}
          />
        ))}
        <div className="mcp-kv-list__footer">
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            onClick={() => onChange([...rows, { key: "", value: "" }])}
          >
            {kind === "env" ? t("mcp.dialog.kv.addEnv") : t("mcp.dialog.kv.addHeader")}
          </button>
        </div>
        <p className="mcp-form-hint">{t("mcp.dialog.secretsHint", { command: `${BIN_NAME} secrets set <ALIAS>` })}</p>
      </div>
    </div>
  );
}

const ENV_OPTION = "__env";

type KeyValueRowProps = {
  index: number;
  row: KeyValue;
  aliases: string[];
  onChange: (row: KeyValue) => void;
  onRemove: () => void;
};

function KeyValueRow({ index, row, aliases, onChange, onRemove }: KeyValueRowProps) {
  const [askingEnv, setAskingEnv] = useState(false);
  const [envName, setEnvName] = useState("");
  const [envError, setEnvError] = useState("");
  const position = index + 1;
  const options: AppSelectOption<string>[] = [
    { value: ENV_OPTION, label: t("mcp.dialog.ref.env") },
    ...aliases.map((alias) => ({ value: `secret:${alias}`, label: t("mcp.dialog.ref.secret", { alias }) })),
  ];

  function insert(reference: string) {
    onChange({ ...row, value: `${row.value}${reference}` });
  }

  function handlePick(value: string) {
    if (value === ENV_OPTION) {
      setAskingEnv(true);
      setEnvName("");
      setEnvError("");
    } else {
      insert(`\${${value}}`);
    }
  }

  function insertEnv() {
    const name = envName.trim();
    if (!ENV_NAME.test(name)) {
      setEnvError(t("mcp.dialog.ref.envInvalid"));
      return;
    }
    insert(`\${env:${name}}`);
    setAskingEnv(false);
  }

  return (
    <div className="mcp-kv-row">
      <div className="mcp-kv-row__fields">
        <input
          aria-label={t("mcp.dialog.kv.keyAria", { n: position })}
          placeholder={t("mcp.dialog.kv.key")}
          value={row.key}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => onChange({ ...row, key: event.target.value })}
        />
        <input
          aria-label={t("mcp.dialog.kv.valueAria", { n: position })}
          placeholder={t("mcp.dialog.kv.value")}
          value={row.value}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => onChange({ ...row, value: event.target.value })}
        />
        <AppSelect
          className="mcp-kv-row__ref"
          value=""
          options={options}
          selectedLabel={t("mcp.dialog.ref.label")}
          ariaLabel={t("mcp.dialog.ref.aria", { n: position })}
          minMenuWidth={200}
          onChange={handlePick}
        />
        <button
          className="skill-card__icon-button"
          type="button"
          aria-label={t("mcp.dialog.kv.remove", { n: position })}
          data-tooltip={t("mcp.dialog.kv.removeTooltip")}
          onClick={onRemove}
        >
          ×
        </button>
      </div>
      {askingEnv ? (
        <div className="mcp-kv-row__env">
          <input
            aria-label={t("mcp.dialog.ref.envName")}
            placeholder={t("mcp.dialog.ref.envName")}
            value={envName}
            autoFocus
            spellCheck={false}
            autoComplete="off"
            onChange={(event) => setEnvName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                insertEnv();
              }
            }}
          />
          <button className="secondary-button secondary-button--compact" type="button" onClick={insertEnv}>
            {t("mcp.dialog.ref.insert")}
          </button>
          <button className="ghost-button" type="button" onClick={() => setAskingEnv(false)}>
            {t("mcp.dialog.cancel")}
          </button>
          {envError ? <span className="mcp-form-error">{envError}</span> : null}
        </div>
      ) : null}
    </div>
  );
}

export type ClientGridProps = {
  clients: readonly ClientInfo[];
  choice: ClientChoice;
  onChange: (choice: ClientChoice) => void;
};

/** The client checkbox grid with "All detected clients". */
export function ClientGrid({ clients, choice, onChange }: ClientGridProps) {
  const { allDetected, clients: chosen } = choice;

  function setAllDetected(checked: boolean) {
    onChange(
      checked
        ? { allDetected: true, clients: [] }
        : {
            allDetected: false,
            clients: clients.filter((client) => client.detected && client.supported).map((client) => client.id),
          },
    );
  }

  function setClient(id: string, checked: boolean) {
    const rest = chosen.filter((item) => item !== id);
    onChange({ allDetected: false, clients: checked ? [...rest, id] : rest });
  }

  return (
    <div className="mcp-form-field">
      <span>{t("mcp.dialog.clients")}</span>
      <label className={`mcp-form-app mcp-form-app--all${allDetected ? " is-selected" : ""}`}>
        <input type="checkbox" checked={allDetected} onChange={(event) => setAllDetected(event.target.checked)} />
        <span>{t("mcp.dialog.allDetected")}</span>
      </label>
      <div className="mcp-form-apps">
        {clients.map((client) => {
          const checked = allDetected ? client.detected && client.supported : chosen.includes(client.id);
          const disabled = allDetected || !client.supported || (!client.detected && !checked);
          const label = !client.supported
            ? t("mcp.dialog.clientUnsupported", { name: client.name })
            : !client.detected
              ? t("mcp.dialog.clientNotInstalled", { name: client.name })
              : client.name;
          return (
            <label key={client.id} className={`mcp-form-app${checked ? " is-selected" : ""}${disabled ? " is-disabled" : ""}`}>
              <input
                type="checkbox"
                aria-label={label}
                checked={checked}
                disabled={disabled}
                onChange={(event) => setClient(client.id, event.target.checked)}
              />
              <ToolLogo clientId={client.id} name={client.name} />
              <span>{label}</span>
            </label>
          );
        })}
      </div>
    </div>
  );
}

type PasteBoxProps = {
  clients: readonly ClientInfo[];
  autoFocus: boolean;
  onApply: (draft: ServerDraft) => void;
};

/** Paste-anything (SPEC 13.2): converts any client's snippet to a draft. */
function PasteBox({ clients, autoFocus, onApply }: PasteBoxProps) {
  const { commands } = useTransport();
  const [text, setText] = useState("");
  const [results, setResults] = useState<ConvertedServer[] | null>(null);
  const [filled, setFilled] = useState<ConvertedServer | null>(null);
  const [error, setError] = useState("");
  const [isConverting, setIsConverting] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => {
    if (autoFocus) {
      textareaRef.current?.focus();
    }
  }, [autoFocus]);

  function apply(result: ConvertedServer) {
    if (result.draft) {
      onApply(result.draft);
      setFilled(result);
    }
  }

  async function convert(snippet: string) {
    if (!snippet.trim()) {
      return;
    }
    setError("");
    setFilled(null);
    setIsConverting(true);
    try {
      const converted = unwrap(await commands.convertSnippet(snippet));
      setResults(converted);
      const only = converted.length === 1 ? converted[0] : undefined;
      if (only && only.draft && !only.problem) {
        apply(only);
      }
    } catch (failure) {
      setResults(null);
      setError(failureText(failure));
    } finally {
      setIsConverting(false);
    }
  }

  function handlePaste(event: ReactClipboardEvent<HTMLTextAreaElement>) {
    const pasted = event.clipboardData.getData("text/plain");
    const target = event.currentTarget;
    const next = `${text.slice(0, target.selectionStart)}${pasted}${text.slice(target.selectionEnd)}`;
    void convert(next);
  }

  const pickable = (results ?? []).length > 1;

  return (
    <div className="mcp-form-field mcp-paste-box">
      <span>{t("mcp.dialog.paste.label")}</span>
      <textarea
        ref={textareaRef}
        className="mcp-json-editor mcp-paste-box__input"
        aria-label={t("mcp.dialog.paste.label")}
        placeholder={t("mcp.dialog.paste.placeholder")}
        value={text}
        spellCheck={false}
        onChange={(event) => setText(event.target.value)}
        onPaste={handlePaste}
      />
      <div className="mcp-paste-box__actions">
        <span className="mcp-form-hint">{t("mcp.dialog.paste.hint")}</span>
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          onClick={() => void convert(text)}
          disabled={isConverting || !text.trim()}
        >
          {isConverting ? t("mcp.dialog.paste.converting") : t("mcp.dialog.paste.convert")}
        </button>
      </div>
      {error ? <div className="dialog-error" role="alert">{error}</div> : null}
      {results && results.length === 0 ? <p className="mcp-form-hint">{t("mcp.dialog.paste.none")}</p> : null}
      {filled ? (
        <p className="mcp-paste-box__filled" role="status">
          {t("mcp.dialog.paste.filled", { name: filled.name, client: clientName(clients, filled.sourceClient) })}
        </p>
      ) : null}
      {results && (pickable || results.some((result) => result.problem || !result.draft)) ? (
        <ul className="mcp-paste-results" aria-label={t("mcp.dialog.paste.results")}>
          {pickable ? <li className="mcp-form-hint">{t("mcp.dialog.paste.pick")}</li> : null}
          {results.map((result, index) => {
            const usable = result.draft && !result.problem;
            return (
              <li key={`${index}-${result.name}`} className={`mcp-paste-result${usable ? "" : " is-problem"}`}>
                <span className="mcp-paste-result__name">{result.name}</span>
                <span className="mcp-paste-result__source">
                  {t("mcp.dialog.paste.from", { client: clientName(clients, result.sourceClient) })}
                </span>
                {result.problem ? (
                  <span className="mcp-paste-result__problem">{errorMessage(result.problem.code)}</span>
                ) : null}
                {usable && pickable ? (
                  <button
                    className="secondary-button secondary-button--compact"
                    type="button"
                    aria-label={t("mcp.dialog.paste.useAria", { name: result.name })}
                    onClick={() => apply(result)}
                  >
                    {t("mcp.dialog.paste.use")}
                  </button>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}

function PreviewPanel({ clients, draft }: { clients: readonly ClientInfo[]; draft: ServerDraft }) {
  const { commands } = useTransport();
  const noClients = !draft.clients.allDetected && draft.clients.clients.length === 0;
  const preview = useQuery({
    queryKey: ["preview_server", draft],
    queryFn: async () => unwrap(await commands.previewServer(draft)),
    enabled: !noClients,
    gcTime: 0,
    staleTime: 0,
  });

  if (noClients) {
    return <p className="mcp-edit-dialog__note">{t("mcp.dialog.preview.noClients")}</p>;
  }
  if (preview.isError) {
    return <div className="dialog-error" role="alert">{failureText(preview.error)}</div>;
  }
  if (!preview.data) {
    return <p className="mcp-edit-dialog__note">{t("mcp.dialog.preview.loading")}</p>;
  }
  if (preview.data.length === 0) {
    return <p className="mcp-edit-dialog__note">{t("mcp.dialog.preview.empty")}</p>;
  }
  return (
    <div className="mcp-preview-list">
      {preview.data.map((entry) => {
        const name = clientName(clients, entry.client);
        return (
          <section key={entry.client} className={`mcp-preview${entry.problem ? " is-blocked" : ""}`} aria-label={name}>
            <header className="mcp-preview__header">
              <ToolLogo clientId={entry.client} name={name} />
              <strong>{name}</strong>
              <span className="status-badge tone-neutral">{entry.format}</span>
              <span className="mcp-preview__entry">{t("mcp.dialog.preview.entry", { name: entry.name })}</span>
            </header>
            {entry.problem ? (
              <p className="mcp-preview__problem">{errorMessage(entry.problem.code)}</p>
            ) : null}
            {entry.text !== null ? <pre className="mcp-preview__text">{entry.text}</pre> : null}
            {entry.delivery ? (
              <p className="mcp-form-hint">{t("mcp.dialog.preview.delivery", { mode: entry.delivery })}</p>
            ) : null}
            {entry.warnings.map((warning, index) => (
              <p key={`${index}-${warning.code}`} className="mcp-preview__warning">{errorMessage(warning.code)}</p>
            ))}
          </section>
        );
      })}
    </div>
  );
}
