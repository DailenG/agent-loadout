// Install > Browse (M45, #26): search the online catalogs the user turned
// on (M44) and hand an entry to the reviews that already exist. Nothing is
// requested before the user answers the one-time question about online
// catalogs; with them off, searches answer from what is saved on this
// computer and say so. Each catalog is searched on its own, so one slow or
// failing catalog never holds up the others. Catalog text is untrusted data:
// it is rendered as plain text, and no remote icon is ever loaded.

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { OfficialBadge } from "@/app/components/TrustDialog";
import { useNotifications } from "@/app/components/notifications";
import { t } from "@/i18n/en";
import type { CatalogEntry, CatalogInfo, CatalogResults, CatalogsOverview } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { failureText } from "../../skills/labels";
import { useReviewInstall } from "../useReviewInstall";
import {
  catalogQueryKeys,
  failureLine,
  formatDate,
  KIND_LABELS,
  pinningText,
  publisherText,
  statusLine,
  type StatusLine,
} from "./catalogText";
import { CatalogOrigin, EntryDetail } from "./EntryDetail";

export type BrowsePanelProps = {
  /** "View in Skills" on the success toast of a skill install. */
  onOpenSkills: () => void;
  /** Opens Settings, where Catalogs lives. */
  onOpenSettings?: () => void;
};

type Selected = { catalog: CatalogInfo; entry: CatalogEntry };

