import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { DEMO_USER_CATALOGS, MOCK_CATALOGS } from "@/test/catalogData";
import { renderWithProviders } from "@/test/renderWithProviders";
import { CatalogsSection } from "./CatalogsSection";

function invoked(transport: { invocations: { command: string; args: unknown[] }[] }, command: string) {
  return transport.invocations.filter((call) => call.command === command).map((call) => call.args);
}

describe("CatalogsSection", () => {
  it("lists the catalogs and turns the online setting on, contacting nothing itself", async () => {
    const { user, transport } = renderWithProviders(<CatalogsSection />);
    const list = await screen.findByRole("list", { name: "Catalogs" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(3);
    expect(screen.getByText(/Off: nothing is contacted/)).toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: "Online catalogs" }));
    await waitFor(() => expect(invoked(transport, "setOnlineCatalogs")).toEqual([[true]]));
    expect(await screen.findByText(/On: Browse contacts registry\.modelcontextprotocol\.io, github\.com/)).toBeInTheDocument();
    expect(transport.calls).not.toContain("searchCatalog");
    expect(transport.calls).not.toContain("refreshCatalog");
  });

  it("turns a built-in off but offers no Remove for it", async () => {
    const { user, transport } = renderWithProviders(<CatalogsSection />);
    const use = await screen.findByRole("checkbox", { name: "Use Official MCP Registry" });
    expect(use).toBeChecked();
    await user.click(use);
    await waitFor(() => expect(invoked(transport, "setCatalogEnabled")).toEqual([["mcp-registry", false]]));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "Use Official MCP Registry" })).not.toBeChecked());
    expect(screen.queryByRole("button", { name: "Remove Official MCP Registry" })).toBeNull();
  });

  it("adds a git tap with a branch", async () => {
    const { user, transport } = renderWithProviders(<CatalogsSection />);
    await user.click(await screen.findByRole("button", { name: "Add catalog…" }));
    const dialog = await screen.findByRole("dialog", { name: "Add a catalog" });
    await user.click(within(dialog).getByRole("radio", { name: /Git tap/ }));
    await user.type(within(dialog).getByRole("textbox", { name: "Address" }), "https://github.com/acme/skills");
    await user.type(within(dialog).getByRole("textbox", { name: "Branch, tag or commit (optional)" }), "main");
    await user.type(within(dialog).getByRole("textbox", { name: "ID (optional)" }), "team-skills");
    await user.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() =>
      expect(invoked(transport, "addCatalog")).toEqual([["git_tap", "https://github.com/acme/skills", "team-skills", "main", null]]),
    );
    expect(await screen.findByRole("checkbox", { name: "Use https://github.com/acme/skills" })).toBeChecked();
  });

  it("removes a user-added catalog after confirmation", async () => {
    const { user, transport } = renderWithProviders(<CatalogsSection />, {
      catalogs: { catalogs: [...MOCK_CATALOGS, ...DEMO_USER_CATALOGS] },
    });
    await user.click(await screen.findByRole("button", { name: "Remove Team skills" }));
    const confirm = await screen.findByRole("alertdialog", { name: "Remove Team skills?" });
    await waitFor(() => expect(within(confirm).getByRole("button", { name: "Cancel" })).toHaveFocus());
    await user.click(within(confirm).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(invoked(transport, "removeCatalog")).toEqual([["team-skills"]]));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Remove Team skills" })).toBeNull());
  });
});
