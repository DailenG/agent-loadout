import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { UpdatesSection, useStartupUpdateCheck } from "@/app/routes/settings/updates/UpdatesSection";
import { errorMessage, t } from "@/i18n/en";
import type { UpdateOffer, UpdaterStatus } from "@/ipc/bindings";
import { mockIpcError, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const OFFER: UpdateOffer = { version: "0.2.0", currentVersion: "0.1.0", notes: null };

function withUpdater(status: UpdaterStatus, commands: MockTransportOptions["commands"] = {}): MockTransportOptions {
  let current = status;
  return {
    commands: {
      updaterStatus: async () => ok(current),
      setCheckForUpdates: async (enabled) => {
        current = { ...current, enabled };
        return ok(current);
      },
      ...commands,
    },
  };
}

function StartupCheck() {
  useStartupUpdateCheck();
  return null;
}

describe("UpdatesSection", () => {
  it("says where updates come from when the build has no updater, and never checks", async () => {
    const { transport } = renderWithProviders(<UpdatesSection />);
    expect(await screen.findByText(t("settings.updates.unavailable.description"))).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(transport.calls).not.toContain("checkForUpdate");
  });

  it("checks on request and offers the install", async () => {
    const { transport, user } = renderWithProviders(
      <UpdatesSection />,
      withUpdater({ available: true, enabled: true }, { checkForUpdate: async () => ok(OFFER), installUpdate: async () => ok(null) }),
    );
    expect(await screen.findByRole("checkbox", { name: t("settings.updates.switch.label") })).toBeChecked();
    expect(transport.calls).not.toContain("checkForUpdate");

    await user.click(screen.getByRole("button", { name: t("settings.updates.check.action") }));
    expect(
      await screen.findByText(t("settings.updates.offer", { version: "0.2.0", current: "0.1.0" })),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: t("settings.updates.install") }));
    await waitFor(() => expect(transport.calls).toContain("installUpdate"));
  });

  it("reports the newest version and failures", async () => {
    let answer = ok<UpdateOffer | null>(null);
    const { user } = renderWithProviders(
      <UpdatesSection />,
      withUpdater({ available: true, enabled: true }, { checkForUpdate: async () => answer }),
    );
    await user.click(await screen.findByRole("button", { name: t("settings.updates.check.action") }));
    expect(await screen.findByText(t("settings.updates.current"))).toBeInTheDocument();

    answer = mockIpcError({ code: "update_failed", message: "offline" });
    await user.click(screen.getByRole("button", { name: t("settings.updates.check.action") }));
    expect(await screen.findByText(errorMessage("update_failed"))).toBeInTheDocument();
  });

  it("the switch turns checking off and disables Check now", async () => {
    const { transport, user } = renderWithProviders(
      <UpdatesSection />,
      withUpdater({ available: true, enabled: true }),
    );
    const toggle = await screen.findByRole("checkbox", { name: t("settings.updates.switch.label") });
    await user.click(toggle);
    await waitFor(() => expect(toggle).not.toBeChecked());
    expect(transport.invocations).toContainEqual({ command: "setCheckForUpdates", args: [false] });
    expect(screen.getByRole("button", { name: t("settings.updates.check.action") })).toBeDisabled();
    expect(screen.getByText(t("settings.updates.check.off"))).toBeInTheDocument();
  });
});

describe("useStartupUpdateCheck", () => {
  it("checks once at startup and announces a new release", async () => {
    const { transport } = renderWithProviders(
      <StartupCheck />,
      withUpdater({ available: true, enabled: true }, { checkForUpdate: async () => ok(OFFER) }),
    );
    expect(await screen.findByText(t("settings.updates.startupOffer", { version: "0.2.0" }))).toBeInTheDocument();
    expect(transport.calls.filter((call) => call === "checkForUpdate")).toHaveLength(1);
  });

  it("stays offline when the switch is off or the build has no updater", async () => {
    for (const status of [
      { available: true, enabled: false },
      { available: false, enabled: true },
    ]) {
      const { transport, unmount } = renderWithProviders(<StartupCheck />, withUpdater(status));
      await waitFor(() => expect(transport.calls).toContain("updaterStatus"));
      expect(transport.calls).not.toContain("checkForUpdate");
      unmount();
    }
  });

  it("says nothing when the startup check fails", async () => {
    const { transport } = renderWithProviders(
      <StartupCheck />,
      withUpdater({ available: true, enabled: true }, {
        checkForUpdate: async () => mockIpcError({ code: "update_failed", message: "offline" }),
      }),
    );
    await waitFor(() => expect(transport.calls).toContain("checkForUpdate"));
    expect(screen.queryByText(errorMessage("update_failed"))).not.toBeInTheDocument();
  });
});
