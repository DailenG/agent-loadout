import { screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { t } from "@/i18n/en";
import { renderWithProviders } from "@/test/renderWithProviders";
import { SkillSourceSwitcher, STORE_SOURCE, type SourceTab } from "./SkillSourceSwitcher";

const TOOLS: SourceTab[] = [
  { id: "claude-code", name: "Claude Code", count: 3 },
  { id: "omp", name: "omp", count: 2 },
  { id: "codex", name: "Codex", count: 1 },
  { id: "pi", name: "Pi", count: 0 },
  { id: "copilot", name: "Copilot", count: 0 },
  { id: "cursor", name: "Cursor", count: 0 },
  { id: "agents-cli", name: "Agents CLI", count: 0 },
];

describe("SkillSourceSwitcher", () => {
  it("lists omp first and names every tool tab without hovering", () => {
    renderWithProviders(<SkillSourceSwitcher active={STORE_SOURCE} storeCount={4} tools={TOOLS} onChange={vi.fn()} />);
    const tabs = within(screen.getByRole("tablist")).getAllByRole("tab").slice(1);
    expect(tabs.map((tab) => tab.getAttribute("aria-label"))).toEqual([
      "omp 2",
      "Claude Code 3",
      "Codex 1",
      "Pi 0",
      "Copilot 0",
    ]);
    for (const [index, tab] of tabs.entries()) {
      const name = ["omp", "Claude Code", "Codex", "Pi", "Copilot"][index];
      expect(within(tab).getByText(name)).toBeVisible();
      expect(tab).not.toHaveAttribute("title");
    }
    const mark = within(tabs[0]).getByRole("img", { name: t("recommended.badge") });
    expect(mark).toHaveAttribute("data-tooltip", t("recommended.tooltip"));
    expect(tabs[0]).toHaveAccessibleDescription(t("recommended.tooltip"));
    expect(within(tabs[1]).queryByRole("img", { name: t("recommended.badge") })).not.toBeInTheDocument();
    expect(tabs[1]).not.toHaveAttribute("aria-describedby");
  });

  it("names the tools in the More menu too", async () => {
    const { user } = renderWithProviders(
      <SkillSourceSwitcher active={STORE_SOURCE} storeCount={4} tools={TOOLS} onChange={vi.fn()} />,
    );
    await user.click(screen.getByRole("button", { name: t("skills.source.more", { count: 2 }) }));
    const items = within(screen.getByRole("menu")).getAllByRole("menuitem");
    expect(items.map((item) => item.getAttribute("aria-label"))).toEqual(["Cursor 0", "Agents CLI 0"]);
    expect(within(items[0]).getByText("Cursor")).toBeVisible();
    expect(within(items[1]).getByText("Agents CLI")).toBeVisible();
  });
});
