import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { App, createQueryClient, type FeatureFlags } from "@/app/App";
import { THEME_STORAGE_KEY } from "@/app/components/theme";
import { errorMessage, t } from "@/i18n/en";
import { APP_ID, PRODUCT_NAME, type Platform, type ProjectRow } from "@/ipc/bindings";
import { tauriTransport } from "@/ipc/transport";
import {
  createMockTransport,
  MOCK_DATA_HOME,
  MOCK_REMOTES,
  mockIpcError,
  ok,
  type MockTransportOptions,
} from "@/test/mockTransport";
import { USER_EVENT_OPTIONS } from "@/test/renderWithProviders";

// Paths arrive under their placeholder roots, as the backend sends them.
const TRUSTED_PROJECT: ProjectRow = {
  id: "app-a1b2",
  name: "app",
  path: "~/code/app",
  layout: "shared",
  trust: "trusted",
  error: null,
  servers: 1,
  clients: 2,
  pending: 0,
};

const UNTRUSTED_PROJECT: ProjectRow = {
  ...TRUSTED_PROJECT,
  id: "lab-c3d4",
  name: "home-lab",
  path: "%EXTERNAL%/home-lab",
  trust: "untrusted",
};

function renderApp(options: MockTransportOptions = {}, features?: FeatureFlags) {
  const transport = createMockTransport(options);
  const user = userEvent.setup(USER_EVENT_OPTIONS);
  const view = render(
    <App transport={transport} queryClient={createQueryClient()} features={features} />,
  );
  return { transport, user, ...view };
}

function withPlatform(platform: Platform): MockTransportOptions {
  return { appInfo: { version: "1.2.3", platform } };
}

function primaryNav() {
  return screen.getByRole("navigation", { name: t("app.nav.primary") });
}

function navLabels() {
  return within(primaryNav())
    .getAllByRole("button")
    .map((button) => button.textContent);
}

function header() {
  return screen.getByRole("banner");
}

