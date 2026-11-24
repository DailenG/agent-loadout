// M38 read-only contexts (#4) on the mock transport: every surface that
// shows a read-only file, folder or project says so with the lock, the label
// and the reason in plain words, and disables the controls that would write it
// (never hides them). Viewing, importing and copying stay available.

import { screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ApplyDrawer } from "@/app/apply/ApplyDrawer";
import { ReadOnlyBadge, readOnlyReason, readOnlyTooltip } from "@/app/components/ReadOnlyBadge";
import { McpImportSection } from "@/app/routes/import/McpImportSection";
import { DiscoverProjectsSection } from "@/app/routes/import/projects/DiscoverProjectsSection";
import { UnmanagedSection } from "@/app/routes/import/UnmanagedSection";
import { DISABLE_ALL_CHOICE, enableAllChoice } from "@/app/routes/mcp/ClientPill";
import { McpPage } from "@/app/routes/mcp/McpPage";
import { EffectiveMatrixView } from "@/app/routes/projects/EffectiveMatrix";
import { ProjectsPage } from "@/app/routes/projects/ProjectsPage";
import { SkillSourceSwitcher, STORE_SOURCE } from "@/app/routes/skills/SkillSourceSwitcher";
import { ToolSyncPanel } from "@/app/routes/skills/ToolSyncPanel";
import { ToolsPage } from "@/app/routes/tools/ToolsPage";
import { ScopeSwitcher } from "@/app/scope/ScopeSwitcher";
import { errorMessage, t } from "@/i18n/en";
import type {
  ClientPill,
  EffectiveMatrix,
  ImportCandidate,
  PendingPlan,
  ProjectRow,
  ProjectServerList,
  ProjectServerRow,
  ReadOnly,
  ServerRow,
  SkillClient,
  SkillInfo,
  ToolInfo,
} from "@/ipc/bindings";
import { MOCK_APPLY_RUN, MOCK_CHANGE, MOCK_CLIENTS, ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { ScopeDriver, driveProject } from "@/test/ScopeDriver";

const ACL: ReadOnly = { kind: "acl" };
const ATTRIBUTE: ReadOnly = { kind: "attribute" };

const LOCKED: ProjectRow = {
  id: "secure-9d40",
  name: "secure-portal",
  path: "~/Code/secure-portal",
  layout: "shared",
  trust: "trusted",
  error: null,
  servers: 1,
  clients: 2,
  pending: 1,
  readOnly: ACL,
  readOnlyFiles: [
    { file: "<P>/.loadout/project.toml", kind: "acl" },
    { file: "<P>/.mcp.json", kind: "acl" },
  ],
};

const PARTLY: ProjectRow = {
  ...LOCKED,
  id: "gateway-81c2",
  name: "api-gateway",
  path: "~/Code/api-gateway",
  readOnly: null,
  readOnlyFiles: [{ file: "<P>/.loadout/project.toml", kind: "attribute" }],
};

function pill(client: string, selected: boolean, readOnly: ReadOnly | null = null): ClientPill {
  return { client, selected, state: selected ? "enabled" : "disabled", reason: null, readOnly };
}

function serverRow(pills: ClientPill[]): ServerRow {
  return {
    id: "github",
    displayName: "GitHub",
    description: null,
    transport: "stdio",
    launch: "github-mcp",
    origin: null,
    tags: [],
    clients: { allDetected: false, clients: pills.filter((item) => item.selected).map((item) => item.client) },
    pills,
    problems: [],
    invalid: null,
  };
}

describe("ReadOnlyBadge", () => {
  it.each<[ReadOnly["kind"], string]>([
    ["acl", "its permissions do not let your account write it"],
    ["attribute", "it is marked read only"],
    ["owner", "another user owns it and your account has no write permission"],
    ["share", "the network share or drive it is on does not allow writing"],
  ])("says why in plain words for %s", (kind, reason) => {
    expect(readOnlyReason(kind)).toBe(reason);
    expect(readOnlyTooltip({ kind }, "~/.cursor/mcp.json")).toBe(
      `~/.cursor/mcp.json is read only: ${reason}. Loadout shows it and does not change it.`,
    );
  });

  it("shows the lock and the label, focusable, described by the reason; nothing when writable", () => {
    const { container } = renderWithProviders(
      <>
        <ReadOnlyBadge readOnly={ATTRIBUTE} path="~/.claude.json" />
        <ReadOnlyBadge readOnly={null} path="~/.codex/config.toml" />
      </>,
    );
    const badge = screen.getByText(t("readOnly.label")).closest(".read-only-badge");
    expect(badge).toHaveAttribute("tabindex", "0");
    expect(badge).toHaveAccessibleDescription(readOnlyTooltip(ATTRIBUTE, "~/.claude.json"));
    expect(badge?.querySelector("svg")).not.toBeNull();
    expect(container.querySelectorAll(".read-only-badge")).toHaveLength(1);
  });
});

describe("MCP list (user scope)", () => {
  it("disables a read-only pill with the reason and marks the row; Disable all sends what it always did", async () => {
    const row = serverRow([pill("claude-code", true), pill("codex", false), pill("cursor", true, ACL)]);
    const { user, transport } = renderWithProviders(
      <McpPage onOpenPending={vi.fn()} onOpenInstall={vi.fn()} onOpenImport={vi.fn()} />,
      { commands: { listServers: async () => ok({ servers: [row], clients: MOCK_CLIENTS, warnings: [] }) } },
    );
    const card = await screen.findByRole("article", { name: "GitHub" });
    expect(within(card).getByText(t("readOnly.label"))).toBeInTheDocument();
    await user.click(within(card).getByRole("button", { name: t("mcp.card.expandAria", { name: "GitHub" }) }));

    const cursor = await screen.findByRole("button", { name: /^Cursor: enabled, Read only$/ });
    expect(cursor).toBeDisabled();
    expect(cursor).toHaveAccessibleDescription(readOnlyTooltip(ACL));
    expect(cursor.closest(".mcp-pill-wrap")).toHaveAttribute("data-tooltip", readOnlyTooltip(ACL));

    await user.click(screen.getByRole("button", { name: t("mcp.card.disableAllApps", { name: "GitHub" }) }));
    const call = transport.invocations.find((item) => item.command === "setServerClients");
    expect(call?.args).toEqual(["github", DISABLE_ALL_CHOICE]);
  });

  it("Enable all keeps a read-only client in the choice: file permissions never change the store", () => {
    const row = serverRow([pill("claude-code", false), pill("codex", true), pill("cursor", false, ACL)]);
    const clients = MOCK_CLIENTS.map((client) => (client.id === "cursor" ? { ...client, detected: true } : client));
    expect(enableAllChoice(row, clients).clients).toContain("cursor");
  });

  it("the apply toast counts the files that were read only and not written", async () => {
    const row = serverRow([pill("claude-code", true), pill("codex", false)]);
    const file = {
      file: "~/.cursor/mcp.json",
      clients: ["cursor"],
      scope: "user" as const,
      outcome: "blocked" as const,
      written: false,
      counts: { create: 0, update: 0, remove: 0, disable: 0 },
      error: { code: "read_only_target" as const, message: "access is denied" },
      notes: [],
      warnings: [],
      readOnly: ACL,
    };
    const { user } = renderWithProviders(
      <McpPage onOpenPending={vi.fn()} onOpenInstall={vi.fn()} onOpenImport={vi.fn()} />,
      {
        commands: {
          listServers: async () => ok({ servers: [row], clients: MOCK_CLIENTS, warnings: [] }),
          setServerClients: async () => ok({ ...MOCK_CHANGE, apply: { ...MOCK_APPLY_RUN, files: [file] } }),
        },
      },
    );
    const card = await screen.findByRole("article", { name: "GitHub" });
    await user.click(within(card).getByRole("button", { name: t("mcp.card.expandAria", { name: "GitHub" }) }));
    await user.click(await screen.findByRole("button", { name: /^Codex: disabled$/ }));
    expect(await screen.findByText(new RegExp(t("apply.readOnly.one").replace(".", "\\.")))).toBeInTheDocument();
  });
});

describe("MCP page at a read-only project scope", () => {
  const editRow: ProjectServerRow = {
    name: "vault",
    kind: "define",
    from: null,
    layers: ["shared"],
    editLayer: "shared",
    transport: "stdio",
    launch: "vault-mcp",
    clients: { allDetected: false, clients: ["claude-code"] },
    usesDefaults: false,
    pills: [pill("claude-code", true)],
    problems: [],
    invalid: null,
  };

  function list(project: Partial<ProjectServerList["project"]>): ProjectServerList {
    return {
      project: {
        id: PARTLY.id,
        name: PARTLY.name,
        path: PARTLY.path,
        trust: "trusted",
        hash: "h",
        layers: ["shared", "local"],
        defaultClients: { allDetected: true, clients: [] },
        readOnly: null,
        readOnlyLayers: [],
        ...project,
      },
      servers: [editRow, { ...editRow, name: "notes", layers: ["local"], editLayer: "local" }],
      inherited: [{ id: "noisy", displayName: "Noisy", transport: "stdio", launch: "noisy-mcp" }],
      clients: MOCK_CLIENTS,
      storeServers: ["noisy"],
      warnings: [],
    };
  }

  async function render(data: ProjectServerList) {
    const view = renderWithProviders(
      <>
        <ScopeDriver projectIds={[PARTLY.id]} />
        <McpPage onOpenPending={vi.fn()} onOpenInstall={vi.fn()} onOpenImport={vi.fn()} />
      </>,
      { commands: { listProjects: async () => ok([PARTLY]), listProjectServers: async () => ok(data) } },
    );
    await view.user.click(screen.getByRole("button", { name: driveProject(PARTLY.id) }));
    return view;
  }

  it("a read-only shared manifest: its rows are view-only, the private manifest's rows stay editable", async () => {
    await render(list({ readOnlyLayers: [{ layer: "shared", kind: "attribute" }] }));
    expect(
      await screen.findByText(t("readOnly.layer.shared", { reason: readOnlyReason("attribute") })),
    ).toBeInTheDocument();
    const vault = screen.getByRole("article", { name: "vault" });
    expect(within(vault).getByRole("button", { name: t("mcp.card.edit", { name: "vault" }) })).toBeDisabled();
    expect(within(vault).getByRole("button", { name: t("mcp.card.delete", { name: "vault" }) })).toBeDisabled();
    expect(within(vault).getByText(t("readOnly.label"))).toBeInTheDocument();
    const notes = screen.getByRole("article", { name: "notes" });
    expect(within(notes).getByRole("button", { name: t("mcp.card.edit", { name: "notes" }) })).toBeEnabled();
    // Hide writes into the writable private manifest instead.
    expect(screen.getByRole("button", { name: t("mcp.project.inherited.hideAria", { name: "Noisy" }) })).toBeEnabled();
    expect(screen.getByRole("button", { name: t("mcp.toolbar.add") })).toBeEnabled();
  });

  it("a whole read-only project: the note says why and every edit is disabled", async () => {
    await render(list({ readOnly: ACL, readOnlyLayers: [{ layer: "shared", kind: "acl" }, { layer: "local", kind: "acl" }] }));
    expect(
      await screen.findByText(t("readOnly.project.whole", { name: PARTLY.name, reason: readOnlyReason("acl") })),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("mcp.project.inherited.hideAria", { name: "Noisy" }) })).toBeDisabled();
    expect(screen.getByRole("button", { name: t("mcp.project.inherited.overrideAria", { name: "Noisy" }) })).toBeDisabled();
    expect(screen.getByRole("button", { name: t("mcp.toolbar.add") })).toBeDisabled();
    // Viewing stays: the rows can still be expanded.
    const notes = screen.getByRole("article", { name: "notes" });
    expect(within(notes).getByRole("button", { name: t("mcp.card.expandAria", { name: "notes" }) })).toBeEnabled();
  });
});

