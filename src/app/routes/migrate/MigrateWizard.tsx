// Migration wizard (SPEC 15.1, 11.7): SkillDock's `~/.skilldock` (read only)
// into the store. Steps: the dry-run report (migrated / needs review /
// skipped, each with its reason and offers), the secret extraction choice
// per flagged value (OS keyring with a one-way value, a 1Password reference,
// or an environment variable), the confirmation and the migration (one
// commit), then the follow-ups: "Replace SkillDock wrapper" per wrapped
// client entry and the first apply in Pending changes. New (SkillDock had
// no importer); the dialog shell is the Settings one.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useInvalidateStore } from "@/app/apply/changeNotice";
import { AppSelect, type AppSelectOption } from "@/app/components/AppSelect";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type {
  MigrationApplied,
  MigrationExtraction,
  MigrationItemStatus,
  MigrationOffer,
  MigrationSecretChoice,
} from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { SettingsDialog } from "../settings/SettingsDialog";

/** Query key of the dry run. */
export const MIGRATION_PLAN_KEY = ["plan_skilldock_migration"] as const;

type Step = "report" | "secrets" | "confirm" | "done";

export type SecretChoice = "keyring" | "onepassword" | "env" | "later";

type SecretAnswer = { choice: SecretChoice; text: string };

const STATUS_KEYS = {
  migrated: "migrate.status.migrated",
  needs_review: "migrate.status.needsReview",
  skipped: "migrate.status.skipped",
} as const satisfies Record<MigrationItemStatus, MessageKey>;

const STATUS_TONES: Record<MigrationItemStatus, string> = {
  migrated: "tone-positive",
  needs_review: "tone-warning",
  skipped: "tone-neutral",
};

const CHOICES: AppSelectOption<SecretChoice>[] = [
  { value: "keyring", label: t("migrate.secret.keyring") },
  { value: "onepassword", label: t("migrate.secret.onepassword") },
  { value: "env", label: t("migrate.secret.env") },
  { value: "later", label: t("migrate.secret.later") },
];

