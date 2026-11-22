import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { errorMessage } from "@/i18n/en";
import type { ClientPill as ClientPillData, PillState, ServerRow } from "@/ipc/bindings";
import { MOCK_CLIENTS } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { ClientPill, visiblePills } from "./ClientPill";

function pill(client: string, state: PillState, selected: boolean, reason: ClientPillData["reason"] = null): ClientPillData {
  return { client, state, selected, reason };
}

function makeRow(overrides: Partial<ServerRow> = {}): ServerRow {
  return {
    id: "demo",
    displayName: "Demo",
    description: null,
    transport: "stdio",
    launch: "demo-server --flag",
    origin: null,
    tags: [],
    clients: { allDetected: false, clients: ["claude-code"] },
    pills: [
      pill("claude-code", "enabled", true),
      pill("codex", "disabled", false),
      pill("cursor", "disabled", false),
      pill("claude-desktop", "not_supported", false),
    ],
    problems: [],
    invalid: null,
    ...overrides,
  };
}

function renderPill(row: ServerRow, clientId: string, onOpenPending = vi.fn()) {
  const entry = visiblePills(row, MOCK_CLIENTS).find((item) => item.client.id === clientId);
  if (!entry) {
    throw new Error(`no visible pill for ${clientId}`);
  }
  return { onOpenPending, ...renderWithProviders(<ClientPill row={row} entry={entry} onOpenPending={onOpenPending} />) };
}

describe("visiblePills", () => {
  it("hides undetected, unselected clients with nothing to say", () => {
    const ids = visiblePills(makeRow(), MOCK_CLIENTS).map((entry) => entry.client.id);
    expect(ids).toEqual(["claude-code", "codex", "claude-desktop"]);
  });

  it("shows an undetected client that is selected", () => {
    const row = makeRow({ pills: [pill("cursor", "pending", true)] });
    expect(visiblePills(row, MOCK_CLIENTS).map((entry) => entry.client.id)).toEqual(["cursor"]);
  });
});

describe("ClientPill", () => {
  const cases: { state: PillState; label: string; className: string; tooltip: string; selected: boolean }[] = [
    { state: "enabled", label: "Codex: enabled", className: "is-enabled", tooltip: "Enabled. Click to disable", selected: true },
    { state: "disabled", label: "Codex: disabled", className: "is-disabled", tooltip: "Disabled. Click to enable", selected: false },
    {
      state: "pending",
      label: "Codex: pending",
      className: "is-pending",
      tooltip: "Enabled; waits for the next apply. Click to disable",
      selected: true,
    },
    {
      state: "drift",
      label: "Codex: drift",
      className: "is-drift",
      tooltip: "Changed outside Loadout. Click to review in Pending changes",
      selected: true,
    },
  ];

  it.each(cases)("renders the $state state", ({ state, label, className, tooltip, selected }) => {
    renderPill(makeRow({ pills: [pill("codex", state, selected)] }), "codex");
    const button = screen.getByRole("button", { name: label });
    expect(button).toHaveClass("tool-pill", "mcp-pill", className);
    expect(button).toHaveAttribute("data-tooltip", tooltip);
    expect(button).toBeEnabled();
  });

  it("renders a blocked pill with its reason as the tooltip", () => {
    const row = makeRow({
      pills: [pill("codex", "blocked", true, { code: "missing_secret", message: "detail" })],
    });
    renderPill(row, "codex");
    const button = screen.getByRole("button", { name: "Codex: blocked" });
    expect(button).toHaveClass("is-blocked");
    expect(button).toHaveAttribute("data-tooltip", errorMessage("missing_secret"));
    expect(button).toHaveTextContent("blocked");
  });

  it("renders a not supported pill disabled, with a tooltip", async () => {
    const { transport, user } = renderPill(makeRow(), "claude-desktop");
    const button = screen.getByRole("button", { name: "Claude Desktop: not supported" });
    expect(button).toHaveClass("is-not-supported");
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("data-tooltip", "Claude Desktop has no MCP config here.");
    await user.click(button);
    expect(transport.calls).not.toContain("setServerClients");
  });

  it("enables a client by adding it to the explicit list", async () => {
    const { transport, user } = renderPill(makeRow(), "codex");
    await user.click(screen.getByRole("button", { name: "Codex: disabled" }));
    await waitFor(() => expect(transport.calls).toContain("setServerClients"));
    const call = transport.invocations.find((item) => item.command === "setServerClients");
    expect(call?.args.slice(0, 2)).toEqual(["demo", { allDetected: false, clients: ["claude-code", "codex"] }]);
  });

  it("disables a client from an all-detected choice using the selected pills", async () => {
    const row = makeRow({
      clients: { allDetected: true, clients: [] },
      pills: [pill("claude-code", "enabled", true), pill("codex", "pending", true)],
    });
    const { transport, user } = renderPill(row, "claude-code");
    await user.click(screen.getByRole("button", { name: "Claude Code: enabled" }));
    await waitFor(() => expect(transport.calls).toContain("setServerClients"));
    const call = transport.invocations.find((item) => item.command === "setServerClients");
    expect(call?.args.slice(0, 2)).toEqual(["demo", { allDetected: false, clients: ["codex"] }]);
  });

  it("opens Pending changes for drift instead of toggling", async () => {
    const { transport, user, onOpenPending } = renderPill(
      makeRow({ pills: [pill("codex", "drift", true)] }),
      "codex",
    );
    await user.click(screen.getByRole("button", { name: "Codex: drift" }));
    expect(onOpenPending).toHaveBeenCalledTimes(1);
    expect(transport.calls).not.toContain("setServerClients");
  });
});
