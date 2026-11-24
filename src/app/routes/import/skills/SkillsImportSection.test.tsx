import { renderHook, screen, waitFor, within } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";
import { createQueryClient } from "@/app/App";
import { t } from "@/i18n/en";
import type { ProjectRow, SkillAdoptResult, UnmanagedSkills } from "@/ipc/bindings";
import { TransportContext } from "@/ipc/transport";
import { MOCK_SKILL_CHANGE, createMockTransport, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders, type RenderOptions } from "@/test/renderWithProviders";
import { SkillsImportSection } from "./SkillsImportSection";
import { useUnmanagedSkillCount } from "./useUnmanagedSkillCount";

// Paths arrive under their placeholder roots, as the backend sends them.
const SCAN: UnmanagedSkills = {
  candidates: [
    {
      name: "pdf-tools",
      conflict: false,
      variants: [
        {
          hash: "h-pdf",
          folders: [
            { client: "claude-code", path: "~/.claude/skills/pdf-tools" },
            { client: "codex", path: "~/.codex/skills/pdf-tools" },
          ],
          inStore: false,
          description: "Fill and split PDF files.",
          bytes: 4096,
        },
      ],
    },
    {
      name: "release-notes",
      conflict: true,
      variants: [
        {
          hash: "h-rn-claude",
          folders: [{ client: "claude-code", path: "~/.claude/skills/release-notes" }],
          inStore: false,
          description: "Draft release notes from merged pull requests.",
          bytes: 2048,
        },
        {
          hash: "h-rn-omp",
          folders: [
            { client: "omp", path: "~/.omp/agent/skills/release-notes" },
            { client: "codex", path: "~/.codex/skills/release-notes" },
          ],
          inStore: false,
          description: "Release notes with a changelog section.",
          bytes: 3072,
        },
      ],
    },
  ],
  problems: [
    {
      client: "claude-code",
      name: "Notes",
      path: "~/.claude/skills/Notes",
      error: { code: "invalid_id", message: "\"Notes\" is not a skill id" },
    },
  ],
};

const STORE_CONFLICT: UnmanagedSkills = {
  candidates: [
    {
      name: "code-review",
      conflict: true,
      variants: [
        { hash: "h-store", folders: [], inStore: true, description: "Review a diff.", bytes: 3000 },
        {
          hash: "h-local",
          folders: [{ client: "omp", path: "~/.omp/agent/skills/code-review" }],
          inStore: false,
          description: "Review a diff, the team's way.",
          bytes: 3500,
        },
      ],
    },
  ],
  problems: [],
};

const RESULT: SkillAdoptResult = {
  commit: "0000000000000000000000000000000000000002",
  added: ["pdf-tools", "release-notes-omp"],
  reused: [],
  folders: [
    {
      client: "claude-code",
      name: "pdf-tools",
      path: "~/.claude/skills/pdf-tools",
      skillId: "pdf-tools",
      status: "adopted",
      trash: "%LOCALAPPDATA%/Loadout/backups/trash/20261123T101500Z-claude-code-pdf-tools",
      error: null,
    },
    {
      client: "codex",
      name: "pdf-tools",
      path: "~/.codex/skills/pdf-tools",
      skillId: "pdf-tools",
      status: "kept",
      trash: null,
      error: {
        code: "invalid_value",
        message: "the project's repository tracks files in the folder (teammates get it from there); it is kept",
      },
    },
    {
      client: "omp",
      name: "release-notes",
      path: "~/.omp/agent/skills/release-notes",
      skillId: "release-notes-omp",
      status: "adopted",
      trash: "%LOCALAPPDATA%/Loadout/backups/trash/20261123T101500Z-omp-release-notes",
      error: null,
    },
  ],
  problems: [],
  links: MOCK_SKILL_CHANGE.links,
  needsAttention: true,
};

const UNTRUSTED: ProjectRow = {
  id: "app-a1b2",
  name: "app",
  path: "~/code/app",
  layout: "private",
  trust: "untrusted",
  error: null,
  servers: 0,
  clients: 2,
  pending: 0,
  readOnly: null,
  readOnlyFiles: [],
};

function renderSection(options: RenderOptions = {}) {
  return renderWithProviders(<SkillsImportSection />, {
    ...options,
    commands: {
      scanUnmanagedSkills: async () => ok(SCAN),
      adoptSkills: async () => ok(RESULT),
      ...options.commands,
    },
  });
}

