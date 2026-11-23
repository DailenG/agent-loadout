// The import view's MCP servers (#3, SPEC 8.6): every foreign entry of the
// client files, user and known projects', grouped by name. Identical entries
// are one row with their sources; a conflict shows its definitions side by
// side with a choice (one, all under their own ids, or skip). Each literal
// secret gets a destination (OS keyring by default, a 1Password reference,
// an environment variable, or skip the server); no value is ever shown or
// sent. Review, then one apply.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { orderByRecommended } from "@/app/clients";
import { AppSelect, type AppSelectOption } from "@/app/components/AppSelect";
import { projectRelativePath } from "@/app/routes/projects/labels";
import { useProjectList } from "@/app/scope/ActiveScope";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type { FileScope, ImportCandidate, ImportVariant } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { ImportReviewDialog } from "./ImportReviewDialog";
import {
  answerOf,
  choiceOf,
  chosenSecretFields,
  chosenVariants,
  editKey,
  idOf,
  importableVariants,
  matchesFilter,
  IMPORT_SCAN_KEY,
  planImport,
  SCAN_ALL,
  secretProblem,
  sharedSecretFields,
  type CandidateChoice,
  type ImportNames,
  type ImportState,
  type ImportTarget,
  type SecretAnswer,
  type SourceFilter,
} from "./model";

const SECRET_CHOICES: AppSelectOption<SecretAnswer["choice"]>[] = [
  { value: "keyring", label: t("import.secret.keyring") },
  { value: "onepassword", label: t("import.secret.onepassword") },
  { value: "env", label: t("import.secret.env") },
  { value: "skip", label: t("import.secret.skip") },
];

/** A field both chosen definitions hold: one reference can't stand for two values. */
const SHARED_SECRET_CHOICES = SECRET_CHOICES.filter((option) => option.value === "keyring" || option.value === "skip");

const SCOPE_KEYS = {
  user: "import.source.user",
  project: "import.source.project",
  project_local: "import.source.projectLocal",
} as const satisfies Record<FileScope, MessageKey>;