function failureText(error: unknown) {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

function offerText(offer: MigrationOffer): string {
  switch (offer.kind) {
    case "replace_wrapper":
      return t("migrate.offer.replaceWrapper", { entry: `${offer.client}/${offer.name}` });
    case "review_entry":
      return t("migrate.offer.reviewEntry", { entry: `${offer.client}/${offer.name}` });
    case "set_secret":
      return t("migrate.offer.setSecret", { alias: offer.alias });
    case "machine_path":
      return t("migrate.offer.machinePath", { server: offer.server });
    case "keyring_token":
      return t("migrate.offer.keyringToken");
    case "add_remote":
      return t("migrate.offer.addRemote", { url: offer.url });
  }
}

/** `op://vault/item/field` or `op://vault/item/section/field` (SPEC 11.2). */
const OP_REFERENCE = /^op:\/\/[^/\s]+\/[^/\s]+\/[^/\s]+(\/[^/\s]+)?$/;
/** An environment variable name. */
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Why `answer` cannot be used yet, or null when it is complete and valid. */
export function answerProblem(answer: SecretAnswer): MessageKey | null {
  const text = answer.text.trim();
  switch (answer.choice) {
    case "later":
      return null;
    case "keyring":
      return answer.text === "" ? "migrate.secret.missing" : null;
    case "onepassword":
      return text === "" ? "migrate.secret.missing" : OP_REFERENCE.test(text) ? null : "migrate.secret.badOp";
    case "env":
      return text === "" ? "migrate.secret.missing" : ENV_NAME.test(text) ? null : "migrate.secret.badEnv";
  }
}

/** The command's choice for an answered extraction; none for "Set later". */
function choiceOf(alias: string, answer: SecretAnswer): MigrationSecretChoice | null {
  switch (answer.choice) {
    case "keyring":
      return { kind: "keyring", alias, value: answer.text };
    case "onepassword":
      return { kind: "onepassword", alias, reference: answer.text.trim() };
    case "env":
      return { kind: "env", alias, name: answer.text.trim() };
    case "later":
      return null;
  }
}

type SecretRowProps = {
  extraction: MigrationExtraction;
  answer: SecretAnswer;
  onChange: (answer: SecretAnswer) => void;
};

function SecretRow({ extraction, answer, onChange }: SecretRowProps) {
  const label = `${extraction.server} ${extraction.field}`;
  return (
    <li className="migrate-secret">
      <div className="migrate-secret__copy">
        <strong>{label}</strong>
        <span>{t("migrate.secret.alias", { alias: extraction.alias })}</span>
        <span className="migrate-secret__masked" aria-hidden="true">••••••••</span>
      </div>
      <AppSelect
        className="migrate-secret__choice"
        value={answer.choice}
        options={CHOICES}
        ariaLabel={t("migrate.secret.choiceAria", { field: label })}
        onChange={(choice) => onChange({ choice, text: "" })}
      />
      {answer.choice === "keyring" ? (
        <input
          className="migrate-secret__input"
          type="password"
          autoComplete="off"
          spellCheck={false}
          aria-label={t("migrate.secret.valueAria", { field: label })}
          placeholder={t("migrate.secret.valuePlaceholder")}
          value={answer.text}
          onChange={(event) => onChange({ choice: "keyring", text: event.target.value })}
        />
      ) : answer.choice === "onepassword" ? (
        <input
          className="migrate-secret__input"
          type="text"
          spellCheck={false}
          aria-label={t("migrate.secret.refAria", { field: label })}
          placeholder="op://vault/item/field"
          value={answer.text}
          onChange={(event) => onChange({ choice: "onepassword", text: event.target.value })}
        />
      ) : answer.choice === "env" ? (
        <input
          className="migrate-secret__input"
          type="text"
          spellCheck={false}
          aria-label={t("migrate.secret.envAria", { field: label })}
          placeholder="NAME"
          value={answer.text}
          onChange={(event) => onChange({ choice: "env", text: event.target.value })}
        />
      ) : (
        <span className="migrate-secret__later">{t("migrate.secret.laterHint")}</span>
      )}
      {answer.text.trim() !== "" && answerProblem(answer) ? (
        <span className="migrate-secret__problem" role="alert">{t(answerProblem(answer)!)}</span>
      ) : null}
    </li>
  );
}

export type MigrateWizardProps = {
  onClose: () => void;
  /** Opens Pending changes for the first apply. */
  onOpenPending: () => void;
};

export function MigrateWizard({ onClose, onOpenPending }: MigrateWizardProps) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();
  const [step, setStep] = useState<Step>("report");
  const [filter, setFilter] = useState<MigrationItemStatus | "all">("all");
  const [answers, setAnswers] = useState<Record<string, SecretAnswer>>({});
  const [isMigrating, setIsMigrating] = useState(false);
  const [applied, setApplied] = useState<MigrationApplied | null>(null);
  const [migrateError, setMigrateError] = useState<string | null>(null);
  const [replaced, setReplaced] = useState<string[]>([]);
  const plan = useQuery({
    queryKey: MIGRATION_PLAN_KEY,
    queryFn: async () => unwrap(await commands.planSkilldockMigration()),
    retry: false,
  });

  const report = plan.data;
  const extractions = report?.extractions ?? [];
  const answerOf = (alias: string): SecretAnswer => answers[alias] ?? { choice: "keyring", text: "" };
  const allAnswered = extractions.every((extraction) => answerProblem(answerOf(extraction.alias)) === null);
  const notFound = plan.error instanceof IpcFailure && plan.error.code === "not_found";

  async function migrate() {
    setIsMigrating(true);
    setMigrateError(null);
    try {
      // The answered extractions of the plan the user reviewed; their
      // aliases and keyring values go into the migration's one commit.
      const choices = extractions
        .map((extraction) => choiceOf(extraction.alias, answerOf(extraction.alias)))
        .filter((choice): choice is MigrationSecretChoice => choice !== null);
      const result = unwrap(await commands.applySkilldockMigration(choices));
      setAnswers({});
      setApplied(result);
      setStep("done");
      notify({
        tone: "success",
        message: result.commit
          ? t("migrate.done.toast", { servers: result.report.servers.length, skills: result.report.skills.length })
          : t("migrate.done.nothing"),
      });
    } catch (error) {
      // Nothing was committed: the answers stay for a retry.
      setMigrateError(failureText(error));
      notify({ tone: "error", message: failureText(error) });
    } finally {
      setIsMigrating(false);
      await invalidate();
    }
  }

  async function replaceWrapper(client: string, name: string) {
    const entry = `${client}/${name}`;
    try {
      unwrap(await commands.replaceSkilldockWrapper(client, name));
      setReplaced((current) => [...current, entry]);
      notify({ tone: "success", message: t("migrate.wrapper.done", { entry }) });
    } catch (error) {
      notify({ tone: "error", message: failureText(error) });
    } finally {
      await invalidate();
    }
  }

  const wrapperOffers = (applied?.report ?? report)?.items
    .flatMap((item) => item.offers)
    .filter((offer): offer is Extract<MigrationOffer, { kind: "replace_wrapper" }> => offer.kind === "replace_wrapper") ?? [];

  let body: JSX.Element;
  let actions: JSX.Element;
  if (plan.isPending) {
    body = <p className="migrate-note">{t("migrate.loading")}</p>;
    actions = <button className="secondary-button" type="button" onClick={onClose}>{t("migrate.close")}</button>;
  } else if (plan.isError || !report) {
    body = (
      <p className="migrate-note" role="alert">
        {notFound ? t("migrate.notFound") : failureText(plan.error)}
      </p>
    );
    actions = <button className="secondary-button" type="button" onClick={onClose}>{t("migrate.close")}</button>;
  } else if (step === "report") {
    const items = report.items.filter((item) => filter === "all" || item.status === filter);
    const tabs: { key: MigrationItemStatus | "all"; label: string }[] = [
      { key: "all", label: t("migrate.filter.all", { count: report.items.length }) },
      { key: "migrated", label: t("migrate.filter.migrated", { count: report.counts.migrated }) },
      { key: "needs_review", label: t("migrate.filter.needsReview", { count: report.counts.needsReview }) },
      { key: "skipped", label: t("migrate.filter.skipped", { count: report.counts.skipped }) },
    ];
    body = (
      <>
        <p className="migrate-note">{t("migrate.report.intro", { source: report.source })}</p>
        <div className="migrate-filter" role="tablist" aria-label={t("migrate.filter.aria")}>
          {tabs.map((tab) => (
            <button
              key={tab.key}
              className={`filter-tab${filter === tab.key ? " active is-selected" : ""}`}
              type="button"
              role="tab"
              aria-selected={filter === tab.key}
              onClick={() => setFilter(tab.key)}
            >
              {tab.label}
            </button>
          ))}
        </div>
        <ul className="migrate-items" aria-label={t("migrate.report.aria")}>
          {items.map((item, index) => (
            <li key={`${item.kind}-${item.name}-${index}`} className="migrate-item">
              <div className="migrate-item__head">
                <strong>{item.name}</strong>
                <span className="status-badge tone-neutral">{t(`migrate.kind.${item.kind}` as MessageKey)}</span>
                <span className={`status-badge ${STATUS_TONES[item.status]}`}>{t(STATUS_KEYS[item.status])}</span>
              </div>
              <p className="migrate-item__detail">{item.code ? `${errorMessage(item.code)} ${item.detail}` : item.detail}</p>
              {item.offers.length > 0 ? (
                <ul className="migrate-item__offers">
                  {item.offers.map((offer, offerIndex) => (
                    <li key={offerIndex}>{offerText(offer)}</li>
                  ))}
                </ul>
              ) : null}
            </li>
          ))}
        </ul>
      </>
    );
    actions = (
      <>
        <button className="secondary-button" type="button" onClick={onClose}>{t("migrate.cancel")}</button>
        <button
          className="primary-button"
          type="button"
          onClick={() => setStep(extractions.length > 0 ? "secrets" : "confirm")}
        >
          {t("migrate.next")}
        </button>
      </>
    );
  } else if (step === "secrets") {
    body = (
      <>
        <p className="migrate-note">{t("migrate.secrets.intro", { count: extractions.length })}</p>
        <ul className="migrate-secrets">
          {extractions.map((extraction) => (
            <SecretRow
              key={extraction.alias}
              extraction={extraction}
              answer={answerOf(extraction.alias)}
              onChange={(answer) => setAnswers((current) => ({ ...current, [extraction.alias]: answer }))}
            />
          ))}
        </ul>
      </>
    );
    actions = (
      <>
        <button className="secondary-button" type="button" onClick={() => setStep("report")}>{t("migrate.back")}</button>
        <button className="primary-button" type="button" disabled={!allAnswered} onClick={() => setStep("confirm")}>
          {t("migrate.next")}
        </button>
      </>
    );
  } else if (step === "confirm") {
    body = (
      <>
        <p className="migrate-note">
          {t("migrate.confirm.question", { servers: report.servers.length, skills: report.skills.length })}
        </p>
        <ul className="migrate-items">
          <li>{t("migrate.confirm.readOnly", { source: report.source })}</li>
          <li>{t("migrate.confirm.oneCommit")}</li>
          <li>{t("migrate.confirm.noClientFiles")}</li>
        </ul>
        {migrateError ? (
          <p className="migrate-note" role="alert">{t("migrate.confirm.failed", { error: migrateError })}</p>
        ) : null}
      </>
    );
    actions = (
      <>
        <button className="secondary-button" type="button" onClick={() => setStep(extractions.length > 0 ? "secrets" : "report")}>
          {t("migrate.back")}
        </button>
        <button className="primary-button" type="button" disabled={isMigrating} onClick={() => void migrate()}>
          {isMigrating ? t("migrate.migrating") : t("migrate.confirm.action")}
        </button>
      </>
    );
  } else {
    body = (
      <>
        <p className="migrate-note">
          {applied?.commit
            ? t("migrate.done.summary", {
                servers: applied.report.servers.length,
                skills: applied.report.skills.length,
                tag: applied.tag,
              })
            : t("migrate.done.nothing")}
        </p>
        {applied?.backupImported ? <p className="migrate-note">{t("migrate.done.backup")}</p> : null}
        <p className="migrate-note">{t("migrate.done.recommend")}</p>
        {wrapperOffers.length > 0 ? (
          <ul className="migrate-items" aria-label={t("migrate.wrapper.aria")}>
            {wrapperOffers.map((offer) => {
              const entry = `${offer.client}/${offer.name}`;
              const done = replaced.includes(entry);
              return (
                <li key={entry} className="migrate-item migrate-item--row">
                  <span>{t("migrate.wrapper.entry", { entry })}</span>
                  <button
                    className="secondary-button secondary-button--compact"
                    type="button"
                    disabled={done}
                    onClick={() => void replaceWrapper(offer.client, offer.name)}
                  >
                    {done ? t("migrate.wrapper.replaced") : t("migrate.wrapper.replace")}
                  </button>
                </li>
              );
            })}
          </ul>
        ) : null}
      </>
    );
    actions = (
      <>
        <button className="secondary-button" type="button" onClick={onClose}>{t("migrate.close")}</button>
        <button
          className="primary-button"
          type="button"
          onClick={() => {
            onClose();
            onOpenPending();
          }}
        >
          {t("migrate.done.openPending")}
        </button>
      </>
    );
  }

  return (
    <SettingsDialog
      wide
      title={t("migrate.title")}
      subtitle={t(`migrate.step.${step}` as MessageKey)}
      closeLabel={t("migrate.close")}
      onClose={onClose}
      actions={actions}
    >
      {body}
    </SettingsDialog>
  );
}
