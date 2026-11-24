import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { expectModalKeyboard } from "@/app/components/modalKeyboard";
import { t } from "@/i18n/en";
import type { ChangeResult, ProjectRow, ProjectServerList, ProjectServerRow, TrustReview } from "@/ipc/bindings";
import type { CommandResult } from "@/ipc/transport";
import { MOCK_CHANGE, MOCK_CLIENTS, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { DRIVE_USER, ScopeDriver, driveProject } from "@/test/ScopeDriver";
import { McpPage } from "./McpPage";

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

function makeRow(overrides: Partial<ProjectServerRow>): ProjectServerRow {
  return {
    name: "docs",
    kind: "define",
    from: null,
    layers: ["shared"],
    editLayer: "shared",
    transport: "stdio",
    launch: "docs-mcp --stdio",
    clients: { allDetected: false, clients: ["claude-code"] },
    usesDefaults: false,
    pills: [
      { client: "claude-code", selected: true, state: "enabled", reason: null, readOnly: null },
      { client: "codex", selected: false, state: "disabled", reason: null, readOnly: null },
      { client: "cursor", selected: false, state: "disabled", reason: null, readOnly: null },
      {
        client: "claude-desktop",
        selected: false,
        state: "not_supported",
        reason: { code: "scope_not_supported", message: "detail" },
        readOnly: null,
      },
    ],
    problems: [],
    invalid: null,
    ...overrides,
  };
}

const LIST: ProjectServerList = {
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
    makeRow({}),
    makeRow({
      name: "github",
      kind: "override",
      from: "github",
      layers: ["local"],
      editLayer: "local",
      launch: "github-mcp --project",
      clients: { allDetected: true, clients: [] },
      usesDefaults: true,
    }),
  ],
  inherited: [{ id: "noisy", displayName: "Noisy", transport: "stdio", launch: "noisy-mcp" }],
  clients: MOCK_CLIENTS,
  storeServers: ["github", "noisy"],
  warnings: [],
};

const REVIEW: TrustReview = {
  id: PROJECT.id,
  name: "web",
  path: "~/Code/web",
  trust: "changed",
  hash: "sha256:reviewed",
  files: [
    {
      file: "project.toml",
      path: "<P>/.loadout/project.toml",
      change: "changed",
      content: null,
      diff: ["--- trusted project.toml", "+++ <P>/.loadout/project.toml", '+command = "pulled-mcp"'].join("\n"),
    },
  ],
  warnings: [],
};

async function renderProjectScope(commands: MockTransportOptions["commands"] = {}) {
  const onOpenPending = vi.fn();
  const view = renderWithProviders(
    <>
      <ScopeDriver projectIds={[PROJECT.id]} />
      <McpPage onOpenPending={onOpenPending} onOpenInstall={() => undefined} onOpenImport={() => undefined} />
    </>,
    {
      commands: {
        listProjects: async () => ok([PROJECT]),
        listProjectServers: async () => ok(LIST),
        ...commands,
      },
    },
  );
  // The page renders no scope select of its own: the sidebar's scope drives it.
  expect(screen.queryByRole("combobox", { name: /scope/i })).not.toBeInTheDocument();
  await view.user.click(screen.getByRole("button", { name: driveProject(PROJECT.id) }));
  return { onOpenPending, ...view };
}

function invocation(transport: { invocations: { command: string; args: unknown[] }[] }, command: string) {
  return transport.invocations.find((item) => item.command === command)?.args;
}