describe("Projects list and scope switcher", () => {
  it("shows a read-only project with the lock, a visible reason and Apply disabled; a partly read-only one lists its files", async () => {
    renderWithProviders(<ProjectsPage />, { projects: [LOCKED, PARTLY] });
    const locked = await screen.findByRole("article", { name: "secure-portal" });
    const whole = t("readOnly.project.whole", { name: "secure-portal", reason: readOnlyReason("acl") });
    expect(within(locked).getByText(t("readOnly.label"))).toBeInTheDocument();
    expect(within(locked).getByRole("note")).toHaveTextContent(whole);
    const apply = within(locked).getByRole("button", { name: t("projects.action.apply") });
    expect(apply).toBeDisabled();
    expect(apply).toHaveAccessibleDescription(whole);
    expect(within(locked).getByRole("button", { name: t("projects.action.effective") })).toBeEnabled();

    const partly = screen.getByRole("article", { name: "api-gateway" });
    const files = within(partly).getByRole("list", { name: t("readOnly.project.filesAria", { name: "api-gateway" }) });
    expect(within(files).getByText("~/Code/api-gateway/.loadout/project.toml")).toBeInTheDocument();
    expect(within(partly).getByRole("button", { name: t("projects.action.apply") })).toBeEnabled();
  });

  it("marks a read-only project in the scope switcher", async () => {
    const { user } = renderWithProviders(<ScopeSwitcher onExpandSidebar={vi.fn()} />, { projects: [LOCKED, PARTLY] });
    await user.click(await screen.findByRole("combobox", { name: t("scope.switcher.aria") }));
    const option = await screen.findByRole("option", { name: /secure-portal/ });
    expect(within(option).getByRole("img", { name: t("readOnly.label") })).toBeInTheDocument();
    expect(option.getAttribute("data-tooltip")).toContain(
      t("readOnly.project.whole", { name: "secure-portal", reason: readOnlyReason("acl") }),
    );
    const other = screen.getByRole("option", { name: /api-gateway/ });
    expect(within(other).queryByRole("img", { name: t("readOnly.label") })).not.toBeInTheDocument();
  });
});

