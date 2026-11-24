import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { McpPage } from "@/app/routes/mcp/McpPage";
import { ToolsPage } from "@/app/routes/tools/ToolsPage";
import { t } from "@/i18n/en";
import type { ImportScope, ProjectRow, ProjectServerList, UnmanagedSummary } from "@/ipc/bindings";
import { MOCK_CLIENTS, MOCK_SERVER_LIST, MOCK_TOOLS, ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const PROJECT: ProjectRow = {
  id: "web-shop-3f9a",
  name: "web-shop",
  path: "~/Code/web-shop",
  layout: "shared",
  trust: "trusted",
  error: null,
  servers: 0,
  clients: 0,
  pending: 0,
  readOnly: null,
  readOnlyFiles: [],
};

const USER_SUMMARY: UnmanagedSummary = {
  groups: [
    {
      client: "claude-code",
      scope: "user",
      projectId: null,
      files: ["~/.claude.json"],
      entries: [
        { id: "claude-code/docs", name: "docs", new: true },
        { id: "claude-code/fetch", name: "fetch", new: false },
      ],
      readOnly: null,
    },
    {
      client: "codex",
      scope: "user",
      projectId: null,
      files: ["~/.codex/config.toml"],
      entries: [{ id: "codex/docs", name: "docs", new: false }],
      readOnly: null,
    },
  ],
  total: 3,
  new: 1,
};

const PROJECT_SUMMARY: UnmanagedSummary = {
  groups: [
    {
      client: "claude-code",
      scope: "project",
      projectId: PROJECT.id,
      files: ["<P>/.mcp.json"],
      entries: [{ id: `claude-code@${PROJECT.id}/linear`, name: "linear", new: false }],
      readOnly: null,
    },
  ],
  total: 1,
  new: 0,
};

const PROJECT_SERVERS: ProjectServerList = {
  project: {
    id: PROJECT.id,
    name: PROJECT.name,
    path: PROJECT.path,
    trust: "trusted",
    hash: "h",
    layers: ["shared", "local"],
    defaultClients: { allDetected: true, clients: [] },
    readOnly: null,
    readOnlyLayers: [],
  },
  servers: [],
  inherited: [],
  clients: MOCK_CLIENTS,
  storeServers: [],
  warnings: [],
};

/** Every scope as the backend answers it: with the notice on, the lists cut `all`; off, they ask for their scope. */
const unmanagedSummary = async (scope: ImportScope | null) => {
  if (scope?.kind === "all") {
    return ok({ groups: [...USER_SUMMARY.groups, ...PROJECT_SUMMARY.groups], total: 4, new: 1 });
  }
  return ok(scope?.kind === "project" ? PROJECT_SUMMARY : USER_SUMMARY);
};

describe("not managed by Loadout (#3)", () => {
  it("lists the MCP page's foreign entries per client and file, with counts", async () => {
    const onOpenImport = vi.fn();
    const { user } = renderWithProviders(
      <McpPage onOpenPending={vi.fn()} onOpenInstall={vi.fn()} onOpenImport={onOpenImport} />,
      { commands: { listServers: async () => ok(MOCK_SERVER_LIST), unmanagedSummary } },
    );
    expect(await screen.findByText("0 managed, 3 not managed by Loadout")).toBeInTheDocument();
    const section = screen.getByRole("region", { name: "Not managed by Loadout (3)" });
    const claude = within(section).getByRole("group", { name: "Claude Code" });
    expect(within(claude).getByText("~/.claude.json")).toBeInTheDocument();
    expect(within(claude).getByText("2 not managed")).toBeInTheDocument();
    expect(within(claude).getByText(t("import.unmanaged.new"))).toBeInTheDocument();
    // Read only: the one action is Import…, which points the Import page at the entry.
    await user.click(within(section).getByRole("button", { name: "Import docs from Codex…" }));
    expect(onOpenImport).toHaveBeenCalledWith({ name: "docs", findingId: "codex/docs" });
  });

  it("lists a project's foreign entries at project scope under its folder", async () => {
    renderWithProviders(<McpPage onOpenPending={vi.fn()} onOpenInstall={vi.fn()} onOpenImport={vi.fn()} />, {
      scope: { kind: "project", id: PROJECT.id },
      projects: [PROJECT],
      commands: { listProjectServers: async () => ok(PROJECT_SERVERS), unmanagedSummary },
    });
    expect(await screen.findByText("0 managed, 1 not managed by Loadout")).toBeInTheDocument();
    const group = screen.getByRole("group", { name: "Claude Code in web-shop" });
    expect(within(group).getByText("~/Code/web-shop/.mcp.json")).toBeInTheDocument();
    expect(within(group).getByText("linear")).toBeInTheDocument();
  });

  it("shows nothing when every entry is managed", async () => {
    const { transport } = renderWithProviders(
      <McpPage onOpenPending={vi.fn()} onOpenInstall={vi.fn()} onOpenImport={vi.fn()} />,
    );
    expect(await screen.findByText("0 managed, 0 not managed by Loadout")).toBeInTheDocument();
    expect(transport.calls).toContain("unmanagedSummary");
    expect(screen.queryByRole("region", { name: /Not managed by Loadout/ })).toBeNull();
  });

  it("counts a client's entries on its Tools card and lists them in Manage", async () => {
    const onOpenImport = vi.fn();
    const { user } = renderWithProviders(<ToolsPage onOpenMcp={vi.fn()} onOpenImport={onOpenImport} />, {
      commands: { listTools: async () => ok(MOCK_TOOLS), listServers: async () => ok(MOCK_SERVER_LIST), unmanagedSummary },
    });
    const card = await screen.findByRole("article", { name: "Claude Code" });
    expect(await within(card).findByText("2 not managed")).toBeInTheDocument();

    await user.click(within(card).getByRole("button", { name: t("tools.manageAria", { name: "Claude Code" }) }));
    const dialog = await screen.findByRole("dialog", { name: t("tools.dialog.title", { name: "Claude Code" }) });
    expect(within(dialog).getByText("0 managed, 2 not managed by Loadout")).toBeInTheDocument();
    const section = within(dialog).getByRole("region", { name: "Not managed by Loadout (2)" });
    // Only this client's entries.
    expect(within(section).queryByRole("group", { name: "Codex" })).toBeNull();
    await user.click(within(section).getByRole("button", { name: "Import fetch from Claude Code…" }));
    expect(onOpenImport).toHaveBeenCalledWith({ name: "fetch", findingId: "claude-code/fetch" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
