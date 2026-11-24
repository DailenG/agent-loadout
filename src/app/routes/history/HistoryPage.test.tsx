import { screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectModalKeyboard } from "@/app/components/modalKeyboard";
import { HISTORY_PAGE_SIZE, HistoryPage } from "@/app/routes/history/HistoryPage";
import { errorMessage, t } from "@/i18n/en";
import type {
  BackupRunEntry,
  CommitDetail,
  HistoryEntry,
  RemoteEntry,
  RemoteStatusDto,
  RestorePreview,
  UndoResult,
} from "@/ipc/bindings";
import {
  MOCK_APPLY_RUN,
  MOCK_REMOTES,
  mockIpcError,
  ok,
  type MockTransport,
  type MockTransportOptions,
} from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const LATEST_APPLY: HistoryEntry = {
  id: "a".repeat(40),
  shortId: "aaaaaaa",
  time: "2026-11-20T10:05:00Z",
  summary: "apply: 1 file (claude-code)",
  op: "apply",
  entities: [],
  scope: "system",
  machine: "desktop",
  thisMachine: true,
  apply: { applyId: "apply-2", outcome: "ok", undoable: true, latest: true },
  tags: [],
};

const SERVER_EDIT: HistoryEntry = {
  ...LATEST_APPLY,
  id: "b".repeat(40),
  shortId: "bbbbbbb",
  time: "2026-11-20T10:00:00Z",
  summary: "server: update github (env.GITHUB_TOOLSETS)",
  op: "server",
  entities: ["server/github", "secrets"],
  apply: null,
  tags: ["pre-change", "auto/pre-restore/20261120T100000Z"],
};

const OLDER_APPLY: HistoryEntry = {
  ...LATEST_APPLY,
  id: "c".repeat(40),
  shortId: "ccccccc",
  time: "2026-11-19T09:00:00Z",
  summary: "apply: 2 files (claude-code, codex)",
  machine: "laptop",
  apply: { applyId: "apply-1", outcome: "partial", undoable: true, latest: false },
};

const DETAIL: CommitDetail = {
  entry: SERVER_EDIT,
  body: "",
  changes: [
    {
      entity: "server/github",
      file: "servers/github.toml",
      change: "changed",
      fields: [{ field: "env.GITHUB_TOOLSETS", before: '"repos"', after: '"repos,issues"' }],
      text: null,
    },
  ],
  raw: 'diff --git a/servers/github.toml b/servers/github.toml\n-GITHUB_TOOLSETS = "repos"\n+GITHUB_TOOLSETS = "repos,issues"\n',
  applied: null,
};

const PREVIEW: RestorePreview = {
  target: SERVER_EDIT.id,
  shortId: SERVER_EDIT.shortId,
  added: ["server/postgres"],
  changed: ["server/github", "secrets"],
  removed: [],
  files: ["servers/postgres.toml", "servers/github.toml", "secrets.toml"],
  sharedManifests: [],
  unplaced: [],
};

const undone = (applyId: string): UndoResult => ({
  applyId,
  restored: [],
  tag: "auto/pre-rollback/1",
  commit: null,
  apply: MOCK_APPLY_RUN,
  unavailableOriginals: [],
});

function remote(name: string, status: Partial<RemoteStatusDto>): RemoteEntry {
  return {
    name,
    kind: "folder",
    location: `~/backups/${name}`,
    mode: "repo_per_machine",
    auth: null,
    username: null,
    push: "on-commit",
    pull: "manual",
    tokenStored: null,
    syncTool: null,
    status: { reachable: true, pushedAt: null, pulledAt: null, ahead: 0, behind: 0, lastError: null, ...status },
  };
}

function renderPage(options: MockTransportOptions = {}) {
  const onOpenPending = vi.fn();
  const onOpenSettings = vi.fn();
  const view = renderWithProviders(<HistoryPage onOpenPending={onOpenPending} onOpenSettings={onOpenSettings} />, {
    commands: {
      listHistory: async () => ok([LATEST_APPLY, SERVER_EDIT, OLDER_APPLY]),
      commitDetail: async () => ok(DETAIL),
      restorePreview: async (rev, entities) =>
        ok(entities.length === 0 ? PREVIEW : { ...PREVIEW, added: [], changed: entities }),
      ...options.commands,
    },
  });
  return { ...view, onOpenPending, onOpenSettings };
}

