// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Pill markup (tool-pill, tool-pill__logo, tool-pill__name) ported from the "Enable in
// Tools" grid of DailenG/skilldock@181b52b src/app/routes/mcp.tsx; the states (SPEC 13.2)
// and the client choice logic are new.

import { useCallback, useState } from "react";
import { useChangeNotice } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { ToolLogo } from "@/app/components/ToolLogo";
import { errorMessage, t, type MessageKey } from "@/i18n/en";
import {
  PRODUCT_NAME,
  type ClientChoice,
  type ClientInfo,
  type ClientPill as ClientPillData,
  type PillState,
  type ServerRow,
} from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";

const STATE_KEYS = {
  enabled: "mcp.pill.state.enabled",
  disabled: "mcp.pill.state.disabled",
  pending: "mcp.pill.state.pending",
  drift: "mcp.pill.state.drift",
  blocked: "mcp.pill.state.blocked",
  not_supported: "mcp.pill.state.not_supported",
} as const satisfies Record<PillState, MessageKey>;

export type VisiblePill = { pill: ClientPillData; client: ClientInfo };

/**
 * The pills a row shows, in client order: clients that are detected, or
 * selected, or have something other than "disabled" to say.
 */
export function visiblePills(row: ServerRow, clients: readonly ClientInfo[]): VisiblePill[] {
  return pillsToShow(row.pills, clients);
}

/** [`visiblePills`] for any row's pills. */
export function pillsToShow(pills: readonly ClientPillData[], clients: readonly ClientInfo[]): VisiblePill[] {
  const byId = new Map(clients.map((client) => [client.id, client]));
  return pills.flatMap((pill) => {
    const client = byId.get(pill.client) ?? { id: pill.client, name: pill.client, detected: false, supported: false };
    return client.detected || pill.selected || pill.state !== "disabled" ? [{ pill, client }] : [];
  });
}

/** The clients a row is enabled for, as an explicit list. */
export function selectedClients(row: ServerRow): string[] {
  return row.clients.allDetected
    ? row.pills.filter((pill) => pill.selected).map((pill) => pill.client)
    : row.clients.clients;
}

/** The row's choice with `clientId` added or removed, as an explicit list. */
export function toggledChoice(row: ServerRow, clientId: string, enable: boolean): ClientChoice {
  const rest = selectedClients(row).filter((id) => id !== clientId);
  return { allDetected: false, clients: enable ? [...rest, clientId] : rest };
}

/** Every detected client that can run the server, plus those already chosen. */
export function enableAllChoice(row: ServerRow, clients: readonly ClientInfo[]): ClientChoice {
  const chosen = selectedClients(row);
  const ready = row.pills
    .filter((pill) => {
      const client = clients.find((item) => item.id === pill.client);
      return client?.detected && client.supported && pill.state !== "not_supported";
    })
    .map((pill) => pill.client)
    .filter((id) => !chosen.includes(id));
  return { allDetected: false, clients: [...chosen, ...ready] };
}

export const DISABLE_ALL_CHOICE: ClientChoice = { allDetected: false, clients: [] };

/** Saves a row's client choice and reports the change; false when it failed. */
export function useSetServerClients() {
  const { commands } = useTransport();
  const changeNotice = useChangeNotice();
  const { notify } = useNotifications();
  return useCallback(
    async (id: string, choice: ClientChoice, message: string) => {
      try {
        changeNotice(unwrap(await commands.setServerClients(id, choice)), message);
        return true;
      } catch (error) {
        notify({
          tone: "error",
          message: error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown"),
        });
        return false;
      }
    },
    [changeNotice, commands, notify],
  );
}

function pillTooltip({ pill, client }: VisiblePill): string {
  switch (pill.state) {
    case "enabled":
      return t("mcp.pill.tooltip.enabled");
    case "disabled":
      return t("mcp.pill.tooltip.disabled");
    case "pending":
      return t(pill.selected ? "mcp.pill.tooltip.pendingOn" : "mcp.pill.tooltip.pendingOff");
    case "drift":
      return t("mcp.pill.tooltip.drift", { product: PRODUCT_NAME });
    case "blocked":
      return pill.reason ? errorMessage(pill.reason.code) : t("mcp.pill.tooltip.blocked");
    case "not_supported":
      return pill.reason ? errorMessage(pill.reason.code) : t("mcp.pill.tooltip.notSupported", { client: client.name });
  }
}

type PillButtonProps = {
  entry: VisiblePill;
  /** A change of this pill or its row is running. */
  busy: boolean;
  onClick: () => void;
};

/** A client pill's markup and states (SPEC 13.2), for any scope. */
export function PillButton({ entry, busy, onClick }: PillButtonProps) {
  const { pill, client } = entry;
  const state = pill.state;
  const stateLabel = t(STATE_KEYS[state]);
  const tooltip = pillTooltip(entry);
  const notSupported = state === "not_supported";
  const showsState = state !== "enabled" && state !== "disabled";

  const button = (
    <button
      className={`tool-pill mcp-pill is-${state.replace("_", "-")}${state === "enabled" ? " is-enabled" : ""}`}
      type="button"
      onClick={onClick}
      disabled={notSupported || busy}
      aria-pressed={notSupported || state === "drift" ? undefined : pill.selected}
      aria-label={t("mcp.pill.aria", { client: client.name, state: stateLabel })}
      data-state={state}
      data-tooltip={tooltip}
    >
      <span className="tool-pill__logo">
        <ToolLogo clientId={client.id} name={client.name} />
      </span>
      <span className="tool-pill__name">{client.name}</span>
      {showsState ? <span className="mcp-pill__state" aria-hidden="true">{stateLabel}</span> : null}
    </button>
  );

  // A disabled button gets no pointer events; the wrapper shows the tooltip.
  return notSupported ? (
    <span className="mcp-pill-wrap" data-tooltip={tooltip}>
      {button}
    </span>
  ) : button;
}

type ClientPillProps = {
  row: ServerRow;
  entry: VisiblePill;
  /** Opens Pending changes, where drift is resolved. */
  onOpenPending: () => void;
  /** A bulk change of this row is running. */
  busy?: boolean;
};

/** One client of a store server row (SPEC 13.2). */
export function ClientPill({ row, entry, onOpenPending, busy = false }: ClientPillProps) {
  const setServerClients = useSetServerClients();
  const [isUpdating, setIsUpdating] = useState(false);
  const { pill, client } = entry;

  async function handleClick() {
    if (pill.state === "drift") {
      onOpenPending();
      return;
    }
    setIsUpdating(true);
    try {
      await setServerClients(
        row.id,
        toggledChoice(row, client.id, !pill.selected),
        t("mcp.toggled", { name: row.displayName ?? row.id, client: client.name }),
      );
    } finally {
      setIsUpdating(false);
    }
  }

  return <PillButton entry={entry} busy={isUpdating || busy} onClick={() => void handleClick()} />;
}
