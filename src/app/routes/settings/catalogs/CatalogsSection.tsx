// Settings > Catalogs (M45, #26): the online setting, the catalogs Browse
// searches (built-ins can be turned off, not removed) and the user's own:
// an MCP registry URL or a Git tap. The list lives in the store
// (`catalogs.toml`, synced); the online setting is this computer's only.
// Nothing here contacts a catalog.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { SettingsGroup, SettingsItem } from "@/app/components/SettingsGroup";
import { OfficialBadge } from "@/app/components/TrustDialog";
import { useNotifications } from "@/app/components/notifications";
import { t } from "@/i18n/en";
import type { CatalogInfo, CatalogsOverview } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";
import { useCatalogsOverview } from "@/app/routes/install/catalog/BrowsePanel";
import { CATALOG_KIND_LABELS, catalogQueryKeys } from "@/app/routes/install/catalog/catalogText";
import { failureText } from "@/app/routes/skills/labels";
import { SettingsDialog } from "../SettingsDialog";

export function CatalogsSection() {
  const { commands } = useTransport();
  const queryClient = useQueryClient();
  const { notify } = useNotifications();
  const { overview, setOnline } = useCatalogsOverview();
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<CatalogInfo | null>(null);

  const setEnabled = useMutation({
    mutationFn: async ({ id, enabled }: { id: string; enabled: boolean }) =>
      unwrap(await commands.setCatalogEnabled(id, enabled)),
    onSuccess: (data: CatalogsOverview) => queryClient.setQueryData(catalogQueryKeys.overview, data),
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const remove = useMutation({
    mutationFn: async (catalog: CatalogInfo) => unwrap(await commands.removeCatalog(catalog.id)),
    onSuccess: (data: CatalogsOverview, catalog) => {
      queryClient.setQueryData(catalogQueryKeys.overview, data);
      setRemoving(null);
      notify({ tone: "success", message: t("settings.catalogs.removed", { catalog: catalog.title }) });
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const data = overview.data;
  const hosts = data && data.contacts.length > 0 ? data.contacts.join(", ") : t("catalog.optIn.noHosts");

  return (
    <SettingsGroup title={t("settings.catalogs.title")}>
      <SettingsItem
        label={t("settings.catalogs.online.label")}
        description={
          data
            ? t(data.online ? "settings.catalogs.online.on" : "settings.catalogs.online.off", { hosts })
            : overview.isError
              ? failureText(overview.error)
              : t("catalog.loading")
        }
      >
        <input
          type="checkbox"
          checked={data?.online ?? false}
          disabled={!data || setOnline.isPending}
          aria-label={t("settings.catalogs.online.label")}
          onChange={(event) => setOnline.mutate(event.target.checked)}
        />
      </SettingsItem>
      <SettingsItem readonly label={t("settings.catalogs.list.label")} description={t("settings.catalogs.list.description")}>
        <button className="secondary-button secondary-button--compact" type="button" onClick={() => setAdding(true)}>
          {t("settings.catalogs.add")}
        </button>
      </SettingsItem>
      {data ? (
        <ul className="sources-list" aria-label={t("settings.catalogs.listAria")}>
          {data.catalogs.map((catalog) => (
            <li key={catalog.id} className="sources-list__item">
              <div className="sources-list__copy">
                <div className="sources-list__title">
                  <strong>{catalog.title}</strong>
                  <span className="status-badge tone-neutral">{t(CATALOG_KIND_LABELS[catalog.kind])}</span>
                  {catalog.builtin ? (
                    <span className="status-badge tone-neutral">{t("settings.catalogs.builtin")}</span>
                  ) : null}
                  {catalog.official ? <OfficialBadge /> : null}
                </div>
                <p>
                  <code>{catalog.url}</code>
                  {catalog.gitRef ? ` ${t("settings.catalogs.ref", { ref: catalog.gitRef })}` : ""}
                </p>
              </div>
              <div className="sources-list__actions">
                <label className="catalog-toggle">
                  <input
                    type="checkbox"
                    checked={catalog.enabled}
                    disabled={setEnabled.isPending}
                    onChange={(event) => setEnabled.mutate({ id: catalog.id, enabled: event.target.checked })}
                  />
                  <span>{t("settings.catalogs.use", { catalog: catalog.title })}</span>
                </label>
                {catalog.builtin ? null : (
                  <button
                    className="secondary-button secondary-button--compact"
                    type="button"
                    aria-label={t("settings.catalogs.removeAria", { catalog: catalog.title })}
                    onClick={() => setRemoving(catalog)}
                  >
                    {t("settings.catalogs.remove")}
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
      ) : null}
      {adding ? <AddCatalogDialog onClose={() => setAdding(false)} /> : null}
      {removing ? (
        <RemoveDialog
          catalog={removing}
          busy={remove.isPending}
          onCancel={() => setRemoving(null)}
          onConfirm={() => remove.mutate(removing)}
        />
      ) : null}
    </SettingsGroup>
  );
}

function RemoveDialog(props: { catalog: CatalogInfo; busy: boolean; onCancel: () => void; onConfirm: () => void }) {
  const describedBy = useId();
  return (
    <SettingsDialog
      title={t("settings.catalogs.removeTitle", { catalog: props.catalog.title })}
      closeLabel={t("trust.close")}
      role="alertdialog"
      describedBy={describedBy}
      onClose={props.onCancel}
      actions={
        <>
          {/* Starts on Cancel: removing is the choice that needs a deliberate move. */}
          <button className="secondary-button secondary-button--compact" type="button" data-autofocus onClick={props.onCancel}>
            {t("settings.sources.cancel")}
          </button>
          <button
            className="secondary-button secondary-button--compact danger-button"
            type="button"
            disabled={props.busy}
            onClick={props.onConfirm}
          >
            {t("settings.catalogs.remove")}
          </button>
        </>
      }
    >
      <p id={describedBy}>{t("settings.catalogs.removeDescription")}</p>
    </SettingsDialog>
  );
}

type AddKind = "mcp_registry" | "git_tap";

/** Adds an MCP registry (any server of the MCP Registry API) or a Git tap. */
function AddCatalogDialog({ onClose }: { onClose: () => void }) {
  const { commands } = useTransport();
  const queryClient = useQueryClient();
  const { notify } = useNotifications();
  const formId = useId();
  const [kind, setKind] = useState<AddKind>("mcp_registry");
  const [url, setUrl] = useState("");
  const [id, setId] = useState("");
  const [gitRef, setGitRef] = useState("");
  const [title, setTitle] = useState("");
  const [error, setError] = useState("");

  const add = useMutation({
    mutationFn: async () =>
      unwrap(
        await commands.addCatalog(
          kind,
          url.trim(),
          id.trim() || null,
          kind === "git_tap" ? gitRef.trim() || null : null,
          title.trim() || null,
        ),
      ),
    onSuccess: (data: CatalogsOverview) => {
      queryClient.setQueryData(catalogQueryKeys.overview, data);
      notify({ tone: "success", message: t("settings.catalogs.added", { url: url.trim() }) });
      onClose();
    },
    onError: (failure) => setError(failureText(failure)),
  });

  return (
    <SettingsDialog
      title={t("settings.catalogs.addTitle")}
      closeLabel={t("trust.close")}
      dismissible={!add.isPending}
      onClose={onClose}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
            {t("settings.sources.cancel")}
          </button>
          <button
            className="primary-button primary-button--compact"
            type="submit"
            form={formId}
            disabled={add.isPending || !url.trim()}
          >
            {add.isPending ? t("settings.catalogs.adding") : t("settings.catalogs.addConfirm")}
          </button>
        </>
      }
    >
      <form
        id={formId}
        className="project-dialog__field"
        onSubmit={(event) => {
          event.preventDefault();
          setError("");
          add.mutate();
        }}
      >
        <fieldset className="trust-dialog__choices">
          <legend>{t("settings.catalogs.kind")}</legend>
          {(["mcp_registry", "git_tap"] as const).map((option) => (
            <label key={option} className={`trust-dialog__choice${kind === option ? " is-selected" : ""}`}>
              <input type="radio" name="catalog-kind" checked={kind === option} onChange={() => setKind(option)} />
              <span className="trust-dialog__choice-copy">
                <span>{t(CATALOG_KIND_LABELS[option])}</span>
                <span className="project-dialog__hint">
                  {t(option === "mcp_registry" ? "settings.catalogs.kind.registryHint" : "settings.catalogs.kind.tapHint")}
                </span>
              </span>
            </label>
          ))}
        </fieldset>
        <label className="project-dialog__field">
          <span>{t("settings.catalogs.url")}</span>
          <input
            className="tool-manage-dialog__search"
            type="text"
            spellCheck={false}
            autoComplete="off"
            placeholder={kind === "mcp_registry" ? "https://registry.example.com" : "https://github.com/acme/skills"}
            value={url}
            onChange={(event) => setUrl(event.target.value)}
          />
        </label>
        {kind === "git_tap" ? (
          <label className="project-dialog__field">
            <span>{t("settings.catalogs.ref.label")}</span>
            <input
              className="tool-manage-dialog__search"
              type="text"
              spellCheck={false}
              autoComplete="off"
              placeholder="main"
              value={gitRef}
              onChange={(event) => setGitRef(event.target.value)}
            />
          </label>
        ) : null}
        <label className="project-dialog__field">
          <span>{t("settings.catalogs.titleField")}</span>
          <input
            className="tool-manage-dialog__search"
            type="text"
            autoComplete="off"
            value={title}
            onChange={(event) => setTitle(event.target.value)}
          />
        </label>
        <label className="project-dialog__field">
          <span>{t("settings.catalogs.id")}</span>
          <input
            className="tool-manage-dialog__search"
            type="text"
            spellCheck={false}
            autoComplete="off"
            value={id}
            onChange={(event) => setId(event.target.value)}
          />
        </label>
        <span className="project-dialog__hint">{t("settings.catalogs.addHint")}</span>
        {error ? <div className="dialog-error" role="alert">{error}</div> : null}
      </form>
    </SettingsDialog>
  );
}