describe("Tools cards", () => {
  const cursor: ToolInfo = {
    id: "cursor",
    name: "Cursor",
    tier: "b",
    detected: true,
    supported: true,
    readOnly: false,
    userFiles: [{ path: "~/.cursor/mcp.json", exists: true, readOnly: ACL }],
    projectFiles: ["<P>/.cursor/mcp.json", "<P>/.mcp.json"],
    privateProjectTarget: false,
    transports: { stdio: true, http: true, sse: false },
    envRefs: true,
    restartRequired: false,
    warnings: [],
    homepage: null,
  };

  it("marks each read-only file path, user files and the active project's files", async () => {
    const view = renderWithProviders(
      <>
        <ScopeDriver projectIds={[LOCKED.id]} />
        <ToolsPage onOpenMcp={vi.fn()} onOpenImport={vi.fn()} />
      </>,
      { commands: { listTools: async () => ok([cursor]), listProjects: async () => ok([LOCKED]) } },
    );
    const card = await screen.findByRole("article", { name: "Cursor" });
    const userFiles = within(card).getByRole("list", { name: t("tools.userFilesAria", { name: "Cursor" }) });
    const badge = within(userFiles).getByText(t("readOnly.label")).closest(".read-only-badge");
    expect(badge).toHaveAccessibleDescription(readOnlyTooltip(ACL, "~/.cursor/mcp.json"));

    await view.user.click(screen.getByRole("button", { name: driveProject(LOCKED.id) }));
    const projectFiles = await within(card).findByRole("list", { name: t("tools.projectFilesAria", { name: "Cursor" }) });
    const items = within(projectFiles).getAllByRole("listitem");
    expect(within(items[0]).queryByText(t("readOnly.label"))).not.toBeInTheDocument();
    expect(within(items[1]).getByText(t("readOnly.label"))).toBeInTheDocument();
  });
});