function failureText(error: unknown): string {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

/** "omp, Cursor in web-shop": who has this definition. */
export function variantLabel(variant: ImportVariant, names: ImportNames): string {
  const parts = orderByRecommended(variant.sources, (source) => source.client).map((source) => {
    const client = names.client(source.client);
    if (source.scope === "user") {
      return client;
    }
    return t("import.variant.inProject", { client, project: names.project(source.projectId)?.name ?? source.projectId ?? "" });
  });
  return [...new Set(parts)].join(", ");
}

/** The command line or URL as it would be stored (secrets are `${secret:…}` already). */
export function launchOf(variant: ImportVariant): string {
  const draft = variant.draft;
  if (!draft) {
    return "";
  }
  return draft.url ?? [draft.command ?? "", ...draft.args].join(" ").trim();
}

function VariantCard(props: { candidate: ImportCandidate; variant: ImportVariant; names: ImportNames }) {
  const { variant, names } = props;
  const draft = variant.draft;
  const keys = [...(draft?.env ?? []), ...(draft?.headers ?? [])].map((row) => row.key);
  return (
    <div className="import-variant">
      <div className="import-variant__head">
        <strong>{variantLabel(variant, names)}</strong>
        {draft ? <span className="status-badge tone-neutral">{t(`mcp.transport.${draft.transport}` as MessageKey)}</span> : null}
        {draft && !variant.enables ? (
          <span className="status-badge tone-warning" data-tooltip={t("import.variant.definitionOnlyHint")}>
            {t("import.variant.definitionOnly")}
          </span>
        ) : null}
      </div>
      {draft ? <code className="import-variant__launch">{launchOf(variant)}</code> : null}
      {keys.length > 0 ? <p className="import-variant__note">{t("import.variant.keys", { keys: keys.join(", ") })}</p> : null}
      {variant.error ? (
        <p className="import-variant__error" role="note">
          {errorMessage(variant.error.code)}
        </p>
      ) : null}
      <ul className="import-variant__sources" aria-label={t("import.variant.sourcesAria")}>
        {orderByRecommended(variant.sources, (source) => source.client).map((source) => {
          const project = names.project(source.projectId);
          return (
            <li key={source.key}>
              <span>
                {names.client(source.client)} · {t(SCOPE_KEYS[source.scope], { project: project?.name ?? source.projectId ?? "" })}
              </span>
              <span className="import-variant__file" title={source.file}>
                {projectRelativePath(source.file, project?.path)}
              </span>
              {source.trusted === false ? (
                <span className="status-badge tone-warning">{t("import.source.untrusted")}</span>
              ) : null}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

type CandidateCardProps = {
  candidate: ImportCandidate;
  choice: CandidateChoice;
  state: ImportState;
  names: ImportNames;
  idProblems: Map<string, MessageKey>;
  focusRef?: (element: HTMLElement | null) => void;
  onChoice: (choice: CandidateChoice) => void;
  onId: (variant: ImportVariant, id: string) => void;
  onSecret: (field: string, answer: SecretAnswer) => void;
};

function CandidateCard({ candidate, choice, state, names, idProblems, focusRef, onChoice, onId, onSecret }: CandidateCardProps) {
  const importable = importableVariants(candidate);
  const chosen = chosenVariants(candidate, choice);
  const fields = chosenSecretFields(chosen);
  const shared = sharedSecretFields(chosen);
  const identical = candidate.variants.length === 1 && candidate.variants[0].sources.length > 1;
  const name = candidate.name;
  return (
    <article ref={focusRef} tabIndex={-1} className="import-candidate panel-card" aria-label={name}>
      <div className="import-candidate__head">
        <strong className="import-candidate__name">{name}</strong>
        {identical ? (
          <span className="status-badge tone-positive">
            {t("import.candidate.identical", { count: candidate.variants[0].sources.length })}
          </span>
        ) : null}
        {candidate.conflict ? (
          <span className="status-badge tone-warning">{t("import.candidate.conflict", { count: importable.length })}</span>
        ) : null}
        {candidate.inStore ? <span className="status-badge tone-neutral">{t("import.candidate.inStore")}</span> : null}
        {importable.length === 0 ? (
          <span className="status-badge tone-neutral">{t("import.candidate.notImportable")}</span>
        ) : null}
        {importable.length > 0 && !candidate.conflict ? (
          <label className="import-candidate__include">
            <input
              type="checkbox"
              checked={choice.mode !== "skip"}
              onChange={(event) =>
                onChoice({ mode: event.target.checked ? "pick" : "skip", variant: importable[0].key })}
            />
            {t("import.candidate.include", { name })}
          </label>
        ) : null}
      </div>
      {candidate.inStore ? <p className="import-candidate__note">{t("import.candidate.inStoreHint")}</p> : null}

      {candidate.conflict ? (
        <fieldset className="import-candidate__choice">
          <legend>{t("import.choice.legend", { name })}</legend>
          {importable.map((variant) => (
            <label key={variant.key}>
              <input
                type="radio"
                name={`import-choice-${name}`}
                checked={choice.mode === "pick" && choice.variant === variant.key}
                onChange={() => onChoice({ mode: "pick", variant: variant.key })}
              />
              {t("import.choice.pick", { label: variantLabel(variant, names) })}
            </label>
          ))}
          <label>
            <input
              type="radio"
              name={`import-choice-${name}`}
              checked={choice.mode === "both"}
              onChange={() => onChoice({ mode: "both", variant: choice.variant })}
            />
            {t(importable.length === 2 ? "import.choice.both" : "import.choice.all")}
          </label>
          <label>
            <input
              type="radio"
              name={`import-choice-${name}`}
              checked={choice.mode === "skip"}
              onChange={() => onChoice({ mode: "skip", variant: choice.variant })}
            />
            {t("import.choice.skip")}
          </label>
        </fieldset>
      ) : null}

      <div className={`import-candidate__variants${candidate.variants.length > 1 ? " is-side-by-side" : ""}`}>
        {candidate.variants.map((variant) => (
          <VariantCard key={variant.key} candidate={candidate} variant={variant} names={names} />
        ))}
      </div>

      {chosen.map((variant) => {
        const key = editKey(name, variant.key);
        const problem = idProblems.get(key);
        const inputId = `import-id-${key.replace(/\W/g, "-")}`;
        return (
          <div key={variant.key} className="import-candidate__id">
            <label htmlFor={inputId}>
              {chosen.length > 1
                ? t("import.id.labelFor", { label: variantLabel(variant, names) })
                : t("import.id.label")}
            </label>
            <input
              id={inputId}
              type="text"
              spellCheck={false}
              autoComplete="off"
              aria-invalid={problem ? true : undefined}
              aria-describedby={problem ? `${inputId}-problem` : undefined}
              value={idOf(state, candidate, variant, choice.mode)}
              onChange={(event) => onId(variant, event.target.value)}
            />
            {problem ? (
              <span id={`${inputId}-problem`} className="import-candidate__problem">
                {t(problem)}
              </span>
            ) : null}
          </div>
        );
      })}

      {fields.length > 0 ? (
        <div className="import-candidate__secrets" role="group" aria-label={t("import.secret.groupAria", { name })}>
          <p className="import-candidate__note">{t("import.secret.intro")}</p>
          <ul className="migrate-secrets">
            {fields.map((field) => {
              const isShared = shared.has(field);
              const answer = answerOf(state, name, field, isShared);
              const problem = secretProblem(answer);
              const alias = chosen
                .flatMap((variant) =>
                  variant.secrets
                    .filter((secret) => secret.field === field && idOf(state, candidate, variant, choice.mode) === variant.suggestedId)
                    .map((secret) => secret.suggestedAlias))[0];
              const label = `${name} ${field}`;
              return (
                <li key={field} className="migrate-secret">
                  <div className="migrate-secret__copy">
                    <strong>{field}</strong>
                    <span>{alias ? t("import.secret.alias", { alias }) : t("import.secret.aliasLater")}</span>
                    <span className="migrate-secret__masked" aria-hidden="true">••••••••</span>
                  </div>
                  <AppSelect
                    className="migrate-secret__choice"
                    value={answer.choice}
                    options={isShared ? SHARED_SECRET_CHOICES : SECRET_CHOICES}
                    ariaLabel={t("import.secret.choiceAria", { field: label })}
                    onChange={(next) => onSecret(field, { choice: next, text: "" })}
                  />
                  {answer.choice === "onepassword" ? (
                    <input
                      className="migrate-secret__input"
                      type="text"
                      spellCheck={false}
                      aria-label={t("import.secret.refAria", { field: label })}
                      placeholder="op://vault/item/field"
                      value={answer.text}
                      onChange={(event) => onSecret(field, { choice: "onepassword", text: event.target.value })}
                    />
                  ) : answer.choice === "env" ? (
                    <input
                      className="migrate-secret__input"
                      type="text"
                      spellCheck={false}
                      aria-label={t("import.secret.envAria", { field: label })}
                      placeholder="NAME"
                      value={answer.text}
                      onChange={(event) => onSecret(field, { choice: "env", text: event.target.value })}
                    />
                  ) : (
                    <span className="migrate-secret__later">
                      {t(answer.choice === "skip" ? "import.secret.skipHint" : "import.secret.keyringHint")}
                    </span>
                  )}
                  {isShared ? <span className="migrate-secret__later import-secret__shared">{t("import.secret.sharedNote")}</span> : null}
                  {problem ? (
                    <span className="migrate-secret__problem" role="alert">
                      {t(problem)}
                    </span>
                  ) : null}
                </li>
              );
            })}
          </ul>
        </div>
      ) : null}
    </article>
  );
}

export type McpImportSectionProps = {
  /** An entry an "Import…" button points at: it alone starts picked. */
  target: ImportTarget | null;
  onOpenPending: () => void;
};

export function McpImportSection({ target, onOpenPending }: McpImportSectionProps) {
  const { commands } = useTransport();
  const scan = useQuery({
    queryKey: IMPORT_SCAN_KEY,
    queryFn: async () => unwrap(await commands.scanImport(SCAN_ALL)),
    meta: { inlineError: true },
  });
  const servers = useQuery({
    queryKey: storeQueryKeys.servers,
    queryFn: async () => unwrap(await commands.listServers()),
  });
  const projects = useProjectList();
  const [filter, setFilter] = useState<SourceFilter>("all");
  const [state, setState] = useState<ImportState>({ choices: {}, ids: {}, secrets: {} });
  const [reviewing, setReviewing] = useState(false);
  const targetRef = useRef<HTMLElement | null>(null);

  const candidates = scan.data?.candidates ?? [];
  const storeIds = useMemo(() => new Set(servers.data?.servers.map((row) => row.id) ?? []), [servers.data]);
  const names = useMemo<ImportNames>(() => {
    const clients = new Map(servers.data?.clients.map((client) => [client.id, client.name]) ?? []);
    const rows = new Map(projects.data?.map((row) => [row.id, row]) ?? []);
    return {
      client: (id) => clients.get(id) ?? id,
      project: (id) => (id ? rows.get(id) : undefined),
    };
  }, [servers.data, projects.data]);
  const shown = useMemo(() => candidates.filter((candidate) => matchesFilter(candidate, filter)), [candidates, filter]);
  // Only what is on screen is imported or checked: a filter never hides a blocking field.
  const plan = useMemo(() => planImport(shown, state, target, storeIds), [shown, state, target, storeIds]);
  const blocked = plan.problems.size > 0;

  // The pointed-at entry is scrolled to and focused once the scan has it.
  const hasTarget = Boolean(target && candidates.some((candidate) => candidate.name === target.name));
  useEffect(() => {
    if (hasTarget && targetRef.current) {
      targetRef.current.scrollIntoView?.({ block: "nearest" });
      targetRef.current.focus();
    }
  }, [hasTarget]);

  const counts: Record<SourceFilter, number> = {
    all: candidates.length,
    user: candidates.filter((candidate) => matchesFilter(candidate, "user")).length,
    projects: candidates.filter((candidate) => matchesFilter(candidate, "projects")).length,
  };
  const chips: { key: SourceFilter; label: string }[] = [
    { key: "all", label: t("import.filter.all", { count: counts.all }) },
    { key: "user", label: t("import.filter.user", { count: counts.user }) },
    { key: "projects", label: t("import.filter.projects", { count: counts.projects }) },
  ];

  return (
    <div className="import-mcp">
      <div className="import-mcp__toolbar">
        <div className="skills-source-tabs" role="group" aria-label={t("import.filter.aria")}>
          {chips.map((chip) => (
            <button
              key={chip.key}
              className={`skills-source-tab${filter === chip.key ? " is-selected" : ""}`}
              type="button"
              aria-pressed={filter === chip.key}
              onClick={() => setFilter(chip.key)}
            >
              {chip.label}
            </button>
          ))}
        </div>
        <button
          className="secondary-button secondary-button--compact"
          type="button"
          disabled={scan.isFetching}
          onClick={() => void scan.refetch()}
        >
          {scan.isFetching ? t("import.scanning") : t("import.rescan")}
        </button>
      </div>

      {scan.isPending ? <p className="migrate-note">{t("import.scanning")}</p> : null}
      {scan.isError ? (
        <div className="dialog-error" role="alert">
          <span>{failureText(scan.error)}</span>
          <button className="secondary-button secondary-button--compact" type="button" onClick={() => void scan.refetch()}>
            {t("app.action.retry")}
          </button>
        </div>
      ) : null}
      {scan.data?.warnings.map((warning, index) => (
        <p key={`${index}-${warning.code}`} className="mcp-page-warning" role="note">
          {errorMessage(warning.code)}
        </p>
      ))}
      {scan.data && candidates.length === 0 ? <p className="migrate-note">{t("import.empty")}</p> : null}
      {scan.data && candidates.length > 0 && shown.length === 0 ? (
        <p className="migrate-note">{t("import.emptyFilter")}</p>
      ) : null}

      <div className="import-candidates">
        {shown.map((candidate) => (
          <CandidateCard
            key={candidate.name}
            candidate={candidate}
            choice={choiceOf(state, candidate, target)}
            state={state}
            names={names}
            idProblems={plan.idProblems}
            focusRef={target?.name === candidate.name ? (element) => (targetRef.current = element) : undefined}
            onChoice={(choice) =>
              setState((current) => ({ ...current, choices: { ...current.choices, [candidate.name]: choice } }))}
            onId={(variant, id) =>
              setState((current) => ({ ...current, ids: { ...current.ids, [editKey(candidate.name, variant.key)]: id } }))}
            onSecret={(field, answer) =>
              setState((current) => ({
                ...current,
                secrets: { ...current.secrets, [editKey(candidate.name, field)]: answer },
              }))}
          />
        ))}
      </div>

      {candidates.length > 0 ? (
        <div className="import-mcp__footer">
          <span role="status">
            {blocked ? t("import.footer.blocked") : t("import.footer.selected", { count: plan.picks.length })}
          </span>
          <button
            className="primary-button"
            type="button"
            disabled={blocked || plan.picks.length === 0}
            onClick={() => setReviewing(true)}
          >
            {t("import.review.open")}
          </button>
        </div>
      ) : null}

      {reviewing ? (
        <ImportReviewDialog
          plan={plan}
          names={names}
          onClose={() => setReviewing(false)}
          onImported={() => setState({ choices: {}, ids: {}, secrets: {} })}
          onOpenPending={onOpenPending}
        />
      ) : null}
    </div>
  );
}
