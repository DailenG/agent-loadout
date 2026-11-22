import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ProjectRow, SkillInfo, SkillList, SkillScope, ToolSkillDir } from "@/ipc/bindings";
import {
  MOCK_SKILL_CHANGE,
  MOCK_SKILL_CLIENTS,
  MOCK_SKILL_INSTALLED,
  mockIpcError,
  ok,
  type MockTransportOptions,
} from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { SkillsPage } from "./SkillsPage";

function makeSkill(overrides: Partial<SkillInfo>): SkillInfo {
  return {
    id: "repo-notes",
    name: "repo-notes",
    description: "Notes about this repository.",
    fields: [
      { key: "name", value: "repo-notes" },
      { key: "description", value: "Notes about this repository." },
      { key: "license", value: "MIT" },
    ],
    tags: [],
    source: { kind: "local", url: null, path: null, gitRef: null, commit: null },
    allDetected: false,
    clients: [],
    method: "link",
    projectEnabled: false,
    projectShared: false,
    links: [],
    bytes: 2048,
    hasContent: true,
    problems: [],
    ...overrides,
  };
}

const LIST: SkillList = {
  clients: MOCK_SKILL_CLIENTS,
  project: null,
  agentsCli: null,
  sizeWarning: null,
  skills: [
    makeSkill({}),
    makeSkill({
      id: "alpha",
      name: "alpha",
      description: "Alpha skill.",
      source: { kind: "git", url: "https://github.com/acme/skills", path: "skills/alpha", gitRef: "main", commit: "3f2a9c1aa" },
      clients: ["claude-code"],
      links: [{ client: "claude-code", scope: "user", projectId: null, path: "~/.claude/skills/alpha", method: "junction" }],
      tags: ["team"],
    }),
  ],
};

const PROJECT: ProjectRow = {
  id: "web-7k2f",
  name: "web",
  path: "~/Code/web",
  layout: "private",
  trust: "trusted",
  error: null,
  servers: 0,
  clients: 0,
  pending: 0,
};

const CLAUDE_DIR: ToolSkillDir = {
  client: "claude-code",
  name: "Claude Code",
  dir: "~/.claude/skills",
  exists: true,
  entries: [
    { name: "alpha", path: "~/.claude/skills/alpha", class: "managed", linkTarget: null, skillId: "alpha", description: "Alpha skill." },
    { name: "hand-made", path: "~/.claude/skills/hand-made", class: "unmanaged", linkTarget: null, skillId: null, description: "Made by hand." },
    { name: "elsewhere", path: "~/.claude/skills/elsewhere", class: "mismatch", linkTarget: "~/other/elsewhere", skillId: null, description: null },
  ],
};

function renderPage(commands: MockTransportOptions["commands"] = {}) {
  const onOpenInstall = vi.fn();
  const view = renderWithProviders(<SkillsPage onOpenInstall={onOpenInstall} />, {
    commands: {
      listSkills: async () => ok(LIST),
      listSkillToolDirs: async () => ok([CLAUDE_DIR]),
      ...commands,
    },
  });
  return { onOpenInstall, ...view };
}