describe("Effective matrix", () => {
  it("marks a cell whose winning file is read only, and the file in its provenance", async () => {
    const data: EffectiveMatrix = {
      projectId: LOCKED.id,
      projectName: LOCKED.name,
      path: LOCKED.path,
      clients: [{ id: "claude-code", name: "Claude Code", detected: true }],
      rows: [
        {
          name: "vault",
          cells: [
            {
              client: "claude-code",
              winner: { layer: "project", file: "<P>/.mcp.json", readOnly: ACL },
              source: "project:vault",
              loaded: true,
              shadowed: [{ layer: "system", file: "~/.claude.json", readOnly: null }],
              alsoVisibleTo: [],
              status: "synced",
              warnings: [],
              snippet: null,
            },
          ],
        },
      ],
      warnings: [],
    };
    const { user } = renderWithProviders(<EffectiveMatrixView projectId={LOCKED.id} onBack={vi.fn()} />, {
      commands: { effectiveMatrix: async () => ok(data) },
    });
    const cell = await screen.findByRole("button", { name: /, Read only$/ });
    expect(cell).toHaveAttribute("data-tooltip", readOnlyTooltip(ACL, "<P>/.mcp.json"));
    await user.click(cell);
    const detail = screen.getByRole("complementary");
    expect(within(detail).getAllByText(t("readOnly.label"))).toHaveLength(1);
  });
});

