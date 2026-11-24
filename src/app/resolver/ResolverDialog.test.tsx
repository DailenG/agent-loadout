import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ApplyDrawer } from "@/app/apply/ApplyDrawer";
import { ResolverDialog } from "@/app/resolver/ResolverDialog";
import { errorMessage, t } from "@/i18n/en";
import type { EntryRef, PendingPlan, ResolveAction, ResolverDetail } from "@/ipc/bindings";
import { MOCK_CLIENTS, mockIpcError, ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const ENTRY: EntryRef = {
  file: { projectId: null, scope: "user", file: "~/.claude.json" },
  client: "claude-code",
  name: "github",
};

const DRIFTED: ResolverDetail = {
  file: "~/.claude.json",
  project: null,
  client: "claude-code",
  name: "github",
  server: "github",
  state: "drifted",
  lastWritten: '{ "mcpServers": { "github": { "args": ["--v1"] } } }',
  current: '{ "mcpServers": { "github": { "args": ["--edited"] } } }',
  desired: '{ "mcpServers": { "github": { "args": ["--v1"] } } }',
  actions: ["overwrite", "import_theirs", "release", "skip"],
  problem: null,
  fingerprint: "hmac-sha256:shown",
};

function renderResolver(detail: ResolverDetail) {
  return renderWithProviders(
    <ResolverDialog entry={ENTRY} clientName={(id) => (id === "claude-code" ? "Claude Code" : id)} onClose={() => undefined} />,
    { commands: { resolverDetail: async () => ok(detail) } },
  );
}

function actionButtons(dialog: HTMLElement) {
  return within(within(dialog).getByRole("group", { name: t("resolver.actions") }))
    .getAllByRole("button")
    .map((button) => button.textContent);
}

describe("ResolverDialog", () => {
  it("shows last written, current file and desired side by side", async () => {
    renderResolver(DRIFTED);
    const dialog = await screen.findByRole("dialog", { name: t("resolver.title", { name: "github" }) });
    const last = await within(dialog).findByRole("region", { name: t("resolver.column.lastWritten") });
    expect(last).toHaveTextContent("--v1");
    expect(within(dialog).getByRole("region", { name: t("resolver.column.current") })).toHaveTextContent("--edited");
    expect(within(dialog).getByRole("region", { name: t("resolver.column.desired") })).toHaveTextContent("--v1");
    expect(within(dialog).getByText(t("apply.state.drifted"))).toBeInTheDocument();
    expect(actionButtons(dialog)).toEqual([
      t("resolver.action.overwrite"),
      t("resolver.action.importTheirs"),
      t("resolver.action.release"),
      t("resolver.action.skip"),
    ]);
  });

  it.each<[ResolveAction, string]>([
    ["overwrite", "resolver.action.overwrite"],
    ["import_theirs", "resolver.action.importTheirs"],
    ["release", "resolver.action.release"],
  ])("sends %s for the entry", async (action, label) => {
    const { user, transport } = renderResolver(DRIFTED);
    const dialog = await screen.findByRole("dialog");
    await user.click(await within(dialog).findByRole("button", { name: t(label as never) }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "resolveEntry")?.args).toEqual([
        ENTRY,
        action,
        "hmac-sha256:shown",
      ]),
    );
  });

  it("offers re-create and respect deletion for a missing entry", async () => {
    const { user, transport } = renderResolver({
      ...DRIFTED,
      state: "missing",
      current: null,
      actions: ["recreate", "respect_deletion", "skip"],
    });
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(t("resolver.column.deleted"))).toBeInTheDocument();
    expect(actionButtons(dialog)).toEqual([
      t("resolver.action.recreate"),
      t("resolver.action.respectDeletion"),
      t("resolver.action.skip"),
    ]);
    await user.click(within(dialog).getByRole("button", { name: t("resolver.action.respectDeletion") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "resolveEntry")?.args).toEqual([
        ENTRY,
        "respect_deletion",
        "hmac-sha256:shown",
      ]),
    );
  });

  it("shows a conflict whose last written version cannot be recovered", async () => {
    renderResolver({ ...DRIFTED, state: "conflict", lastWritten: null, desired: '{ "args": ["--v2"] }' });
    const dialog = await screen.findByRole("dialog");
    const last = await within(dialog).findByRole("region", { name: t("resolver.column.lastWritten") });
    expect(last).toHaveTextContent(t("resolver.column.unknown"));
    expect(within(dialog).getByRole("region", { name: t("resolver.column.desired") })).toHaveTextContent("--v2");
  });

  it("offers adopt and overwrite, import theirs and skip for a collision", async () => {
    const { user, transport } = renderResolver({
      ...DRIFTED,
      state: "collision",
      lastWritten: null,
      actions: ["overwrite", "import_theirs", "skip"],
    });
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(t("resolver.column.neverWritten"))).toBeInTheDocument();
    expect(actionButtons(dialog)).toEqual([
      t("resolver.action.overwrite"),
      t("resolver.action.importTheirs"),
      t("resolver.action.skip"),
    ]);
    await user.click(within(dialog).getByRole("button", { name: t("resolver.action.skip") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "resolveEntry")?.args).toEqual([
        ENTRY,
        "skip",
        "hmac-sha256:shown",
      ]),
    );
  });

  it("shows the entry again when it changed since it was shown", async () => {
    let version = 0;
    const { user, transport } = renderWithProviders(
      <ResolverDialog entry={ENTRY} clientName={(id) => id} onClose={() => undefined} />,
      {
        commands: {
          resolverDetail: async () => {
            version += 1;
            return ok(
              version === 1
                ? DRIFTED
                : { ...DRIFTED, current: '{ "args": ["--edited-again"] }', fingerprint: "hmac-sha256:now" },
            );
          },
          resolveEntry: async () =>
            mockIpcError({ code: "entry_changed", message: "github changed since it was shown" }),
        },
      },
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(await within(dialog).findByRole("button", { name: t("resolver.action.importTheirs") }));
    expect(await screen.findByText(errorMessage("entry_changed"))).toBeInTheDocument();
    await waitFor(() =>
      expect(within(dialog).getByRole("region", { name: t("resolver.column.current") })).toHaveTextContent(
        "--edited-again",
      ),
    );
    // The next action carries the new fingerprint.
    await user.click(within(dialog).getByRole("button", { name: t("resolver.action.overwrite") }));
    await waitFor(() =>
      expect(transport.invocations.filter((call) => call.command === "resolveEntry").at(-1)?.args).toEqual([
        ENTRY,
        "overwrite",
        "hmac-sha256:now",
      ]),
    );
  });

  it("opens from the drawer's entry that needs a decision", async () => {
    const plan: PendingPlan = {
      applyMode: "instant",
      files: [
        {
          file: "~/.claude.json",
          clients: ["claude-code"],
          scope: "user",
          projectId: null,
          project: null,
          counts: { create: 0, update: 0, remove: 0, disable: 0 },
          diff: "",
          entries: [
            {
              action: "skip",
              name: "github",
              client: "claude-code",
              server: "github",
              state: "drifted",
              unresolved: true,
              problem: null,
            },
          ],
          problem: null,
          warnings: [],
          readOnly: null,
        },
      ],
      pending: 1,
      writes: 0,
      unresolved: 1,
      warnings: [],
    };
    const { user, transport } = renderWithProviders(<ApplyDrawer open onClose={() => undefined} />, {
      commands: {
        pendingPlan: async () => ok(plan),
        listServers: async () => ok({ servers: [], clients: MOCK_CLIENTS, warnings: [] }),
        resolverDetail: async () => ok(DRIFTED),
      },
    });
    await user.click(await screen.findByRole("button", { name: /~\/\.claude\.json/ }));
    await user.click(screen.getByRole("button", { name: t("apply.entries.resolveAria", { name: "github" }) }));
    await screen.findByRole("dialog", { name: t("resolver.title", { name: "github" }) });
    expect(transport.invocations.find((call) => call.command === "resolverDetail")?.args).toEqual([ENTRY]);
  });
});
