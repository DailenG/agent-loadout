// Add remote (SPEC 9.6, 13.6 item 4): two equal choices, Git URL and
// Folder, neither recommended. A git remote picks its auth (the user's own
// git credentials by default, gh for GitHub URLs, or a token in the
// keyring); a folder remote is picked in the OS dialog (the path stays in
// the backend) and writes one subfolder per machine in the mode chosen.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { failureText } from "@/app/routes/projects/labels";
import { SettingsDialog } from "@/app/routes/settings/SettingsDialog";
import {
  AUTH_KEYS,
  GITHUB_URL,
  KIND_KEYS,
  MODE_HINT_KEYS,
  MODE_KEYS,
  REMOTE_NAME_PATTERN,
  syncToolNote,
} from "@/app/routes/settings/versioning/labels";
import { errorMessage, t } from "@/i18n/en";
import type { AddedRemote, FolderModeDto, GitAuthDto, PickedRemoteFolder, RemoteKindDto } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type AddRemoteDialogProps = {
  open: boolean;
  /** Added from the first run's "Add a backup remote" step. */
  firstRun: boolean;
  /** A choice already made (the first run's step); none is preselected otherwise. */
  initialKind?: RemoteKindDto;
  onClose: () => void;
  onAdded: (added: AddedRemote) => void;
};

const KINDS: RemoteKindDto[] = ["git", "folder"];
const AUTHS: GitAuthDto[] = ["system", "gh", "token"];
const MODES: FolderModeDto[] = ["repo_per_machine", "mirror", "archive"];

const KIND_HINT_KEYS = {
  git: "settings.versioning.add.gitHint",
  folder: "settings.versioning.add.folderHint",
} as const;

export function AddRemoteDialog({ open, ...props }: AddRemoteDialogProps) {
  // Mounted per opening, so every opening starts from an empty form.
  return open ? <AddRemoteForm {...props} /> : null;
}

