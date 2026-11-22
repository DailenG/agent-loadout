import { act, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App, createQueryClient } from "@/app/App";
import { ApplyDrawer } from "@/app/apply/ApplyDrawer";
import { McpPage } from "@/app/routes/mcp/McpPage";
import { EffectiveMatrixView } from "@/app/routes/projects/EffectiveMatrix";
import { ProjectsPage } from "@/app/routes/projects/ProjectsPage";
import { t } from "@/i18n/en";
import type { Commands } from "@/ipc/transport";
import { createMockTransport, type MockTransport } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

function callsOf(transport: MockTransport, command: keyof Commands) {
  return transport.calls.filter((call) => call === command).length;
}

/** Waits for the first call of `command`, then returns the count. */
async function settled(transport: MockTransport, command: keyof Commands) {
  await waitFor(() => expect(callsOf(transport, command)).toBeGreaterThan(0));
  return callsOf(transport, command);
}

function renderApp() {
  const transport = createMockTransport();
  const view = render(<App transport={transport} queryClient={createQueryClient()} />);
  return { transport, ...view };
}

describe("drift triggers (SPEC 8.5)", () => {
  it("rechecks the pending plan when the watcher reports a change", async () => {
    const { transport } = renderApp();
    const before = await settled(transport, "pendingPlan");

    act(() => transport.changeTargets());

    await waitFor(() => expect(callsOf(transport, "pendingPlan")).toBe(before + 1));
  });

  it("rechecks the pending plan each time the window gains focus", async () => {
    const { transport } = renderApp();
    const before = await settled(transport, "pendingPlan");

    act(() => transport.focus());
    await waitFor(() => expect(callsOf(transport, "pendingPlan")).toBe(before + 1));

    act(() => transport.focus());
    await waitFor(() => expect(callsOf(transport, "pendingPlan")).toBe(before + 2));
  });
});

describe("Refresh buttons", () => {
  it("rechecks the project list from the Projects toolbar", async () => {
    const { transport, user } = renderWithProviders(<ProjectsPage />);
    const before = await settled(transport, "listProjects");

    const toolbar = screen.getByTestId("header-toolbar");
    await user.click(within(toolbar).getByRole("button", { name: t("projects.action.refresh") }));

    await waitFor(() => expect(callsOf(transport, "listProjects")).toBe(before + 1));
  });

  it("rechecks the effective matrix from its toolbar", async () => {
    const { transport, user } = renderWithProviders(
      <EffectiveMatrixView projectId="app-a1b2" onBack={() => undefined} />,
    );
    const before = await settled(transport, "effectiveMatrix");

    const toolbar = screen.getByTestId("header-toolbar");
    await user.click(within(toolbar).getByRole("button", { name: t("projects.action.refresh") }));

    await waitFor(() => expect(callsOf(transport, "effectiveMatrix")).toBe(before + 1));
  });

  it("rechecks the pending plan from the drawer", async () => {
    const { transport, user } = renderWithProviders(<ApplyDrawer open onClose={() => undefined} />);
    const before = await settled(transport, "pendingPlan");

    const drawer = screen.getByRole("dialog", { name: t("apply.drawer.title") });
    await user.click(within(drawer).getByRole("button", { name: t("apply.drawer.refresh") }));

    await waitFor(() => expect(callsOf(transport, "pendingPlan")).toBe(before + 1));
  });

  it("rechecks the servers from the MCP toolbar", async () => {
    const { transport, user } = renderWithProviders(
      <McpPage onOpenPending={() => undefined} onOpenInstall={() => undefined} />,
    );
    const before = await settled(transport, "listServers");

    const toolbar = screen.getByTestId("header-toolbar");
    await user.click(within(toolbar).getByRole("button", { name: t("mcp.toolbar.refresh") }));

    await waitFor(() => expect(callsOf(transport, "listServers")).toBe(before + 1));
  });
});