describe("McpPage at project scope", () => {
  it("switches to a project and lists its manifest servers and what it inherits", async () => {
    const { transport } = await renderProjectScope();
    const docs = await screen.findByRole("article", { name: "docs" });
    expect(invocation(transport, "listProjectServers")).toEqual([PROJECT.id]);
    expect(
      screen.getByText(t("mcp.project.note", { name: "web", path: "~/Code/web" })),
    ).toBeInTheDocument();
    expect(within(docs).getByText(t("mcp.project.kind.define"))).toBeInTheDocument();
    expect(within(docs).getByText(t("mcp.project.layer.shared"))).toBeInTheDocument();
    expect(within(docs).getByText("docs-mcp --stdio")).toBeInTheDocument();

    const github = screen.getByRole("article", { name: "github" });
    expect(within(github).getByText(t("mcp.project.kind.override", { from: "github" }))).toBeInTheDocument();
    expect(within(github).getByText(t("mcp.project.layer.local"))).toBeInTheDocument();

    const inherited = screen.getByRole("region", { name: t("mcp.project.inherited.title") });
    expect(within(inherited).getByText("Noisy")).toBeInTheDocument();
    // The store-only toolbar actions are not offered here.
    expect(screen.queryByRole("button", { name: t("mcp.toolbar.paste") })).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("toggles a client in the entry's manifest file with the shown hash", async () => {
    const setProjectServerClients = vi.fn(async () => ok(MOCK_CHANGE));
    const { user } = await renderProjectScope({ setProjectServerClients });
    const docs = await screen.findByRole("article", { name: "docs" });
    await user.click(within(docs).getByRole("button", { name: t("mcp.card.expandAria", { name: "docs" }) }));

    const codex = within(docs).getByRole("button", {
      name: t("mcp.pill.aria", { client: "Codex", state: t("mcp.pill.state.disabled") }),
    });
    await user.click(codex);
    await waitFor(() =>
      expect(setProjectServerClients).toHaveBeenCalledWith(PROJECT.id, HASH, "docs", "shared", {
        allDetected: false,
        clients: ["claude-code", "codex"],
      }),
    );
    // Claude Desktop has no project config: the pill cannot be toggled.
    expect(
      within(docs).getByRole("button", {
        name: t("mcp.pill.aria", { client: "Claude Desktop", state: t("mcp.pill.state.not_supported") }),
      }),
    ).toBeDisabled();

    // Disable All writes an empty list (no client), not the defaults. Every
    // action waits until the toggle's refetch is done.
    const disableAll = within(docs).getByRole("button", { name: t("mcp.card.disableAllApps", { name: "docs" }) });
    await waitFor(() => expect(disableAll).toBeEnabled());
    await user.click(disableAll);
    await waitFor(() =>
      expect(setProjectServerClients).toHaveBeenLastCalledWith(PROJECT.id, HASH, "docs", "shared", {
        allDetected: false,
        clients: [],
      }),
    );
  });

  it("adds a server to the private manifest through the dialog", async () => {
    const saveProjectServer = vi.fn(async () => ok(MOCK_CHANGE));
    const { user } = await renderProjectScope({ saveProjectServer });
    await screen.findByRole("article", { name: "docs" });
    await user.click(screen.getByRole("button", { name: t("mcp.toolbar.add") }));

    const dialog = await screen.findByRole("dialog", { name: t("mcp.project.dialog.createTitle") });
    await user.type(within(dialog).getByLabelText(t("mcp.project.dialog.name")), "postgres");
    await user.click(within(dialog).getByRole("combobox", { name: t("mcp.project.dialog.layer") }));
    await user.click(await screen.findByRole("option", { name: t("mcp.project.dialog.layer.local") }));
    await user.type(within(dialog).getByLabelText(t("mcp.dialog.command")), "uvx");
    await user.type(within(dialog).getByLabelText(t("mcp.dialog.args")), "postgres-mcp{Enter}");
    // Off "Use the project's default clients": pick Claude Code only.
    await user.click(within(dialog).getByRole("checkbox", { name: t("mcp.project.dialog.useDefaults") }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Claude Code" }));
    await user.click(within(dialog).getByRole("button", { name: t("mcp.dialog.save") }));

    await waitFor(() =>
      expect(saveProjectServer).toHaveBeenCalledWith(
        PROJECT.id,
        HASH,
        {
          name: "postgres",
          layer: "local",
          kind: "define",
          from: null,
          transport: "stdio",
          command: "uvx",
          args: ["postgres-mcp"],
          url: null,
          env: [],
          headers: [],
          clients: { allDetected: false, clients: ["claude-code"] },
          overShared: false,
        },
        true,
      ),
    );
    expect(await screen.findByText(t("mcp.project.saved", { name: "postgres", project: "web" }))).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: t("mcp.project.dialog.createTitle") })).not.toBeInTheDocument();
  });

  it("hides an inherited store server and overrides another from the store", async () => {
    const saveProjectServer = vi.fn(async () => ok(MOCK_CHANGE));
    const { user } = await renderProjectScope({ saveProjectServer });
    const inherited = await screen.findByRole("region", { name: t("mcp.project.inherited.title") });
    await user.click(within(inherited).getByRole("button", { name: t("mcp.project.inherited.hideAria", { name: "Noisy" }) }));
    await waitFor(() =>
      expect(saveProjectServer).toHaveBeenCalledWith(
        PROJECT.id,
        HASH,
        expect.objectContaining({ name: "noisy", kind: "suppress", layer: "shared", clients: null }),
        true,
      ),
    );

    await user.click(
      within(inherited).getByRole("button", { name: t("mcp.project.inherited.overrideAria", { name: "Noisy" }) }),
    );
    const dialog = await screen.findByRole("dialog", { name: t("mcp.project.dialog.createTitle") });
    expect(within(dialog).getByLabelText(t("mcp.project.dialog.name"))).toHaveValue("noisy");
    await user.type(within(dialog).getByLabelText(t("mcp.dialog.args")), "--quiet{Enter}");
    await user.click(within(dialog).getByRole("button", { name: t("mcp.dialog.save") }));
    await waitFor(() =>
      expect(saveProjectServer).toHaveBeenLastCalledWith(
        PROJECT.id,
        HASH,
        expect.objectContaining({ name: "noisy", kind: "override", from: "noisy", transport: null, args: ["--quiet"] }),
        true,
      ),
    );
  });

  it("edits a table in its own file and removes a server on the second click", async () => {
    const getProjectServer = vi.fn(async () =>
      ok({
        name: "github",
        layer: "local" as const,
        kind: "override" as const,
        from: "github",
        transport: null,
        command: null,
        args: ["--project"],
        url: null,
        env: [],
        headers: [],
        clients: null,
        overShared: false,
      }),
    );
    const removeProjectServer = vi.fn(async () => ok(MOCK_CHANGE));
    const { user } = await renderProjectScope({ getProjectServer, removeProjectServer });
    const github = await screen.findByRole("article", { name: "github" });
    await user.click(within(github).getByRole("button", { name: t("mcp.card.edit", { name: "github" }) }));
    const dialog = await screen.findByRole("dialog", { name: t("mcp.project.dialog.editTitle") });
    expect(getProjectServer).toHaveBeenCalledWith(PROJECT.id, "github", "local");
    expect(within(dialog).getByText("--project")).toBeInTheDocument();
    expect(within(dialog).getByLabelText(t("mcp.project.dialog.name"))).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: t("mcp.dialog.cancel") }));

    await user.click(within(github).getByRole("button", { name: t("mcp.card.delete", { name: "github" }) }));
    expect(removeProjectServer).not.toHaveBeenCalled();
    await user.click(
      within(github).getByRole("button", {
        name: t("mcp.card.deleteConfirmAria", { state: t("mcp.card.deleteConfirm"), name: "github" }),
      }),
    );
    await waitFor(() => expect(removeProjectServer).toHaveBeenCalledWith(PROJECT.id, HASH, "github", "local"));
  });

  it("keeps clients this machine opts out of when a toggle writes the manifest's list", async () => {
    const setProjectServerClients = vi.fn(async () => ok(MOCK_CHANGE));
    const optedOut = makeRow({
      // project.toml names Cursor; this machine opts out of it, so its pill
      // is not selected here.
      clients: { allDetected: false, clients: ["claude-code", "cursor"] },
      pills: [
        { client: "claude-code", selected: true, state: "enabled", reason: null, readOnly: null },
        { client: "codex", selected: false, state: "disabled", reason: null, readOnly: null },
        { client: "cursor", selected: false, state: "disabled", reason: null, readOnly: null },
      ],
    });
    const { user } = await renderProjectScope({
      listProjectServers: async () => ok({ ...LIST, servers: [optedOut] }),
      setProjectServerClients,
    });
    const docs = await screen.findByRole("article", { name: "docs" });
    await user.click(within(docs).getByRole("button", { name: t("mcp.card.expandAria", { name: "docs" }) }));
    await user.click(
      within(docs).getByRole("button", {
        name: t("mcp.pill.aria", { client: "Codex", state: t("mcp.pill.state.disabled") }),
      }),
    );
    await waitFor(() =>
      expect(setProjectServerClients).toHaveBeenCalledWith(PROJECT.id, HASH, "docs", "shared", {
        allDetected: false,
        clients: ["claude-code", "cursor", "codex"],
      }),
    );
  });

  it("runs one project edit at a time and waits for the fresh hash", async () => {
    // The lib target (ES2022) has no Promise.withResolvers.
    let finish: (value: CommandResult<ChangeResult>) => void = () => undefined;
    const setProjectServerClients = vi.fn(
      () => new Promise<CommandResult<ChangeResult>>((resolve) => {
        finish = resolve;
      }),
    );
    const listProjectServers = vi.fn(async () => ok(LIST));
    const { user } = await renderProjectScope({ listProjectServers, setProjectServerClients });
    const docs = await screen.findByRole("article", { name: "docs" });
    const github = screen.getByRole("article", { name: "github" });
    await user.click(within(docs).getByRole("button", { name: t("mcp.card.expandAria", { name: "docs" }) }));
    await user.click(
      within(docs).getByRole("button", {
        name: t("mcp.pill.aria", { client: "Codex", state: t("mcp.pill.state.disabled") }),
      }),
    );
    // While the first edit runs, the other card cannot start one with the
    // same (soon stale) hash. (One card is open at a time.)
    await user.click(within(github).getByRole("button", { name: t("mcp.card.expandAria", { name: "github" }) }));
    const otherPill = within(github).getByRole("button", {
      name: t("mcp.pill.aria", { client: "Codex", state: t("mcp.pill.state.disabled") }),
    });
    await waitFor(() => expect(otherPill).toBeDisabled());
    expect(within(github).getByRole("button", { name: t("mcp.card.delete", { name: "github" }) })).toBeDisabled();
    expect(screen.getByRole("button", { name: t("mcp.toolbar.add") })).toBeDisabled();
    await user.click(otherPill);
    expect(setProjectServerClients).toHaveBeenCalledTimes(1);

    const listed = listProjectServers.mock.calls.length;
    finish(ok(MOCK_CHANGE));
    // Free again once the list was fetched anew.
    await waitFor(() => expect(otherPill).toBeEnabled());
    expect(listProjectServers.mock.calls.length).toBeGreaterThan(listed);
    await user.click(otherPill);
    await waitFor(() => expect(setProjectServerClients).toHaveBeenCalledTimes(2));
    expect(setProjectServerClients).toHaveBeenLastCalledWith(PROJECT.id, HASH, "github", "local", expect.anything());
  });

  it("labels a private table over the shared entry as a change of that entry", async () => {
    const getProjectServer = vi.fn(async () =>
      ok({
        name: "docs",
        layer: "local" as const,
        kind: "define" as const,
        from: null,
        transport: null,
        command: null,
        args: [],
        url: null,
        env: [],
        headers: [],
        clients: { allDetected: false, clients: ["claude-code"] },
        overShared: true,
      }),
    );
    const saveProjectServer = vi.fn(async () => ok(MOCK_CHANGE));
    const { user } = await renderProjectScope({
      listProjectServers: async () =>
        ok({ ...LIST, servers: [makeRow({ layers: ["shared", "local"], editLayer: "local" })] }),
      getProjectServer,
      saveProjectServer,
    });
    const docs = await screen.findByRole("article", { name: "docs" });
    await user.click(within(docs).getByRole("button", { name: t("mcp.card.edit", { name: "docs" }) }));
    const dialog = await screen.findByRole("dialog", { name: t("mcp.project.dialog.editTitle") });
    expect(within(dialog).getByText(t("mcp.project.dialog.kind.overShared"))).toBeInTheDocument();
    expect(within(dialog).getByText(t("mcp.project.dialog.overSharedHint"))).toBeInTheDocument();
    expect(within(dialog).getByText(t("mcp.project.dialog.transportShared"))).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("mcp.dialog.save") }));
    // Saved as it was: no transport is added to the partial table.
    await waitFor(() =>
      expect(saveProjectServer).toHaveBeenCalledWith(
        PROJECT.id,
        HASH,
        expect.objectContaining({ layer: "local", kind: "define", transport: null }),
        false,
      ),
    );
  });

  it("keeps an untrusted or changed project read only and offers the trust review", async () => {
    const trustProject = vi.fn(async (id: string) => ok({ id, trust: "trusted" as const, ...MOCK_CHANGE }));
    const { user, transport } = await renderProjectScope({
      listProjectServers: async () => ok({ ...LIST, project: { ...LIST.project, trust: "changed" } }),
      reviewProject: async () => ok(REVIEW),
      trustProject,
    });
    const docs = await screen.findByRole("article", { name: "docs" });
    expect(screen.getByRole("alert")).toHaveTextContent(t("mcp.project.changed"));
    expect(screen.getByRole("button", { name: t("mcp.toolbar.add") })).toBeDisabled();
    expect(within(docs).getByRole("button", { name: t("mcp.card.edit", { name: "docs" }) })).toBeDisabled();
    expect(within(docs).getByRole("button", { name: t("mcp.card.delete", { name: "docs" }) })).toBeDisabled();
    await user.click(within(docs).getByRole("button", { name: t("mcp.card.expandAria", { name: "docs" }) }));
    expect(
      within(docs).getByRole("button", {
        name: t("mcp.pill.aria", { client: "Codex", state: t("mcp.pill.state.disabled") }),
      }),
    ).toBeDisabled();
    const inherited = screen.getByRole("region", { name: t("mcp.project.inherited.title") });
    expect(within(inherited).getByRole("button", { name: t("mcp.project.inherited.hideAria", { name: "Noisy" }) })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: t("mcp.project.review") }));
    await user.click(await screen.findByRole("button", { name: t("projects.review.trust") }));
    await waitFor(() => expect(trustProject).toHaveBeenCalledWith(PROJECT.id, REVIEW.hash));
    const edits = ["saveProjectServer", "removeProjectServer", "setProjectServerClients"];
    expect(transport.calls.some((command) => edits.includes(command))).toBe(false);
  });

  it("moves focus into the project server dialog and back to Add (#11)", async () => {
    const { user } = await renderProjectScope();
    await screen.findByRole("article", { name: "docs" });
    await expectModalKeyboard(user, {
      trigger: screen.getByRole("button", { name: t("mcp.toolbar.add") }),
      name: t("mcp.project.dialog.createTitle"),
      initial: (dialog) => within(dialog).getByLabelText(t("mcp.project.dialog.name")),
    });
  });

  it("moves focus into the trust review and back to Review (#11)", async () => {
    const { user } = await renderProjectScope({
      listProjectServers: async () => ok({ ...LIST, project: { ...LIST.project, trust: "changed" } }),
      reviewProject: async () => ok(REVIEW),
    });
    await screen.findByRole("article", { name: "docs" });
    await expectModalKeyboard(user, {
      trigger: screen.getByRole("button", { name: t("mcp.project.review") }),
      name: t("projects.review.title", { name: REVIEW.name }),
      initial: (dialog) => dialog,
    });
  });

  it("follows the sidebar back to User and closes an open editor", async () => {
    const { user } = await renderProjectScope();
    await screen.findByRole("article", { name: "docs" });
    const add = screen.getByRole("button", { name: t("mcp.toolbar.add") });
    await waitFor(() => expect(add).toBeEnabled());
    await user.click(add);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: DRIVE_USER }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.queryByRole("article", { name: "docs" })).not.toBeInTheDocument();
    expect(within(screen.getByTestId("header-toolbar")).getByRole("button", { name: t("mcp.toolbar.paste") })).toBeInTheDocument();
  });
});
