import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AgentSkillStatus } from "@/ipc/bindings";
import { MOCK_AGENT_SKILL_STATUS, ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { AgentSection, useAgentSkillStartup } from "./AgentSection";

const INSTALLED: AgentSkillStatus = {
  ...MOCK_AGENT_SKILL_STATUS,
  bundledVersion: "0.6.0",
  copy: { version: "0.6.0", state: "current" },
  clients: [
    { id: "omp", name: "oh-my-pi", detected: true, enabled: true },
    { id: "claude-code", name: "Claude Code", detected: true, enabled: false },
    { id: "codex", name: "Codex", detected: false, enabled: false },
    { id: "cursor", name: "Cursor", detected: false, enabled: true },
  ],
};

function StartupProbe() {
  useAgentSkillStartup();
  return null;
}

describe("AgentSection", () => {
  it("offers detected clients only, with nothing installed yet", async () => {
    renderWithProviders(<AgentSection />);
    const list = await screen.findByRole("list", { name: "Clients with the Loadout skill" });
    const boxes = within(list).getAllByRole("checkbox");
    expect(boxes).toHaveLength(2);
    expect(boxes[0]).toHaveAccessibleName("Loadout skill in oh-my-pi");
    expect(boxes.every((box) => !(box as HTMLInputElement).checked)).toBe(true);
    expect(screen.getByText(/Bundled with Loadout 0\.0\.0-mock\./)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove from store" })).not.toBeInTheDocument();
  });

  it("turns the skill on and off per client through the backend", async () => {
    const { user, transport } = renderWithProviders(<AgentSection />, {
      commands: { agentSkillStatus: async () => ok(INSTALLED) },
    });
    // An enabled client that is no longer detected stays listed so it can be turned off.
    const list = await screen.findByRole("list", { name: "Clients with the Loadout skill" });
    expect(within(list).getAllByRole("checkbox")).toHaveLength(3);
    expect(within(list).getByText("Not detected")).toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: "Loadout skill in Claude Code" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({ command: "setAgentSkillClient", args: ["claude-code", true] }),
    );
    await user.click(screen.getByRole("checkbox", { name: "Loadout skill in oh-my-pi" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({ command: "setAgentSkillClient", args: ["omp", false] }),
    );
    expect(screen.getByText("Version 0.6.0 is in your store and up to date.")).toBeInTheDocument();
  });

  it("removes the skill from the store after confirmation", async () => {
    const { user, transport } = renderWithProviders(<AgentSection />, {
      commands: { agentSkillStatus: async () => ok(INSTALLED) },
    });
    await user.click(await screen.findByRole("button", { name: "Remove from store" }));
    const confirm = screen.getByRole("alertdialog", { name: "Remove the Loadout skill?" });
    await user.click(within(confirm).getByRole("button", { name: "Remove from store" }));
    await waitFor(() => expect(transport.calls).toContain("removeAgentSkill"));
  });

  it("restores Loadout's version of a customized copy after confirmation", async () => {
    const { user, transport } = renderWithProviders(<AgentSection />, {
      commands: {
        agentSkillStatus: async () => ok({ ...INSTALLED, copy: { version: "0.5.0", state: "customized" } }),
      },
    });
    expect(
      await screen.findByText("Your store copy (0.5.0) was edited, so Loadout keeps it as it is and doesn't update it to 0.6.0."),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Restore Loadout's version" }));
    const confirm = screen.getByRole("alertdialog", { name: "Restore Loadout's version?" });
    expect(transport.calls).not.toContain("resetAgentSkill");
    await user.click(within(confirm).getByRole("button", { name: "Restore Loadout's version" }));
    await waitFor(() => expect(transport.calls).toContain("resetAgentSkill"));
  });

  it("updates an outdated copy without asking", async () => {
    const { user, transport } = renderWithProviders(<AgentSection />, {
      commands: {
        agentSkillStatus: async () => ok({ ...INSTALLED, copy: { version: "0.5.0", state: "outdated" } }),
      },
    });
    await user.click(await screen.findByRole("button", { name: "Restore Loadout's version" }));
    await waitFor(() => expect(transport.calls).toContain("resetAgentSkill"));
  });

  it("hides Restore for a current copy", async () => {
    renderWithProviders(<AgentSection />, { commands: { agentSkillStatus: async () => ok(INSTALLED) } });
    await screen.findByRole("button", { name: "Remove from store" });
    expect(screen.queryByRole("button", { name: "Restore Loadout's version" })).not.toBeInTheDocument();
  });

  it("shows how to free the id when the user's own skill takes it", async () => {
    renderWithProviders(<AgentSection />, {
      commands: { agentSkillStatus: async () => ok({ ...MOCK_AGENT_SKILL_STATUS, taken: true }) },
    });
    expect(await screen.findByText(/loadout skills rm loadout/)).toBeInTheDocument();
    expect(screen.queryByText(/rename/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });

  it("opens the bundled skill files", async () => {
    const { user, transport } = renderWithProviders(<AgentSection />);
    await user.click(await screen.findByRole("button", { name: "View skill files" }));
    await waitFor(() => expect(transport.calls).toContain("revealAgentSkill"));
  });

  it("reports the startup update once, and client folders that need a decision", async () => {
    const { transport } = renderWithProviders(<StartupProbe />, {
      commands: {
        refreshAgentSkill: async () => ok({ kind: "updated", from: "0.5.0", to: "0.6.0", needsAttention: true }),
      },
    });
    expect(await screen.findByText("Updated the Loadout skill in your store from 0.5.0 to 0.6.0.")).toBeInTheDocument();
    expect(screen.getByText("Some skill folders need a decision; see the Skills page.")).toBeInTheDocument();
    expect(transport.calls.filter((call) => call === "refreshAgentSkill")).toHaveLength(1);
  });
});
