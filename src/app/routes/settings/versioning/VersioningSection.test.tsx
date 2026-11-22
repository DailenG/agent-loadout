import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { VersioningSection } from "@/app/routes/settings/versioning/VersioningSection";
import { errorMessage, t } from "@/i18n/en";
import type { RemoteEntry, RemoteStatusDto } from "@/ipc/bindings";
import { MOCK_REMOTES, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const STATUS: RemoteStatusDto = {
  reachable: true,
  pushedAt: null,
  pulledAt: null,
  ahead: 0,
  behind: 0,
  lastError: null,
};

// Locations arrive with the home folder as `~`, as the backend sends them.
function folderRemote(name: string, status: Partial<RemoteStatusDto>, extra: Partial<RemoteEntry> = {}): RemoteEntry {
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
    status: { ...STATUS, ...status },
    ...extra,
  };
}

const REMOTES: RemoteEntry[] = [
  folderRemote("usb", {}, { syncTool: "dropbox" }),
  folderRemote("nas", { ahead: 2 }),
  folderRemote("drive", { reachable: false, ahead: 1 }),
  {
    ...folderRemote("origin", {
      reachable: null,
      lastError: { error: { code: "remote_auth_failed", message: "denied" }, at: "2026-11-20T10:00:00Z" },
    }),
    kind: "git",
    location: "https://git.example.com/me/store.git",
    mode: null,
    auth: "token",
    tokenStored: true,
  },
];

function renderSection(options: MockTransportOptions = {}, onOpenPending?: () => void) {
  return renderWithProviders(<VersioningSection onOpenPending={onOpenPending} />, {
    commands: {
      remotesOverview: async () => ok({ ...MOCK_REMOTES, remotes: REMOTES }),
      ...options.commands,
    },
  });
}

describe("VersioningSection", () => {
  it('shows "No backup remote" with Add when there is no remote', async () => {
    renderWithProviders(<VersioningSection />);
    expect(await screen.findByText("No backup remote")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Versioning & Backup" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add remote" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: t("settings.versioning.pushAll") })).toBeNull();
    expect(screen.getByText(t("settings.versioning.retention.days", { days: 90 }))).toBeInTheDocument();
    expect(screen.getByText("Automatic tags are kept for 90 days")).toBeInTheDocument();
  });

  it("lists remotes in the order given with their status", async () => {
    renderSection();
    const cards = await screen.findAllByRole("article");
    expect(cards.map((card) => card.getAttribute("aria-label"))).toEqual(["usb", "nas", "drive", "origin"]);

    const usb = screen.getByRole("article", { name: "usb" });
    expect(within(usb).getByText("In sync")).toBeInTheDocument();
    expect(within(usb).getByText("~/backups/usb")).toBeInTheDocument();
    expect(within(usb).getByText(t("settings.versioning.mode.repoPerMachine"))).toBeInTheDocument();
    expect(
      within(usb).getByText("Inside a Dropbox folder: supported, one subfolder per machine"),
    ).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "nas" })).getByText("Ahead 2")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "drive" })).getByText("Unreachable")).toBeInTheDocument();

    const origin = screen.getByRole("article", { name: "origin" });
    expect(
      within(origin).getByText(`Last error: ${errorMessage("remote_auth_failed")}`),
    ).toBeInTheDocument();
    expect(within(origin).getByText(t("settings.versioning.auth.token"))).toBeInTheDocument();
    expect(within(origin).getByText(t("settings.versioning.token.stored"))).toBeInTheDocument();
    expect(within(origin).getByRole("button", { name: "Set token..." })).toBeInTheDocument();
    // Only token remotes carry token buttons.
    expect(within(usb).queryByRole("button", { name: "Set token..." })).toBeNull();
  });

  it("pushes one remote by name, and all remotes with null", async () => {
    const { user, transport } = renderSection({
      commands: {
        pushRemotes: async (name) =>
          ok([{ remote: name ?? "usb", ok: true, head: "abc", pulled: false, error: null }]),
      },
    });
    const usb = await screen.findByRole("article", { name: "usb" });
    await user.click(within(usb).getByRole("button", { name: "Push" }));
    await waitFor(() =>
      expect(transport.invocations.filter((call) => call.command === "pushRemotes").map((call) => call.args)).toEqual([
        ["usb"],
      ]),
    );
    expect(await screen.findByText(t("settings.versioning.push.done", { remotes: "usb" }))).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Push all" }));
    await waitFor(() =>
      expect(transport.invocations.filter((call) => call.command === "pushRemotes").map((call) => call.args)).toEqual([
        ["usb"],
        [null],
      ]),
    );
  });

  it("offers Review and apply after a pull that changed desired state", async () => {
    const onOpenPending = vi.fn();
    const { user, transport } = renderSection(
      {
        commands: {
          pullRemotes: async () =>
            ok([
              {
                remote: "nas",
                ok: true,
                heads: [],
                conflicts: 1,
                changedServers: 2,
                changedSkills: 1,
                artifactsIgnored: 0,
                error: null,
              },
            ]),
        },
      },
      onOpenPending,
    );
    const nas = await screen.findByRole("article", { name: "nas" });
    await user.click(within(nas).getByRole("button", { name: "Pull" }));
    expect(
      await screen.findByText("2 servers, 1 skills changed by pull. Review and apply?"),
    ).toBeInTheDocument();
    expect(transport.invocations.find((call) => call.command === "pullRemotes")?.args).toEqual(["nas"]);
    expect(
      screen.getByText(t("settings.versioning.pull.conflicts", { remote: "nas", count: 1 })),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Review and apply" }));
    expect(onOpenPending).toHaveBeenCalledTimes(1);
  });

  it("sets and clears a remote token without showing it", async () => {
    const { user, transport } = renderSection();
    const origin = await screen.findByRole("article", { name: "origin" });
    await user.click(within(origin).getByRole("button", { name: "Set token..." }));
    const dialog = await screen.findByRole("dialog", { name: t("settings.versioning.token.title", { name: "origin" }) });
    const input = within(dialog).getByLabelText(t("settings.versioning.token.label"));
    expect(input).toHaveAttribute("type", "password");
    await user.type(input, "ghp_secret");
    await user.click(within(dialog).getByRole("button", { name: t("settings.versioning.token.save") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "setRemoteToken")?.args).toEqual([
        "origin",
        "ghp_secret",
      ]),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await user.click(within(origin).getByRole("button", { name: "Clear token" }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "clearRemoteToken")?.args).toEqual(["origin"]),
    );
  });

  it("removes a remote after confirmation", async () => {
    const { user, transport } = renderSection();
    const nas = await screen.findByRole("article", { name: "nas" });
    await user.click(within(nas).getByRole("button", { name: "Remove" }));
    const confirm = await screen.findByRole("alertdialog", { name: t("settings.versioning.remove.title", { name: "nas" }) });
    expect(transport.calls).not.toContain("removeRemote");
    await user.click(within(confirm).getByRole("button", { name: t("settings.versioning.remove.confirm") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "removeRemote")?.args).toEqual(["nas"]),
    );
  });
});
