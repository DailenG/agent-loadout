// One catalog entry in full (M45): everything the catalog says about it and
// the install handoff. Install never takes a shortcut: an MCP server goes to
// the M41 review (drafts, trust, scan, client picker) shown right here, a
// skill to the git skill install review; the backend records the catalog as
// provenance. A plugin from a source Loadout can't install says why and
// offers no install.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { OfficialBadge } from "@/app/components/TrustDialog";
import { useNotifications } from "@/app/components/notifications";
import { t } from "@/i18n/en";
import type { CatalogEntry, CatalogInfo, McpSourceReview } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";
import { failureText } from "../../skills/labels";
import { McpSourceReviewView, useDiscardOnLeave } from "../McpSourcePanel";
import { useReviewInstall } from "../useReviewInstall";
import {
  CATALOG_KIND_LABELS,
  catalogQueryKeys,
  formatAge,
  formatDate,
  KIND_LABELS,
  pinningText,
  publisherText,
  detailStatus,
} from "./catalogText";

/** "From the catalog <title>: catalog:<source>/<id>", the provenance a review carries. */
export function CatalogOrigin({ title, reference }: { title: string; reference: string }) {
  return (
    <p className="catalog-origin" role="note">
      {t("catalog.origin", { catalog: title })} <code>{reference}</code>
    </p>
  );
}

export type EntryDetailProps = {
  catalog: CatalogInfo;
  /** The entry as the search listed it; the full one is read with `show_catalog_entry`. */
  entry: CatalogEntry;
  online: boolean;
  turningOn: boolean;
  onTurnOn: () => void;
  onBack: () => void;
  onOpenSkills: () => void;
};