describe("Pending changes drawer", () => {
  it("shows the lock and the blocked reason row on a read-only file", async () => {
    const plan: PendingPlan = {
      applyMode: "review",
      files: [
        {
          file: "~/.cursor/mcp.json",
          clients: ["cursor"],
          scope: "user",
          projectId: null,
          project: null,
          counts: { create: 0, update: 0, remove: 0, disable: 0 },
          diff: "",
          entries: [
            {
              action: "none",
              name: "github",
              client: "cursor",
              server: "github",
              state: "blocked",
              unresolved: false,
              problem: { code: "read_only_target", message: "access is denied" },
            },
          ],
          problem: null,
          warnings: [],
          readOnly: ACL,
        },
      ],
      pending: 1,
      writes: 0,
      unresolved: 0,
      warnings: [],
    };
    const { user } = renderWithProviders(<ApplyDrawer open onClose={vi.fn()} />, {
      commands: { pendingPlan: async () => ok(plan) },
    });
    expect(await screen.findByText(t("readOnly.drawer.row", { reason: readOnlyReason("acl") }))).toBeInTheDocument();
    expect(screen.getByText(t("readOnly.label"))).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /~\/\.cursor\/mcp\.json/ }));
    expect(screen.getByText(errorMessage("read_only_target"))).toBeInTheDocument();
  });
});

describe("Skills", () => {
  const clients: SkillClient[] = [
    { id: "claude-code", name: "Claude Code", detected: true, readOnly: null },
    { id: "cursor", name: "Cursor", detected: true, readOnly: ACL },
  ];
  const skill: SkillInfo = {
    id: "code-review",
    name: "code-review",
    description: null,
    fields: [],
    tags: [],
    source: null,
    allDetected: false,
    clients: [],
    method: "link",
    projectEnabled: false,
    projectShared: false,
    links: [],
    bytes: 1_024,
    hasContent: true,
    problems: [],
  };

  it("disables a read-only client's pill with the reason; Enable all still sends all detected", async () => {
    const { user, transport } = renderWithProviders(<ToolSyncPanel skill={skill} clients={clients} />);
    const cursor = screen.getByRole("button", { name: t("skill.tools.aria.enable", { name: "Cursor" }) });
    expect(cursor).toBeDisabled();
    const reason = t("readOnly.skills.folderNoPath", { client: "Cursor", reason: readOnlyReason("acl") });
    expect(cursor).toHaveAccessibleDescription(expect.stringContaining(reason));
    await user.click(screen.getByRole("button", { name: t("skill.tools.enableAll") }));
    const call = transport.invocations.find((item) => item.command === "setSkillClients");
    expect(call?.args.slice(0, 3)).toEqual([skill.id, [], true]);
  });

  it("marks a read-only skill folder tab", () => {
    renderWithProviders(
      <SkillSourceSwitcher
        active={STORE_SOURCE}
        storeCount={1}
        tools={[{ id: "cursor", name: "Cursor", count: 1, readOnly: ACL, path: "~/.cursor/skills" }]}
        onChange={vi.fn()}
      />,
    );
    const tab = screen.getByRole("tab", { name: "Cursor 1, Read only" });
    expect(tab).toHaveAccessibleDescription(readOnlyTooltip(ACL, "~/.cursor/skills"));
  });
});

