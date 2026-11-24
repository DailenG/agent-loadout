import { screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectModalKeyboard } from "@/app/components/modalKeyboard";
import { ToolsPage } from "@/app/routes/tools/ToolsPage";
import { t } from "@/i18n/en";
import type { ProjectRow, ProjectServerList, ServerList, ServerRow, ToolInfo, TrustReview } from "@/ipc/bindings";
import {
  MOCK_CHANGE,
  MOCK_CLIENTS,
  mockIpcError,
  ok,
  type MockTransportOptions,
} from "@/test/mockTransport";
import { renderWithProviders, type RenderOptions } from "@/test/renderWithProviders";
import { ScopeDriver, driveProject } from "@/test/ScopeDriver";

const CURSOR: ToolInfo = {
  id: "cursor",
  name: "Cursor",
  tier: "b",
  detected: false,
  supported: true,
  readOnly: false,
  userFiles: [{ path: "~/.cursor/mcp.json", exists: false, readOnly: null }],
  projectFiles: [],
  transports: { stdio: true, http: true, sse: false },
  envRefs: false,
  restartRequired: true,
  warnings: [],
  homepage: null,
};

const CLAUDE: ToolInfo = {
  id: "claude-code",
  name: "Claude Code",
  tier: "a",
  detected: true,
  supported: true,
  readOnly: false,
  userFiles: [{ path: "~/.claude.json", exists: true, readOnly: null }],
  projectFiles: ["<P>/.mcp.json"],
  transports: { stdio: true, http: true, sse: true },
  envRefs: true,
  restartRequired: false,
  warnings: [],
  homepage: null,
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

const OMP: ToolInfo = {
  ...CURSOR,
  id: "omp",
  name: "omp",
  tier: "a",
  userFiles: [{ path: "~/.omp/agent/mcp.json", exists: false, readOnly: null }],
  homepage: "https://github.com/can1357/oh-my-pi",
};

function pills(selected: string[]): ServerRow["pills"] {
  return MOCK_CLIENTS.map((client) => ({
    client: client.id,
    selected: selected.includes(client.id),
    state: selected.includes(client.id) ? "enabled" : "disabled",
    reason: null,
    readOnly: null,
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

const PROJECT: ProjectRow = {
  id: "web-7k2f",
  name: "web",
  path: "~/Code/web",
  layout: "shared",
  trust: "trusted",
  error: null,
  servers: 2,
  clients: 1,
  pending: 0,
  readOnly: null,
  readOnlyFiles: [],
};

const HASH = "sha256:shown";

function manifestRow(name: string, clients: string[]): ProjectServerList["servers"][number] {
  return {
    name,
    kind: "define",
    from: null,
    layers: ["shared"],
    editLayer: "shared",
    transport: "stdio",
    launch: `${name}-mcp`,
    clients: { allDetected: false, clients },
    usesDefaults: false,
    pills: pills(clients),
    problems: [],
    invalid: null,
  };
}

const PROJECT_SERVERS: ProjectServerList = {
  project: {
    id: PROJECT.id,
    name: "web",
    path: "~/Code/web",
    trust: "trusted",
    hash: HASH,
    layers: ["shared", "local"],
    defaultClients: { allDetected: true, clients: [] },
    readOnly: null,
    readOnlyLayers: [],
  },
  servers: [
    manifestRow("docs", ["claude-code", "codex"]),
    { ...manifestRow("quiet", []), kind: "suppress", from: "quiet" },
  ],
  inherited: [{ id: "noisy", displayName: "Noisy", transport: "stdio", launch: "noisy-mcp" }],
  clients: MOCK_CLIENTS,
  storeServers: ["noisy", "quiet"],
  warnings: [],
};

function renderPage(commands: MockTransportOptions["commands"] = {}, options: Omit<RenderOptions, "commands"> = {}) {
  const onOpenMcp = vi.fn();
  const onOpenImport = vi.fn();
  const view = renderWithProviders(
    <>
      <ScopeDriver projectIds={[PROJECT.id]} />
      <ToolsPage onOpenMcp={onOpenMcp} onOpenImport={onOpenImport} />
    </>,
    {
      ...options,
      commands: {
        listTools: async () => ok([CURSOR, CLAUDE, LEGACY]),
        listServers: async () => ok(SERVERS),
        listProjects: async () => ok([PROJECT]),
        listProjectServers: async () => ok(PROJECT_SERVERS),
        ...commands,
      },
    },
  );
  return { onOpenMcp, onOpenImport, ...view };
}

async function openManage(user: UserEvent, name: string) {
  await user.click(await screen.findByRole("button", { name: t("tools.manageAria", { name }) }));
  return screen.findByRole("dialog", { name: t("tools.dialog.title", { name }) });
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
        ".mcp.json",
      ),
    ).toBeInTheDocument();
    expect(claude.getByText("stdio / HTTP / SSE")).toBeInTheDocument();

    const cursor = within(cards[1]);
    const cursorFile = cursor.getByText(/~\/\.cursor\/mcp\.json/);
    expect(cursorFile.closest("li")).toHaveClass("is-missing");
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

  it("keeps detected tools first, puts omp before other undetected tools, and badges it", async () => {
    renderPage({ listTools: async () => ok([CURSOR, CLAUDE, LEGACY, OMP]) });
    const cards = await screen.findAllByRole("article");
    expect(cards.map((card) => card.getAttribute("aria-label"))).toEqual([
      "Claude Code",
      "omp",
      "Cursor",
      "Claude Desktop",
    ]);
    const badge = within(cards[1]).getByText(t("recommended.badge"));
    expect(badge).toHaveAttribute("data-tooltip", t("recommended.tooltip"));
    expect(badge).toHaveAttribute("tabindex", "0");
    expect(badge).toHaveAccessibleDescription(t("recommended.tooltip"));
    expect(within(cards[0]).queryByText(t("recommended.badge"))).not.toBeInTheDocument();
  });

  it("puts a detected omp first and offers no Get action for it", async () => {
    renderPage({ listTools: async () => ok([CURSOR, CLAUDE, { ...OMP, detected: true }]) });
    const cards = await screen.findAllByRole("article");
    expect(cards.map((card) => card.getAttribute("aria-label"))).toEqual(["omp", "Claude Code", "Cursor"]);
    expect(
      within(cards[0]).queryByRole("button", { name: t("tools.getAria", { name: "omp" }) }),
    ).not.toBeInTheDocument();
  });

  it("offers Get omp for an undetected omp and opens its homepage by id", async () => {
    const openToolHomepage = vi.fn(async () => ok(null));
    const { user } = renderPage({ listTools: async () => ok([CLAUDE, CURSOR, OMP]), openToolHomepage });
    const omp = within((await screen.findAllByRole("article"))[1]);
    await user.click(omp.getByRole("button", { name: t("tools.getAria", { name: "omp" }) }));
    await waitFor(() => expect(openToolHomepage).toHaveBeenCalledWith("omp"));
    // Only omp has a homepage: Cursor gets no Get action.
    expect(screen.queryByRole("button", { name: t("tools.getAria", { name: "Cursor" }) })).not.toBeInTheDocument();
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

  it("follows the sidebar to a project and names that project's files", async () => {
    const { user } = renderPage();
    const claude = within((await screen.findAllByRole("article"))[0]);
    expect(claude.getByText(t("tools.projectFiles"))).toBeInTheDocument();
    expect(claude.getByText(".mcp.json")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: driveProject(PROJECT.id) }));
    expect(await claude.findByText(t("tools.projectFilesIn", { name: "web" }))).toBeInTheDocument();
    expect(
      within(claude.getByRole("list", { name: t("tools.projectFilesAria", { name: "Claude Code" }) })).getByText(
        "~/Code/web/.mcp.json",
      ),
    ).toBeInTheDocument();
    // The page renders no scope select of its own.
    expect(screen.queryByRole("combobox", { name: /scope/i })).not.toBeInTheDocument();
  });

  it("toggles the client in a project's manifest entry with the shown hash", async () => {
    const setProjectServerClients = vi.fn(async () => ok(MOCK_CHANGE));
    const { user, transport, onOpenMcp } = renderPage(
      { setProjectServerClients },
      { scope: { kind: "project", id: PROJECT.id } },
    );
    const dialog = await openManage(user, "Claude Code");
    expect(await within(dialog).findByText("docs")).toBeInTheDocument();
    expect(within(dialog).getByText(t("tools.dialog.projectNote", { name: "web" }))).toBeInTheDocument();
    // A suppression has no per-client switch; the store list is not read.
    expect(within(dialog).queryByText("quiet")).not.toBeInTheDocument();
    expect(transport.calls).not.toContain("listServers");
    expect(within(dialog).getByText(t("tools.dialog.summary", { enabled: 1, total: 1 }))).toBeInTheDocument();

    const docsSwitch = within(dialog).getByRole("button", { name: t("tools.dialog.item.disable", { name: "docs" }) });
    await waitFor(() => expect(docsSwitch).toBeEnabled());
    await user.click(docsSwitch);
    await waitFor(() =>
      expect(setProjectServerClients).toHaveBeenCalledWith(PROJECT.id, HASH, "docs", "shared", {
        allDetected: false,
        clients: ["codex"],
      }),
    );
    expect(transport.calls).not.toContain("setServerClients");

    // Inherited store servers are read only here, with a way to the MCP page.
    const inherited = within(dialog).getByRole("region", { name: t("tools.dialog.inherited.title") });
    expect(within(inherited).queryByRole("button", { name: /Enable|Disable/ })).not.toBeInTheDocument();
    await user.click(
      within(inherited).getByRole("button", { name: t("tools.dialog.inherited.openAria", { name: "Noisy" }) }),
    );
    expect(onOpenMcp).toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("keeps the Manage dialog read only for an untrusted project and offers the trust review", async () => {
    const review: TrustReview = {
      id: PROJECT.id,
      name: "web",
      path: "~/Code/web",
      trust: "untrusted",
      hash: "sha256:reviewed",
      files: [],
      warnings: [],
    };
    const { user, transport } = renderPage(
      {
        listProjectServers: async () =>
          ok({ ...PROJECT_SERVERS, project: { ...PROJECT_SERVERS.project, trust: "untrusted" } }),
        reviewProject: async () => ok(review),
      },
      { scope: { kind: "project", id: PROJECT.id } },
    );
    const dialog = await openManage(user, "Claude Code");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("mcp.project.untrusted"));
    expect(
      within(dialog).getByRole("button", { name: t("tools.dialog.item.disable", { name: "docs" }) }),
    ).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: t("mcp.project.review") }));
    expect(await screen.findByRole("button", { name: t("projects.review.trust") })).toBeInTheDocument();
    expect(transport.calls).not.toContain("setProjectServerClients");
  });

  it("moves focus into Manage and back to its button (#11)", async () => {
    const { user } = renderPage();
    const manage = await screen.findByRole("button", { name: t("tools.manageAria", { name: "Claude Code" }) });
    await expectModalKeyboard(user, {
      trigger: manage,
      name: t("tools.dialog.title", { name: "Claude Code" }),
      initial: (dialog) => within(dialog).getByRole("searchbox"),
    });
  });

  it("stacks the trust review over Manage: Escape closes one at a time and focus walks back (#11)", async () => {
    const review: TrustReview = {
      id: PROJECT.id,
      name: "web",
      path: "~/Code/web",
      trust: "untrusted",
      hash: "sha256:reviewed",
      files: [],
      warnings: [],
    };
    const { user } = renderPage(
      {
        listProjectServers: async () =>
          ok({ ...PROJECT_SERVERS, project: { ...PROJECT_SERVERS.project, trust: "untrusted" } }),
        reviewProject: async () => ok(review),
      },
      { scope: { kind: "project", id: PROJECT.id } },
    );
    const manage = await screen.findByRole("button", { name: t("tools.manageAria", { name: "Claude Code" }) });
    manage.focus();
    await user.keyboard("{Enter}");
    const dialog = await screen.findByRole("dialog", { name: t("tools.dialog.title", { name: "Claude Code" }) });
    const reviewButton = await within(dialog).findByRole("button", { name: t("mcp.project.review") });

    await expectModalKeyboard(user, {
      trigger: reviewButton,
      name: t("projects.review.title", { name: "web" }),
      initial: (reviewDialog) => reviewDialog,
    });
    // Only the review closed; Manage has the keyboard again.
    expect(dialog).toBeInTheDocument();
    expect(dialog.closest("[inert]")).toBeNull();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(manage).toHaveFocus();
  });
});
