import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AddRemoteDialog } from "@/app/routes/settings/versioning/AddRemoteDialog";
import { errorMessage, t } from "@/i18n/en";
import type { AddedRemote, PickedRemoteFolder, RemoteEntry } from "@/ipc/bindings";
import { MOCK_REMOTES, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const PICKED: PickedRemoteFolder = { token: "remote-folder-1", path: "~/Dropbox/loadout", syncTool: "dropbox" };

const ENTRY: RemoteEntry = {
  name: "backup",
  kind: "folder",
  location: "~/Dropbox/loadout",
  mode: "repo_per_machine",
  auth: null,
  username: null,
  push: "on-commit",
  pull: "manual",
  tokenStored: null,
  syncTool: "dropbox",
  status: { reachable: true, pushedAt: null, pulledAt: null, ahead: null, behind: null, lastError: null },
};

function renderDialog(options: MockTransportOptions = {}) {
  const onAdded = vi.fn();
  const onClose = vi.fn();
  const view = renderWithProviders(
    <AddRemoteDialog open firstRun={false} onClose={onClose} onAdded={onAdded} />,
    options,
  );
  return { ...view, onAdded, onClose };
}

const kindButton = (key: "settings.versioning.kind.git" | "settings.versioning.kind.folder") =>
  screen.getByRole("button", { name: new RegExp(`^${t(key)}`) });

describe("AddRemoteDialog", () => {
  it("offers Git URL and Folder as equal choices, neither preselected", () => {
    renderDialog();
    expect(screen.getByRole("dialog", { name: t("settings.versioning.add.title") })).toBeInTheDocument();
    expect(kindButton("settings.versioning.kind.git")).toHaveAttribute("aria-pressed", "false");
    expect(kindButton("settings.versioning.kind.folder")).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByRole("button", { name: "Add" })).toBeDisabled();
  });

  it("adds a folder remote from the picked folder, repo-per-machine by default", async () => {
    const added: AddedRemote = { remote: ENTRY, warnings: [] };
    const { user, transport, onAdded } = renderDialog({
      commands: {
        pickRemoteFolder: async () => ok(PICKED),
        addFolderRemote: async () => ok(added),
      },
    });
    await user.click(kindButton("settings.versioning.kind.folder"));
    await user.click(screen.getByRole("button", { name: "Choose folder…" }));
    expect(await screen.findByText("~/Dropbox/loadout")).toBeInTheDocument();
    expect(screen.getByText("Inside a Dropbox folder: supported, one subfolder per machine")).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: new RegExp(`^${t("settings.versioning.mode.repoPerMachine")}`) })).toBeChecked();

    await user.type(screen.getByRole("textbox", { name: t("settings.versioning.add.name") }), "backup");
    await user.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() => expect(onAdded).toHaveBeenCalledWith(added));
    expect(transport.calls.indexOf("pickRemoteFolder")).toBeLessThan(transport.calls.indexOf("addFolderRemote"));
    expect(transport.invocations.find((call) => call.command === "addFolderRemote")?.args).toEqual([
      "backup",
      "remote-folder-1",
      "repo_per_machine",
      false,
    ]);
  });

  it("adds a git remote with the system credentials by default", async () => {
    const added: AddedRemote = {
      remote: { ...ENTRY, name: "origin", kind: "git", mode: null, auth: "system", syncTool: null },
      warnings: [],
    };
    const { user, transport, onAdded } = renderDialog({
      commands: { addGitRemote: async () => ok(added) },
    });
    await user.click(kindButton("settings.versioning.kind.git"));
    await user.type(
      screen.getByRole("textbox", { name: t("settings.versioning.add.url") }),
      "https://git.example.com/me/store.git",
    );
    await user.type(screen.getByRole("textbox", { name: t("settings.versioning.add.name") }), "origin");
    expect(screen.getByRole("radio", { name: new RegExp(`^${t("settings.versioning.auth.system")}`) })).toBeChecked();
    // gh is for github.com URLs, and gh is not installed here.
    expect(screen.getByRole("radio", { name: new RegExp(`^${t("settings.versioning.auth.gh")}`) })).toBeDisabled();
    expect(screen.getByText(t("settings.versioning.auth.ghMissing"))).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() => expect(onAdded).toHaveBeenCalledWith(added));
    expect(transport.invocations.find((call) => call.command === "addGitRemote")?.args).toEqual([
      "origin",
      "https://git.example.com/me/store.git",
      "system",
      null,
      false,
    ]);
  });

  it("enables gh for GitHub URLs when gh is installed", async () => {
    const { user } = renderDialog({
      commands: { remotesOverview: async () => ok({ ...MOCK_REMOTES, ghFound: true }) },
    });
    await user.click(kindButton("settings.versioning.kind.git"));
    const gh = screen.getByRole("radio", { name: new RegExp(`^${t("settings.versioning.auth.gh")}`) });
    await user.type(screen.getByRole("textbox", { name: t("settings.versioning.add.url") }), "https://github.com/me/store.git");
    await waitFor(() => expect(gh).toBeEnabled());
  });

  it("rejects an invalid remote name before sending", async () => {
    const { user, transport } = renderDialog();
    await user.click(kindButton("settings.versioning.kind.git"));
    await user.type(screen.getByRole("textbox", { name: t("settings.versioning.add.url") }), "git@host:me/store.git");
    await user.type(screen.getByRole("textbox", { name: t("settings.versioning.add.name") }), "my remote");
    expect(screen.getByRole("alert")).toHaveTextContent(t("settings.versioning.add.nameInvalid"));
    expect(screen.getByRole("button", { name: "Add" })).toBeDisabled();
    expect(transport.calls).not.toContain("addGitRemote");
  });

  it("asks for the token of a token remote after adding it, and shows warnings", async () => {
    const added: AddedRemote = {
      remote: { ...ENTRY, name: "origin", kind: "git", mode: null, auth: "token", tokenStored: false, syncTool: null },
      warnings: [{ code: "remote_unreachable", message: "offline" }],
    };
    const { user, transport, onAdded } = renderDialog({
      commands: { addGitRemote: async () => ok(added) },
    });
    await user.click(kindButton("settings.versioning.kind.git"));
    await user.type(screen.getByRole("textbox", { name: t("settings.versioning.add.url") }), "https://git.example.com/s.git");
    await user.type(screen.getByRole("textbox", { name: t("settings.versioning.add.name") }), "origin");
    await user.click(screen.getByRole("radio", { name: new RegExp(`^${t("settings.versioning.auth.token")}`) }));
    await user.type(screen.getByRole("textbox", { name: t("settings.versioning.add.username") }), "me");
    await user.click(screen.getByRole("button", { name: "Add" }));

    const dialog = await screen.findByRole("dialog", { name: t("settings.versioning.add.addedTitle", { name: "origin" }) });
    expect(transport.invocations.find((call) => call.command === "addGitRemote")?.args).toEqual([
      "origin",
      "https://git.example.com/s.git",
      "token",
      "me",
      false,
    ]);
    expect(within(dialog).getByText(errorMessage("remote_unreachable"))).toBeInTheDocument();
    const input = within(dialog).getByLabelText(t("settings.versioning.token.label"));
    expect(input).toHaveAttribute("type", "password");
    await user.type(input, "tok-123");
    await user.click(within(dialog).getByRole("button", { name: t("settings.versioning.token.save") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "setRemoteToken")?.args).toEqual([
        "origin",
        "tok-123",
      ]),
    );
    await waitFor(() => expect(onAdded).toHaveBeenCalledWith(added));
  });
});
