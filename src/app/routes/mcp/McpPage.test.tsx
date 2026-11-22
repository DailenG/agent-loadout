import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { errorMessage } from "@/i18n/en";
import type { ServerList, ServerRow } from "@/ipc/bindings";
import { MOCK_APPLY_RUN, MOCK_CLIENTS, ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { McpPage } from "./McpPage";

function makeRow(overrides: Partial<ServerRow>): ServerRow {
  return {
    id: "demo",
    displayName: null,
    description: null,
    transport: "stdio",
    launch: "demo-server",
    origin: null,
    tags: [],
    clients: { allDetected: false, clients: [] },
    pills: [
      { client: "claude-code", selected: false, state: "disabled", reason: null },
      { client: "codex", selected: false, state: "disabled", reason: null },
    ],
    problems: [],
    invalid: null,
    ...overrides,
  };
}

const LIST: ServerList = {
  clients: MOCK_CLIENTS,
  warnings: [],
  servers: [
    makeRow({
      id: "issues",
      displayName: "Issue Tracker",
      description: "Issues and pull requests",
      launch: "issue-server --token ${secret:issue-token}",
      clients: { allDetected: false, clients: ["claude-code"] },
      pills: [
        { client: "claude-code", selected: true, state: "blocked", reason: { code: "missing_secret", message: "detail" } },
        { client: "codex", selected: false, state: "disabled", reason: null },
      ],
      problems: [{ code: "missing_secret", message: "detail" }],
    }),
    makeRow({ id: "files", launch: "files-server /tmp/fake-root" }),
    makeRow({
      id: "broken",
      transport: null,
      launch: "",
      pills: [],
      invalid: { code: "parse_error", message: "detail" },
    }),
  ],
};

function renderPage() {
  const onOpenPending = vi.fn();
  const onOpenInstall = vi.fn();
  const view = renderWithProviders(<McpPage onOpenPending={onOpenPending} onOpenInstall={onOpenInstall} />, {
    commands: { listServers: async () => ok(LIST) },
  });
  return { onOpenPending, onOpenInstall, ...view };
}

describe("McpPage", () => {
  it("lists servers with problems and invalid rows", async () => {
    renderPage();
    const issues = await screen.findByRole("article", { name: "Issue Tracker" });
    expect(within(issues).getByText(errorMessage("missing_secret"))).toBeInTheDocument();
    expect(within(issues).getByText("1 enabled")).toBeInTheDocument();
    expect(within(issues).getByText("STDIO")).toBeInTheDocument();
    expect(within(issues).getByText("System")).toBeInTheDocument();

    const broken = screen.getByRole("article", { name: "broken" });
    expect(within(broken).getByText(errorMessage("parse_error"))).toBeInTheDocument();
    expect(within(broken).getByText("Invalid")).toBeInTheDocument();
    expect(within(broken).getByRole("button", { name: "Edit broken" })).toBeInTheDocument();
    expect(within(broken).getByRole("button", { name: "Delete broken" })).toBeInTheDocument();
    expect(within(broken).getAllByRole("button")).toHaveLength(2);
  });

  it("filters by id, display name and launch text", async () => {
    const { user } = renderPage();
    await screen.findByRole("article", { name: "Issue Tracker" });
    const search = screen.getByRole("searchbox", { name: "Search MCP" });

    await user.type(search, "fake-root");
    expect(screen.getByRole("article", { name: "files" })).toBeInTheDocument();
    expect(screen.queryByRole("article", { name: "Issue Tracker" })).toBeNull();

    await user.clear(search);
    await user.type(search, "tracker");
    expect(screen.getByRole("article", { name: "Issue Tracker" })).toBeInTheDocument();
    expect(screen.queryByRole("article", { name: "files" })).toBeNull();

    await user.clear(search);
    await user.type(search, "nothing-matches");
    expect(screen.getByText("No matching MCP")).toBeInTheDocument();
  });

  it("deletes a server only on the second click", async () => {
    const { user, transport } = renderPage();
    const files = await screen.findByRole("article", { name: "files" });
    await user.click(within(files).getByRole("button", { name: "Delete files" }));
    expect(transport.calls).not.toContain("removeServers");

    await user.click(within(files).getByRole("button", { name: "Confirm files" }));
    await waitFor(() => expect(transport.calls).toContain("removeServers"));
    const call = transport.invocations.find((item) => item.command === "removeServers");
    expect(call?.args[0]).toEqual(["files"]);
  });

  it("shows the pills of an expanded row and enables all ready clients", async () => {
    const { user, transport, onOpenPending } = renderPage();
    await screen.findByRole("article", { name: "Issue Tracker" });
    await user.click(screen.getByRole("button", { name: "Expand Issue Tracker" }));
    expect(screen.getByText("issue-server --token ${secret:issue-token}")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Claude Code: blocked" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Codex: disabled" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Cursor:/ })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Enable Issue Tracker in all tools" }));
    await waitFor(() => expect(transport.calls).toContain("setServerClients"));
    const call = transport.invocations.find((item) => item.command === "setServerClients");
    expect(call?.args.slice(0, 2)).toEqual(["issues", { allDetected: false, clients: ["claude-code", "codex"] }]);
    expect(onOpenPending).not.toHaveBeenCalled();
  });

  it("offers Undo this apply after a save and undoes that run", async () => {
    const { user, transport } = renderPage();
    await screen.findByRole("article", { name: "Issue Tracker" });
    const toolbar = screen.getByTestId("header-toolbar");
    await user.click(within(toolbar).getByRole("button", { name: "Add" }));
    await user.type(screen.getByLabelText("ID"), "new-server");
    await user.click(screen.getByRole("button", { name: "Save" }));

    const undo = await screen.findByRole("button", { name: "Undo this apply" });
    expect(screen.getByText("Saved new-server.")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
    await user.click(undo);
    await waitFor(() => expect(transport.calls).toContain("undoApply"));
    const call = transport.invocations.find((item) => item.command === "undoApply");
    expect(call?.args.slice(0, 2)).toEqual([MOCK_APPLY_RUN.applyId, false]);
  });

  it("opens the dialog on the paste box from Paste config", async () => {
    const { user } = renderPage();
    await screen.findByRole("article", { name: "Issue Tracker" });
    await user.click(within(screen.getByTestId("header-toolbar")).getByRole("button", { name: "Paste config" }));
    expect(screen.getByLabelText("Paste a config snippet")).toHaveFocus();
  });

  it("shows the empty state with Add and Paste config", async () => {
    renderWithProviders(<McpPage onOpenPending={vi.fn()} onOpenInstall={vi.fn()} />);
    expect(await screen.findByText("No MCP servers yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add server" })).toBeInTheDocument();
    const toolbar = screen.getByTestId("header-toolbar");
    expect(screen.getAllByRole("button", { name: "Paste config" }).some((button) => !toolbar.contains(button))).toBe(true);
  });
});