describe("SkillsImportSection", () => {
  it("collapses one content into one row and asks for a choice on several", async () => {
    renderSection();
    const identical = await screen.findByRole("region", { name: t("adopt.identical.title") });
    const pdf = within(identical).getByRole("checkbox", { name: /pdf-tools/ });
    expect(pdf).toBeChecked();
    expect(within(identical).getByText(t("adopt.sameIn", { count: 2 }))).toBeInTheDocument();
    expect(within(identical).getByText("~/.claude/skills/pdf-tools")).toBeInTheDocument();
    expect(within(identical).getByText("~/.codex/skills/pdf-tools")).toBeInTheDocument();

    const conflicts = screen.getByRole("region", { name: t("adopt.conflicts.title") });
    // A name with several contents waits for the user.
    expect(within(conflicts).getByRole("radio", { name: t("adopt.mode.skip") })).toBeChecked();
    expect(within(conflicts).queryByRole("group", { name: t("adopt.variant.legend", { name: "release-notes" }) })).toBeNull();

    // A folder that cannot be adopted is listed with its reason.
    const problems = screen.getByText(t("adopt.problems.title", { count: 1 }));
    expect(problems.closest("details")).toHaveTextContent('"Notes" is not a skill id.');
    expect(screen.getByRole("button", { name: t("adopt.action", { count: 1 }) })).toBeEnabled();
  });

  it("adopts with one explicit choice per name and shows what was adopted and kept", async () => {
    const { user, transport } = renderSection();
    const conflicts = await screen.findByRole("region", { name: t("adopt.conflicts.title") });
    await user.click(within(conflicts).getByRole("radio", { name: t("adopt.mode.both") }));
    const variants = within(conflicts).getByRole("group", { name: t("adopt.variant.legend", { name: "release-notes" }) });
    // The content most clients hold keeps the name; the other becomes name-<client>.
    expect(within(variants).getAllByRole("radio")[1]).toBeChecked();
    expect(conflicts).toHaveTextContent(t("adopt.mode.bothHint", { ids: "release-notes-claude-code" }));
    expect(conflicts).toHaveTextContent(t("adopt.renamedHint", { name: "release-notes" }));
    await user.click(within(variants).getAllByRole("radio")[0]);
    expect(conflicts).toHaveTextContent(t("adopt.mode.bothHint", { ids: "release-notes-omp" }));

    const section = screen.getByRole("region", { name: t("adopt.section.title") });
    // The summary's live region is there, empty, before the adopt, so the result is announced.
    const status = within(section).getByRole("status");
    expect(status).toBeEmptyDOMElement();
    await user.click(screen.getByRole("button", { name: t("adopt.action", { count: 2 }) }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "adoptSkills")?.args).toEqual([
        { kind: "user" },
        null,
        [
          { name: "pdf-tools", action: { kind: "pick", variant: "h-pdf" } },
          { name: "release-notes", action: { kind: "both", primary: "h-rn-claude" } },
        ],
      ]),
    );

    await waitFor(() =>
      expect(status).toHaveTextContent(t("adopt.result.summary", { adopted: 2, kept: 1, skipped: 0 })),
    );
    expect(within(section).getByRole("status")).toBe(status);
    // Adopt is gone with the list: focus moves to the result's heading, not the page.
    expect(within(section).getByRole("heading", { name: t("adopt.result.title") })).toHaveFocus();
    const adopted = within(section).getByRole("region", { name: t("adopt.result.adopted") });
    expect(adopted).toHaveTextContent("release-notes-omp");
    expect(adopted).toHaveTextContent(t("adopt.result.renamed", { id: "release-notes-omp", name: "release-notes" }));
    // The trash location, a placeholder path.
    expect(within(adopted).getByText("%LOCALAPPDATA%/Loadout/backups/trash")).toBeInTheDocument();
    const kept = within(section).getByRole("region", { name: t("adopt.result.kept") });
    expect(kept).toHaveTextContent(
      "The project's repository tracks files in the folder (teammates get it from there); it is kept.",
    );
    expect(await screen.findByText(t("adopt.doneAttention", { adopted: 2, kept: 1 }))).toBeInTheDocument();
  });

  it("skips an unchecked name and leaves Adopt off when nothing is chosen", async () => {
    const { user } = renderSection();
    await user.click(await screen.findByRole("checkbox", { name: /pdf-tools/ }));
    expect(screen.getByRole("button", { name: t("adopt.action", { count: 0 }) })).toBeDisabled();
  });

  it("keeps the store copy's name when the store holds other content", async () => {
    const { user, transport } = renderSection({ commands: { scanUnmanagedSkills: async () => ok(STORE_CONFLICT) } });
    const conflicts = await screen.findByRole("region", { name: t("adopt.conflicts.title") });
    await user.click(within(conflicts).getByRole("radio", { name: t("adopt.mode.both") }));
    expect(conflicts).toHaveTextContent(t("adopt.variant.storeFixed"));
    const [store, local] = within(conflicts).getAllByRole("radio").slice(3);
    expect(store).toBeChecked();
    expect(local).toBeDisabled();
    await user.click(screen.getByRole("button", { name: t("adopt.action", { count: 1 }) }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "adoptSkills")?.args[2]).toEqual([
        { name: "code-review", action: { kind: "both", primary: "h-store" } },
      ]),
    );
  });

  it("scans one client when its filter is picked", async () => {
    const { user, transport } = renderSection();
    await screen.findByRole("region", { name: t("adopt.identical.title") });
    await user.click(screen.getByRole("button", { name: "Codex" }));
    await waitFor(() =>
      expect(transport.invocations.filter((call) => call.command === "scanUnmanagedSkills").at(-1)?.args).toEqual([
        { kind: "user" },
        "codex",
      ]),
    );
    expect(screen.getByRole("button", { name: "Codex" })).toHaveAttribute("aria-pressed", "true");
  });

  it("offers the trust review first in an untrusted project", async () => {
    const { user, transport } = renderSection({
      projects: [UNTRUSTED],
      scope: { kind: "project", id: UNTRUSTED.id },
      commands: {
        reviewProject: async () =>
          ok({ id: UNTRUSTED.id, name: "app", path: "~/code/app", trust: "untrusted", hash: "sha256-x", files: [], warnings: [] }),
      },
    });
    expect(await screen.findByText(t("adopt.untrusted", { name: "app" }))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("adopt.action", { count: 1 }) })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: t("adopt.review") }));
    await waitFor(() => expect(transport.calls).toContain("reviewProject"));
    expect(transport.invocations.find((call) => call.command === "scanUnmanagedSkills")?.args).toEqual([
      { kind: "project", id: UNTRUSTED.id },
      null,
    ]);
  });
});

