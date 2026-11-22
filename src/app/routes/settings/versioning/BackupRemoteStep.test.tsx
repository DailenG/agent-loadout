import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { BackupRemoteStep, FirstRunBackupRemote } from "@/app/routes/settings/versioning/BackupRemoteStep";
import { t } from "@/i18n/en";
import type { AddedRemote } from "@/ipc/bindings";
import { MOCK_REMOTES, ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const ADDED: AddedRemote = {
  remote: {
    name: "usb",
    kind: "folder",
    location: "E:/Backups/loadout",
    mode: "repo_per_machine",
    auth: null,
    username: null,
    push: "on-commit",
    pull: "manual",
    tokenStored: null,
    syncTool: null,
    status: { reachable: true, pushedAt: null, pulledAt: null, ahead: null, behind: null, lastError: null },
  },
  warnings: [],
};

describe("BackupRemoteStep", () => {
  it("offers Git URL and Folder; Not now leaves the store local-only", async () => {
    const onDone = vi.fn();
    const { user, transport } = renderWithProviders(<BackupRemoteStep onDone={onDone} />);
    expect(screen.getByRole("dialog", { name: "Add a backup remote" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Git URL" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Folder" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(transport.calls).toContain("dismissBackupRemoteOffer");
    expect(transport.calls).not.toContain("addFolderRemote");
    expect(transport.calls).not.toContain("addGitRemote");
  });

  it("adds a folder remote as part of the first run", async () => {
    const onDone = vi.fn();
    const { user, transport } = renderWithProviders(<BackupRemoteStep onDone={onDone} />, {
      commands: {
        pickRemoteFolder: async () => ok({ token: "remote-folder-2", path: "E:/Backups/loadout", syncTool: null }),
        addFolderRemote: async () => ok(ADDED),
      },
    });
    await user.click(screen.getByRole("button", { name: "Folder" }));
    await screen.findByRole("dialog", { name: t("settings.versioning.add.title") });
    await user.click(screen.getByRole("button", { name: "Choose folder..." }));
    await screen.findByText("E:/Backups/loadout");
    await user.type(screen.getByRole("textbox", { name: t("settings.versioning.add.name") }), "usb");
    await user.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(transport.invocations.find((call) => call.command === "addFolderRemote")?.args).toEqual([
      "usb",
      "remote-folder-2",
      "repo_per_machine",
      true,
    ]);
    expect(transport.calls).not.toContain("dismissBackupRemoteOffer");
  });
});

describe("FirstRunBackupRemote", () => {
  it("renders nothing when the step is not offered", async () => {
    const { transport } = renderWithProviders(<FirstRunBackupRemote />);
    await waitFor(() => expect(transport.calls).toContain("remotesOverview"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows the step while it is offered, and hides it after Not now", async () => {
    const { user } = renderWithProviders(<FirstRunBackupRemote />, {
      commands: { remotesOverview: async () => ok({ ...MOCK_REMOTES, offerBackupRemote: true }) },
    });
    await screen.findByRole("dialog", { name: "Add a backup remote" });
    await user.click(screen.getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
