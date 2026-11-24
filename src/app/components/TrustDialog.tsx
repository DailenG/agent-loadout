// The source trust dialog (M39 #24, M42): asked before anything from a git
// repository (or, for MCP servers, a local folder) is saved to the store,
// shared by Install skill from Git, skill updates and MCP from Git or
// folder. Trust decisions are never delegable to agents (SPEC 17): only
// this dialog or an interactive CLI prompt answers it.
//
// `useTrustPrompt().resolveTrust(info)` resolves to what the command needs:
// `{ trust: null }` when no answer is needed (no trust info, or already
// trusted), the user's grant, or null when the user canceled or the source
// is blocked (the caller then does nothing).

import { useCallback, useId, useRef, useState, type ReactNode } from "react";
import { SettingsDialog } from "@/app/routes/settings/SettingsDialog";
import { t, type MessageKey } from "@/i18n/en";
import type { SourceAskReason, SourceChange, SourceLapse, SourceScope, SourceTrustInfo, TrustGrantArg } from "@/ipc/bindings";

const ALWAYS_LABELS: Record<SourceScope, MessageKey> = {
  repo: "trust.choice.repo",
  owner: "trust.choice.owner",
  host: "trust.choice.host",
};

const ALWAYS_HINTS: Record<SourceScope, MessageKey> = {
  repo: "trust.choice.repoHint",
  owner: "trust.choice.ownerHint",
  host: "trust.choice.hostHint",
};

const ASK_REASONS: Record<SourceAskReason, MessageKey> = {
  first_time: "trust.reason.firstTime",
  not_trusted: "trust.reason.notTrusted",
  lapsed: "trust.reason.lapsed",
};

/** Why trust lapsed or what changed, in plain words. */
export function changeText(change: SourceChange | SourceLapse): string {
  const kind = "kind" in change ? change.kind : change.reason;
  if (kind === "moved") {
    return change.detail ? t("trust.change.movedTo", { key: change.detail }) : t("trust.change.moved");
  }
  return t("trust.change.historyRewritten");
}

type Choice = "once" | SourceScope;

function grantOf(choice: Choice): TrustGrantArg {
  return choice === "once" ? { kind: "once" } : { kind: "always", scope: choice };
}

/** The "Official" marker: shown, never a reason to skip the question. */
export function OfficialBadge() {
  return (
    <span className="status-badge tone-info" title={t("trust.official.hint")}>
      {t("trust.official")}
    </span>
  );
}

type TrustDialogProps = {
  info: SourceTrustInfo;
  /**
   * Set when `info` is where the host redirected the requested repository
   * (asked after it): the requested address, shown next to the new one.
   */
  movedFrom?: string | null;
  onAnswer: (grant: TrustGrantArg | null) => void;
};

export function TrustDialog({ info, movedFrom = null, onAnswer }: TrustDialogProps) {
  const describedBy = useId();
  const groupId = useId();
  const [choice, setChoice] = useState<Choice>("once");
  const state = info.state;
  const reason = state.kind === "ask" ? state.reason : null;
  const lapse = state.kind === "ask" ? state.lapse : null;
  const change = info.change ?? lapse;

  return (
    <SettingsDialog
      title={t("trust.title")}
      closeLabel={t("trust.close")}
      role="alertdialog"
      portal
      describedBy={describedBy}
      onClose={() => onAnswer(null)}
      actions={
        <>
          {/* The least destructive control starts focused (an alertdialog): a held or doubled Enter cancels. */}
          <button
            className="secondary-button secondary-button--compact"
            type="button"
            data-autofocus
            onClick={() => onAnswer(null)}
          >
            {t("trust.cancel")}
          </button>
          <button className="primary-button primary-button--compact" type="button" onClick={() => onAnswer(grantOf(choice))}>
            {choice === "once" ? t("trust.confirm.once") : t("trust.confirm.always")}
          </button>
        </>
      }
    >
      <div className="trust-dialog">
        <div className="trust-dialog__source">
          <code>{info.display}</code>
          {info.official ? <OfficialBadge /> : null}
        </div>
        <div id={describedBy} className="trust-dialog__copy">
          {movedFrom !== null ? (
            <p className="dialog-warning">{t("trust.movedSource", { from: movedFrom, to: info.display })}</p>
          ) : null}
          {info.insecure ? <p className="dialog-warning">{t("trust.insecure")}</p> : null}
          {reason ? <p>{t(ASK_REASONS[reason])}</p> : null}
          {change ? <p className="dialog-warning">{changeText(change)}</p> : null}
          <p>{t("trust.explain")}</p>
          {info.official ? <p className="project-dialog__hint">{t("trust.official.hint")}</p> : null}
          {info.installedSkills.length > 0 ? (
            <p className="project-dialog__hint">
              {t("trust.installedSkills", { skills: info.installedSkills.join(", ") })}
            </p>
          ) : null}
        </div>
        <fieldset className="trust-dialog__choices">
          <legend id={groupId}>{t("trust.choice.legend")}</legend>
          <TrustChoice
            checked={choice === "once"}
            label={t("trust.choice.once")}
            hint={t("trust.choice.onceHint")}
            onSelect={() => setChoice("once")}
          />
          {info.scopes.map((option) => (
            <TrustChoice
              key={option.scope}
              checked={choice === option.scope}
              label={t(ALWAYS_LABELS[option.scope], { key: option.display })}
              hint={t(ALWAYS_HINTS[option.scope])}
              onSelect={() => setChoice(option.scope)}
            />
          ))}
        </fieldset>
      </div>
    </SettingsDialog>
  );
}

