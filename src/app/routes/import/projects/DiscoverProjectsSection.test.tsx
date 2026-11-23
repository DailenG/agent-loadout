import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { t } from "@/i18n/en";
import type { DiscoveredProjectsAdded, ProjectDiscovery, ToolInfo } from "@/ipc/bindings";
import { MOCK_TOOLS, ok } from "@/test/mockTransport";
import { renderWithProviders, type RenderOptions } from "@/test/renderWithProviders";
import { DiscoverProjectsSection } from "./DiscoverProjectsSection";

// Paths arrive under their placeholder roots, as the backend sends them.
const DISCOVERY: ProjectDiscovery = {
  candidates: [
    {
      id: "d-0a1b2c3d4e5f",
      name: "web-shop",
      path: "~/Code/web-shop",
      clients: ["claude-code", "omp"],
      manifest: { id: "web-shop-3f9a", name: "Web shop", layout: "shared", known: false },
      error: null,
    },
    {
      id: "d-111111111111",
      name: "scratch",
      path: "~/Code/scratch",
      clients: ["codex"],
      manifest: null,
      error: null,
    },
    {
      id: "d-222222222222",
      name: "broken",
      path: "%EXTERNAL%/broken",
      clients: ["claude-code"],
      manifest: null,
      error: { code: "parse_error", message: "the manifest is not valid TOML" },
    },
  ],
  warnings: [{ code: "parse_error", message: "~/.codex/config.toml is not valid TOML" }],
};

const ADDED: DiscoveredProjectsAdded = {
  commit: "0000000000000000000000000000000000000003",
  added: [
    { candidate: "d-0a1b2c3d4e5f", id: "web-shop-3f9a", path: "~/Code/web-shop", createdManifest: false, trust: "untrusted" },
  ],
  failed: [
    {
      candidate: "d-111111111111",
      path: "~/Code/scratch",
      error: { code: "already_exists", message: "project scratch-77aa is known at another folder" },
    },
  ],
};

const TOOLS: ToolInfo[] = [
  ...MOCK_TOOLS,
  { ...MOCK_TOOLS[0], id: "omp", name: "omp" },
  { ...MOCK_TOOLS[0], id: "codex", name: "Codex" },
];

function renderSection(options: RenderOptions = {}) {
  return renderWithProviders(<DiscoverProjectsSection />, {
    ...options,
    commands: {
      discoverProjects: async () => ok(DISCOVERY),
      addDiscoveredProjects: async () => ok(ADDED),
      listTools: async () => ok(TOOLS),
      ...options.commands,
    },
  });
}

describe("DiscoverProjectsSection", () => {
  it("lists folders with the clients that know them and their manifest", async () => {
    renderSection();
    const list = await screen.findByRole("list", { name: t("discover.listAria") });
    const webShop = within(list).getByRole("checkbox", { name: /Web shop/ });
    expect(webShop).not.toBeChecked();
    expect(webShop).toHaveAccessibleDescription(
      new RegExp(`${t("discover.knownBy", { clients: "omp, Claude Code" })}.*${t("projects.layout.shared")}`),
    );
    expect(within(list).getByRole("checkbox", { name: /scratch/ })).toHaveAccessibleDescription(
      new RegExp(t("discover.noManifest")),
    );
    // A folder that cannot be added says why and can't be picked.
    const broken = within(list).getByRole("checkbox", { name: /broken/ });
    expect(broken).toBeDisabled();
    expect(broken).toHaveAccessibleDescription(/The manifest is not valid TOML\./);
    // An unreadable client file is a note; the other clients still were read.
    expect(screen.getByText("~/.codex/config.toml is not valid TOML.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("discover.add", { count: 0 }) })).toBeDisabled();
  });

  it("adds the selected folders untrusted and offers each review", async () => {
    const { user, transport } = renderSection({
      commands: {
        reviewProject: async () =>
          ok({ id: "web-shop-3f9a", name: "Web shop", path: "~/Code/web-shop", trust: "untrusted", hash: "sha256-x", files: [], warnings: [] }),
      },
    });
    await user.click(await screen.findByRole("button", { name: t("discover.selectAll") }));
    // Folders without a manifest get a private one.
    expect(screen.getByText(t("discover.privateNote"))).toBeInTheDocument();
    const section = screen.getByRole("region", { name: t("discover.title") });
    // The summary's live region is there, empty, before the add, so the result is announced.
    const status = within(section).getByRole("status");
    expect(status).toBeEmptyDOMElement();
    await user.click(screen.getByRole("button", { name: t("discover.add", { count: 2 }) }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "addDiscoveredProjects")?.args).toEqual([
        ["d-0a1b2c3d4e5f", "d-111111111111"],
      ]),
    );

    await waitFor(() => expect(status).toHaveTextContent(t("discover.result.summary", { added: 1, failed: 1 })));
    expect(within(section).getByRole("status")).toBe(status);
    expect(within(section).getByRole("heading", { name: t("discover.result.title") })).toHaveFocus();
    const result = section;
    const added = within(result).getByRole("region", { name: t("discover.result.added") });
    expect(added).toHaveTextContent(t("projects.trust.untrusted"));
    const failed = within(result).getByRole("region", { name: t("discover.result.failed") });
    expect(failed).toHaveTextContent("scratch");
    expect(failed).toHaveTextContent("Project scratch-77aa is known at another folder.");

    await user.click(within(added).getByRole("button", { name: t("discover.result.reviewAria", { name: "Web shop" }) }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "reviewProject")?.args).toEqual(["web-shop-3f9a"]),
    );
  });
});
