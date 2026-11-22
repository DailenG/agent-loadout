// Effective config matrix (SPEC 6.4, 13.3): rows are server names, columns
// clients; each cell shows the layer letter and whether the client loads the
// entry. Clicking a cell shows its provenance chain: the winner's file, the
// entries it shadows, the other clients that read the same file, warnings
// and the native snippet (masked by the backend).

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { STATE_KEYS } from "@/app/apply/labels";
import { HeaderToolbar } from "@/app/components/HeaderSlots";
import { LAYER_KEYS, LAYER_LETTERS, failureText } from "@/app/routes/projects/labels";
import { errorMessage, t } from "@/i18n/en";
import type { MatrixCell, MatrixClient } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

type Selected = { server: string; cell: MatrixCell };

function cellSymbol(cell: MatrixCell) {
  if (cell.warnings.length > 0) {
    return "⚠";
  }
  return cell.loaded ? "✓" : "⊘";
}

function CellButton(props: {
  server: string;
  client: MatrixClient;
  cell: MatrixCell;
  selected: boolean;
  onSelect: () => void;
}) {
  const { server, client, cell } = props;
  const state = cell.status ? t(STATE_KEYS[cell.status]) : t(cell.loaded ? "matrix.detail.loaded" : "matrix.detail.notLoaded");
  return (
    <button
      type="button"
      className={`matrix-cell${props.selected ? " is-selected" : ""}${cell.loaded ? "" : " is-off"}${
        cell.warnings.length > 0 ? " has-warning" : ""
      }`}
      aria-pressed={props.selected}
      aria-label={t("matrix.cell.aria", {
        server,
        client: client.name,
        layer: t(LAYER_KEYS[cell.winner.layer]),
        state,
      })}
      onClick={props.onSelect}
    >
      <span className="matrix-cell__layer">{LAYER_LETTERS[cell.winner.layer]}</span>
      <span className="matrix-cell__mark">{cellSymbol(cell)}</span>
    </button>
  );
}

function Provenance({ selected, clientName, onClose }: {
  selected: Selected;
  clientName: (id: string) => string;
  onClose: () => void;
}) {
  const { server, cell } = selected;
  return (
    <aside className="panel-card matrix-detail" aria-label={t("matrix.detail.title", { server, client: clientName(cell.client) })}>
      <div className="matrix-detail__head">
        <h4>{t("matrix.detail.title", { server, client: clientName(cell.client) })}</h4>
        <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={t("matrix.detail.close")}>
          ×
        </button>
      </div>
      <dl className="matrix-detail__list">
        <dt>{t("matrix.detail.winner")}</dt>
        <dd>
          <span className="matrix-detail__layer">{t(LAYER_KEYS[cell.winner.layer])}</span>{" "}
          <code>{cell.winner.file}</code>
        </dd>
        <dt>{t("matrix.detail.source")}</dt>
        <dd>
          <code>{cell.source}</code>
        </dd>
        <dt>{t("matrix.detail.status")}</dt>
        <dd>
          {t(cell.loaded ? "matrix.detail.loaded" : "matrix.detail.notLoaded")}
          {cell.status ? ` · ${t(STATE_KEYS[cell.status])}` : ""}
        </dd>
        <dt>{t("matrix.detail.shadowed")}</dt>
        <dd>
          {cell.shadowed.length === 0 ? (
            t("matrix.detail.none")
          ) : (
            <ul className="matrix-detail__chain" aria-label={t("matrix.detail.shadowed")}>
              {cell.shadowed.map((entry, index) => (
                <li key={index}>
                  {t(LAYER_KEYS[entry.layer])} <code>{entry.file}</code>
                </li>
              ))}
            </ul>
          )}
        </dd>
        <dt>{t("matrix.detail.alsoVisible")}</dt>
        <dd>
          {cell.alsoVisibleTo.length === 0
            ? t("matrix.detail.none")
            : cell.alsoVisibleTo.map(clientName).join(", ")}
        </dd>
        {cell.warnings.length > 0 ? (
          <>
            <dt>{t("matrix.detail.warnings")}</dt>
            <dd>
              <ul className="matrix-detail__warnings" aria-label={t("matrix.detail.warnings")}>
                {cell.warnings.map((warning, index) => (
                  <li key={index}>
                    <strong>{errorMessage(warning.code)}</strong>
                    <span className="matrix-detail__message">{warning.message}</span>
                  </li>
                ))}
              </ul>
            </dd>
          </>
        ) : null}
      </dl>
      {cell.snippet ? (
        <pre className="apply-diff" aria-label={t("matrix.detail.snippet")}>
          {cell.snippet}
        </pre>
      ) : null}
    </aside>
  );
}