export function EntryDetail({ catalog, entry: listed, online, turningOn, onTurnOn, onBack, onOpenSkills }: EntryDetailProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const headingRef = useRef<HTMLHeadingElement>(null);
  const skillGroup = useId();
  const servers = useQuery({
    queryKey: storeQueryKeys.servers,
    queryFn: async () => unwrap(await commands.listServers()),
  });
  const shown = useQuery({
    queryKey: [...catalogQueryKeys.show, catalog.id, catalog.url, catalog.gitRef, listed.id, online],
    queryFn: async () => unwrap(await commands.showCatalogEntry(catalog.id, listed.id)),
    staleTime: Infinity,
  });
  const entry = shown.data?.entry ?? listed;
  const [mcpReview, setMcpReview] = useState<McpSourceReview | null>(null);
  const tokenRef = useDiscardOnLeave(mcpReview);
  const { review: reviewSkill, dialog: skillDialog } = useReviewInstall(onOpenSkills);
  const [skill, setSkill] = useState<string | null>(null);
  const [busy, setBusy] = useState<"mcp" | "skill" | null>(null);
  const [saving, setSaving] = useState(false);
  const installHeadingRef = useRef<HTMLHeadingElement>(null);
  const mounted = useRef(true);
  const wasOnline = useRef(online);

  useEffect(() => {
    mounted.current = true;
    headingRef.current?.focus();
    return () => {
      mounted.current = false;
    };
  }, []);

  // Turned on from here: the note and its button are gone; focus goes to what replaced them.
  useEffect(() => {
    if (online && !wasOnline.current) {
      installHeadingRef.current?.focus();
    }
    wasOnline.current = online;
  }, [online]);

  // A plugin with several skills needs a pick; one skill is the obvious pick.
  const skillChoices = entry.kind === "plugin" ? entry.skills : [];
  const pickedSkill = skillChoices.length === 1 ? skillChoices[0] : skill;
  const needsPick = skillChoices.length > 1 && pickedSkill === null;
  // A picked skill is reviewed (and recorded by the backend) as its own entry of the catalog.
  const reference = entry.kind === "plugin" && pickedSkill ? `catalog:${catalog.id}/${pickedSkill}` : entry.ref;
  const origin = <CatalogOrigin title={catalog.title} reference={reference} />;

  async function reviewMcp() {
    if (mcpReview) {
      void commands.discardMcpSourceReview(mcpReview.token);
      setMcpReview(null);
    }
    setBusy("mcp");
    try {
      const review = unwrap(await commands.reviewCatalogMcp(catalog.id, entry.id, null));
      if (!mounted.current) {
        // Finished after the view went away: drop it with its temporary clone.
        void commands.discardMcpSourceReview(review.token);
        return;
      }
      setMcpReview(review);
    } catch (failure) {
      if (mounted.current) {
        notify({ tone: "error", message: failureText(failure) });
      }
    } finally {
      if (mounted.current) {
        setBusy(null);
      }
    }
  }

  async function reviewSkillEntry() {
    setBusy("skill");
    await reviewSkill(
      () => commands.reviewCatalogSkill(catalog.id, entry.id, entry.kind === "plugin" ? pickedSkill : null),
      null,
      () => undefined,
      origin,
    );
    if (mounted.current) {
      setBusy(null);
    }
  }

  async function openLink(link: "repository" | "website") {
    try {
      unwrap(await commands.openCatalogLink(catalog.id, entry.id, link));
    } catch (failure) {
      notify({ tone: "error", message: failureText(failure) });
    }
  }

  const status = shown.data ? detailStatus(catalog.title, shown.data) : null;
  const publisher = publisherText(entry);
  const canInstall = entry.problem === null && (entry.installMcp || entry.installSkill);

  return (
    <article className="catalog-detail" aria-labelledby={`${skillGroup}-title`}>
      <button
        className="secondary-button secondary-button--compact catalog-detail__back"
        type="button"
        disabled={saving || busy !== null}
        onClick={onBack}
      >
        {t("catalog.back")}
      </button>
      <div className="catalog-detail__head">
        <h3 id={`${skillGroup}-title`} ref={headingRef} tabIndex={-1}>
          {entry.name}
        </h3>
        <span className="status-badge tone-neutral">{t(KIND_LABELS[entry.kind])}</span>
        {entry.official ? <OfficialBadge /> : null}
        {entry.deprecated ? <span className="status-badge tone-warning">{t("catalog.deprecated")}</span> : null}
      </div>
      {shown.isPending ? <p className="catalog-muted" role="status">{t("catalog.detail.loading")}</p> : null}
      {shown.isError ? <p className="catalog-status catalog-status--danger" role="status">{failureText(shown.error)}</p> : null}
      {status ? <p className={`catalog-status catalog-status--${status.tone}`} role="status">{status.text}</p> : null}
      {entry.description ? <p className="catalog-detail__description">{entry.description}</p> : null}
      {entry.deprecated ? (
        <>
          <p className="dialog-warning" role="note">
            {t("catalog.deprecatedNote")}
          </p>
          {entry.statusMessage ? (
            // The publisher's own words: quoted and attributed, never worded as Loadout's.
            <p className="catalog-publisher-says">
              {t("catalog.publisherSays")} <q>{entry.statusMessage}</q>
            </p>
          ) : null}
        </>
      ) : null}
      {entry.warning ? <p className="dialog-warning" role="note">{entry.warning}</p> : null}
      <dl className="catalog-facts">
        <Fact label={t("catalog.fact.source")}>
          {catalog.title} ({t(CATALOG_KIND_LABELS[catalog.kind])})
        </Fact>
        <Fact label={t("catalog.fact.id")}>
          <code>{entry.id}</code>
        </Fact>
        {publisher ? <Fact label={t("catalog.fact.publisher")}>{publisher}</Fact> : null}
        {entry.version ? <Fact label={t("catalog.fact.version")}><code>{entry.version}</code></Fact> : null}
        <Fact label={t("catalog.fact.pinning")}>{pinningText(entry)}</Fact>
        {entry.updatedAt ? <Fact label={t("catalog.fact.updated")}>{formatDate(entry.updatedAt)}</Fact> : null}
        {entry.transports.length > 0 ? (
          <Fact label={t("catalog.fact.transports")}>{entry.transports.join(", ")}</Fact>
        ) : null}
        {entry.kind !== "skill" ? (
          <Fact label={t("catalog.fact.secrets")}>
            {entry.requiredSecrets.length > 0 ? (
              <span className="catalog-chips">
                {entry.requiredSecrets.map((name) => (
                  <code key={name}>{name}</code>
                ))}
              </span>
            ) : (
              t("catalog.fact.secretsNone")
            )}
          </Fact>
        ) : null}
        {entry.license ? <Fact label={t("catalog.fact.license")}>{entry.license}</Fact> : null}
        {entry.path ? <Fact label={t("catalog.fact.path")}><code>{entry.path}</code></Fact> : null}
        {entry.commit ? <Fact label={t("catalog.fact.commit")}><code>{entry.commit.slice(0, 7)}</code></Fact> : null}
        {entry.repository ? (
          <Fact label={t(catalog.kind === "mcp_registry" ? "catalog.fact.repositoryClaimed" : "catalog.fact.repository")}>
            <span className="catalog-link">
              <code>{entry.repository}</code>
              {/* Only https addresses open; others (http, ssh) stay text. */}
              {entry.repository.toLowerCase().startsWith("https://") ? (
                <button
                  className="secondary-button secondary-button--compact"
                  type="button"
                  aria-label={t("catalog.openRepositoryAria", { name: entry.name })}
                  onClick={() => void openLink("repository")}
                >
                  {t("catalog.open")}
                </button>
              ) : null}
            </span>
          </Fact>
        ) : null}
        {entry.website ? (
          <Fact label={t("catalog.fact.website")}>
            <span className="catalog-link">
              <code>{entry.website}</code>
              {entry.website.toLowerCase().startsWith("https://") ? (
                <button
                  className="secondary-button secondary-button--compact"
                  type="button"
                  aria-label={t("catalog.openWebsiteAria", { name: entry.name })}
                  onClick={() => void openLink("website")}
                >
                  {t("catalog.open")}
                </button>
              ) : null}
            </span>
          </Fact>
        ) : null}
      </dl>
      {entry.packages.length > 0 ? (
        <section className="catalog-detail__list" aria-label={t("catalog.packages")}>
          <h4>{t("catalog.packages")}</h4>
          <ul>
            {entry.packages.map((item) => (
              <li key={`${item.registryType}:${item.identifier}`}>
                <code>
                  {item.registryType} {item.identifier}
                  {item.version ? `@${item.version}` : ""}
                </code>
                {item.transport ? <span>{item.transport}</span> : null}
                {item.pinned ? null : <span className="status-badge tone-warning">{t("catalog.pin.none")}</span>}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {entry.remotes.length > 0 ? (
        <section className="catalog-detail__list" aria-label={t("catalog.remotes")}>
          <h4>{t("catalog.remotes")}</h4>
          <ul>
            {entry.remotes.map((remote) => (
              <li key={`${remote.transport}:${remote.url}`}>
                <code>{remote.url}</code>
                <span>{remote.transport}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {entry.kind === "plugin" ? (
        <section className="catalog-detail__list" aria-label={t("catalog.plugin.contents")}>
          <h4>{t("catalog.plugin.contents")}</h4>
          <ul>
            <li>{entry.skills.length > 0 ? t("catalog.plugin.skills", { names: entry.skills.join(", ") }) : t("catalog.plugin.noSkills")}</li>
            <li>
              {entry.servers.length > 0
                ? t("catalog.plugin.servers", { names: entry.servers.join(", ") })
                : entry.source?.kind === "relative"
                  ? t("catalog.plugin.noServers")
                  : t("catalog.plugin.serversUnknown")}
            </li>
            {entry.notImported.length > 0 ? (
              <li>{t("catalog.plugin.notImported", { parts: entry.notImported.join(", ") })}</li>
            ) : null}
          </ul>
        </section>
      ) : null}
      {catalog.attribution ? <p className="catalog-attribution">{catalog.attribution}</p> : null}

      <section className="catalog-install" aria-labelledby={`${skillGroup}-install`}>
        <h4 id={`${skillGroup}-install`} ref={installHeadingRef} tabIndex={-1}>
          {t("catalog.install.title")}
        </h4>
        {entry.problem ? (
          <p className="dialog-error" role="note">
            {t("catalog.install.refused", { reason: entry.problem })}
          </p>
        ) : !canInstall ? (
          <p className="catalog-muted">{t("catalog.install.nothing")}</p>
        ) : !online ? (
          <div className="catalog-notice" role="note">
            <p>{t("catalog.install.needsOnline")}</p>
            <button className="secondary-button secondary-button--compact" type="button" disabled={turningOn} onClick={onTurnOn}>
              {t("catalog.turnOn")}
            </button>
          </div>
        ) : (
          <>
            <p className="catalog-muted">{t("catalog.install.explain")}</p>
            {entry.installSkill && skillChoices.length > 1 ? (
              <fieldset className="catalog-skill-pick">
                <legend>{t("catalog.install.pickSkill")}</legend>
                {skillChoices.map((name) => (
                  <label key={name} className={`trust-dialog__choice${pickedSkill === name ? " is-selected" : ""}`}>
                    <input
                      type="radio"
                      name={`${skillGroup}-skill`}
                      checked={pickedSkill === name}
                      onChange={() => setSkill(name)}
                    />
                    <span>{name}</span>
                  </label>
                ))}
              </fieldset>
            ) : null}
            <div className="catalog-result__actions">
              {entry.installSkill ? (
                <button
                  className="primary-button"
                  type="button"
                  disabled={busy !== null || saving || needsPick}
                  onClick={() => void reviewSkillEntry()}
                >
                  {busy === "skill"
                    ? t("install.mcpSource.reviewing")
                    : pickedSkill && entry.kind === "plugin"
                      ? t("catalog.install.skillNamed", { name: pickedSkill })
                      : t("catalog.install.skill")}
                </button>
              ) : null}
              {entry.installMcp ? (
                <button
                  className={entry.installSkill ? "secondary-button" : "primary-button"}
                  type="button"
                  disabled={busy !== null || saving}
                  onClick={() => void reviewMcp()}
                >
                  {busy === "mcp" ? t("install.mcpSource.reviewing") : t("catalog.install.mcp")}
                </button>
              ) : null}
            </div>
          </>
        )}
      </section>
      {mcpReview ? (
        <McpSourceReviewView
          key={mcpReview.token}
          review={mcpReview}
          clients={servers.data?.clients ?? []}
          origin={origin}
          onDiscard={() => {
            void commands.discardMcpSourceReview(mcpReview.token);
            setMcpReview(null);
          }}
          onSavingChange={setSaving}
          onSaved={() => {
            // Saved: the backend consumed the review.
            setSaving(false);
            tokenRef.current = null;
            setMcpReview(null);
          }}
        />
      ) : null}
      {skillDialog}
      {shown.data?.ageSecs != null && shown.data.status === "cached" ? (
        <p className="catalog-muted">{t("catalog.detail.savedAge", { age: formatAge(shown.data.ageSecs) })}</p>
      ) : null}
    </article>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}
