import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ToolsPage } from "@/app/routes/tools/ToolsPage";
import { t } from "@/i18n/en";
import type { ServerList, ServerRow, ToolInfo } from "@/ipc/bindings";
import {
  MOCK_CHANGE,
  MOCK_CLIENTS,
  mockIpcError,
  ok,
  type MockTransportOptions,
} from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const CURSOR: ToolInfo = {
  id: "cursor",
  name: "Cursor",
  tier: "b",
  detected: false,
  supported: true,
  readOnly: false,
  userFiles: [{ path: "~/.cursor/mcp.json", exists: false }],
  projectFiles: [],
  transports: { stdio: true, http: true, sse: false },
  envRefs: false,
  restartRequired: true,
  warnings: [],
};

const CLAUDE: ToolInfo = {
  id: "claude-code",
  name: "Claude Code",
  tier: "a",
  detected: true,
  supported: true,
  readOnly: false,
  userFiles: [{ path: "~/.claude.json", exists: true }],
  projectFiles: ["<P>/.mcp.json"],
  transports: { stdio: true, http: true, sse: true },
  envRefs: true,
  restartRequired: false,
  warnings: [],
};

const LEGACY: ToolInfo = {
  ...CURSOR,
  id: "claude-desktop",
  name: "Claude Desktop",
  tier: "a",
  detected: false,
  supported: false,
  readOnly: true,
  userFiles: [],
};

function pills(selected: string[]): ServerRow["pills"] {
  return MOCK_CLIENTS.map((client) => ({
    client: client.id,
    selected: selected.includes(client.id),
    state: selected.includes(client.id) ? "enabled" : "disabled",
    reason: null,
  }));
}

function server(id: string, selected: string[]): ServerRow {
  return {
    id,
    displayName: null,
    description: null,
    transport: "stdio",
    launch: "demo-server",
    origin: null,
    tags: [],
    clients: { allDetected: false, clients: selected },
    pills: pills(selected),
    problems: [],
    invalid: null,
  };
}

const SERVERS: ServerList = {
  servers: [
    server("alpha", ["codex"]),
    server("beta", ["claude-code", "codex"]),
    { ...server("broken", []), invalid: { code: "parse_error", message: "synthetic" } },
  ],
  clients: MOCK_CLIENTS,
  warnings: [],
};

function renderPage(commands: MockTransportOptions["commands"] = {}) {
  return renderWithProviders(<ToolsPage />, {
    commands: {
      listTools: async () => ok([CURSOR, CLAUDE, LEGACY]),
      listServers: async () => ok(SERVERS),
      ...commands,
    },
  });
}

describe("ToolsPage", () => {
  it("renders a card per tool, detected first, with badges", async () => {
    renderPage();
    const cards = await screen.findAllByRole("article");
    expect(cards.map((card) => card.getAttribute("aria-label"))).toEqual([
      "Claude Code",
      "Cursor",
      "Claude Desktop",
    ]);

    const claude = within(cards[0]);
    expect(claude.getByText(t("tools.tier", { tier: "A" }))).toBeInTheDocument();
    expect(claude.getByText(t("tools.detected"))).toBeInTheDocument();

    const cursor = within(cards[1]);
    expect(cursor.getByText(t("tools.notDetected"))).toBeInTheDocument();
    expect(cursor.getByText(t("tools.tier", { tier: "B" }))).toBeInTheDocument();

    const legacy = within(cards[2]);
    expect(legacy.getByText(t("tools.readOnly"))).toBeInTheDocument();
    expect(legacy.getByText(t("tools.unsupportedOs"))).toBeInTheDocument();
  });

  it("shows paths per scope and the capability summary", async () => {
    renderPage();
    const cards = await screen.findAllByRole("article");
    const claude = within(cards[0]);
    expect(
      within(claude.getByRole("list", { name: t("tools.userFilesAria", { name: "Claude Code" }) })).getByText(
        "~/.claude.json",
      ),
    ).toBeInTheDocument();
    expect(
      within(claude.getByRole("list", { name: t("tools.projectFilesAria", { name: "Claude Code" }) })).getByText(
        "<P>/.mcp.json",
      ),
    ).toBeInTheDocument();
    expect(claude.getByText("stdio / HTTP / SSE")).toBeInTheDocument();

    const cursor = within(cards[1]);
    const cursorFile = cursor.getByText(/~\/\.cursor\/mcp\.json/);
    expect(cursorFile).toHaveClass("is-missing");
    expect(cursorFile).toHaveTextContent(t("tools.fileMissing"));
    expect(cursor.getByText(t("tools.noProjectFiles"))).toBeInTheDocument();
    expect(cursor.getByText("stdio / HTTP")).toBeInTheDocument();

    const legacy = within(cards[2]);
    expect(
      legacy.getByRole("button", { name: t("tools.openFolderAria", { name: "Claude Desktop" }) }),
    ).toBeDisabled();
  });

  it("opens the folder through revealToolFolder and reports failures", async () => {
    const revealToolFolder = vi.fn(async () => mockIpcError({ code: "io_error", message: "synthetic" }));
    const { user } = renderPage({ revealToolFolder });
    await user.click(
      await screen.findByRole("button", { name: t("tools.openFolderAria", { name: "Cursor" }) }),
    );
    await waitFor(() => expect(revealToolFolder).toHaveBeenCalledWith("cursor"));
    expect(await screen.findByText("A file could not be read or written.")).toBeInTheDocument();
  });

  it("refreshes from the header toolbar", async () => {
    const { user, transport } = renderPage();
    await screen.findAllByRole("article");
    const toolbar = screen.getByTestId("header-toolbar");
    await user.click(within(toolbar).getByRole("button", { name: t("tools.refresh") }));
    await waitFor(() =>
      expect(transport.calls.filter((call) => call === "listTools")).toHaveLength(2),
    );
  });

  it("toggles a server for the managed client with an explicit choice", async () => {
    const setServerClients = vi.fn(async () => ok(MOCK_CHANGE));
    const { user } = renderPage({ setServerClients });
    await user.click(
      await screen.findByRole("button", { name: t("tools.manageAria", { name: "Claude Code" }) }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: t("tools.dialog.title", { name: "Claude Code" }),
    });
    expect(await within(dialog).findByText("alpha")).toBeInTheDocument();
    expect(within(dialog).queryByText("broken")).not.toBeInTheDocument();
    expect(within(dialog).getByText(t("tools.dialog.summary", { enabled: 1, total: 2 }))).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: t("tools.dialog.item.enable", { name: "alpha" }) }));
    await waitFor(() =>
      expect(setServerClients).toHaveBeenCalledWith("alpha", {
        allDetected: false,
        clients: ["claude-code", "codex"],
      }),
    );

    await user.click(within(dialog).getByRole("button", { name: t("tools.dialog.item.disable", { name: "beta" }) }));
    await waitFor(() =>
      expect(setServerClients).toHaveBeenLastCalledWith("beta", { allDetected: false, clients: ["codex"] }),
    );

    await user.click(within(dialog).getByRole("button", { name: t("tools.dialog.done") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