export function EffectiveMatrixView({ projectId, onBack }: { projectId: string; onBack: () => void }) {
  const { commands } = useTransport();
  const matrix = useQuery({
    queryKey: [...storeQueryKeys.effective, projectId],
    queryFn: async () => unwrap(await commands.effectiveMatrix(projectId)),
  });
  const [selected, setSelected] = useState<Selected | null>(null);
  const data = matrix.data;
  const names = new Map(data?.clients.map((client) => [client.id, client.name]) ?? []);
  const clientName = (id: string) => names.get(id) ?? id;

  return (
    <div className="projects-route matrix-route">
      <HeaderToolbar>
        <button className="secondary-button secondary-button--compact" type="button" onClick={onBack}>
          {t("matrix.back")}
        </button>
      </HeaderToolbar>
      <section className="projects-section">
        <div className="matrix-head">
          <h3>{t("matrix.title", { name: data?.projectName ?? projectId })}</h3>
          {data ? <span className="project-card__path">{data.path}</span> : null}
        </div>
        {matrix.isPending ? <p className="tools-note">{t("matrix.loading")}</p> : null}
        {matrix.isError ? (
          <div className="panel-card empty-state page-empty-state" role="alert">
            <p>{failureText(matrix.error)}</p>
            <button className="secondary-button" type="button" onClick={() => void matrix.refetch()}>
              {t("app.action.retry")}
            </button>
          </div>
        ) : null}
        {data && data.rows.length === 0 ? <p className="tools-note">{t("matrix.empty")}</p> : null}
        {data && data.rows.length > 0 ? (
          <div className="matrix-layout">
            <div className="matrix-scroll">
              <table className="matrix-table" aria-label={t("matrix.title", { name: data.projectName })}>
                <thead>
                  <tr>
                    <th scope="col">{t("matrix.server")}</th>
                    {data.clients.map((client) => (
                      <th key={client.id} scope="col">
                        {client.name}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {data.rows.map((row) => (
                    <tr key={row.name}>
                      <th scope="row">{row.name}</th>
                      {data.clients.map((client) => {
                        const cells = row.cells.filter((cell) => cell.client === client.id);
                        return (
                          <td key={client.id}>
                            {cells.length === 0 ? (
                              <span className="matrix-cell__none" aria-label={t("matrix.cell.none")}>
                                -
                              </span>
                            ) : (
                              cells.map((cell, index) => (
                                <CellButton
                                  key={index}
                                  server={row.name}
                                  client={client}
                                  cell={cell}
                                  selected={selected?.cell === cell}
                                  onSelect={() => setSelected({ server: row.name, cell })}
                                />
                              ))
                            )}
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                </tbody>
              </table>
              <p className="matrix-legend">{t("matrix.legend")}</p>
            </div>
            {selected ? (
              <Provenance selected={selected} clientName={clientName} onClose={() => setSelected(null)} />
            ) : (
              <p className="tools-note">{t("matrix.select")}</p>
            )}
          </div>
        ) : null}
        {data?.warnings.map((warning, index) => (
          <p key={index} className="tools-note">
            {errorMessage(warning.code)}
          </p>
        ))}
      </section>
    </div>
  );
}
