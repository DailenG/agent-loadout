import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { RestoreFromRemoteDialog } from "@/app/routes/settings/versioning/RestoreFromRemoteDialog";
import { t } from "@/i18n/en";
import type { AliasCheck, RemoteMachine, RestoreFromResult } from "@/ipc/bindings";
import { ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const LAPTOP: RemoteMachine = {
  storeId: "01STORE00000000000000000000",
  sameStore: true,
  machineId: "01LAPTOP0000000000000000000",
  label: "laptop",
  os: "windows",
  pushedAt: "2026-11-20T10:00:00Z",
  head: "abc1234",
  subject: "save: server/github",
  thisMachine: false,
};

const DESKTOP: RemoteMachine = {
  ...LAPTOP,
  storeId: "01OTHER00000000000000000000",
  sameStore: false,
  machineId: "01DESKTOP000000000000000000",
  label: "desktop",
  os: "linux",
  subject: null,
  thisMachine: true,
};

const RESULT: RestoreFromResult = {
  kind: "merged",
  from: "laptop (01LAPTOP0000000000000000000)",
  merge: "merged",
  conflicts: 0,
  tag: "auto/pre-restore/20261122",
  head: "def5678",
  apply: null,
};

const MISSING: AliasCheck = {
  alias: "GITHUB_PAT",
  reference: "keyring://loadout/GITHUB_PAT",
  source: "store",
  status: "missing",
  message: null,
  neededBy: [{ server: "github", clients: ["claude-code", "codex"] }],
  blocking: true,
};

describe("RestoreFromRemoteDialog", () => {
  it("lists the machines of a picked folder and restores the one picked", async () => {
    const onRestored = vi.fn();
    const onOpenPending = vi.fn();
    const onClose = vi.fn();
    const { user, transport } = renderWithProviders(
      <RestoreFromRemoteDialog open onClose={onClose} onRestored={onRestored} onOpenPending={onOpenPending} />,
      {
        commands: {
          pickRemoteFolder: async () => ok({ token: "restore-1", path: "~/NAS/loadout", syncTool: null }),
          listRestoreMachines: async () => ok({ direct: false, machines: [LAPTOP, DESKTOP], artifactsIgnored: 0 }),
          restoreFromRemote: async () => ok(RESULT),
          checkSecrets: async () => ok([MISSING]),
        },
      },
    );
    const dialog = screen.getByRole("dialog", { name: t("settings.versioning.restore.title") });
    await user.click(within(dialog).getByRole("button", { name: "Choose folder…" }));

    const table = await within(dialog).findByRole("table");
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText("laptop")).toBeInTheDocument();
    expect(within(rows[0]).getByText("Windows")).toBeInTheDocument();
    expect(within(rows[0]).getByText("save: server/github")).toBeInTheDocument();
    expect(within(rows[1]).getByText(t("settings.versioning.restore.thisMachine"))).toBeInTheDocument();
    expect(within(rows[1]).getByText(t("settings.versioning.restore.otherStore"))).toBeInTheDocument();
    expect(transport.invocations.find((call) => call.command === "listRestoreMachines")?.args).toEqual([
      { kind: "folder", token: "restore-1" },
    ]);
    // Nothing restores before a machine is picked.
    const restore = within(dialog).getByRole("button", { name: t("settings.versioning.restore.submit") });
    expect(restore).toBeDisabled();
    expect(transport.calls).not.toContain("checkSecrets");

    await user.click(within(table).getByRole("radio", { name: "laptop" }));
    await user.click(restore);

    const done = await screen.findByRole("dialog", { name: t("settings.versioning.restore.doneTitle") });
    expect(transport.invocations.find((call) => call.command === "restoreFromRemote")?.args).toEqual([
      { kind: "folder", token: "restore-1" },
      "01LAPTOP0000000000000000000",
    ]);
    expect(onRestored).toHaveBeenCalledWith(RESULT);
    expect(within(done).getByText(t("settings.versioning.restore.result.merged", { from: RESULT.from }))).toBeInTheDocument();

    // The checklist runs once, right after the restore.
    const row = await within(done).findByRole("article", { name: "GITHUB_PAT" });
    expect(within(row).getByText(t("settings.secrets.status.missing"))).toBeInTheDocument();
    expect(within(row).getByText("needed by github (claude-code, codex)")).toBeInTheDocument();
    expect(transport.invocations.filter((call) => call.command === "checkSecrets").map((call) => call.args)).toEqual([
      [[]],
    ]);

    await user.click(within(done).getByRole("button", { name: "Review and apply" }));
    expect(onOpenPending).toHaveBeenCalledTimes(1);
  });

  it("restores a direct source without a machine id", async () => {
    const { user, transport } = renderWithProviders(<RestoreFromRemoteDialog open onClose={() => undefined} />, {
      commands: {
        pickRemoteFolder: async () => ok({ token: "restore-2", path: "~/USB/mirror", syncTool: null }),
        listRestoreMachines: async () => ok({ direct: true, machines: [], artifactsIgnored: 0 }),
        restoreFromRemote: async () => ok({ ...RESULT, kind: "restored", from: "mirror" }),
      },
    });
    await user.click(screen.getByRole("button", { name: "Choose folder…" }));
    await screen.findByText(t("settings.versioning.restore.direct"));
    await user.click(screen.getByRole("button", { name: t("settings.versioning.restore.submit") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "restoreFromRemote")?.args).toEqual([
        { kind: "folder", token: "restore-2" },
        null,
      ]),
    );
  });
});
