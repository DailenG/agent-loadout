// The Skills list's windowed rendering (#17) with a stand-in observer.

import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { t } from "@/i18n/en";
import type { SkillInfo, SkillList } from "@/ipc/bindings";
import { FakeIntersectionObserver as Fake } from "@/test/fakeIntersectionObserver";
import { MOCK_SKILL_CLIENTS, ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { SkillsPage } from "./SkillsPage";

function skill(id: string): SkillInfo {
  return {
    id,
    name: id,
    description: `${id} skill.`,
    fields: [],
    tags: [],
    source: { kind: "local", url: null, path: null, gitRef: null, commit: null },
    allDetected: false,
    clients: [],
    method: "link",
    projectEnabled: false,
    projectShared: false,
    links: [],
    bytes: 1024,
    hasContent: true,
    problems: [],
  };
}

function listOf(ids: string[]): SkillList {
  return { clients: MOCK_SKILL_CLIENTS, project: null, agentsCli: null, sizeWarning: null, skills: ids.map(skill) };
}

describe("Skills list slots (#17)", () => {
  beforeEach(() => {
    vi.stubGlobal("IntersectionObserver", Fake);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    Fake.last = null;
  });

  it("keeps a card with its tag editor open rendered when it scrolls away", async () => {
    const { user } = renderWithProviders(<SkillsPage onOpenInstall={() => undefined} />, {
      commands: {
        listSkills: async () => ok(listOf(["alpha", "beta"])),
        listSkillToolDirs: async () => ok([]),
      },
    });
    const alpha = await screen.findByRole("article", { name: "alpha" });
    await user.click(within(alpha).getByRole("button", { name: "Add tag" }));
    const editor = screen.getByRole("dialog", { name: "Set tag for alpha" });
    await user.type(within(editor).getByRole("textbox"), "doc");

    Fake.last!.report(() => false);
    expect(screen.getByRole("article", { name: "alpha" })).toBeInTheDocument();
    expect(within(screen.getByRole("dialog", { name: "Set tag for alpha" })).getByRole("textbox")).toHaveValue("doc");
    // The other card left; its placeholder still names it.
    expect(screen.queryByRole("article", { name: "beta" })).toBeNull();
    expect(screen.getByRole("heading", { name: "beta" })).toBeInTheDocument();
  });

  it("keys grid rows by position: a skill added above keeps every row's slot", async () => {
    let ids = ["b", "c", "d", "e"];
    const { user, container } = renderWithProviders(<SkillsPage onOpenInstall={() => undefined} />, {
      commands: {
        listSkills: async () => ok(listOf(ids)),
        listSkillToolDirs: async () => ok([]),
      },
    });
    await screen.findByRole("article", { name: "b" });
    await user.click(screen.getByRole("button", { name: t("skills.view.grid") }));
    await screen.findByRole("article", { name: "b" });
    const rowsBefore = [...container.querySelectorAll(".skill-card-grid__row")];
    expect(rowsBefore).toHaveLength(2);

    ids = ["a", ...ids];
    await user.click(screen.getByRole("button", { name: t("skills.refresh") }));
    await screen.findByRole("article", { name: "a" });
    await waitFor(() => expect(container.querySelectorAll(".skill-card-grid__row")).toHaveLength(2));
    const rowsAfter = [...container.querySelectorAll(".skill-card-grid__row")];
    expect(rowsAfter[0]).toBe(rowsBefore[0]);
    expect(rowsAfter[1]).toBe(rowsBefore[1]);
  });
});