describe("Import", () => {
  it("marks a read-only source file and keeps importing its definition available", async () => {
    const candidate: ImportCandidate = {
      name: "docs",
      inStore: false,
      conflict: false,
      variants: [
        {
          key: "cursor",
          suggestedId: "docs",
          draft: {
            id: "docs",
            displayName: null,
            description: null,
            transport: "stdio",
            command: "docs-mcp",
            args: [],
            url: null,
            cwd: null,
            env: [],
            headers: [],
            clients: { allDetected: false, clients: [] },
            delivery: null,
            toolsAllow: [],
            toolsDeny: [],
          },
          enables: true,
          sources: [
            {
              key: "cursor",
              id: "cursor/docs",
              client: "cursor",
              scope: "user",
              projectId: null,
              trusted: null,
              file: "~/.cursor/mcp.json",
              readOnly: ACL,
            },
          ],
          secrets: [],
          error: null,
        },
      ],
    };
    renderWithProviders(<McpImportSection target={null} onOpenPending={vi.fn()} />, {
      commands: { scanImport: async () => ok({ candidates: [candidate], warnings: [] }) },
    });
    const badge = (await screen.findByText(t("readOnly.label"))).closest(".read-only-badge");
    expect(badge).toHaveAccessibleDescription(
      t("readOnly.import.source", { file: "~/.cursor/mcp.json", reason: readOnlyReason("acl") }),
    );
  });

  it("marks a read-only not-managed group; Import stays enabled", () => {
    renderWithProviders(
      <UnmanagedSection
        summary={{
          groups: [
            {
              client: "cursor",
              scope: "user",
              projectId: null,
              files: ["~/.cursor/mcp.json"],
              entries: [{ id: "cursor/docs", name: "docs", new: true }],
              readOnly: ACL,
            },
          ],
          total: 1,
          new: 1,
        }}
        clientNames={new Map([["cursor", "Cursor"]])}
        onImport={vi.fn()}
      />,
    );
    expect(screen.getByText(t("readOnly.label"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("import.unmanaged.importAria", { name: "docs", client: "Cursor" }) })).toBeEnabled();
  });

  it("Discover: a read-only folder without a manifest cannot be added and says why; one with a manifest can", async () => {
    renderWithProviders(<DiscoverProjectsSection />, {
      commands: {
        discoverProjects: async () =>
          ok({
            candidates: [
              { id: "d-1", name: "payments", path: "~/Code/payments", clients: ["cursor"], manifest: null, error: null, readOnly: ACL },
              {
                id: "d-2",
                name: "docs",
                path: "~/Code/docs",
                clients: ["cursor"],
                manifest: { id: "docs-1", name: "Docs", layout: "shared", known: false },
                error: null,
                readOnly: ACL,
              },
            ],
            warnings: [],
          }),
      },
    });
    const boxes = await screen.findAllByRole("checkbox");
    expect(boxes[0]).toBeDisabled();
    expect(boxes[1]).toBeEnabled();
    expect(screen.getByText(t("readOnly.discover.noManifest", { reason: readOnlyReason("acl") }))).toBeInTheDocument();
  });
});