describe("App shell", () => {
  it("shows the brand, the User scope and the v1 nav items in order", async () => {
    const { transport } = renderApp();

    expect(screen.getByText(PRODUCT_NAME, { selector: ".brand-title" })).toBeInTheDocument();
    expect(navLabels()).toEqual([
      "Skills",
      "MCP",
      "Projects",
      "Tools",
      "Install",
      "Import",
      "History",
      "Settings",
      "About",
    ]);
    expect(within(header()).getByRole("heading", { level: 1 })).toHaveTextContent("Skills");
    expect(within(header()).getByText(t("app.nav.skills.description"))).toBeInTheDocument();

    const scope = screen.getByRole("combobox", { name: t("scope.switcher.aria") });
    expect(scope).toHaveTextContent(t("scope.user"));
    await waitFor(() => expect(transport.calls).toContain("listProjects"));
  });

  it("hides Plugins and Publish unless their feature flags are on", () => {
    renderApp({}, { plugins: true, publish: true });

    expect(navLabels()).toEqual([
      "Skills",
      "MCP",
      "Plugins",
      "Projects",
      "Tools",
      "Install",
      "Import",
      "Publish",
      "History",
      "Settings",
      "About",
    ]);
  });

  it("offers User, a hint and Add folder when no project is known", async () => {
    const { user } = renderApp();
    const scope = screen.getByRole("combobox", { name: t("scope.switcher.aria") });

    await user.click(scope);
    const listbox = screen.getByRole("listbox");
    await waitFor(() => expect(within(listbox).getByText(t("scope.noProjects"))).toBeInTheDocument());
    const options = within(listbox).getAllByRole("option");
    expect(options.map((option) => option.textContent)).toEqual([
      `✓${t("scope.userOption")}`,
      t("scope.noProjects"),
      t("scope.addFolder"),
    ]);
    expect(options[1]).toBeDisabled();
  });

  it("lists every project with its trust and path, then Add folder, and switches to one", async () => {
    const { user } = renderApp({ projects: [TRUSTED_PROJECT, UNTRUSTED_PROJECT] });
    const scope = screen.getByRole("combobox", { name: t("scope.switcher.aria") });

    await user.click(scope);
    const listbox = screen.getByRole("listbox");
    await waitFor(() => expect(within(listbox).getAllByRole("option")).toHaveLength(4));
    const options = within(listbox).getAllByRole("option");
    expect(options.map((option) => option.textContent)).toEqual([
      `✓${t("scope.userOption")}`,
      "app",
      "home-lab",
      t("scope.addFolder"),
    ]);
    expect(within(options[1]).getByRole("img", { name: t("projects.trust.trusted") })).toBeInTheDocument();
    expect(within(options[2]).getByRole("img", { name: t("projects.trust.untrusted") })).toBeInTheDocument();
    expect(options[2]).toHaveAttribute(
      "data-tooltip",
      t("scope.projectTooltip", { path: "%EXTERNAL%/home-lab", trust: t("projects.trust.untrusted") }),
    );

    // An untrusted project can be selected.
    await user.click(options[2]);
    expect(scope).toHaveTextContent("home-lab");
    await user.click(scope);
    expect(within(screen.getByRole("listbox")).getByRole("option", { selected: true })).toHaveTextContent(
      "home-lab",
    );
  });

  it("shows the current scope in the collapsed sidebar's tooltip", async () => {
    const { user } = renderApp({ projects: [TRUSTED_PROJECT] });
    await user.click(screen.getByRole("button", { name: t("app.sidebar.collapse") }));
    const userLabel = t("scope.collapsed", { name: t("scope.userOption") });
    expect(screen.getByRole("button", { name: userLabel })).toHaveAttribute("data-tooltip", userLabel);

    // The icon reopens the sidebar, where the switcher is.
    await user.click(screen.getByRole("button", { name: userLabel }));
    await user.click(screen.getByRole("combobox", { name: t("scope.switcher.aria") }));
    await user.click(await screen.findByRole("option", { name: /app$/ }));
    await user.click(screen.getByRole("button", { name: t("app.sidebar.collapse") }));
    const projectLabel = t("scope.collapsed", { name: "app" });
    expect(screen.getByRole("button", { name: projectLabel })).toHaveAttribute("data-tooltip", projectLabel);
  });

  it("Add folder in the sidebar runs the folder flow and selects the new project", async () => {
    let known: ProjectRow[] = [];
    const fresh: ProjectRow = { ...TRUSTED_PROJECT, id: "fresh-e5f6", name: "fresh", path: "~/code/fresh" };
    const { user, transport } = renderApp({
      commands: {
        listProjects: async () => ok(known),
        pickProjectFolder: async () =>
          ok({ token: "folder-1", path: "~/code/fresh", name: "fresh", manifest: null }),
        initProject: async () => {
          known = [fresh];
          return ok({ id: fresh.id, trust: "trusted", commit: null, apply: null });
        },
      },
    });
    const scope = screen.getByRole("combobox", { name: t("scope.switcher.aria") });

    await user.click(scope);
    await user.click(within(screen.getByRole("listbox")).getByRole("option", { name: t("scope.addFolder") }));
    const dialog = await screen.findByRole("dialog", { name: t("projects.folder.titleAdd") });
    await user.click(within(dialog).getByRole("button", { name: t("projects.folder.create") }));

    await waitFor(() => expect(scope).toHaveTextContent("fresh"));
    expect(transport.calls).toContain("initProject");
  });

  it("switches pages and shows each page's empty state", async () => {
    const { user } = renderApp();

    await user.click(within(primaryNav()).getByRole("button", { name: "Projects" }));
    expect(within(header()).getByRole("heading", { level: 1 })).toHaveTextContent("Projects");
    // Pages after Skills load on their first visit (lazy routes).
    expect(await screen.findByText(t("page.empty.projects"))).toBeInTheDocument();
    expect(within(primaryNav()).getByRole("button", { name: "Projects" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    await user.click(within(primaryNav()).getByRole("button", { name: "History" }));
    expect(within(header()).getByRole("heading", { level: 1 })).toHaveTextContent("History");
    expect(await screen.findByText(t("page.empty.history"))).toBeInTheDocument();
  });

  it("first run: Not now leaves no remote and Settings shows No backup remote", async () => {
    let answered = false;
    const { user, transport } = renderApp({
      commands: {
        remotesOverview: async () => ok({ ...MOCK_REMOTES, offerBackupRemote: !answered }),
        dismissBackupRemoteOffer: async () => {
          answered = true;
          return ok(null);
        },
      },
    });

    const step = await screen.findByRole("dialog", { name: "Add a backup remote" });
    expect(within(step).getByRole("button", { name: "Git URL" })).toBeInTheDocument();
    expect(within(step).getByRole("button", { name: "Folder" })).toBeInTheDocument();
    await user.click(within(step).getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(transport.calls).toContain("dismissBackupRemoteOffer");
    expect(transport.calls).not.toContain("addFolderRemote");
    expect(transport.calls).not.toContain("addGitRemote");

    await user.click(within(primaryNav()).getByRole("button", { name: "Settings" }));
    expect(await screen.findByText("No backup remote")).toBeInTheDocument();
  });

  it("renders page toolbars in the header slot", async () => {
    const { user } = renderApp();

    await user.click(within(primaryNav()).getByRole("button", { name: "MCP" }));
    const install = await within(header()).findByRole("button", { name: t("toolbar.goInstall") });

    await user.click(install);
    expect(within(header()).getByRole("heading", { level: 1 })).toHaveTextContent("Install");
    expect(within(header()).queryByRole("button", { name: t("toolbar.goInstall") })).toBeNull();
  });

  it("collapses the sidebar and moves nav labels into tooltips", async () => {
    const { user, container } = renderApp();
    const shell = container.querySelector(".app-shell");

    await user.click(screen.getByRole("button", { name: t("app.sidebar.collapse") }));
    expect(shell).toHaveClass("is-sidebar-collapsed");
    expect(within(primaryNav()).getByRole("button", { name: "Tools" })).toHaveAttribute(
      "data-tooltip",
      "Tools",
    );

    await user.click(screen.getByRole("button", { name: t("app.sidebar.expand") }));
    expect(shell).not.toHaveClass("is-sidebar-collapsed");
  });
});

describe("window chrome", () => {
  it("draws custom window controls on Windows", async () => {
    const { user, transport, container } = renderApp(withPlatform("windows"));

    const controls = await screen.findByLabelText(t("app.window.controls"));
    expect(container.querySelector(".app-shell")).toHaveClass("is-windows-window");

    await user.click(within(controls).getByRole("button", { name: t("app.window.minimize") }));
    await user.click(within(controls).getByRole("button", { name: t("app.window.maximize") }));
    await within(controls).findByRole("button", { name: t("app.window.restore") });
    await user.click(within(controls).getByRole("button", { name: t("app.window.close") }));
    expect(transport.windowCalls).toEqual(["minimize", "toggleMaximize", "close"]);
  });

  it("follows the window's maximized state after a resize", async () => {
    const { transport } = renderApp(withPlatform("windows"));
    const controls = await screen.findByLabelText(t("app.window.controls"));
    await within(controls).findByRole("button", { name: t("app.window.maximize") });

    await transport.window.toggleMaximize();
    transport.resize();
    await within(controls).findByRole("button", { name: t("app.window.restore") });
  });

  it("leaves Linux windows to native decorations", async () => {
    const { container, transport } = renderApp(withPlatform("linux"));
    await waitFor(() => expect(transport.calls).toContain("appInfo"));

    expect(screen.queryByLabelText(t("app.window.controls"))).toBeNull();
    expect(container.querySelector(".brand-block .sidebar-toggle")).not.toBeNull();
    expect(container.querySelector(".app-shell")).not.toHaveClass("is-windows-window");
  });

  it("uses the overlay title bar layout on macOS", async () => {
    const { container } = renderApp(withPlatform("macos"));

    await waitFor(() =>
      expect(container.querySelector(".app-shell")).toHaveClass("is-macos-window"),
    );
    expect(screen.queryByLabelText(t("app.window.controls"))).toBeNull();
    expect(container.querySelector(".sidebar-toggle--edge")).not.toBeNull();
  });
});

describe("settings and about", () => {
  it("switches between light, dark and system themes", async () => {
    const { user } = renderApp();
    await user.click(within(primaryNav()).getByRole("button", { name: "Settings" }));
    const picker = screen.getByRole("group", { name: t("settings.theme.label") });

    await user.click(within(picker).getByRole("button", { name: t("settings.theme.option.dark") }));
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");

    await user.click(within(picker).getByRole("button", { name: t("settings.theme.option.light") }));
    expect(document.documentElement).toHaveAttribute("data-theme", "light");

    await user.click(
      within(picker).getByRole("button", { name: t("settings.theme.option.system") }),
    );
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBeNull();
    expect(
      within(picker).getByRole("button", { name: t("settings.theme.option.system") }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("shows the version, identifier and data directory from the backend", async () => {
    const { user } = renderApp(withPlatform("windows"));
    await user.click(within(primaryNav()).getByRole("button", { name: "About" }));

    expect(await screen.findByText(`${PRODUCT_NAME} 1.2.3 on Windows.`)).toBeInTheDocument();
    expect(screen.getByText(APP_ID)).toBeInTheDocument();
    expect(
      await screen.findByText(`${MOCK_DATA_HOME.path} (set by environment variable)`),
    ).toBeInTheDocument();
  });

  it("localizes a command's error code and shows it as a toast", async () => {
    const { user } = renderApp({
      dataHome: mockIpcError({
        code: "profile_not_found",
        message: "cannot determine the user profile directory",
      }),
    });
    await user.click(within(primaryNav()).getByRole("button", { name: "About" }));

    const toast = await screen.findByRole("alert");
    expect(toast).toHaveTextContent(errorMessage("profile_not_found"));
    expect(screen.getAllByText(errorMessage("profile_not_found"))).toHaveLength(2);
    expect(screen.queryByText(/cannot determine/)).toBeNull();
  });

  it("does not repeat an error the page shows inline as a toast", async () => {
    const { user } = renderApp({
      commands: { planSkilldockMigration: async () => mockIpcError({ code: "not_found", message: "none" }) },
    });
    await user.click(within(primaryNav()).getByRole("button", { name: "Settings" }));
    // Settings points at the Import page, where the migration opens now.
    await user.click(await screen.findByRole("button", { name: t("migrate.section.openImport") }));
    expect(within(primaryNav()).getByRole("button", { name: "Import" })).toHaveAttribute("aria-current", "page");
    await user.click(await screen.findByRole("button", { name: t("migrate.section.open") }));

    expect(await screen.findByText(t("migrate.notFound"))).toBeInTheDocument();
    expect(screen.queryByText(errorMessage("not_found"))).toBeNull();
  });
});

describe("transport", () => {
  it("has no silent fallback outside Tauri", async () => {
    await expect(tauriTransport.commands.appInfo()).rejects.toBeDefined();
  });
});
