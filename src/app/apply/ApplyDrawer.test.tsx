import { screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { ApplyDrawer } from "@/app/apply/ApplyDrawer";
import { expectModalKeyboard } from "@/app/components/modalKeyboard";
import { t } from "@/i18n/en";
import type { AppliedRun, PendingPlan, PendingStoreChange, PlannedFile } from "@/ipc/bindings";
import {
  MOCK_APPLY_RUN,
  MOCK_CHANGE,
  MOCK_CLIENTS,
  mockIpcError,
  ok,
  type MockTransportOptions,
} from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const USER_FILE: PlannedFile = {
  // The backend sends placeholder paths, never the profile path.
  file: "~/.claude.json",
  clients: ["claude-code"],
  scope: "user",
  projectId: null,
  project: null,
  counts: { create: 1, update: 2, remove: 0, disable: 0 },
  diff: [
    "--- a/.claude.json",
    "+++ b/.claude.json",
    "@@ -1,3 +1,4 @@",
    ' "mcpServers": {',
    '-  "old": {}',
    '+  "demo": { "env": { "TOKEN": "********" } }',
    " }",
  ].join("\n"),
  entries: [
    {
      action: "skip",
      name: "drifty",
      client: "claude-code",
      server: "drifty",
      state: "drifted",
      unresolved: true,
      problem: null,
    },
    {
      action: "skip",
      name: "locked-out",
      client: "claude-code",
      server: "locked-out",
      state: "blocked",
      unresolved: false,
      problem: { code: "missing_secret", message: "synthetic" },
    },
  ],
  problem: null,
  warnings: [],
  readOnly: null,
};

const CODEX_FILE: PlannedFile = {
  ...USER_FILE,
  file: "~/.codex/config.toml",
  clients: ["codex"],
  counts: { create: 0, update: 0, remove: 3, disable: 0 },
  diff: "",
  entries: [],
};

const PROJECT_FILE: PlannedFile = {
  ...USER_FILE,
  file: "<P>/.mcp.json",
  clients: ["claude-code", "cursor"],
  scope: "project",
  projectId: "demo-project",
  project: "~/code/demo",
  counts: { create: 0, update: 0, remove: 0, disable: 4 },
  diff: "",
  entries: [],
  problem: { code: "trust_required", message: "synthetic" },
};

// Same placeholder path as PROJECT_FILE, in another project.
const OTHER_PROJECT_FILE: PlannedFile = {
  ...PROJECT_FILE,
  projectId: "other-project",
  project: "~/code/other",
  problem: null,
};

const PLAN: PendingPlan = {
  applyMode: "instant",
  files: [USER_FILE, CODEX_FILE, PROJECT_FILE, OTHER_PROJECT_FILE],
  pending: 9,
  writes: 3,
  unresolved: 1,
  warnings: [],
};

const RUNS: AppliedRun[] = [
  {
    applyId: "01MOCKRUNB000000000000000000",
    commit: "0000000000000000000000000000000000000002",
    time: "2026-01-02T03:04:05Z",
    summary: "apply: 1 file (codex)",
    latest: true,
  },
  {
    applyId: "01MOCKRUNA000000000000000000",
    commit: "0000000000000000000000000000000000000001",
    time: "2026-01-01T03:04:05Z",
    summary: "apply: 2 files (claude-code)",
    latest: false,
  },
];

function renderDrawer(options: MockTransportOptions = {}) {
  return renderWithProviders(<ApplyDrawer open onClose={() => undefined} />, {
    commands: {
      pendingPlan: async () => ok(PLAN),
      listServers: async () => ok({ servers: [], clients: MOCK_CLIENTS, warnings: [] }),
      ...options.commands,
    },
  });
}

describe("ApplyDrawer", () => {
  it("renders nothing and queries nothing while closed", () => {
    const { transport } = renderWithProviders(<ApplyDrawer open={false} onClose={() => undefined} />);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(transport.calls).not.toContain("pendingPlan");
  });

  it("groups files by scope with op counts and clients", async () => {
    renderDrawer();
    // Wait on a text query: `findByRole` polls the whole drawer's accessibility
    // tree every 50ms, which under the full suite's load outran its 1s budget.
    await screen.findAllByText(t("apply.scope.user"));
    const user = screen.getByRole("region", { name: t("apply.scope.user") });
    const project = screen.getByRole("region", { name: t("apply.scope.project") });
    expect(screen.queryByRole("region", { name: t("apply.scope.projectLocal") })).not.toBeInTheDocument();

    expect(within(user).getByText(USER_FILE.file)).toBeInTheDocument();
    expect(within(user).getByText(CODEX_FILE.file)).toBeInTheDocument();
    expect(within(project).getAllByText(PROJECT_FILE.file)).toHaveLength(2);
    expect(within(project).getByText("~/code/demo")).toBeInTheDocument();
    expect(within(project).getByText("~/code/other")).toBeInTheDocument();
    expect(
      within(user).getByLabelText(t("apply.counts.aria", { create: 1, update: 2, remove: 0, disable: 0 })),
    ).toHaveTextContent("+1~2-0!0");
    expect(within(project).getAllByText("Claude Code, Cursor")).toHaveLength(2);
    expect(within(project).getByText(/needs to be trusted/)).toBeInTheDocument();
  });

  it("expands a file to its masked diff and the entries needing attention", async () => {
    const { user } = renderDrawer();
    const toggle = await screen.findByRole("button", { name: new RegExp(USER_FILE.file.replace(/\./g, "\\.")) });
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");

    const diff = screen.getByLabelText(t("apply.file.diff", { file: USER_FILE.file }));
    const added = within(diff).getByText(/"demo"/);
    expect(added).toHaveClass("apply-diff__line--added");
    expect(added).toHaveTextContent("********");
    expect(within(diff).getByText(/"old"/)).toHaveClass("apply-diff__line--removed");

    const entries = screen.getByRole("list", { name: t("apply.entries.aria") });
    expect(within(entries).getByText("drifty")).toBeInTheDocument();
    expect(
      within(entries).getByText(t("apply.entries.needsDecision", { state: t("apply.state.drifted") })),
    ).toBeInTheDocument();
    expect(within(entries).getByText("locked-out")).toBeInTheDocument();
    expect(within(entries).getByText(t("apply.state.blocked"))).toBeInTheDocument();
    expect(within(entries).getByText("A secret has no value.")).toBeInTheDocument();
  });

  it("applies only the selected files, told apart by project", async () => {
    const runApply = vi.fn(async () => ok(MOCK_CHANGE));
    const { user, transport } = renderDrawer({ commands: { runApply } });
    const selectedButton = await screen.findByRole("button", { name: t("apply.action.selected") });
    expect(selectedButton).toBeDisabled();

    await user.click(await screen.findByRole("checkbox", { name: t("apply.file.select", { file: USER_FILE.file }) }));
    const demo = screen.getByRole("checkbox", {
      name: t("apply.file.selectInProject", { file: PROJECT_FILE.file, project: "~/code/demo" }),
    });
    const other = screen.getByRole("checkbox", {
      name: t("apply.file.selectInProject", { file: PROJECT_FILE.file, project: "~/code/other" }),
    });
    await user.click(demo);
    expect(demo).toBeChecked();
    expect(other).not.toBeChecked();
    await user.click(selectedButton);

    await waitFor(() => expect(runApply).toHaveBeenCalled());
    const call = transport.invocations.find((item) => item.command === "runApply");
    expect(call?.args[0]).toEqual([
      { projectId: null, scope: "user", file: "~/.claude.json" },
      { projectId: "demo-project", scope: "project", file: "<P>/.mcp.json" },
    ]);
    expect(await screen.findByText(t("apply.applied"))).toBeInTheDocument();
  });

  it("applies everything with null", async () => {
    const { user, transport } = renderDrawer();
    const all = await screen.findByRole("button", { name: t("apply.action.all") });
    // The button renders disabled until the plan has loaded.
    await waitFor(() => expect(all).toBeEnabled());
    await user.click(all);
    await waitFor(() =>
      expect(transport.invocations.find((item) => item.command === "runApply")?.args[0]).toBeNull(),
    );
  });

  it("shows the empty state when nothing is pending", async () => {
    renderDrawer({
      commands: {
        pendingPlan: async () => ok({ ...PLAN, files: [], pending: 0, writes: 0, unresolved: 0 }),
      },
    });
    expect(await screen.findByText(t("apply.empty.title"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("apply.action.all") })).toBeDisabled();
  });

  it("closes on Escape and on the backdrop", async () => {
    const onClose = vi.fn();
    const { user, container } = renderWithProviders(<ApplyDrawer open onClose={onClose} />);
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
    await user.click(container.querySelector(".apply-drawer-backdrop") as HTMLElement);
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  describe("keyboard (#11)", () => {
    function DrawerHarness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>
            Pending
          </button>
          <ApplyDrawer open={open} onClose={() => setOpen(false)} />
        </>
      );
    }

    function renderHarness() {
      return renderWithProviders(<DrawerHarness />, {
        commands: {
          pendingPlan: async () => ok(PLAN),
          listServers: async () => ok({ servers: [], clients: MOCK_CLIENTS, warnings: [] }),
        },
      });
    }

    it("moves focus into the drawer and back to its trigger", async () => {
      const { user } = renderHarness();
      await expectModalKeyboard(user, {
        trigger: screen.getByRole("button", { name: "Pending" }),
        name: t("apply.drawer.title"),
        initial: (drawer) => drawer,
      });
    });

    it("stacks the resolver over the drawer and gives the drawer the keyboard back", async () => {
      const { user } = renderHarness();
      const pending = screen.getByRole("button", { name: "Pending" });
      pending.focus();
      await user.keyboard("{Enter}");
      const drawer = await screen.findByRole("dialog", { name: t("apply.drawer.title") });
      const fileName = new RegExp(USER_FILE.file.replace(/\./g, "\\."));
      await user.click(await within(drawer).findByRole("button", { name: fileName }));

      await expectModalKeyboard(user, {
        trigger: within(drawer).getByRole("button", { name: t("apply.entries.resolveAria", { name: "drifty" }) }),
        name: t("resolver.title", { name: "drifty" }),
      });
      expect(drawer).toBeInTheDocument();
      expect(drawer.closest("[inert]")).toBeNull();
      await user.keyboard("{Escape}");
      await waitFor(() => expect(drawer).not.toBeInTheDocument());
      expect(pending).toHaveFocus();
    });
  });

  it("undoes a recent apply", async () => {
    const { user, transport } = renderDrawer({ commands: { listAppliedRuns: async () => ok(RUNS) } });
    expect(await screen.findByText(RUNS[0].summary)).toBeInTheDocument();
    const buttons = screen.getAllByRole("button", { name: t("apply.undo.action") });
    expect(buttons).toHaveLength(2);
    await user.click(buttons[0]);
    await waitFor(() =>
      expect(transport.invocations.find((item) => item.command === "undoApply")?.args).toEqual([
        RUNS[0].applyId,
        false,
      ]),
    );
  });

  it("asks before undoing when later applies changed the same entries", async () => {
    const undoApply = vi.fn(async (applyId: string, confirmed: boolean) =>
      confirmed
        ? ok({
            applyId,
            restored: [],
            tag: "auto/pre-rollback/mock",
            commit: null,
            apply: MOCK_APPLY_RUN,
            unavailableOriginals: [],
          })
        : mockIpcError({ code: "confirmation_required", message: "synthetic" }),
    );
    const { user } = renderDrawer({
      commands: { listAppliedRuns: async () => ok(RUNS), undoApply },
    });
    await screen.findByText(RUNS[1].summary);
    await user.click(screen.getAllByRole("button", { name: t("apply.undo.action") })[1]);

    expect(await screen.findByText(t("apply.undo.confirmMessage"))).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: t("apply.undo.confirmAction") }));
    await waitFor(() => expect(undoApply).toHaveBeenLastCalledWith(RUNS[1].applyId, true));
    await waitFor(() =>
      expect(screen.queryByText(t("apply.undo.confirmMessage"))).not.toBeInTheDocument(),
    );
  });

  describe("Discard pending (review mode)", () => {
    const STORE_CHANGES: PendingStoreChange[] = [
      { entity: "server/github", kind: "changed" },
      { entity: "server/docs", kind: "added" },
      { entity: "clients", kind: "changed" },
    ];

    function renderReview(commands: MockTransportOptions["commands"] = {}) {
      return renderDrawer({
        commands: {
          pendingPlan: async () => ok({ ...PLAN, applyMode: "review" }),
          pendingStoreChanges: async () => ok(STORE_CHANGES),
          ...commands,
        },
      });
    }

    it("is not offered in instant mode", async () => {
      const { transport } = renderDrawer();
      await screen.findByRole("region", { name: t("apply.scope.user") });
      expect(screen.queryByRole("button", { name: t("apply.discard.allAria") })).not.toBeInTheDocument();
      expect(screen.queryByText(t("apply.discard.title"))).not.toBeInTheDocument();
      expect(transport.calls).not.toContain("pendingStoreChanges");
    });

    it("lists the store changes and discards one after confirming", async () => {
      const discardPending = vi.fn(async (entities: string[] | null) =>
        ok({ discarded: entities ?? [], commit: MOCK_CHANGE.commit, tag: "auto/pre-restore/mock" }),
      );
      const { user } = renderReview({ discardPending });
      const section = await screen.findByRole("region", { name: t("apply.discard.title") });
      expect(within(section).getByText(t("apply.discard.entity.server", { id: "github" }))).toBeInTheDocument();
      expect(within(section).getByText(t("apply.discard.entity.clients"))).toBeInTheDocument();
      expect(within(section).getByText(t("apply.discard.kind.added"))).toBeInTheDocument();

      const entity = t("apply.discard.entity.server", { id: "docs" });
      await user.click(within(section).getByRole("button", { name: t("apply.discard.oneAria", { entity }) }));
      // Nothing is discarded before the confirmation.
      const message = await screen.findByText(t("apply.discard.confirmOne", { entity }));
      const confirm = message.closest("[role='alert']") as HTMLElement;
      expect(discardPending).not.toHaveBeenCalled();

      await user.click(within(confirm).getByRole("button", { name: t("apply.discard.confirmAction") }));
      await waitFor(() => expect(discardPending).toHaveBeenCalledWith(["server/docs"]));
      expect(await screen.findByText(t("apply.discard.doneOne", { entity }))).toBeInTheDocument();
      expect(screen.queryByText(t("apply.discard.confirmOne", { entity }))).not.toBeInTheDocument();
    });

    it("discards exactly the listed changes after confirming, and cancel keeps it", async () => {
      const discardPending = vi.fn(async (entities: string[] | null) =>
        ok({ discarded: entities ?? [...STORE_CHANGES.map((change) => change.entity), "server/late"], commit: null, tag: null }),
      );
      const { user } = renderReview({ discardPending });
      const discardAll = await screen.findByRole("button", { name: t("apply.discard.allAria") });
      await waitFor(() => expect(discardAll).toBeEnabled());

      await user.click(discardAll);
      expect(await screen.findByText(t("apply.discard.confirmAll", { count: 3 }))).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: t("apply.cancel") }));
      expect(screen.queryByText(t("apply.discard.confirmAll", { count: 3 }))).not.toBeInTheDocument();
      expect(discardPending).not.toHaveBeenCalled();

      await user.click(discardAll);
      await user.click(await screen.findByRole("button", { name: t("apply.discard.confirmAction") }));
      // The listed entities, never null: a change that arrived after the list
      // was shown (here `server/late`) is not discarded unseen.
      await waitFor(() =>
        expect(discardPending).toHaveBeenCalledWith(["server/github", "server/docs", "clients"]),
      );
      expect(discardPending.mock.calls.flat(2)).not.toContain("server/late");
      expect(await screen.findByText(t("apply.discard.done", { count: 3 }))).toBeInTheDocument();
    });

    it("disables Discard pending when no store edit waits", async () => {
      renderReview({ pendingStoreChanges: async () => ok([]) });
      const discardAll = await screen.findByRole("button", { name: t("apply.discard.allAria") });
      expect(discardAll).toBeDisabled();
      expect(screen.queryByText(t("apply.discard.title"))).not.toBeInTheDocument();
    });
  });
});