/** Selects a commit row and returns its action toolbar. */
async function selectRow(user: UserEvent, summary: string) {
  const row = await screen.findByRole("listitem", { name: summary });
  await user.click(within(row).getByText(summary));
  return { row, actions: within(row).getByRole("toolbar") };
}

function argsOf(transport: MockTransport, command: string) {
  return transport.invocations.filter((call) => call.command === command).map((call) => call.args);
}

describe("HistoryPage", () => {
  it("lists commits with machine, apply outcome and tags; user tags can be deleted", async () => {
    const { user, transport } = renderPage();
    const row = await screen.findByRole("listitem", { name: SERVER_EDIT.summary });
    expect(within(row).getByText("pre-change")).toBeInTheDocument();
    expect(within(row).getByText("auto/pre-restore/20261120T100000Z")).toBeInTheDocument();
    expect(within(row).getByText(/desktop/)).toBeInTheDocument();
    // Automatic tags are pruned by age, never deleted by hand.
    expect(
      within(row).queryByRole("button", { name: t("history.tag.delete", { name: "auto/pre-restore/20261120T100000Z" }) }),
    ).toBeNull();

    const older = screen.getByRole("listitem", { name: OLDER_APPLY.summary });
    expect(within(older).getByText(t("history.outcome.partial"))).toBeInTheDocument();
    expect(within(older).getByText(/laptop/)).toBeInTheDocument();
    // The first page asks one commit more, to know whether "Load more" is needed.
    expect(argsOf(transport, "listHistory")[0]).toEqual([{ scope: "all", entity: null, limit: HISTORY_PAGE_SIZE + 1, before: null }]);
    expect(screen.queryByRole("button", { name: t("history.loadMore") })).toBeNull();

    await user.click(within(row).getByRole("button", { name: t("history.tag.delete", { name: "pre-change" }) }));
    await waitFor(() => expect(argsOf(transport, "deleteTag")).toEqual([["pre-change"]]));
  });

  it("filters by scope and entity", async () => {
    const { user, transport } = renderPage();
    await screen.findByRole("listitem", { name: SERVER_EDIT.summary });
    await user.click(screen.getByRole("combobox", { name: t("history.toolbar.scope") }));
    await user.click(await screen.findByRole("option", { name: t("history.scope.user") }));
    await waitFor(() =>
      expect(argsOf(transport, "listHistory")).toContainEqual([
        { scope: "user", entity: null, limit: HISTORY_PAGE_SIZE + 1, before: null },
      ]),
    );
    await user.type(screen.getByRole("searchbox", { name: t("history.toolbar.entity") }), "server/github{Enter}");
    await waitFor(() =>
      expect(argsOf(transport, "listHistory")).toContainEqual([
        { scope: "user", entity: "server/github", limit: HISTORY_PAGE_SIZE + 1, before: null },
      ]),
    );
  });

  it("loads the list a page at a time after the last shown commit, and keeps the focus on Load more", async () => {
    const many = Array.from({ length: HISTORY_PAGE_SIZE * 2 + 5 }, (_, index) => ({
      ...SERVER_EDIT,
      id: index.toString(16).padStart(40, "0"),
      summary: `commit ${index}`,
      tags: [],
    }));
    const { user, transport } = renderPage({
      commands: {
        listHistory: async (filter) => {
          const start = filter.before ? many.findIndex((entry) => entry.id === filter.before) + 1 : 0;
          return ok(many.slice(start, start + (filter.limit ?? many.length)));
        },
      },
    });
    await screen.findByRole("listitem", { name: "commit 0" });
    expect(screen.getAllByRole("listitem")).toHaveLength(HISTORY_PAGE_SIZE);
    expect(screen.queryByRole("status")).toBeNull();

    const loadMore = screen.getByRole("button", { name: t("history.loadMore") });
    await user.click(loadMore);
    await screen.findByRole("listitem", { name: `commit ${HISTORY_PAGE_SIZE * 2 - 1}` });
    expect(screen.getAllByRole("listitem")).toHaveLength(HISTORY_PAGE_SIZE * 2);
    // The next page starts after the last shown commit; the loaded ones are not asked for again.
    expect(argsOf(transport, "listHistory").at(-1)).toEqual([
      { scope: "all", entity: null, limit: HISTORY_PAGE_SIZE + 1, before: many[HISTORY_PAGE_SIZE - 1].id },
    ]);
    expect(screen.getByRole("status")).toHaveTextContent(t("history.shown", { count: HISTORY_PAGE_SIZE * 2 }));
    // The button keeps the focus, so Enter loads the next page from the keyboard.
    expect(screen.getByRole("button", { name: t("history.loadMore") })).toHaveFocus();

    await user.keyboard("{Enter}");
    await screen.findByRole("listitem", { name: `commit ${HISTORY_PAGE_SIZE * 2 + 4}` });
    expect(screen.getAllByRole("listitem")).toHaveLength(many.length);
    expect(screen.queryByRole("button", { name: t("history.loadMore") })).toBeNull();
  });

  it("shows an empty state for an empty store", async () => {
    renderPage({ commands: { listHistory: async () => ok([]) } });
    expect(await screen.findByText(t("page.empty.history"))).toBeInTheDocument();
  });

  it("tags the current version and refuses invalid names without a call", async () => {
    const { user, transport } = renderPage();
    await user.click(await screen.findByRole("button", { name: t("history.toolbar.tagCurrent") }));
    const dialog = await screen.findByRole("dialog", { name: t("history.tag.titleCurrent") });
    const name = within(dialog).getByRole("textbox", { name: t("history.tag.name") });
    await user.type(name, "pre change!");
    await user.click(within(dialog).getByRole("button", { name: t("history.tag.create") }));
    expect(within(dialog).getByRole("alert")).toHaveTextContent(t("history.tag.nameInvalid"));
    expect(transport.calls).not.toContain("createTag");

    await user.clear(name);
    await user.type(name, "pre-change");
    await user.type(within(dialog).getByRole("textbox", { name: t("history.tag.message") }), "before the edit");
    await user.click(within(dialog).getByRole("button", { name: t("history.tag.create") }));
    await waitFor(() => expect(argsOf(transport, "createTag")).toEqual([["pre-change", null, "before the edit"]]));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("tags a selected commit", async () => {
    const { user, transport } = renderPage();
    const { actions } = await selectRow(user, SERVER_EDIT.summary);
    await user.click(within(actions).getByRole("button", { name: t("history.action.tag") }));
    const dialog = await screen.findByRole("dialog", { name: t("history.tag.title", { id: SERVER_EDIT.shortId }) });
    await user.type(within(dialog).getByRole("textbox", { name: t("history.tag.name") }), "v1.0");
    await user.click(within(dialog).getByRole("button", { name: t("history.tag.create") }));
    await waitFor(() => expect(argsOf(transport, "createTag")).toEqual([["v1.0", SERVER_EDIT.id, null]]));
  });

  it("shows the semantic diff, and the raw diff on request", async () => {
    const { user, transport } = renderPage();
    const { actions } = await selectRow(user, SERVER_EDIT.summary);
    await user.click(within(actions).getByRole("button", { name: t("history.action.showDiff") }));
    expect(
      await screen.findByText(
        t("history.detail.field", { field: "env.GITHUB_TOOLSETS", before: '"repos"', after: '"repos,issues"' }),
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("listitem", { name: "server/github" })).toBeInTheDocument();
    expect(argsOf(transport, "commitDetail")).toEqual([[SERVER_EDIT.id]]);
    expect(screen.queryByLabelText(t("history.detail.rawAria"))).toBeNull();

    await user.click(screen.getByRole("button", { name: t("history.detail.raw") }));
    expect(screen.getByLabelText(t("history.detail.rawAria"))).toHaveTextContent('+GITHUB_TOOLSETS = "repos,issues"');
  });

  it("shows the files an apply commit touched", async () => {
    const { user } = renderPage({
      commands: {
        commitDetail: async () =>
          ok({
            ...DETAIL,
            entry: LATEST_APPLY,
            changes: [],
            applied: {
              trigger: "instant",
              outcome: "ok",
              targets: [
                {
                  client: "claude-code",
                  scope: "user",
                  projectId: null,
                  file: "~/.claude.json",
                  counts: { create: 1, update: 0, remove: 0, disable: 0 },
                  outcome: "ok",
                },
              ],
            },
          }),
      },
    });
    const { actions } = await selectRow(user, LATEST_APPLY.summary);
    await user.click(within(actions).getByRole("button", { name: t("history.action.showDiff") }));
    const table = await screen.findByRole("table");
    expect(within(table).getByText("~/.claude.json")).toBeInTheDocument();
    expect(within(table).getByText(t("history.outcome.ok"))).toBeInTheDocument();
  });

  it("undoes the latest apply without asking", async () => {
    const { user, transport } = renderPage();
    const { actions } = await selectRow(user, LATEST_APPLY.summary);
    await user.click(within(actions).getByRole("button", { name: t("apply.undo.action") }));
    await waitFor(() => expect(argsOf(transport, "undoApply")).toEqual([["apply-2", false]]));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("names the files whose previous content is gone after their backups were removed", async () => {
    const { user } = renderPage({
      commands: {
        undoApply: async (applyId) => ok({ ...undone(applyId), unavailableOriginals: ["~/.claude.json"] }),
      },
    });
    const { actions } = await selectRow(user, LATEST_APPLY.summary);
    await user.click(within(actions).getByRole("button", { name: t("apply.undo.action") }));
    expect(
      await screen.findByText(t("apply.undo.doneUnavailable", { id: "apply-2", files: "~/.claude.json" })),
    ).toBeInTheDocument();
  });

  it("asks before undoing an older apply, then confirms", async () => {
    const { user, transport } = renderPage({
      commands: {
        undoApply: async (applyId, confirmed) =>
          confirmed
            ? ok(undone(applyId))
            : mockIpcError({ code: "confirmation_required", message: "later applies changed the same entries" }),
      },
    });
    const { actions } = await selectRow(user, OLDER_APPLY.summary);
    await user.click(within(actions).getByRole("button", { name: t("apply.undo.action") }));
    const confirm = await screen.findByRole("alertdialog");
    expect(confirm).toHaveTextContent(t("apply.undo.confirmMessage"));
    expect(argsOf(transport, "undoApply")).toEqual([["apply-1", false]]);
    await user.click(within(confirm).getByRole("button", { name: t("apply.undo.confirmAction") }));
    await waitFor(() =>
      expect(argsOf(transport, "undoApply")).toEqual([
        ["apply-1", false],
        ["apply-1", true],
      ]),
    );
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("offers no undo for a commit without an undoable apply", async () => {
    const { user } = renderPage();
    const { actions } = await selectRow(user, SERVER_EDIT.summary);
    expect(within(actions).queryByRole("button", { name: t("apply.undo.action") })).toBeNull();
  });

  it("restores a version after its preview, then opens Pending changes", async () => {
    const { user, transport, onOpenPending } = renderPage();
    const { actions } = await selectRow(user, SERVER_EDIT.summary);
    await user.click(within(actions).getByRole("button", { name: t("history.action.restoreVersion") }));
    const dialog = await screen.findByRole("dialog", {
      name: t("history.restore.titleVersion", { id: SERVER_EDIT.shortId }),
    });
    expect(
      await within(dialog).findByText(t("history.restore.counts", { added: 1, changed: 2, removed: 0 })),
    ).toBeInTheDocument();
    expect(within(dialog).getByText("server/postgres")).toBeInTheDocument();
    expect(within(dialog).getByText("secrets")).toBeInTheDocument();
    expect(argsOf(transport, "restorePreview")).toEqual([[SERVER_EDIT.id, []]]);
    expect(within(dialog).queryByRole("checkbox")).toBeNull();

    await user.click(within(dialog).getByRole("button", { name: t("history.restore.action") }));
    await waitFor(() => expect(argsOf(transport, "restoreStoreVersion")).toEqual([[SERVER_EDIT.id, [], false]]));
    await waitFor(() => expect(onOpenPending).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("requires the shared manifests checkbox and sends the confirmation", async () => {
    const { user, transport } = renderPage({
      commands: { restorePreview: async () => ok({ ...PREVIEW, sharedManifests: ["<P>/.loadout/project.toml"] }) },
    });
    const { actions } = await selectRow(user, SERVER_EDIT.summary);
    await user.click(within(actions).getByRole("button", { name: t("history.action.restoreVersion") }));
    const dialog = await screen.findByRole("dialog", {
      name: t("history.restore.titleVersion", { id: SERVER_EDIT.shortId }),
    });
    expect(await within(dialog).findByText("<P>/.loadout/project.toml")).toBeInTheDocument();
    const restore = within(dialog).getByRole("button", { name: t("history.restore.action") });
    expect(restore).toBeDisabled();
    await user.click(within(dialog).getByRole("checkbox", { name: t("history.restore.sharedConfirm") }));
    expect(restore).toBeEnabled();
    await user.click(restore);
    await waitFor(() => expect(argsOf(transport, "restoreStoreVersion")).toEqual([[SERVER_EDIT.id, [], true]]));
  });

  it("restores only the picked entities", async () => {
    const { user, transport, onOpenPending } = renderPage();
    const { actions } = await selectRow(user, SERVER_EDIT.summary);
    await user.click(within(actions).getByRole("button", { name: t("history.action.restoreEntity") }));
    const dialog = await screen.findByRole("dialog", {
      name: t("history.restore.titleEntity", { id: SERVER_EDIT.shortId }),
    });
    const restore = within(dialog).getByRole("button", { name: t("history.restore.action") });
    expect(restore).toBeDisabled();
    expect(within(dialog).getByRole("checkbox", { name: "server/github" })).not.toBeChecked();
    await user.click(within(dialog).getByRole("checkbox", { name: "secrets" }));
    await waitFor(() => expect(argsOf(transport, "restorePreview")).toEqual([[SERVER_EDIT.id, ["secrets"]]]));
    expect(
      await within(dialog).findByText(t("history.restore.counts", { added: 0, changed: 1, removed: 0 })),
    ).toBeInTheDocument();
    await waitFor(() => expect(restore).toBeEnabled());
    await user.click(restore);
    await waitFor(() =>
      expect(argsOf(transport, "restoreStoreVersion")).toEqual([[SERVER_EDIT.id, ["secrets"], false]]),
    );
    await waitFor(() => expect(onOpenPending).toHaveBeenCalledTimes(1));
  });

  it("saves a version as zip", async () => {
    const { user, transport } = renderPage({ commands: { saveVersionZip: async () => ok(true) } });
    const { actions } = await selectRow(user, SERVER_EDIT.summary);
    await user.click(within(actions).getByRole("button", { name: t("history.action.zip") }));
    await waitFor(() => expect(argsOf(transport, "saveVersionZip")).toEqual([[SERVER_EDIT.id]]));
    expect(await screen.findByText(t("history.zip.saved", { id: SERVER_EDIT.shortId }))).toBeInTheDocument();
  });

  it("restores client files from a backup run, one file or all", async () => {
    const run: BackupRunEntry = {
      run: "20261120T100500Z-apply-2",
      applyId: "apply-2",
      time: "2026-11-20T10:05:00Z",
      undoOfRestore: false,
      files: ["~/.claude.json", "%APPDATA%/Code/User/mcp.json"],
    };
    const { user, transport } = renderPage({
      commands: {
        listFileBackups: async () => ok([run]),
        restoreClientFiles: async (_applyId, file) =>
          ok(
            file
              ? [{ file, outcome: "restored" as const, error: null }]
              : [
                  { file: "~/.claude.json", outcome: "unchanged" as const, error: null },
                  {
                    file: "%APPDATA%/Code/User/mcp.json",
                    outcome: "failed" as const,
                    error: { code: "io_error" as const, message: "denied" },
                  },
                ],
          ),
      },
    });
    await user.click(await screen.findByRole("tab", { name: t("history.tabs.backups") }));
    const card = await screen.findByRole("article");
    expect(within(card).getByText("%APPDATA%/Code/User/mcp.json")).toBeInTheDocument();

    await user.click(
      within(card).getByRole("button", { name: t("history.backups.restoreFileAria", { file: "~/.claude.json" }) }),
    );
    await waitFor(() =>
      expect(argsOf(transport, "restoreClientFiles")).toEqual([["20261120T100500Z-apply-2", "~/.claude.json"]]),
    );
    expect(await within(card).findByText(t("history.backups.outcome.restored"))).toBeInTheDocument();

    await user.click(within(card).getByRole("button", { name: t("history.backups.restoreAll") }));
    await waitFor(() =>
      expect(argsOf(transport, "restoreClientFiles")[1]).toEqual(["20261120T100500Z-apply-2", null]),
    );
    expect(await within(card).findByText(errorMessage("io_error"))).toBeInTheDocument();
    expect(within(card).getByText(t("history.backups.outcome.unchanged"))).toBeInTheDocument();
  });

  it("shows each remote's state in the strip", async () => {
    renderPage({
      commands: {
        remotesOverview: async () =>
          ok({
            ...MOCK_REMOTES,
            remotes: [
              remote("nas", { ahead: 0, behind: 0 }),
              remote("usb", { ahead: 2, behind: null }),
              remote("share", { reachable: false }),
              remote("origin", {
                reachable: null,
                lastError: { error: { code: "remote_auth_failed", message: "401" }, at: "2026-11-20T10:00:00Z" },
              }),
              remote("fresh", { ahead: null }),
            ],
          }),
      },
    });
    const strip = await screen.findByRole("region", { name: t("history.strip.aria") });
    await within(strip).findByText("nas");
    expect(within(strip).getByText(t("history.remotes.inSync"))).toBeInTheDocument();
    expect(within(strip).getByText("↑2")).toBeInTheDocument();
    expect(within(strip).getByText(t("history.remotes.unreachable"))).toBeInTheDocument();
    expect(
      within(strip).getByText(t("history.remotes.error", { message: errorMessage("remote_auth_failed") })),
    ).toBeInTheDocument();
    expect(within(strip).getByText(t("history.remotes.notPushed"))).toBeInTheDocument();
    expect(within(strip).queryByText(t("history.remotes.none"))).toBeNull();
  });

  it("shows No backup remote with Add, and Remotes opens Settings", async () => {
    const { user, onOpenSettings } = renderPage();
    const strip = await screen.findByRole("region", { name: t("history.strip.aria") });
    expect(await within(strip).findByText(t("history.remotes.none"))).toBeInTheDocument();
    await user.click(within(strip).getByRole("button", { name: t("history.remotes.add") }));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: t("history.toolbar.remotes") }));
    expect(onOpenSettings).toHaveBeenCalledTimes(2);
  });

  it("lists merge conflicts", async () => {
    renderPage({
      commands: {
        listMergeConflicts: async () =>
          ok([{ source: "nas/laptop", entity: "secrets", file: "secrets.toml", field: "GITHUB_PAT" }]),
      },
    });
    const strip = await screen.findByRole("region", { name: t("history.strip.aria") });
    expect(await within(strip).findByText(t("history.conflicts.label", { count: 1 }))).toBeInTheDocument();
    expect(within(strip).getByText("secrets GITHUB_PAT")).toBeInTheDocument();
    expect(within(strip).getByText(t("history.conflicts.hint"))).toBeInTheDocument();
  });

  it("moves focus into the tag dialog and back to its button (#11)", async () => {
    const { user } = renderPage();
    await expectModalKeyboard(user, {
      trigger: await screen.findByRole("button", { name: t("history.toolbar.tagCurrent") }),
      name: t("history.tag.titleCurrent"),
      initial: (dialog) => within(dialog).getByRole("textbox", { name: t("history.tag.name") }),
    });
  });

  it("moves focus into the restore dialog and back to its button (#11)", async () => {
    const { user } = renderPage();
    const { actions } = await selectRow(user, SERVER_EDIT.summary);
    await expectModalKeyboard(user, {
      trigger: within(actions).getByRole("button", { name: t("history.action.restoreVersion") }),
      name: t("history.restore.titleVersion", { id: SERVER_EDIT.shortId }),
    });
  });

  it("moves focus to Cancel in the undo confirm and back to Undo (#11)", async () => {
    const { user } = renderPage({
      commands: {
        undoApply: async () =>
          mockIpcError({ code: "confirmation_required", message: "later applies changed the same entries" }),
      },
    });
    const { actions } = await selectRow(user, OLDER_APPLY.summary);
    await expectModalKeyboard(user, {
      trigger: within(actions).getByRole("button", { name: t("apply.undo.action") }),
      name: t("apply.undo.action"),
      role: "alertdialog",
      description: t("apply.undo.confirmMessage"),
      initial: (dialog) => within(dialog).getByRole("button", { name: t("history.cancel") }),
    });
  });
});