function TrustChoice(props: { checked: boolean; label: string; hint: string; onSelect: () => void }) {
  const labelId = useId();
  const hintId = useId();
  return (
    <label className={`trust-dialog__choice${props.checked ? " is-selected" : ""}`}>
      <input
        type="radio"
        name="trust-choice"
        checked={props.checked}
        aria-labelledby={labelId}
        aria-describedby={hintId}
        onChange={props.onSelect}
      />
      <span className="trust-dialog__choice-copy">
        <span id={labelId}>{props.label}</span>
        <span id={hintId} className="project-dialog__hint">{props.hint}</span>
      </span>
    </label>
  );
}

/** What a command's `trust` and `movedTrust` arguments are: grants, or none needed. */
export type TrustAnswer = { trust: TrustGrantArg | null; movedTrust: TrustGrantArg | null };

/**
 * The sources a review or update check asks about: the requested one, and
 * where its host redirected it when that is another source (M39).
 */
export type TrustSources = { trust: SourceTrustInfo | null; movedTrust?: SourceTrustInfo | null };

export function useTrustPrompt(): {
  /**
   * Asks about `trust`, then `movedTrust`, when they need an answer. Null:
   * canceled, or blocked (`onBlocked` was called). Otherwise the arguments
   * to send.
   */
  resolveTrust: (sources: TrustSources, onBlocked?: (info: SourceTrustInfo) => void) => Promise<TrustAnswer | null>;
  dialog: ReactNode;
} {
  // `key` gives every prompt a fresh dialog: no choice or focus carries from the first prompt to the second.
  const [asking, setAsking] = useState<{ info: SourceTrustInfo; movedFrom: string | null; key: number } | null>(null);
  const resolveRef = useRef<((grant: TrustGrantArg | null) => void) | null>(null);
  const promptCount = useRef(0);

  const resolveTrust = useCallback(
    async (sources: TrustSources, onBlocked?: (info: SourceTrustInfo) => void): Promise<TrustAnswer | null> => {
      // A block anywhere refuses before any question is asked.
      const blocked = [sources.trust, sources.movedTrust].find((info) => info?.state.kind === "blocked");
      if (blocked) {
        onBlocked?.(blocked);
        return null;
      }
      const ask = async (info: SourceTrustInfo | null | undefined, movedFrom: string | null) => {
        if (!info || info.state.kind !== "ask") {
          return { grant: null };
        }
        promptCount.current += 1;
        const key = promptCount.current;
        const grant = await new Promise<TrustGrantArg | null>((resolve) => {
          resolveRef.current = resolve;
          setAsking({ info, movedFrom, key });
        });
        return grant ? { grant } : null;
      };
      const primary = await ask(sources.trust, null);
      if (!primary) {
        return null;
      }
      const moved = await ask(sources.movedTrust, sources.trust?.display ?? "");
      return moved ? { trust: primary.grant, movedTrust: moved.grant } : null;
    },
    [],
  );

  const answer = useCallback((grant: TrustGrantArg | null) => {
    setAsking(null);
    resolveRef.current?.(grant);
    resolveRef.current = null;
  }, []);

  return {
    resolveTrust,
    dialog: asking ? (
      <TrustDialog key={asking.key} info={asking.info} movedFrom={asking.movedFrom} onAnswer={answer} />
    ) : null,
  };
}

const STATE_LABELS: Record<SourceTrustInfo["state"]["kind"], MessageKey> = {
  trusted: "trust.status.trusted",
  blocked: "trust.status.blocked",
  ask: "trust.status.ask",
};

const STATE_TONES: Record<SourceTrustInfo["state"]["kind"], string> = {
  trusted: "tone-positive",
  blocked: "tone-danger",
  ask: "tone-warning",
};

/** One line in a review: the source, its trust state and what happens next. */
export function TrustSummary({ info, label = t("trust.summary.label") }: { info: SourceTrustInfo; label?: string }) {
  const state = info.state;
  const detail =
    state.kind === "trusted"
      ? t("trust.status.trustedDetail", { key: state.display })
      : state.kind === "blocked"
        ? t("trust.status.blockedDetail", { key: state.display })
        : t("trust.status.askDetail");
  const change = info.change ?? (state.kind === "ask" ? state.lapse : null);
  return (
    <div className="trust-summary">
      <div className="trust-summary__head">
        <span className="trust-summary__label">{label}</span>
        <code>{info.display}</code>
        {info.official ? <OfficialBadge /> : null}
        <span className={`status-badge ${STATE_TONES[state.kind]}`}>{t(STATE_LABELS[state.kind])}</span>
      </div>
      <p className="project-dialog__hint">{detail}</p>
      {change ? <p className="dialog-warning">{changeText(change)}</p> : null}
      {info.insecure ? <p className="dialog-warning">{t("trust.insecure")}</p> : null}
    </div>
  );
}