describe("useUnmanagedSkillCount", () => {
  function wrapperFor(options: MockTransportOptions) {
    const transport = createMockTransport(options);
    const queryClient = createQueryClient();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <TransportContext.Provider value={transport}>
        <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
      </TransportContext.Provider>
    );
    return { transport, queryClient, wrapper };
  }

  it("counts the user scope's unmanaged folders per client with a stable key each", async () => {
    const { wrapper } = wrapperFor({ commands: { scanUnmanagedSkills: async () => ok(SCAN) } });
    const { result } = renderHook(() => useUnmanagedSkillCount(), { wrapper });
    expect(result.current).toBeUndefined();
    await waitFor(() => expect(result.current).toBeDefined());
    expect(result.current).toEqual({
      count: 5,
      byClient: { "claude-code": 2, codex: 2, omp: 1 },
      keys: [
        "claude-code:pdf-tools:h-pdf",
        "codex:pdf-tools:h-pdf",
        "claude-code:release-notes:h-rn-claude",
        "omp:release-notes:h-rn-omp",
        "codex:release-notes:h-rn-omp",
      ],
    });
  });

  it("does not scan while turned off", async () => {
    const { transport, wrapper } = wrapperFor({ commands: { scanUnmanagedSkills: async () => ok(SCAN) } });
    const { result } = renderHook(() => useUnmanagedSkillCount(false), { wrapper });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(result.current).toBeUndefined();
    expect(transport.calls).not.toContain("scanUnmanagedSkills");
  });

  it("keeps a failing scan inline: the app's error toasts skip it", async () => {
    const { queryClient, wrapper } = wrapperFor({
      commands: { scanUnmanagedSkills: async () => ({ status: "error", error: { code: "io_error", message: "denied" } }) },
    });
    const { result } = renderHook(() => useUnmanagedSkillCount(), { wrapper });
    const query = () => queryClient.getQueryCache().find({ queryKey: ["scan_unmanaged_skills"], exact: false });
    await waitFor(() => expect(query()?.state.status).toBe("error"));
    expect(result.current).toBeUndefined();
    // `useQueryErrorToasts` (App) toasts every failed query unless its meta says `inlineError`.
    expect(query()?.meta?.inlineError).toBe(true);
  });
});
