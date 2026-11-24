import { onlineManager } from "@tanstack/react-query";
import { screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { CatalogResults } from "@/ipc/bindings";
import type { CommandResult } from "@/ipc/transport";
import {
  DEMO_USER_CATALOGS,
  MOCK_CATALOG_ENTRIES,
  MOCK_CATALOGS,
  type CatalogMockState,
} from "@/test/catalogData";
import { MOCK_MCP_REVIEW, MOCK_SKILL_REVIEW, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { BrowsePanel } from "./BrowsePanel";

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function page(catalog: string, fields: Partial<CatalogResults> = {}): CatalogResults {
  return {
    catalog,
    status: "fresh",
    ageSecs: 0,
    fetchedAt: null,
    error: null,
    entries: [],
    nextCursor: null,
    link: null,
    attribution: null,
    ...fields,
  };
}

const NETWORK_COMMANDS = [
  "searchCatalog",
  "showCatalogEntry",
  "refreshCatalog",
  "reviewCatalogMcp",
  "reviewCatalogSkill",
  "openCatalogLink",
] as const;

function renderBrowse(catalogs: Partial<CatalogMockState> = {}, commands: MockTransportOptions["commands"] = {}) {
  return renderWithProviders(<BrowsePanel onOpenSkills={() => undefined} />, { catalogs, commands });
}

function invoked(transport: { invocations: { command: string; args: unknown[] }[] }, command: string) {
  return transport.invocations.filter((call) => call.command === command).map((call) => call.args);
}

function section(name: string) {
  return screen.getByRole("region", { name });
}

async function openDetails(user: UserEvent, name: string) {
  await user.click(await screen.findByRole("button", { name: `Details of ${name}` }));
  return screen.findByRole("article", { name });
}

async function search(user: UserEvent, text: string) {
  const field = screen.getByRole("searchbox", { name: "Search catalogs" });
  await user.clear(field);
  await user.type(field, text);
  await user.click(screen.getByRole("button", { name: "Search" }));
}

describe("BrowsePanel", () => {
  it("asks once, naming what it contacts, and requests nothing before the answer", async () => {
    const { transport } = renderBrowse();
    expect(await screen.findByRole("heading", { name: "Turn on online catalogs?" })).toBeInTheDocument();
    expect(
      screen.getByText(/Loadout will contact registry\.modelcontextprotocol\.io, github\.com, only when you/),
    ).toBeInTheDocument();
    expect(screen.getByText(/installing also contacts the entry's own repository or package registry/)).toBeInTheDocument();
    expect(screen.queryByRole("searchbox")).toBeNull();
    for (const command of NETWORK_COMMANDS) {
      expect(transport.calls).not.toContain(command);
    }
  });

  it("searches each enabled catalog on its own after the opt-in, never the link-only one", async () => {
    const { user, transport } = renderBrowse();
    await user.click(await screen.findByRole("button", { name: "Turn on online catalogs" }));
    await waitFor(() => expect(invoked(transport, "setOnlineCatalogs")).toEqual([[true]]));
    // The answer's button is gone: focus moves to the search field.
    expect(await screen.findByRole("searchbox", { name: "Search catalogs" })).toHaveFocus();
    await waitFor(() =>
      expect(invoked(transport, "searchCatalog").map((args) => args[0]).sort()).toEqual(["anthropic-skills", "mcp-registry"]),
    );
    // An empty search lists the tap; the registry needs 3 characters.
    expect(
      within(section("Official MCP Registry")).getByText("Type at least 3 characters to search this registry."),
    ).toBeInTheDocument();
    expect(await within(section("Anthropic skills")).findByText("6 results")).toBeInTheDocument();
  });

  it("with online catalogs declined, shows saved results labeled with their age and contacts nothing", async () => {
    const { user, transport } = renderBrowse();
    await user.click(await screen.findByRole("button", { name: "Not now" }));
    await waitFor(() => expect(invoked(transport, "setOnlineCatalogs")).toEqual([[false]]));
    expect(await screen.findByText(/Online catalogs are off, so nothing is contacted/)).toBeInTheDocument();
    expect(screen.getByRole("searchbox", { name: "Search catalogs" })).toHaveFocus();
    expect(
      await within(section("Anthropic skills")).findByText("Online catalogs are off: 6 results saved 3 days ago."),
    ).toBeInTheDocument();
    // As the core: a registry checks the query length first, online or not.
    expect(
      within(section("Official MCP Registry")).getByText("Type at least 3 characters to search this registry."),
    ).toBeInTheDocument();
    await search(user, "weather");
    expect(
      await within(section("Official MCP Registry")).findByText(
        "Online catalogs are off and nothing is saved for this search.",
      ),
    ).toBeInTheDocument();
    expect(transport.calls).not.toContain("refreshCatalog");
  });

  it("labels saved, stale and timed-out catalogs", async () => {
    const { user } = renderBrowse(
      {
        online: true,
        decided: true,
        catalogs: [...MOCK_CATALOGS, ...DEMO_USER_CATALOGS],
        behavior: {
          "anthropic-skills": { kind: "cached", ageSecs: 2 * 3_600 },
          "team-skills": { kind: "stale", ageSecs: 26 * 3_600, message: "could not resolve host github.com" },
          acme: { kind: "error", message: "timed out after 20 s" },
        },
      },
      {},
    );
    await screen.findByRole("searchbox");
    await search(user, "conv");
    expect(await within(section("Anthropic skills")).findByText(/saved 2 hours ago/)).toBeInTheDocument();
    expect(
      await within(section("Team skills")).findByText(
        /^Stale: 1 result saved 1 day ago\. Couldn't refresh: .*could not resolve host github\.com/,
      ),
    ).toBeInTheDocument();
    expect(
      await within(section("Acme registry")).findByText("Timed out: Acme registry took too long to answer."),
    ).toBeInTheDocument();
    expect(within(section("Team skills")).getByRole("button", { name: "Search Team skills again" })).toBeInTheDocument();
  });

  it("keeps answering the other catalogs while one never answers", async () => {
    const { user, transport } = renderBrowse(
      { online: true, decided: true },
      {
        searchCatalog: async (catalog, query) =>
          catalog === "mcp-registry"
            ? new Promise<never>(() => undefined)
            : {
                status: "ok",
                data: {
                  catalog,
                  status: "fresh",
                  ageSecs: 0,
                  fetchedAt: null,
                  error: null,
                  entries: [],
                  nextCursor: null,
                  link: null,
                  attribution: null,
                } satisfies CatalogResults,
              },
      },
    );
    await screen.findByRole("searchbox");
    await search(user, "pdf");
    expect(within(section("Official MCP Registry")).getByText("Searching Official MCP Registry…")).toBeInTheDocument();
    expect(await within(section("Anthropic skills")).findByText("0 results")).toBeInTheDocument();
    expect(invoked(transport, "searchCatalog").some((args) => args[0] === "mcp-registry" && args[1] === "pdf")).toBe(true);
  });

  it("shows registry results with the checked-namespace wording, attribution, pinning and no remote icons", async () => {
    const { user, container } = renderBrowse({ online: true, decided: true });
    await screen.findByRole("searchbox");
    await search(user, "weather");
    const registry = section("Official MCP Registry");
    expect(await within(registry).findByText("Data from the Official MCP Registry")).toBeInTheDocument();
    const weather = within(registry).getByRole("listitem", { name: "Weather" });
    expect(
      within(weather).getByText(/Publisher: io\.github\.acme \(namespace ownership checked by the registry\)/),
    ).toBeInTheDocument();
    expect(within(weather).getByText(/Pinned to version 1\.2\.0/)).toBeInTheDocument();
    // The registry has no "official" marker to give.
    expect(within(registry).queryByText("Official")).toBeNull();
    const legacy = within(registry).getByRole("listitem", { name: "Weather (legacy)" });
    expect(within(legacy).getByText("Deprecated")).toBeInTheDocument();
    expect(within(legacy).getByText(/Not pinned/)).toBeInTheDocument();
    expect(container.querySelector("img, image, object, iframe")).toBeNull();
  });

  it("shows the deprecation message and every fact in the detail view, and goes back to the results", async () => {
    const { user } = renderBrowse({ online: true, decided: true });
    await screen.findByRole("searchbox");
    await search(user, "weather");
    const detail = await openDetails(user, "Weather (legacy)");
    // The publisher's words are quoted as theirs, not worded as Loadout's own warning.
    const quote = within(detail).getByText("Replaced by io.github.acme/weather.");
    expect(quote.tagName).toBe("Q");
    expect(quote.closest(".dialog-warning")).toBeNull();
    expect(quote.closest("p")).toHaveTextContent("The publisher says: Replaced by io.github.acme/weather.");
    expect(within(detail).getByRole("note")).toHaveTextContent("Deprecated: its publisher no longer recommends it.");
    expect(within(detail).getByText("ghcr.io/octo/weather", { exact: false })).toBeInTheDocument();
    expect(within(detail).getByText("Data from the Official MCP Registry")).toBeInTheDocument();
    await user.click(within(detail).getByRole("button", { name: "Back to results" }));
    expect(await screen.findByRole("button", { name: "Details of Weather (legacy)" })).toHaveFocus();
  });

  it("hands a registry server to the MCP review with its catalog provenance", async () => {
    const { user, transport } = renderBrowse({ online: true, decided: true });
    await screen.findByRole("searchbox");
    await search(user, "weather");
    const detail = await openDetails(user, "Weather");
    expect(within(detail).getByText("WEATHER_API_KEY")).toBeInTheDocument();
    await user.click(within(detail).getByRole("button", { name: "Review MCP server" }));
    const review = await screen.findByRole("region", { name: "Proposed servers" });
    expect(invoked(transport, "reviewCatalogMcp")).toEqual([["mcp-registry", "io.github.acme/weather", null]]);
    expect(within(review).getByText("catalog:mcp-registry/io.github.acme/weather")).toBeInTheDocument();
    expect(within(review).getByText("mcp-registry/io.github.acme/weather")).toBeInTheDocument();
    await user.click(within(review).getByRole("button", { name: "Save server" }));
    await waitFor(() =>
      expect(invoked(transport, "saveMcpSourceDrafts").map((args) => args[0])).toEqual([
        "catalog-mcp-mcp-registry-io.github.acme/weather",
      ]),
    );
  });

  it("asks which skill of a plugin to install and hands it to the skill review with provenance", async () => {
    const { user, transport } = renderBrowse({ online: true, decided: true });
    await screen.findByRole("searchbox");
    const detail = await openDetails(user, "document-skills");
    const reviewButton = within(detail).getByRole("button", { name: "Review skill" });
    expect(reviewButton).toBeDisabled();
    await user.click(within(detail).getByRole("radio", { name: "pptx" }));
    await user.click(within(detail).getByRole("button", { name: "Review skill pptx" }));
    const dialog = await screen.findByRole("dialog", { name: "Review pptx" });
    expect(invoked(transport, "reviewCatalogSkill")).toEqual([["anthropic-skills", "plugin/document-skills", "pptx"]]);
    // The backend records the picked skill's own entry, so the review names that one.
    expect(within(dialog).getByText("catalog:anthropic-skills/pptx")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Install" }));
    await waitFor(() =>
      expect(invoked(transport, "installReviewedSkill").map((args) => args[0])).toEqual([
        "catalog-skill-anthropic-skills-pptx",
      ]),
    );
  });

  it("shows why a refused plugin kind can't be installed and offers no install", async () => {
    const { user, transport } = renderBrowse({ online: true, decided: true, catalogs: [...MOCK_CATALOGS, ...DEMO_USER_CATALOGS] });
    await screen.findByRole("searchbox");
    const card = await within(section("Team skills")).findByRole("listitem", { name: "ops-tools" });
    expect(within(card).getByText("Can't be installed")).toBeInTheDocument();
    const detail = await openDetails(user, "ops-tools");
    expect(
      within(detail).getByText("Can't be installed: plugins from npm/archive/command sources are not supported"),
    ).toBeInTheDocument();
    expect(within(detail).queryByRole("button", { name: /^Review/ })).toBeNull();
    expect(within(detail).getByText(/Not imported: hooks, commands\./)).toBeInTheDocument();
    expect(transport.calls).not.toContain("reviewCatalogSkill");
    expect(transport.calls).not.toContain("reviewCatalogMcp");
  });

  it("shows a tap plugin's pin and a branch-following tap entry", async () => {
    renderBrowse({ online: true, decided: true, catalogs: [...MOCK_CATALOGS, ...DEMO_USER_CATALOGS] });
    await screen.findByRole("searchbox");
    const team = section("Team skills");
    expect(await within(team).findByText(/Pinned to a1b2c3d/)).toBeInTheDocument();
    expect(within(within(team).getByRole("listitem", { name: "repo-conventions" })).getByText(/Follows branch main/)).toBeInTheDocument();
  });

  it("needs online catalogs to install, says so, and focuses the install section once turned on", async () => {
    const { user, transport } = renderBrowse({
      online: false,
      decided: true,
      offSaved: ["anthropic-skills", "mcp-registry"],
    });
    await screen.findByRole("searchbox");
    await search(user, "weather");
    const detail = await openDetails(user, "Weather");
    expect(within(detail).getByText(/online catalogs need to be on/)).toBeInTheDocument();
    expect(within(detail).queryByRole("button", { name: "Review MCP server" })).toBeNull();
    expect(within(detail).queryByRole("button", { name: "Review skill" })).toBeNull();
    await user.click(within(detail).getByRole("button", { name: "Turn on online catalogs" }));
    expect(await within(detail).findByRole("button", { name: "Review MCP server" })).toBeInTheDocument();
    expect(within(detail).getByRole("heading", { name: "Install" })).toHaveFocus();
    expect(transport.calls).not.toContain("reviewCatalogMcp");
    expect(transport.calls).not.toContain("reviewCatalogSkill");
  });

  it("links out to skills.sh and reviews a pasted skills.sh address as a git skill", async () => {
    const { user, transport } = renderBrowse({ online: true, decided: true });
    const card = await screen.findByRole("region", { name: "skills.sh" });
    await user.click(within(card).getByRole("button", { name: "Open skills.sh" }));
    expect(invoked(transport, "openCatalogLink")).toEqual([["skills-sh", null, "catalog"]]);
    const field = within(card).getByRole("textbox", { name: "skills.sh skill address" });
    await user.type(field, "https://skills.sh/site/mintlify.com/docs");
    await user.click(within(card).getByRole("button", { name: "Review skill" }));
    expect(within(card).getByRole("alert")).toHaveTextContent(/comes from a website/);
    await user.clear(field);
    await user.type(field, "https://skills.sh/anthropics/skills/pdf");
    await user.click(within(card).getByRole("button", { name: "Review skill" }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    expect(invoked(transport, "reviewGitSkill")).toEqual([["https://skills.sh/anthropics/skills/pdf", null, null]]);
  });

  it("doesn't offer the skills.sh paste while online catalogs are off", async () => {
    const { user, transport } = renderBrowse({ online: false, decided: true });
    const card = await screen.findByRole("region", { name: "skills.sh" });
    expect(within(card).queryByRole("button", { name: "Review skill" })).toBeNull();
    expect(within(card).queryByRole("textbox")).toBeNull();
    expect(within(card).getByText(/online catalogs need to be on/)).toBeInTheDocument();
    // Opening the site is the browser's request, not Loadout's.
    await user.click(within(card).getByRole("button", { name: "Open skills.sh" }));
    expect(transport.calls).not.toContain("reviewGitSkill");
  });

  it("keeps searching from saved results when the network drops mid-session", async () => {
    const { user, transport } = renderBrowse({
      online: true,
      decided: true,
      catalogs: [...MOCK_CATALOGS, ...DEMO_USER_CATALOGS],
      behavior: { "team-skills": { kind: "stale", ageSecs: 26 * 3_600, message: "could not resolve host github.com" } },
    });
    await screen.findByRole("searchbox");
    onlineManager.setOnline(false);
    try {
      await search(user, "conv");
      expect(await within(section("Team skills")).findByText("Offline: 1 result saved 1 day ago.")).toBeInTheDocument();
      expect(invoked(transport, "searchCatalog").some((args) => args[0] === "team-skills" && args[1] === "conv")).toBe(
        true,
      );
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("answers the online question while the network is down", async () => {
    const { user, transport } = renderBrowse();
    onlineManager.setOnline(false);
    try {
      await user.click(await screen.findByRole("button", { name: "Turn on online catalogs" }));
      await waitFor(() => expect(invoked(transport, "setOnlineCatalogs")).toEqual([[true]]));
      expect(await screen.findByRole("searchbox", { name: "Search catalogs" })).toBeInTheDocument();
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("keeps Back off while a review loads and drops a review that finishes after leaving", async () => {
    const pending = deferred<CommandResult<typeof MOCK_MCP_REVIEW>>();
    const { user, transport, unmount } = renderBrowse(
      { online: true, decided: true },
      { reviewCatalogMcp: () => pending.promise },
    );
    await screen.findByRole("searchbox");
    await search(user, "weather");
    const detail = await openDetails(user, "Weather");
    await user.click(within(detail).getByRole("button", { name: "Review MCP server" }));
    expect(within(detail).getByRole("button", { name: "Back to results" })).toBeDisabled();
    unmount();
    pending.resolve(ok({ ...MOCK_MCP_REVIEW, token: "late-mcp" }));
    await waitFor(() => expect(invoked(transport, "discardMcpSourceReview")).toEqual([["late-mcp"]]));
  });

  it("drops a skills.sh review that finishes after the user opened an entry", async () => {
    const pending = deferred<CommandResult<typeof MOCK_SKILL_REVIEW>>();
    const { user, transport } = renderBrowse({ online: true, decided: true }, { reviewGitSkill: () => pending.promise });
    const card = await screen.findByRole("region", { name: "skills.sh" });
    await user.type(within(card).getByRole("textbox", { name: "skills.sh skill address" }), "https://skills.sh/anthropics/skills/pdf");
    await user.click(within(card).getByRole("button", { name: "Review skill" }));
    await openDetails(user, "pdf");
    pending.resolve(ok({ ...MOCK_SKILL_REVIEW, token: "late-skill" }));
    await waitFor(() => expect(invoked(transport, "discardSkillReview")).toEqual([["late-skill"]]));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows a failed More results page with Try again, then loads it", async () => {
    const [first, second] = MOCK_CATALOG_ENTRIES["anthropic-skills"];
    let laterCalls = 0;
    const { user } = renderBrowse(
      { online: true, decided: true },
      {
        searchCatalog: async (catalog, _query, cursor) => {
          if (catalog !== "anthropic-skills") {
            return ok(page(catalog, { status: "query_too_short" }));
          }
          if (cursor === null) {
            return ok(page(catalog, { entries: [first], nextCursor: "page-2" }));
          }
          laterCalls += 1;
          return laterCalls === 1
            ? ok(page(catalog, { status: "error", error: { code: "catalog_unreachable", message: "timed out after 20 s" } }))
            : ok(page(catalog, { entries: [second] }));
        },
      },
    );
    const anthropic = await screen.findByRole("region", { name: "Anthropic skills" });
    await user.click(await within(anthropic).findByRole("button", { name: "More results from Anthropic skills" }));
    expect(await within(anthropic).findByRole("alert")).toHaveTextContent(
      "Couldn't load more results. Timed out: Anthropic skills took too long to answer.",
    );
    await user.click(within(anthropic).getByRole("button", { name: "Load more results from Anthropic skills again" }));
    expect(await within(anthropic).findByRole("listitem", { name: second.name })).toBeInTheDocument();
    expect(within(anthropic).queryByRole("alert")).toBeNull();
  });

  it("searches a stale catalog again with Try again", async () => {
    const { user, transport } = renderBrowse({
      online: true,
      decided: true,
      catalogs: [...MOCK_CATALOGS, ...DEMO_USER_CATALOGS],
      behavior: { "team-skills": { kind: "stale", ageSecs: 26 * 3_600, message: "could not resolve host github.com" } },
    });
    await screen.findByRole("searchbox");
    await search(user, "conv");
    const team = section("Team skills");
    await user.click(await within(team).findByRole("button", { name: "Search Team skills again" }));
    await waitFor(() => expect(invoked(transport, "refreshCatalog")).toEqual([["team-skills"]]));
    await waitFor(() =>
      expect(invoked(transport, "searchCatalog").filter((args) => args[0] === "team-skills" && args[1] === "conv")).toHaveLength(2),
    );
    // The retry failed too: the saved results stay, labeled with their age.
    expect(await within(team).findByText(/^Stale: 1 result saved 1 day ago/)).toBeInTheDocument();
  });

  it("opens only https links", async () => {
    const weather = MOCK_CATALOG_ENTRIES["mcp-registry"][0];
    const { user } = renderBrowse(
      { online: true, decided: true },
      {
        showCatalogEntry: async () =>
          ok({ entry: { ...weather, website: "http://acme.example/weather" }, status: "fresh", ageSecs: 0, error: null }),
      },
    );
    await screen.findByRole("searchbox");
    await search(user, "weather");
    const detail = await openDetails(user, "Weather");
    expect(await within(detail).findByText("http://acme.example/weather")).toBeInTheDocument();
    expect(within(detail).queryByRole("button", { name: "Open the website of Weather in your browser" })).toBeNull();
    expect(within(detail).getByRole("button", { name: "Open the repository of Weather in your browser" })).toBeInTheDocument();
  });
});