describe("SkillsPage", () => {
  it("lists store skills with their origin, enabled tools and tags", async () => {
    renderPage();
    const alpha = await screen.findByRole("article", { name: "alpha" });
    expect(within(alpha).getByText("Git")).toBeInTheDocument();
    expect(within(alpha).getByText("1 enabled")).toBeInTheDocument();
    expect(within(alpha).getByRole("button", { name: "Edit tag team" })).toBeInTheDocument();
    const notes = screen.getByRole("article", { name: "repo-notes" });
    expect(within(notes).getByText("Local")).toBeInTheDocument();
    expect(within(notes).getByText("Disabled")).toBeInTheDocument();
    // Source tabs: the store, then each client folder with its entry count.
    expect(screen.getByRole("tab", { name: /Store/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "Claude Code 3" })).toBeInTheDocument();
  });

  it("enables a skill for Claude Code from its Enable in Tools pill", async () => {
    const { user, transport } = renderPage();
    const notes = await screen.findByRole("article", { name: "repo-notes" });
    await user.click(within(notes).getByRole("button", { name: "Expand repo-notes" }));
    // Every frontmatter field, the source and the size are shown.
    expect(within(notes).getByText("license")).toBeInTheDocument();
    expect(within(notes).getByText("MIT")).toBeInTheDocument();
    expect(within(notes).getByText("2 KB")).toBeInTheDocument();

    await user.click(within(notes).getByRole("button", { name: "Enable Claude Code" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({
        command: "setSkillClients",
        args: ["repo-notes", ["claude-code"], true, null],
      }),
    );
    expect(await screen.findByText("Enabled repo-notes.")).toBeInTheDocument();
  });

  it("disables everywhere from Disable All and names entries that need attention", async () => {
    const { user, transport } = renderPage({
      setSkillClients: async () =>
        ok({
          commit: MOCK_SKILL_CHANGE.commit,
          links: {
            needsAttention: true,
            warnings: [],
            notes: [
              {
                client: "codex",
                scope: "user",
                projectId: null,
                skillId: "alpha",
                path: "~/.codex/skills/alpha",
                method: "junction",
                status: "foreign",
                foundTarget: null,
                error: null,
              },
            ],
          },
        }),
    });
    const alpha = await screen.findByRole("article", { name: "alpha" });
    await user.click(within(alpha).getByRole("button", { name: "Expand alpha" }));
    expect(within(alpha).getByText("~/.claude/skills/alpha")).toBeInTheDocument();
    await user.click(within(alpha).getByRole("button", { name: "Disable All" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({ command: "setSkillClients", args: ["alpha", [], false, null] }),
    );
    expect(
      await screen.findByText(/codex ~\/\.codex\/skills\/alpha: a real folder is in the way/),
    ).toBeInTheDocument();
  });

  it("deletes a skill after a second click", async () => {
    const { user, transport } = renderPage();
    const notes = await screen.findByRole("article", { name: "repo-notes" });
    await user.click(within(notes).getByRole("button", { name: "Delete repo-notes" }));
    expect(transport.calls).not.toContain("removeSkill");
    await user.click(within(notes).getByRole("button", { name: "Confirm repo-notes" }));
    await waitFor(() => expect(transport.calls).toContain("removeSkill"));
  });

  it("adds a tag through the tag editor", async () => {
    const { user, transport } = renderPage();
    const notes = await screen.findByRole("article", { name: "repo-notes" });
    await user.click(within(notes).getByRole("button", { name: "Add tag" }));
    const editor = screen.getByRole("dialog", { name: "Set tag for repo-notes" });
    await user.type(within(editor).getByRole("textbox"), "docs{Enter}");
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({ command: "setSkillTags", args: ["repo-notes", ["docs"]] }),
    );
  });

  it("checks updates on request, previews the diff, then updates", async () => {
    const { user, transport } = renderPage({
      checkSkillUpdates: async () =>
        ok([
          {
            id: "alpha",
            status: "available",
            current: "3f2a9c1aa",
            latest: "9b8c7d6ee",
            changes: [{ path: "SKILL.md", kind: "modified" }],
            error: null,
          },
          { id: "repo-notes", status: "not_tracked", current: null, latest: null, changes: [], error: null },
        ]),
      skillUpdateDiff: async (_id, file) => ok({ path: file, before: "# Alpha\n", after: "# Alpha v2\n", binary: false }),
    });
    await screen.findByRole("article", { name: "alpha" });
    expect(transport.calls).not.toContain("checkSkillUpdates");
    await user.click(screen.getByRole("button", { name: "Check updates" }));
    expect(await screen.findByText("1 skills have updates.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Update (1)" })).toBeInTheDocument();

    const alpha = screen.getByRole("article", { name: "alpha" });
    expect(within(alpha).getByText("Update available")).toBeInTheDocument();
    await user.click(within(alpha).getByRole("button", { name: "Preview the update of alpha" }));
    const dialog = await screen.findByRole("dialog", { name: "Update alpha" });
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({ command: "skillUpdateDiff", args: ["alpha", "SKILL.md"] }),
    );
    expect(within(dialog).getByText("Modified")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Update" }));
    // The update names the commit the preview showed.
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({ command: "updateSkill", args: ["alpha", "9b8c7d6ee"] }),
    );
    // The card's entry is cleared: no stale "Update available" or Update (1).
    await waitFor(() => expect(within(alpha).queryByText("Update available")).toBeNull());
    expect(screen.queryByRole("button", { name: "Update (1)" })).toBeNull();
    expect(screen.getByRole("button", { name: "Check updates" })).toBeInTheDocument();
  });

  it("Update all keeps only the failed entries", async () => {
    const check = (id: string, latest: string) => ({
      id,
      status: "available" as const,
      current: "1111111",
      latest,
      changes: [{ path: "SKILL.md", kind: "modified" as const }],
      error: null,
    });
    const { user, transport } = renderPage({
      checkSkillUpdates: async () => ok([check("alpha", "aaaaaaa"), check("repo-notes", "bbbbbbb")]),
      updateSkill: async (id, latest) =>
        id === "alpha"
          ? ok({ id, from: null, to: latest, commit: MOCK_SKILL_CHANGE.commit, changes: [], links: MOCK_SKILL_CHANGE.links })
          : mockIpcError({ code: "entry_changed", message: "changed" }),
    });
    await screen.findByRole("article", { name: "alpha" });
    await user.click(screen.getByRole("button", { name: "Check updates" }));
    await user.click(await screen.findByRole("button", { name: "Update (2)" }));
    expect(await screen.findByRole("button", { name: "Update (1)" })).toBeInTheDocument();
    expect(transport.invocations.filter((call) => call.command === "updateSkill").map((call) => call.args)).toEqual([
      ["alpha", "aaaaaaa"],
      ["repo-notes", "bbbbbbb"],
    ]);
    expect(within(screen.getByRole("article", { name: "alpha" })).queryByText("Update available")).toBeNull();
    expect(within(screen.getByRole("article", { name: "repo-notes" })).getByText("Update available")).toBeInTheDocument();
  });

  it("switches link or copy without touching the clients", async () => {
    const { user, transport } = renderPage();
    const alpha = await screen.findByRole("article", { name: "alpha" });
    await user.click(within(alpha).getByRole("button", { name: "Expand alpha" }));
    await user.click(within(alpha).getByRole("combobox", { name: "Distribute as" }));
    await user.click(await screen.findByRole("option", { name: /Copy/ }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({ command: "setSkillMethod", args: ["alpha", "copy"] }),
    );
    expect(transport.calls).not.toContain("setSkillClients");
  });

  it("shows a store skill's files", async () => {
    const { user } = renderPage({
      listSkillFiles: async () => ok([{ path: "SKILL.md", bytes: 40 }, { path: "scripts/check.sh", bytes: 8 }]),
      readSkillFile: async (_id, file) => ok({ path: file, text: file === "SKILL.md" ? "# Repo notes" : "echo ok", bytes: 8 }),
    });
    const notes = await screen.findByRole("article", { name: "repo-notes" });
    await user.click(within(notes).getByRole("button", { name: "View repo-notes files" }));
    const dialog = await screen.findByRole("dialog", { name: "repo-notes files" });
    expect(await within(dialog).findByText("# Repo notes")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "check.sh" }));
    expect(await within(dialog).findByText("echo ok")).toBeInTheDocument();
  });

  it("classifies a tool folder and imports or trashes unmanaged entries", async () => {
    const { user, transport } = renderPage({
      importToolSkill: async (_client, _scope, _name, _id, renameIfTaken) =>
        renameIfTaken
          ? ok({ ...MOCK_SKILL_INSTALLED, id: "hand-made-2" })
          : mockIpcError({ code: "already_exists", message: "taken" }),
    });
    await user.click(await screen.findByRole("tab", { name: "Claude Code 3" }));
    expect(screen.getByText("Skill folder: ~/.claude/skills")).toBeInTheDocument();
    const managed = screen.getByRole("article", { name: "alpha" });
    expect(within(managed).getByText("Managed")).toBeInTheDocument();
    expect(within(managed).queryByRole("button", { name: "Move alpha to the trash" })).toBeNull();
    const conflict = screen.getByRole("article", { name: "elsewhere" });
    expect(within(conflict).getByText("Conflict")).toBeInTheDocument();

    const unmanaged = screen.getByRole("article", { name: "hand-made" });
    expect(within(unmanaged).getByText("Unmanaged")).toBeInTheDocument();
    await user.click(within(unmanaged).getByRole("button", { name: "Import to the store" }));
    // The id is taken: the page asks before importing under a new id.
    const confirm = await screen.findByRole("dialog", { name: "Skill id already in the store" });
    await user.click(within(confirm).getByRole("button", { name: "Install as a new skill" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({
        command: "importToolSkill",
        args: ["claude-code", { kind: "user" }, "hand-made", null, true],
      }),
    );

    await user.click(within(unmanaged).getByRole("button", { name: "Move hand-made to the trash" }));
    await user.click(within(unmanaged).getByRole("button", { name: "Confirm hand-made" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({
        command: "trashToolSkill",
        args: ["claude-code", { kind: "user" }, "hand-made"],
      }),
    );

    // A managed entry leads back to its store skill.
    await user.click(within(managed).getByRole("button", { name: "View in the store" }));
    expect(screen.getByRole("tab", { name: /Store/ })).toHaveAttribute("aria-selected", "true");
  });

  it("scopes to a project: lists project-enabled skills and turns one on with the shown hash", async () => {
    const scopes: SkillScope[] = [];
    const { user, transport } = renderPage({
      listProjects: async () => ok([PROJECT]),
      listSkills: async (scope) => {
        scopes.push(scope);
        if (scope.kind === "user") {
          return ok(LIST);
        }
        return ok({
          ...LIST,
          project: {
            id: PROJECT.id,
            name: "web",
            path: "~/Code/web",
            trust: "trusted",
            hash: "sha256:shown",
            clients: ["claude-code"],
            share: false,
          },
          skills: [makeSkill({}), makeSkill({ id: "alpha", name: "alpha", projectEnabled: true })],
        });
      },
    });
    await screen.findByRole("article", { name: "alpha" });
    await user.click(screen.getByRole("combobox", { name: "Skills scope" }));
    await user.click(await screen.findByRole("option", { name: /Project: web/ }));
    expect(
      await screen.findByText("Skills enabled in web. Links go to Claude Code in the project folder."),
    ).toBeInTheDocument();
    expect(scopes).toContainEqual({ kind: "project", id: PROJECT.id });
    // Project-enabled skills come first.
    const cards = screen.getAllByRole("article");
    expect(cards[0]).toHaveAccessibleName("alpha");
    expect(within(cards[0]).getByText("On in project")).toBeInTheDocument();

    const notes = screen.getByRole("article", { name: "repo-notes" });
    await user.click(within(notes).getByRole("button", { name: "Turn repo-notes on in this project" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({
        command: "setProjectSkill",
        args: [PROJECT.id, "repo-notes", true, "sha256:shown"],
      }),
    );
  });

  it("shows the empty state with ways to install", async () => {
    const { user, onOpenInstall } = renderPage({
      listSkills: async () => ok({ ...LIST, skills: [] }),
      listSkillToolDirs: async () => ok([]),
    });
    expect(await screen.findByText("No skills installed yet")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Install from Git" }));
    expect(onOpenInstall).toHaveBeenCalled();
  });
});
