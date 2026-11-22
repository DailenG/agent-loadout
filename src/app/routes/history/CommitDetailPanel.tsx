// "Show diff" of one commit (SPEC 13.5): the semantic diff grouped per
// entity (field: before -> after), the masked raw git diff on request, and
// for an apply commit the client files it touched with their outcomes.

import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { diffLineClass } from "@/app/apply/labels";
import {
  CHANGE_KEYS,
  CHANGE_TONES,
  OUTCOME_KEYS,
  OUTCOME_TONES,
  failureText,
} from "@/app/routes/history/labels";
import { t } from "@/i18n/en";
import type { AppliedDetail, EntityDiff } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type CommitDetailPanelProps = {
  commitId: string;
};

function DiffLines({ text, label }: { text: string; label: string }) {
  const lines = text.replace(/\n$/, "").split("\n");
  return (
    <pre className="apply-diff" aria-label={label}>
      {lines.map((line, index) => (
        <span key={index} className={diffLineClass(line)}>
          {line}
          {"\n"}
        </span>
      ))}
    </pre>
  );
}

function AppliedTargets({ applied }: { applied: AppliedDetail }) {
  const { commands } = useTransport();
  const servers = useQuery({
    queryKey: storeQueryKeys.servers,
    queryFn: async () => unwrap(await commands.listServers()),
  });
  const clientNames = useMemo(
    () => new Map(servers.data?.clients.map((client) => [client.id, client.name] as const)),
    [servers.data],
  );
  return (
    <section className="history-applied" aria-label={t("history.applied.title")}>
      <h4 className="history-detail__title">{t("history.applied.title")}</h4>
      {applied.targets.length === 0 ? (
        <p className="history-note">{t("history.applied.empty")}</p>
      ) : (
        <table className="history-applied__table">
          <thead>
            <tr>
              <th scope="col">{t("history.applied.client")}</th>
              <th scope="col">{t("history.applied.file")}</th>
              <th scope="col">{t("history.applied.changes")}</th>
              <th scope="col">{t("history.applied.outcome")}</th>
            </tr>
          </thead>
          <tbody>
            {applied.targets.map((target, index) => (
              <tr key={`${target.client}-${target.file}-${index}`}>
                <td>{clientNames.get(target.client) ?? target.client}</td>
                <td className="history-mono">{target.file}</td>
                <td>
                  <span className="apply-file__counts" aria-label={t("apply.counts.aria", { ...target.counts })}>
                    <span className="apply-count apply-count--create">+{target.counts.create}</span>
                    <span className="apply-count apply-count--update">~{target.counts.update}</span>
                    <span className="apply-count apply-count--remove">-{target.counts.remove}</span>
                    <span className="apply-count apply-count--disable">!{target.counts.disable}</span>
                  </span>
                </td>
                <td>
                  <span className={`status-badge ${OUTCOME_TONES[target.outcome]}`}>
                    {t(OUTCOME_KEYS[target.outcome])}
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function SemanticDiff({ changes }: { changes: EntityDiff[] }) {
  // One group per entity: a skill's several files read as one change.
  const groups = useMemo(() => {
    const byEntity = new Map<string, EntityDiff[]>();
    changes.forEach((change) => byEntity.set(change.entity, [...(byEntity.get(change.entity) ?? []), change]));
    return [...byEntity.entries()];
  }, [changes]);

  if (groups.length === 0) {
    return <p className="history-note">{t("history.detail.noChanges")}</p>;
  }
  return (
    <ul className="history-detail__groups" aria-label={t("history.detail.semanticAria")}>
      {groups.map(([entity, diffs]) => (
        <li key={entity} className="history-detail__group" aria-label={entity}>
          <div className="history-detail__entity">
            <strong>{entity}</strong>
            <span className={`status-badge ${CHANGE_TONES[diffs[0].change]}`}>{t(CHANGE_KEYS[diffs[0].change])}</span>
          </div>
          {diffs.map((diff) => (
            <div key={diff.file} className="history-detail__file">
              <span className="history-mono history-detail__path">{diff.file}</span>
              {diff.fields.length > 0 ? (
                <ul className="history-detail__fields">
                  {diff.fields.map((field) => (
                    <li key={field.field} className="history-mono">
                      {t("history.detail.field", {
                        field: field.field,
                        before: field.before ?? t("history.detail.none"),
                        after: field.after ?? t("history.detail.none"),
                      })}
                    </li>
                  ))}
                </ul>
              ) : null}
              {diff.text ? <DiffLines text={diff.text} label={t("history.detail.textAria", { file: diff.file })} /> : null}
            </div>
          ))}
        </li>
      ))}
    </ul>
  );
}

export function CommitDetailPanel({ commitId }: CommitDetailPanelProps) {
  const { commands } = useTransport();
  const [raw, setRaw] = useState(false);
  const detail = useQuery({
    queryKey: [...storeQueryKeys.commitDetail, commitId],
    queryFn: async () => unwrap(await commands.commitDetail(commitId)),
  });

  if (detail.isPending) {
    return <p className="history-note">{t("history.detail.loading")}</p>;
  }
  if (detail.isError) {
    return (
      <p className="history-dialog__error" role="alert">
        {failureText(detail.error)}
      </p>
    );
  }
  const data = detail.data;
  return (
    <section className="history-detail" aria-label={t("history.detail.aria", { id: data.entry.shortId })}>
      <div className="history-detail__toolbar">
        {data.body.trim() !== "" ? <p className="history-detail__body">{data.body}</p> : <span />}
        <button
          className="ghost-button"
          type="button"
          aria-pressed={raw}
          onClick={() => setRaw((current) => !current)}
        >
          {t("history.detail.raw")}
        </button>
      </div>
      {raw ? (
        data.raw.trim() === "" ? (
          <p className="history-note">{t("history.detail.noChanges")}</p>
        ) : (
          <DiffLines text={data.raw} label={t("history.detail.rawAria")} />
        )
      ) : (
        <SemanticDiff changes={data.changes} />
      )}
      {data.applied ? <AppliedTargets applied={data.applied} /> : null}
    </section>
  );
}
