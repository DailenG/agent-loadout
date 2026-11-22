import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ProjectsPage } from "@/app/routes/projects/ProjectsPage";
import { t } from "@/i18n/en";
import type { PickedFolder, ProjectRow, TrustReview } from "@/ipc/bindings";
import { MOCK_CHANGE, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

// Paths arrive under their placeholder roots, as the backend sends them.
const TRUSTED: ProjectRow = {
  id: "app-a1b2",
  name: "app",
  path: "~/code/app",
  layout: "shared",
  trust: "trusted",
  error: null,
  servers: 3,
  clients: 5,
  pending: 1,
};

const CHANGED: ProjectRow = {
  ...TRUSTED,
  id: "lab-c3d4",
  name: "home-lab",
  path: "%EXTERNAL%/home-lab",
  layout: "private",
  trust: "changed",
  pending: 0,
};

const EMPTY_FOLDER: PickedFolder = {
  token: "folder-1",
  path: "~/code/fresh",
  name: "fresh",
  manifest: null,
};

const BOUND_FOLDER: PickedFolder = {
  token: "folder-2",
  path: "~/code/lab",
  name: "lab",
  manifest: { id: "lab-c3d4", name: "home-lab", layout: "shared", known: false },
};

const REVIEW: TrustReview = {
  id: "lab-c3d4",
  name: "home-lab",
  path: "%EXTERNAL%/home-lab",
  trust: "changed",
  hash: "sha256-reviewed",
  files: [
    {
      file: "project.toml",
      path: "<P>/.loadout/project.toml",
      change: "changed",
      content: null,
      diff: [
        "--- trusted project.toml",
        "+++ <P>/.loadout/project.toml",
        "@@ -4,1 +4,1 @@",
        '-command = "docs-mcp"',
        '+command = "curl-pipe-sh"',
      ].join("\n"),
    },
  ],
  warnings: [],
};

function renderPage(options: MockTransportOptions = {}) {
  return renderWithProviders(<ProjectsPage />, {
    commands: {
      listProjects: async () => ok([TRUSTED, CHANGED]),
      reviewProject: async () => ok(REVIEW),
      ...options.commands,
    },
  });
}

describe("ProjectsPage", () => {
  it("lists projects with layout, counts and trust", async () => {
    renderPage();
    const card = await screen.findByRole("article", { name: "app" });
    expect(within(card).getByText("~/code/app")).toBeInTheDocument();
    expect(within(card).getByText(t("projects.layout.shared"))).toBeInTheDocument();
    expect(
      within(card).getByText(t("projects.counts", { servers: 3, clients: 5, pending: 1 })),
    ).toBeInTheDocument();
    expect(within(card).getByText(t("projects.trust.trusted"))).toBeInTheDocument();
    expect(within(card).queryByRole("button", { name: t("projects.action.review") })).toBeNull();

    const changed = screen.getByRole("article", { name: "home-lab" });
    expect(within(changed).getByText(t("projects.trust.changed"))).toBeInTheDocument();
    expect(within(changed).getByRole("button", { name: t("projects.action.review") })).toBeEnabled();
    // An untrusted project does not apply.
    expect(within(changed).getByRole("button", { name: t("projects.action.apply") })).toBeDisabled();
  });

  it("offers Init for a folder without a manifest, Private preselected", async () => {
    const { user, transport } = renderPage({
      commands: { pickProjectFolder: async () => ok(EMPTY_FOLDER) },
    });
    await user.click(await screen.findByRole("button", { name: t("projects.action.addFolder") }));
    const dialog = await screen.findByRole("dialog", { name: t("projects.folder.titleAdd") });
    expect(within(dialog).getByText("~/code/fresh")).toBeInTheDocument();
    const privateChoice = within(dialog).getByRole("radio", { name: new RegExp(t("projects.folder.private")) });
    const sharedChoice = within(dialog).getByRole("radio", { name: new RegExp(`^${t("projects.folder.shared")}`) });
    expect(privateChoice).toBeChecked();
    expect(sharedChoice).not.toBeChecked();
    expect(within(dialog).getByRole("textbox")).toHaveValue("fresh");

    await user.click(within(dialog).getByRole("button", { name: t("projects.folder.create") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "initProject")?.args).toEqual([
        "folder-1",
        false,
        "fresh",
      ]),
    );
  });

  it("creates a shared manifest on request", async () => {
    const { user, transport } = renderPage({
      commands: { pickProjectFolder: async () => ok(EMPTY_FOLDER) },
    });
    await user.click(await screen.findByRole("button", { name: t("projects.action.initManifest") }));
    const dialog = await screen.findByRole("dialog", { name: t("projects.folder.titleInit") });
    await user.click(within(dialog).getByRole("radio", { name: new RegExp(`^${t("projects.folder.shared")}`) }));
    const name = within(dialog).getByRole("textbox");
    await user.clear(name);
    await user.type(name, "Fresh app");
    await user.click(within(dialog).getByRole("button", { name: t("projects.folder.create") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "initProject")?.args).toEqual([
        "folder-1",
        true,
        "Fresh app",
      ]),
    );
  });

  it("binds a folder with a manifest, then opens its trust review", async () => {
    const { user, transport } = renderPage({
      commands: {
        pickProjectFolder: async () => ok(BOUND_FOLDER),
        addProject: async () => ok({ id: "lab-c3d4", trust: "untrusted", commit: MOCK_CHANGE.commit, apply: null }),
      },
    });
    await user.click(await screen.findByRole("button", { name: t("projects.action.addFolder") }));
    const dialog = await screen.findByRole("dialog", { name: t("projects.folder.titleAdd") });
    expect(
      within(dialog).getByText(t("projects.folder.found", { name: "home-lab", id: "lab-c3d4" })),
    ).toBeInTheDocument();
    expect(within(dialog).queryByRole("radio")).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: t("projects.folder.add") }));

    await screen.findByRole("dialog", { name: t("projects.review.title", { name: "home-lab" }) });
    expect(transport.invocations.find((call) => call.command === "addProject")?.args).toEqual(["folder-2"]);
    expect(transport.calls).toContain("reviewProject");
    expect(transport.calls).not.toContain("trustProject");
  });

  it("shows the manifest diff before trusting, and trusts only the reviewed content", async () => {
    const { user, transport } = renderPage();
    const card = await screen.findByRole("article", { name: "home-lab" });
    await user.click(within(card).getByRole("button", { name: t("projects.action.review") }));
    const dialog = await screen.findByRole("dialog", { name: t("projects.review.title", { name: "home-lab" }) });
    expect(within(dialog).getByRole("note")).toHaveTextContent(t("projects.review.intro"));
    const diff = await within(dialog).findByLabelText(
      t("projects.review.diff", { file: "<P>/.loadout/project.toml" }),
    );
    expect(diff).toHaveTextContent('+command = "curl-pipe-sh"');
    expect(diff).toHaveTextContent('-command = "docs-mcp"');
    expect(within(dialog).getByText(t("projects.review.change.changed"))).toBeInTheDocument();
    expect(transport.calls).not.toContain("trustProject");

    await user.click(within(dialog).getByRole("button", { name: t("projects.review.trust") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "trustProject")?.args).toEqual([
        "lab-c3d4",
        "sha256-reviewed",
      ]),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("applies and removes a project", async () => {
    const { user, transport } = renderPage();
    const card = await screen.findByRole("article", { name: "app" });
    await user.click(within(card).getByRole("button", { name: t("projects.action.apply") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "applyProjectFiles")?.args).toEqual([
        "app-a1b2",
      ]),
    );

    await user.click(within(card).getByRole("button", { name: t("projects.action.remove") }));
    const confirm = await screen.findByRole("alertdialog");
    expect(confirm).toHaveTextContent(t("projects.remove.confirm", { name: "app" }));
    await user.click(within(confirm).getByRole("button", { name: t("projects.remove.confirmAction") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "removeProject")?.args).toEqual([
        "app-a1b2",
      ]),
    );
  });

  it("does nothing when the folder picker is cancelled", async () => {
    const { user, transport } = renderPage();
    await user.click(await screen.findByRole("button", { name: t("projects.action.addFolder") }));
    await waitFor(() => expect(transport.calls).toContain("pickProjectFolder"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