function AddRemoteForm({ firstRun, initialKind, onClose, onAdded }: Omit<AddRemoteDialogProps, "open">) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const queryClient = useQueryClient();
  const overview = useQuery({
    queryKey: storeQueryKeys.remotes,
    queryFn: async () => unwrap(await commands.remotesOverview()),
  });
  const [kind, setKind] = useState<RemoteKindDto | null>(initialKind ?? null);
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [auth, setAuth] = useState<GitAuthDto>("system");
  const [username, setUsername] = useState("");
  const [picked, setPicked] = useState<PickedRemoteFolder | null>(null);
  const [mode, setMode] = useState<FolderModeDto>("repo_per_machine");
  const [added, setAdded] = useState<AddedRemote | null>(null);
  const [token, setToken] = useState("");

  const gitFound = overview.data?.gitFound ?? true;
  const ghFound = overview.data?.ghFound ?? false;
  const ghUsable = ghFound && GITHUB_URL.test(url.trim());
  // gh falls back to system credentials once the URL leaves GitHub.
  const effectiveAuth: GitAuthDto = auth === "gh" && !ghUsable ? "system" : auth;
  const nameValid = REMOTE_NAME_PATTERN.test(name);
  const canAdd = nameValid && (kind === "git" ? url.trim() !== "" : kind === "folder" && picked !== null);

  const onError = (error: unknown) => notify({ tone: "error", message: failureText(error) });

  function finish(result: AddedRemote, needsToken: boolean) {
    void queryClient.invalidateQueries({ queryKey: storeQueryKeys.remotes });
    if (result.warnings.length === 0 && !needsToken) {
      notify({ tone: "success", message: t("settings.versioning.add.done", { name: result.remote.name }) });
      onAdded(result);
    } else {
      setAdded(result);
    }
  }

  const pick = useMutation({
    mutationFn: async () => unwrap(await commands.pickRemoteFolder()),
    onSuccess: (folder) => {
      if (folder) {
        setPicked(folder);
      }
    },
    onError,
  });

  const addGit = useMutation({
    mutationFn: async () =>
      unwrap(
        await commands.addGitRemote(
          name,
          url.trim(),
          effectiveAuth,
          effectiveAuth === "token" && username.trim() !== "" ? username.trim() : null,
          firstRun,
        ),
      ),
    onSuccess: (result) => finish(result, result.remote.auth === "token" && !result.remote.tokenStored),
    onError,
  });

  const addFolder = useMutation({
    mutationFn: async (folder: PickedRemoteFolder) =>
      unwrap(await commands.addFolderRemote(name, folder.token, mode, firstRun)),
    onSuccess: (result) => finish(result, false),
    onError,
  });

  const saveToken = useMutation({
    mutationFn: async ({ remote, value }: { remote: string; value: string }) =>
      unwrap(await commands.setRemoteToken(remote, value)),
    onSuccess: (_data, { remote }) => {
      notify({ tone: "success", message: t("settings.versioning.token.saved", { name: remote }) });
      void queryClient.invalidateQueries({ queryKey: storeQueryKeys.remotes });
      if (added) {
        onAdded(added);
      }
    },
    onError,
  });

  if (added) {
    const askToken = added.remote.auth === "token" && !added.remote.tokenStored;
    return (
      <SettingsDialog
        title={t("settings.versioning.add.addedTitle", { name: added.remote.name })}
        subtitle={added.remote.location}
        closeLabel={t("settings.versioning.close")}
        onClose={() => onAdded(added)}
        actions={
          <>
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              onClick={() => onAdded(added)}
            >
              {t(askToken ? "settings.versioning.token.later" : "settings.versioning.done")}
            </button>
            {askToken ? (
              <button
                className="primary-button primary-button--compact"
                type="button"
                disabled={token === "" || saveToken.isPending}
                onClick={() => {
                  const value = token;
                  setToken("");
                  saveToken.mutate({ remote: added.remote.name, value });
                }}
              >
                {t("settings.versioning.token.save")}
              </button>
            ) : null}
          </>
        }
      >
        {added.warnings.length > 0 ? (
          <ul className="settings-warnings" aria-label={t("settings.versioning.add.warnings")}>
            {added.warnings.map((warning) => (
              <li key={warning.code}>{errorMessage(warning.code)}</li>
            ))}
          </ul>
        ) : null}
        {askToken ? (
          <div className="project-dialog__field">
            <label className="project-dialog__field">
              <span>{t("settings.versioning.token.label")}</span>
              <input
                className="tool-manage-dialog__search"
                type="password"
                autoComplete="off"
                value={token}
                onChange={(event) => setToken(event.target.value)}
              />
            </label>
            <span className="project-dialog__hint">{t("settings.versioning.token.hint")}</span>
          </div>
        ) : null}
      </SettingsDialog>
    );
  }

  const busy = addGit.isPending || addFolder.isPending;
  return (
    <SettingsDialog
      title={t("settings.versioning.add.title")}
      closeLabel={t("settings.versioning.close")}
      onClose={onClose}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
            {t("settings.versioning.cancel")}
          </button>
          <button
            className="primary-button primary-button--compact"
            type="button"
            disabled={!canAdd || busy}
            onClick={() => {
              if (kind === "git") {
                addGit.mutate();
              } else if (picked) {
                addFolder.mutate(picked);
              }
            }}
          >
            {t("settings.versioning.add.submit")}
          </button>
        </>
      }
    >
      <div className="settings-choice" role="group" aria-label={t("settings.versioning.add.kind")}>
        {KINDS.map((option) => (
          <button
            key={option}
            className={`settings-choice__option${kind === option ? " is-selected" : ""}`}
            type="button"
            aria-pressed={kind === option}
            onClick={() => setKind(option)}
          >
            <strong>{t(KIND_KEYS[option])}</strong>
            <span className="project-dialog__hint">{t(KIND_HINT_KEYS[option])}</span>
          </button>
        ))}
      </div>

      {kind === "folder" ? (
        <div className="project-dialog__field">
          <div className="settings-actions">
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              disabled={pick.isPending}
              onClick={() => pick.mutate()}
            >
              {t("settings.versioning.chooseFolder")}
            </button>
          </div>
          {picked ? (
            <>
              <span className="settings-path" title={picked.path}>
                {picked.path}
              </span>
              {picked.syncTool ? <span className="project-dialog__hint">{syncToolNote(picked.syncTool)}</span> : null}
            </>
          ) : (
            <span className="project-dialog__hint">{t("settings.versioning.add.folderPickHint")}</span>
          )}
        </div>
      ) : null}

      {kind === "git" ? (
        <div className="project-dialog__field">
          <label className="project-dialog__field">
            <span>{t("settings.versioning.add.url")}</span>
            <input
              className="tool-manage-dialog__search"
              type="text"
              spellCheck={false}
              placeholder={t("settings.versioning.add.urlPlaceholder")}
              value={url}
              onChange={(event) => setUrl(event.target.value)}
            />
          </label>
          {!gitFound ? <span className="project-dialog__hint">{t("settings.versioning.add.gitMissing")}</span> : null}
        </div>
      ) : null}

      {kind ? (
        <div className="project-dialog__field">
          <label className="project-dialog__field">
            <span>{t("settings.versioning.add.name")}</span>
            <input
              className="tool-manage-dialog__search"
              type="text"
              spellCheck={false}
              value={name}
              aria-invalid={name !== "" && !nameValid}
              onChange={(event) => setName(event.target.value)}
            />
          </label>
          <span className="project-dialog__hint" role={name !== "" && !nameValid ? "alert" : undefined}>
            {t(name !== "" && !nameValid ? "settings.versioning.add.nameInvalid" : "settings.versioning.add.nameHint")}
          </span>
        </div>
      ) : null}

      {kind === "git" ? (
        <fieldset className="project-dialog__body settings-fieldset">
          <legend>{t("settings.versioning.add.auth")}</legend>
          {AUTHS.map((option) => {
            const disabled = option === "gh" && !ghUsable;
            return (
              <label key={option} className="project-dialog__choice">
                <input
                  type="radio"
                  name="remote-auth"
                  checked={effectiveAuth === option}
                  disabled={disabled}
                  onChange={() => setAuth(option)}
                />
                <span>
                  <strong>{t(AUTH_KEYS[option])}</strong>
                  {option === "system" ? (
                    <span className="project-dialog__hint">{t("settings.versioning.auth.systemHint")}</span>
                  ) : null}
                  {option === "gh" ? (
                    <span className="project-dialog__hint">
                      {t(
                        !ghFound
                          ? "settings.versioning.auth.ghMissing"
                          : ghUsable
                            ? "settings.versioning.auth.ghHint"
                            : "settings.versioning.auth.ghGithubOnly",
                      )}
                    </span>
                  ) : null}
                  {option === "token" ? (
                    <span className="project-dialog__hint">{t("settings.versioning.auth.tokenHint")}</span>
                  ) : null}
                </span>
              </label>
            );
          })}
          {effectiveAuth === "token" ? (
            <label className="project-dialog__field">
              <span>{t("settings.versioning.add.username")}</span>
              <input
                className="tool-manage-dialog__search"
                type="text"
                spellCheck={false}
                placeholder={t("settings.versioning.add.usernamePlaceholder")}
                value={username}
                onChange={(event) => setUsername(event.target.value)}
              />
            </label>
          ) : null}
        </fieldset>
      ) : null}

      {kind === "folder" ? (
        <fieldset className="project-dialog__body settings-fieldset">
          <legend>{t("settings.versioning.add.mode")}</legend>
          {MODES.map((option) => (
            <label key={option} className="project-dialog__choice">
              <input type="radio" name="remote-mode" checked={mode === option} onChange={() => setMode(option)} />
              <span>
                <strong>{t(MODE_KEYS[option])}</strong>
                <span className="project-dialog__hint">{t(MODE_HINT_KEYS[option])}</span>
              </span>
            </label>
          ))}
        </fieldset>
      ) : null}
    </SettingsDialog>
  );
}
