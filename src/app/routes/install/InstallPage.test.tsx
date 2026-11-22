import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MOCK_SKILL_INSTALLED, mockIpcError, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { InstallPage } from "./InstallPage";

function renderPage(commands: MockTransportOptions["commands"] = {}) {
  const onOpenSkills = vi.fn();
  const view = renderWithProviders(<InstallPage onOpenSkills={onOpenSkills} />, { commands });
  return { onOpenSkills, ...view };
}

describe("InstallPage", () => {
  it("offers Skill (Git, Local) and MCP; no marketplace or plugin tabs", () => {
    renderPage();
    const toolbar = screen.getByTestId("header-toolbar");
    expect(within(toolbar).getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Skill", "MCP"]);
    expect(screen.getAllByRole("tab", { name: /Git|Local/ }).map((tab) => tab.textContent)).toEqual(["Git", "Local"]);
    expect(screen.queryByText(/Marketplace|Plugin/)).toBeNull();
  });

  it("installs from a git URL with the optional folder, ref and id", async () => {
    const { user, transport, onOpenSkills } = renderPage();
    const install = screen.getByRole("button", { name: "Install Skill" });
    expect(install).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "Git Repository URL" }), " https://github.com/acme/skills ");
    await user.type(screen.getByRole("textbox", { name: "Skill Folder in the Repository (optional)" }), "skills/alpha");
    await user.type(screen.getByRole("textbox", { name: "Branch or Tag (optional)" }), "main");
    await user.click(install);
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({
        command: "installGitSkill",
        args: ["https://github.com/acme/skills", "skills/alpha", "main", null, false],
      }),
    );
    expect(await screen.findByText("Installed repo-notes as repo-notes.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "View in Skills" }));
    expect(onOpenSkills).toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: "Git Repository URL" })).toHaveValue("");
  });

  it("asks before installing a taken id as a new skill", async () => {
    const { user, transport } = renderPage({
      installGitSkill: async (_url, _path, _ref, _id, renameIfTaken) =>
        renameIfTaken
          ? ok({ ...MOCK_SKILL_INSTALLED, id: "repo-notes-2", renamedFrom: "repo-notes" })
          : mockIpcError({ code: "already_exists", message: "taken" }),
    });
    await user.type(screen.getByRole("textbox", { name: "Git Repository URL" }), "https://github.com/acme/notes");
    await user.click(screen.getByRole("button", { name: "Install Skill" }));
    const confirm = await screen.findByRole("dialog", { name: "Skill id already in the store" });
    // A taken id is a question, not an error toast.
    expect(screen.queryByText("An entry with this name already exists.")).toBeNull();
    await user.click(within(confirm).getByRole("button", { name: "Install as a new skill" }));
    await waitFor(() =>
      expect(transport.invocations.filter((call) => call.command === "installGitSkill").map((call) => call.args[4])).toEqual([
        false,
        true,
      ]),
    );
    expect(await screen.findByText("Installed repo-notes as repo-notes-2.")).toBeInTheDocument();
  });

  it("installs a picked local folder by its token, never a typed path", async () => {
    const { user, transport } = renderPage({
      pickSkillSource: async () => ok({ token: "folder-1", path: "~/Downloads/repo-notes", name: "repo-notes" }),
    });
    await user.click(screen.getByRole("tab", { name: "Local" }));
    expect(screen.queryByRole("textbox", { name: /path/i })).toBeNull();
    const install = screen.getByRole("button", { name: "Install Skill" });
    expect(install).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Choose archive" }));
    expect(transport.invocations).toContainEqual({ command: "pickSkillSource", args: ["archive"] });
    expect(await screen.findByText("~/Downloads/repo-notes")).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "Skill Id (optional)" }), "notes");
    await user.click(install);
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({ command: "installLocalSkill", args: ["folder-1", "notes", false] }),
    );
  });

  it("opens the MCP form and the paste box", async () => {
    const { user } = renderPage();
    await user.click(within(screen.getByTestId("header-toolbar")).getByRole("tab", { name: "MCP" }));
    await user.click(screen.getByRole("button", { name: /Paste config/ }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });
});