/** The overview (local: never contacts a catalog) and the online switch. */
export function useCatalogsOverview() {
  const { commands } = useTransport();
  const queryClient = useQueryClient();
  const { notify } = useNotifications();
  const overview = useQuery({
    queryKey: catalogQueryKeys.overview,
    queryFn: async () => unwrap(await commands.catalogsOverview()),
  });
  const setOnline = useMutation({
    mutationFn: async (on: boolean) => unwrap(await commands.setOnlineCatalogs(on)),
    onSuccess: (data: CatalogsOverview) => {
      queryClient.setQueryData(catalogQueryKeys.overview, data);
      // Results answered while off said "saved results only"; search again with the new setting.
      void queryClient.invalidateQueries({ queryKey: catalogQueryKeys.search });
      void queryClient.invalidateQueries({ queryKey: catalogQueryKeys.show });
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });
  return { overview, setOnline };
}

export function BrowsePanel({ onOpenSkills, onOpenSettings }: BrowsePanelProps) {
  const { overview, setOnline } = useCatalogsOverview();
  const [text, setText] = useState("");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<Selected | null>(null);
  const returnFocus = useRef<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  // Set when the button the user pressed goes away with the answer (the question, "Turn on").
  const [focusSearch, setFocusSearch] = useState(false);
  const searchId = useId();

  // Back from an entry: focus returns to the entry's Details button, else (the list is
  // loading again, for example after turning online catalogs on) to the search field.
  useEffect(() => {
    if (!selected && returnFocus.current) {
      const key = returnFocus.current;
      returnFocus.current = null;
      const button = document.querySelector<HTMLElement>(`[data-catalog-entry="${CSS.escape(key)}"]`);
      (button ?? searchRef.current)?.focus();
    }
  }, [selected]);

  useEffect(() => {
    if (focusSearch && searchRef.current) {
      searchRef.current.focus();
      setFocusSearch(false);
    }
  });

  const data = overview.data;

  function answer(on: boolean) {
    setOnline.mutate(on, { onSuccess: () => setFocusSearch(true) });
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    setQuery(text.trim());
  }

  function back() {
    if (selected) {
      returnFocus.current = selected.entry.ref;
    }
    setSelected(null);
  }

  let body;
  if (overview.isError) {
    body = <p className="dialog-error" role="alert">{failureText(overview.error)}</p>;
  } else if (!data) {
    body = <p className="catalog-muted" role="status">{t("catalog.loading")}</p>;
  } else if (!data.decided) {
    body = (
      <OptInCard
        contacts={data.contacts}
        busy={setOnline.isPending}
        onAnswer={answer}
      />
    );
  } else if (selected) {
    body = (
      <EntryDetail
        catalog={selected.catalog}
        entry={selected.entry}
        online={data.online}
        turningOn={setOnline.isPending}
        onTurnOn={() => setOnline.mutate(true)}
        onBack={back}
        onOpenSkills={onOpenSkills}
      />
    );
  } else {
    const searchable = data.catalogs.filter((catalog) => catalog.enabled && catalog.kind !== "link");
    const links = data.catalogs.filter((catalog) => catalog.enabled && catalog.kind === "link");
    body = (
      <>
        {data.online ? null : (
          <div className="catalog-notice" role="note">
            <p>{t("catalog.offNotice")}</p>
            <button
              className="secondary-button secondary-button--compact"
              type="button"
              disabled={setOnline.isPending}
              onClick={() => answer(true)}
            >
              {t("catalog.turnOn")}
            </button>
          </div>
        )}
        <form className="catalog-search" role="search" onSubmit={submit}>
          <label className="repo-form__field" htmlFor={searchId}>
            <span className="repo-form__label">{t("catalog.search.label")}</span>
          </label>
          <div className="catalog-search__row">
            <input
              ref={searchRef}
              id={searchId}
              type="search"
              spellCheck={false}
              autoComplete="off"
              placeholder={t("catalog.search.placeholder")}
              value={text}
              onChange={(event) => setText(event.target.value)}
            />
            <button className="primary-button" type="submit">
              {t("catalog.search.submit")}
            </button>
          </div>
          <p className="repo-form__hint-title">{t("catalog.search.hint")}</p>
        </form>
        {searchable.length === 0 && links.length === 0 ? (
          <p className="catalog-muted">{t("catalog.noneEnabled")}</p>
        ) : null}
        {searchable.map((catalog) => (
          <CatalogSection
            key={catalog.id}
            catalog={catalog}
            query={query}
            online={data.online}
            onOpen={(entry) => setSelected({ catalog, entry })}
          />
        ))}
        {links.map((catalog) => (
          <SkillsShCard
            key={catalog.id}
            catalog={catalog}
            online={data.online}
            turningOn={setOnline.isPending}
            onTurnOn={() => answer(true)}
            onOpenSkills={onOpenSkills}
          />
        ))}
      </>
    );
  }

  return (
    <section className="panel-card market-panel catalog-browse" aria-labelledby={`${searchId}-title`}>
      <div className="panel-header catalog-browse__header">
        <div>
          <h2 id={`${searchId}-title`}>{t("catalog.title")}</h2>
          <p>{t("catalog.description")}</p>
        </div>
        {onOpenSettings ? (
          <button className="secondary-button secondary-button--compact" type="button" onClick={onOpenSettings}>
            {t("catalog.manage")}
          </button>
        ) : null}
      </div>
      {body}
    </section>
  );
}

/** The one-time question: one plain sentence on what is contacted, then yes or not now. */
function OptInCard(props: { contacts: string[]; busy: boolean; onAnswer: (on: boolean) => void }) {
  const titleId = useId();
  const hosts = props.contacts.length > 0 ? props.contacts.join(", ") : t("catalog.optIn.noHosts");
  return (
    <section className="catalog-opt-in" aria-labelledby={titleId}>
      <h3 id={titleId}>{t("catalog.optIn.title")}</h3>
      <p>{t("catalog.optIn.sentence", { hosts })}</p>
      <p className="catalog-muted">{t("catalog.optIn.later")}</p>
      <div className="catalog-opt-in__actions">
        <button className="secondary-button" type="button" disabled={props.busy} onClick={() => props.onAnswer(false)}>
          {t("catalog.optIn.no")}
        </button>
        <button className="primary-button" type="button" disabled={props.busy} onClick={() => props.onAnswer(true)}>
          {t("catalog.optIn.yes")}
        </button>
      </div>
    </section>
  );
}

type SectionProps = {
  catalog: CatalogInfo;
  query: string;
  online: boolean;
  onOpen: (entry: CatalogEntry) => void;
};

/** One catalog's results and status; searched on its own. */
function CatalogSection({ catalog, query, online, onOpen }: SectionProps) {
  const { commands } = useTransport();
  const queryClient = useQueryClient();
  const headingId = useId();
  const search = useInfiniteQuery({
    // The address is in the key: a catalog removed and added again under its id (or moved by a store pull)
    // never shows the old address's results.
    queryKey: [...catalogQueryKeys.search, catalog.id, catalog.url, catalog.gitRef, query, online],
    queryFn: async ({ pageParam }) => {
      const results = unwrap(await commands.searchCatalog(catalog.id, query, pageParam));
      // A later page that failed (or has nothing saved while off) is not a page: it fails, so the
      // pages so far stay, the cursor is kept and the user can load it again.
      if (pageParam !== null && results.status === "error") {
        throw new IpcFailure(results.error ?? { code: "catalog_unreachable", message: "" });
      }
      if (pageParam !== null && results.status === "off" && results.entries.length === 0) {
        throw new IpcFailure({ code: "catalogs_off", message: "" });
      }
      return results;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last: CatalogResults) => last.nextCursor ?? undefined,
    // Network only on user action: a mount or remount reads what was answered.
    staleTime: Infinity,
  });
  const refresh = useMutation({
    mutationFn: async () => unwrap(await commands.refreshCatalog(catalog.id)),
    onSettled: () => queryClient.resetQueries({ queryKey: [...catalogQueryKeys.search, catalog.id] }),
  });
  const { notify } = useNotifications();
  const openHelp = useMutation({
    mutationFn: async (topic: string) => unwrap(await commands.openHelp(topic)),
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  const pages = search.data?.pages ?? [];
  const first = pages[0];
  const entries = pages.flatMap((page) => page.entries);
  const attribution = first?.attribution ?? catalog.attribution;
  // A failed later page keeps the pages so far; only a failed first search is the catalog's status.
  const searchFailed = search.isError && !search.isFetchNextPageError;
  let line: StatusLine;
  if (searchFailed) {
    line = failureLine(catalog.title, search.error);
  } else if (search.isPending || refresh.isPending || !first) {
    line = { tone: "neutral", text: t("catalog.status.loading", { catalog: catalog.title }) };
  } else {
    line = statusLine(catalog.title, { ...first, entries });
  }
  const canRetry =
    online && !search.isPending && !refresh.isPending && (searchFailed || first?.status === "stale" || first?.status === "error");

  return (
    <section className="catalog-section" aria-labelledby={headingId}>
      <div className="catalog-section__head">
        <h3 id={headingId}>{catalog.title}</h3>
        {catalog.official ? <OfficialBadge /> : null}
        <p className={`catalog-status catalog-status--${line.tone}`} role="status">
          {line.text}
        </p>
        {line.help === "git_missing" ? (
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            aria-label={t("catalog.gitMissingHelpAria")}
            onClick={() => openHelp.mutate("git_missing")}
          >
            {t("catalog.gitMissingHelp")}
          </button>
        ) : null}
        {canRetry ? (
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            aria-label={t("catalog.retryAria", { catalog: catalog.title })}
            onClick={() => refresh.mutate()}
          >
            {t("catalog.retry")}
          </button>
        ) : null}
      </div>
      {attribution ? <p className="catalog-attribution">{attribution}</p> : null}
      {entries.length > 0 ? (
        <ul className="catalog-results" aria-label={t("catalog.resultsAria", { catalog: catalog.title })}>
          {entries.map((entry) => (
            <ResultCard key={entry.ref} entry={entry} onOpen={() => onOpen(entry)} />
          ))}
        </ul>
      ) : null}
      {search.isFetchNextPageError ? (
        <div className="catalog-section__more-failed">
          <p className="catalog-status catalog-status--danger" role="alert">
            {t("catalog.moreFailed", { reason: failureLine(catalog.title, search.error).text })}
          </p>
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            aria-label={t("catalog.moreRetryAria", { catalog: catalog.title })}
            disabled={search.isFetchingNextPage}
            onClick={() => void search.fetchNextPage()}
          >
            {t("catalog.retry")}
          </button>
        </div>
      ) : search.hasNextPage ? (
        <button
          className="secondary-button secondary-button--compact catalog-section__more"
          type="button"
          disabled={search.isFetchingNextPage}
          onClick={() => void search.fetchNextPage()}
        >
          {search.isFetchingNextPage ? t("catalog.loadingMore") : t("catalog.more", { catalog: catalog.title })}
        </button>
      ) : null}
    </section>
  );
}

function ResultCard({ entry, onOpen }: { entry: CatalogEntry; onOpen: () => void }) {
  const titleId = useId();
  const publisher = publisherText(entry);
  return (
    <li className="catalog-result" aria-labelledby={titleId}>
      <div className="catalog-result__head">
        <strong id={titleId}>{entry.name}</strong>
        <span className="status-badge tone-neutral">{t(KIND_LABELS[entry.kind])}</span>
        {entry.official ? <OfficialBadge /> : null}
        {entry.deprecated ? <span className="status-badge tone-warning">{t("catalog.deprecated")}</span> : null}
        {entry.problem ? <span className="status-badge tone-danger">{t("catalog.notInstallable")}</span> : null}
      </div>
      {entry.name !== entry.id ? <code className="catalog-result__id">{entry.id}</code> : null}
      {entry.description ? <p className="catalog-result__description">{entry.description}</p> : null}
      <p className="catalog-result__meta">
        {[
          publisher,
          entry.version ? t("catalog.versionShort", { version: entry.version }) : null,
          pinningText(entry),
          entry.updatedAt ? t("catalog.updated", { date: formatDate(entry.updatedAt) }) : null,
        ]
          .filter(Boolean)
          .join(" · ")}
      </p>
      {entry.warning ? <p className="dialog-warning" role="note">{entry.warning}</p> : null}
      <div className="catalog-result__actions">
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          data-catalog-entry={entry.ref}
          aria-label={t("catalog.detailsAria", { name: entry.name })}
          onClick={onOpen}
        >
          {t("catalog.details")}
        </button>
      </div>
    </li>
  );
}

const SKILLS_SH = /^https:\/\/(www\.)?skills\.sh\/(.+)$/i;

type SkillsShCardProps = {
  catalog: CatalogInfo;
  online: boolean;
  turningOn: boolean;
  onTurnOn: () => void;
  onOpenSkills: () => void;
};

/**
 * skills.sh has no search Loadout can use (research: its API needs a Vercel
 * deployment credential), so it is a link-out: open it in the browser, then
 * paste a skill's address here for the usual git skill review.
 */
function SkillsShCard({ catalog, online, turningOn, onTurnOn, onOpenSkills }: SkillsShCardProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const { review, dialog } = useReviewInstall(onOpenSkills);
  const [url, setUrl] = useState("");
  const [problem, setProblem] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const titleId = useId();
  const fieldId = useId();
  const problemId = useId();

  async function open() {
    try {
      unwrap(await commands.openCatalogLink(catalog.id, null, "catalog"));
    } catch (failure) {
      notify({ tone: "error", message: failureText(failure) });
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const address = url.trim();
    const match = SKILLS_SH.exec(address);
    if (!match || match[2].split("/").filter(Boolean).length < 3) {
      setProblem(t("catalog.skillsSh.badUrl"));
      return;
    }
    if (match[2].toLowerCase().startsWith("site/")) {
      setProblem(t("catalog.skillsSh.siteUrl"));
      return;
    }
    setProblem("");
    setIsLoading(true);
    await review(() => commands.reviewGitSkill(address, null, null), null, () => setUrl(""), (
      <CatalogOrigin title={catalog.title} reference={address} />
    ));
    setIsLoading(false);
  }

  return (
    <section className="catalog-section catalog-link-card" aria-labelledby={titleId}>
      <div className="catalog-section__head">
        <h3 id={titleId}>{catalog.title}</h3>
        <span className="status-badge tone-neutral">{t("catalog.catalogKind.link")}</span>
      </div>
      <p>{t("catalog.skillsSh.explain")}</p>
      <div className="catalog-result__actions">
        <button className="secondary-button secondary-button--compact" type="button" onClick={() => void open()}>
          {t("catalog.skillsSh.open", { catalog: catalog.title })}
        </button>
      </div>
      {online ? (
        <form className="catalog-search" onSubmit={(event) => void submit(event)}>
          <label className="repo-form__field" htmlFor={fieldId}>
            <span className="repo-form__label">{t("catalog.skillsSh.label")}</span>
          </label>
          <div className="catalog-search__row">
            <input
              id={fieldId}
              type="text"
              spellCheck={false}
              autoComplete="off"
              placeholder="https://skills.sh/anthropics/skills/pdf"
              aria-invalid={problem ? true : undefined}
              aria-describedby={problem ? problemId : undefined}
              value={url}
              onChange={(event) => {
                setUrl(event.target.value);
                setProblem("");
              }}
            />
            <button className="primary-button" type="submit" disabled={!url.trim() || isLoading}>
              {isLoading ? t("install.mcpSource.reviewing") : t("catalog.skillsSh.review")}
            </button>
          </div>
          {problem ? (
            <p id={problemId} className="dialog-error" role="alert">
              {problem}
            </p>
          ) : null}
        </form>
      ) : (
        // Installing clones the skill's repository: the same gate as every other Browse install.
        <div className="catalog-notice" role="note">
          <p>{t("catalog.install.needsOnline")}</p>
          <button className="secondary-button secondary-button--compact" type="button" disabled={turningOn} onClick={onTurnOn}>
            {t("catalog.turnOn")}
          </button>
        </div>
      )}
      {dialog}
    </section>
  );
}
