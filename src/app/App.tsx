// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Shell layout ported from DailenG/skilldock@181b52b src/app/App.tsx: grid, sidebar, brand
// block, nav and its icons, header, window chrome and the route error boundary. The data
// layer is new: typed IPC through a `Transport` and TanStack Query (SPEC 4.3, 13.1).

import {
  Component,
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ErrorInfo,
  type ReactNode,
} from "react";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { ApplyDrawer } from "@/app/apply/ApplyDrawer";
import { importQueryKeys, NOTICE_STALE_MS, storeQueryKeys } from "@/app/apply/changeNotice";
import { useDriftTriggers } from "@/app/apply/driftTriggers";
import { PendingChangesPill } from "@/app/apply/PendingChangesPill";
import { AppTooltip } from "@/app/components/AppTooltip";
import { EmptyPage } from "@/app/components/EmptyPage";
import { HeaderToolbarProvider } from "@/app/components/HeaderSlots";
import { NotificationProvider, useNotifications } from "@/app/components/notifications";
import { SettingsGroup, SettingsItem, ThemePicker } from "@/app/components/SettingsGroup";
import { useThemePreference, type ThemePreference } from "@/app/components/theme";
import type { ImportTarget } from "@/app/routes/import/model";
import { UnmanagedNotice, UnmanagedNoticeSetting } from "@/app/routes/import/UnmanagedNotice";
import { MigrateSection } from "@/app/routes/migrate/MigrateSection";
import { AgentSection, useAgentSkillStartup } from "@/app/routes/settings/agent/AgentSection";
import { SecretsSection } from "@/app/routes/settings/secrets/SecretsSection";
import { SourcesSection } from "@/app/routes/settings/sources/SourcesSection";
import { CatalogsSection } from "@/app/routes/settings/catalogs/CatalogsSection";
import { SkillsPage } from "@/app/routes/skills/SkillsPage";
import { FirstRunBackupRemote } from "@/app/routes/settings/versioning/BackupRemoteStep";
import { VersioningSection } from "@/app/routes/settings/versioning/VersioningSection";
import { UpdatesSection, useStartupUpdateCheck } from "@/app/routes/settings/updates/UpdatesSection";
import { ActiveScopeProvider } from "@/app/scope/ActiveScope";
import { ScopeSwitcher } from "@/app/scope/ScopeSwitcher";
import { dataHomeSourceLabels, errorMessage, platformLabels, t, type MessageKey } from "@/i18n/en";
import { APP_ID, PRODUCT_NAME, REPO } from "@/ipc/bindings";
import {
  IpcFailure,
  TransportContext,
  unwrap,
  useTransport,
  type Transport,
} from "@/ipc/transport";

// Skills is the start page and loads with the shell; the other pages load on
// their first visit, so startup parses less.
const HistoryPage = lazy(() =>
  import("@/app/routes/history/HistoryPage").then((module) => ({ default: module.HistoryPage })),
);
const InstallPage = lazy(() =>
  import("@/app/routes/install/InstallPage").then((module) => ({ default: module.InstallPage })),
);
const ImportPage = lazy(() =>
  import("@/app/routes/import/ImportPage").then((module) => ({ default: module.ImportPage })),
);
const McpPage = lazy(() => import("@/app/routes/mcp/McpPage").then((module) => ({ default: module.McpPage })));
const ProjectsPage = lazy(() =>
  import("@/app/routes/projects/ProjectsPage").then((module) => ({ default: module.ProjectsPage })),
);
const ToolsPage = lazy(() => import("@/app/routes/tools/ToolsPage").then((module) => ({ default: module.ToolsPage })));

/** Shown only when a page takes longer than this to load, so a fast load never flashes it. */
const ROUTE_LOADING_DELAY_MS = 300;

function RouteLoading() {
  const [isShown, setIsShown] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setIsShown(true), ROUTE_LOADING_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, []);
  return (
    <div aria-busy="true">
      {isShown ? <p className="mcp-page-loading">{t("app.route.loading")}</p> : null}
    </div>
  );
}

export type RouteKey =
  | "skills"
  | "mcp"
  | "plugins"
  | "projects"
  | "tools"
  | "install"
  | "import"
  | "publish"
  | "history"
  | "settings"
  | "about";

/** Nav items behind a feature flag (SPEC 13.1: hidden in v1). */
export type FeatureFlags = {
  plugins: boolean;
  publish: boolean;
};

export const DEFAULT_FEATURES: FeatureFlags = { plugins: false, publish: false };

type RouteDefinition = {
  key: RouteKey;
  labelKey: MessageKey;
  descriptionKey: MessageKey;
  /** Shown by the placeholder page until the page's milestone. */
  emptyKey: MessageKey;
  /** Rendered as a sub-item of the previous route (MCP under Skills). */
  isSubItem?: boolean;
  flag?: keyof FeatureFlags;
};

const routes: RouteDefinition[] = [
  {
    key: "skills",
    labelKey: "app.nav.skills.label",
    descriptionKey: "app.nav.skills.description",
    emptyKey: "page.empty.skills",
  },
  {
    key: "mcp",
    labelKey: "app.nav.mcp.label",
    descriptionKey: "app.nav.mcp.description",
    emptyKey: "page.empty.mcp",
    isSubItem: true,
  },
  {
    key: "plugins",
    labelKey: "app.nav.plugins.label",
    descriptionKey: "app.nav.plugins.description",
    emptyKey: "page.empty.plugins",
    flag: "plugins",
  },
  {
    key: "projects",
    labelKey: "app.nav.projects.label",
    descriptionKey: "app.nav.projects.description",
    emptyKey: "page.empty.projects",
  },
  {
    key: "tools",
    labelKey: "app.nav.tools.label",
    descriptionKey: "app.nav.tools.description",
    emptyKey: "page.empty.tools",
  },
  {
    key: "install",
    labelKey: "app.nav.install.label",
    descriptionKey: "app.nav.install.description",
    emptyKey: "page.empty.install",
  },
  {
    key: "import",
    labelKey: "app.nav.import.label",
    descriptionKey: "app.nav.import.description",
    emptyKey: "page.empty.title",
  },
  {
    key: "publish",
    labelKey: "app.nav.publish.label",
    descriptionKey: "app.nav.publish.description",
    emptyKey: "page.empty.publish",
    flag: "publish",
  },
  {
    key: "history",
    labelKey: "app.nav.history.label",
    descriptionKey: "app.nav.history.description",
    emptyKey: "page.empty.history",
  },
  {
    key: "settings",
    labelKey: "app.nav.settings.label",
    descriptionKey: "app.nav.settings.description",
    emptyKey: "page.empty.title",
  },
  {
    key: "about",
    labelKey: "app.nav.about.label",
    descriptionKey: "app.nav.about.description",
    emptyKey: "page.empty.title",
  },
];

/** Query keys of the commands the shell reads. */
export const queryKeys = {
  appInfo: ["app_info"],
  dataHome: ["data_home"],
} as const;

export function createQueryClient() {
  const client = new QueryClient({
    defaultOptions: {
      // IPC is local: a retry only delays the error the user should see.
      // Window focus rechecks only the views drift shows up in
      // (`useDriftTriggers`), not every query. `networkMode: "always"`: every
      // command is local IPC, and the backend decides what needs the network
      // (online catalogs answer from the cache), so the webview going offline
      // must never pause a query or mutation before it reaches the backend.
      queries: { retry: false, refetchOnWindowFocus: false, networkMode: "always" },
      mutations: { networkMode: "always" },
    },
  });
  // The plan and the server pills cost a whole plan run each. The watcher,
  // window focus, Refresh and every store change invalidate them explicitly,
  // so a mount (opening the apply drawer, revisiting MCP) reads the cache.
  for (const queryKey of [storeQueryKeys.pendingPlan, storeQueryKeys.servers, importQueryKeys.unmanaged]) {
    client.setQueryDefaults(queryKey, { staleTime: Infinity });
  }
  // The notice's full scans: focus and watcher refetch them at most once a
  // minute (`throttledDriftQueryKeys`); imports and Refresh always do.
  for (const queryKey of [importQueryKeys.unmanagedAll, importQueryKeys.unmanagedSkills]) {
    client.setQueryDefaults(queryKey, { staleTime: NOTICE_STALE_MS });
  }
  return client;
}

function useAppInfo() {
  const { commands } = useTransport();
  return useQuery({
    queryKey: queryKeys.appInfo,
    queryFn: () => commands.appInfo(),
    staleTime: Infinity,
  });
}

function useDataHome() {
  const { commands } = useTransport();
  return useQuery({
    queryKey: queryKeys.dataHome,
    queryFn: async () => unwrap(await commands.dataHome()),
  });
}

type RouteErrorBoundaryProps = {
  children: ReactNode;
  route: RouteKey;
};

type RouteErrorBoundaryState = {
  error: Error | null;
};

class RouteErrorBoundary extends Component<RouteErrorBoundaryProps, RouteErrorBoundaryState> {
  state: RouteErrorBoundaryState = {
    error: null,
  };

  static getDerivedStateFromError(error: Error): RouteErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error("Route render failed", {
      route: this.props.route,
      error,
      errorInfo,
    });
  }

  componentDidUpdate(prevProps: RouteErrorBoundaryProps) {
    if (prevProps.route !== this.props.route && this.state.error) {
      this.setState({ error: null });
    }
  }

  render() {
    if (this.state.error) {
      return (
        <div className="panel-card empty-state">
          <h3>{t("app.error.loadFailed")}</h3>
          <p>
            {this.state.error instanceof IpcFailure
              ? errorMessage(this.state.error.code)
              : t("app.error.unknown")}
          </p>
          <button
            className="secondary-button"
            type="button"
            onClick={() => this.setState({ error: null })}
          >
            {t("app.action.retry")}
          </button>
        </div>
      );
    }

    return this.props.children;
  }
}

function NavRouteIcon(props: { route: RouteKey }) {
  const { route } = props;
  if (route === "mcp") {
    return <McpNavIcon />;
  }

  if (route === "tools") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M14.8 4.6a5.2 5.2 0 0 0-6.2 6.7L4.7 15.2a1.4 1.4 0 0 0 0 2l2.1 2.1a1.4 1.4 0 0 0 2 0l3.9-3.9a5.2 5.2 0 0 0 6.7-6.2l-3 3h-2l-1.4-1.4v-2l3-3Z"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  if (route === "install") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M12 4v10m0 0 4-4m-4 4-4-4M5 19h14"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  if (route === "import") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M4.5 13.5v3.8a1.7 1.7 0 0 0 1.7 1.7h11.6a1.7 1.7 0 0 0 1.7-1.7v-3.8M12 4.5v9m0 0-3.5-3.5M12 13.5l3.5-3.5"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  if (route === "publish") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M12 20V10m0 0-4 4m4-4 4 4M5 5h14"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  if (route === "plugins") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M9 3v5m6-5v5M6.5 8.5h11v4.3a5.5 5.5 0 0 1-11 0V8.5ZM12 18.3V21"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  if (route === "projects") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M3.8 7.2a1.7 1.7 0 0 1 1.7-1.7h4l2 2.2h7a1.7 1.7 0 0 1 1.7 1.7v8.4a1.7 1.7 0 0 1-1.7 1.7H5.5a1.7 1.7 0 0 1-1.7-1.7V7.2Z"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  if (route === "history") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3M4.5 4.5v3.6h3.6"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        <path
          d="M12 8.2V12l2.7 1.8"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  if (route === "settings") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M10.55 3.75h2.9l.5 2.04c.45.1.88.28 1.29.52l1.86-.95l2.05 2.05l-.95 1.86c.24.41.42.84.52 1.29l2.04.5v2.9l-2.04.5c-.1.45-.28.88-.52 1.29l.95 1.86l-2.05 2.05l-1.86-.95c-.41.24-.84.42-1.29.52l-.5 2.04h-2.9l-.5-2.04a5.38 5.38 0 0 1-1.29-.52l-1.86.95l-2.05-2.05l.95-1.86a5.38 5.38 0 0 1-.52-1.29l-2.04-.5v-2.9l2.04-.5c.1-.45.28-.88.52-1.29l-.95-1.86l2.05-2.05l1.86.95c.41-.24.84-.42 1.29-.52l.5-2.04ZM12 9.35a2.65 2.65 0 1 0 0 5.3a2.65 2.65 0 0 0 0-5.3Z"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.7"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  if (route === "about") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M12 21a9 9 0 1 0 0-18a9 9 0 0 0 0 18Z"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        <path d="M12 10.5v5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
        <path d="M12 7.6h.01" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" />
      </svg>
    );
  }

  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M12 4.5 14 10l5.5 2-5.5 2L12 19.5l-2-5.5L4.5 12 10 10 12 4.5Z" fill="currentColor" />
    </svg>
  );
}

function McpNavIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path
        d="M2.4 11.3 11.45 2.26a3.2 3.2 0 0 1 4.53 4.53l-6.84 6.83"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
      <path
        d="m9.24 13.53 6.74-6.74a3.2 3.2 0 0 1 4.52 0l.05.05a3.2 3.2 0 0 1 0 4.52l-8.19 8.19a1.07 1.07 0 0 0 0 1.51l1.68 1.68"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
      <path
        d="m13.71 4.53-6.69 6.69a3.2 3.2 0 0 0 4.53 4.53l6.69-6.7"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
    </svg>
  );
}

/** The app icon (new artwork, not SkillDock's; SPEC 18). */
function BrandIcon() {
  return (
    <svg viewBox="0 0 228 228" role="img">
      <defs>
        <linearGradient
          id="brand-gradient"
          x1="30"
          y1="18"
          x2="200"
          y2="212"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0" stopColor="#1f1548" />
          <stop offset="0.52" stopColor="#5a3fd0" />
          <stop offset="1" stopColor="#f28b3c" />
        </linearGradient>
      </defs>
      <rect x="14" y="14" width="200" height="200" rx="54" fill="url(#brand-gradient)" stroke="none" />
      <rect x="56" y="56" width="116" height="30" rx="15" fill="#ffffff" stroke="none" />
      <rect x="56" y="99" width="116" height="30" rx="15" fill="#ebe5ff" stroke="none" />
      <rect x="56" y="142" width="72" height="30" rx="15" fill="#d5c9ff" stroke="none" />
      <circle cx="157" cy="157" r="17" fill="#ffd8b0" stroke="none" />
    </svg>
  );
}

function SidebarToggleButton(props: {
  isSidebarCollapsed: boolean;
  onToggle: () => void;
  className?: string;
  style?: CSSProperties;
}) {
  const { className, isSidebarCollapsed, onToggle, style } = props;
  const buttonClassName = `sidebar-toggle${className ? ` ${className}` : ""}`;
  const label = isSidebarCollapsed ? t("app.sidebar.expand") : t("app.sidebar.collapse");

  return (
    <button
      className={buttonClassName}
      type="button"
      aria-label={label}
      title={label}
      onClick={onToggle}
      style={style}
    >
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <rect x="3" y="3" width="18" height="18" rx="2" fill="none" stroke="currentColor" strokeWidth="2" />
        <path d="M9 3v18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
        <path
          d={isSidebarCollapsed ? "m14 9 3 3-3 3" : "m16 15-3-3 3-3"}
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    </button>
  );
}

function useWindowsMaximizedState() {
  const { window: appWindow } = useTransport();
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    let isDisposed = false;
    let unlistenResize: (() => void) | undefined;

    async function syncMaximizedState() {
      const maximized = await appWindow.isMaximized();
      if (!isDisposed) {
        setIsMaximized(maximized);
      }
    }

    async function subscribeToResize() {
      const unlisten = await appWindow.onResized(() => {
        void syncMaximizedState().catch(() => undefined);
      });
      if (isDisposed) {
        unlisten();
        return;
      }
      unlistenResize = unlisten;
    }

    void syncMaximizedState().catch(() => undefined);
    void subscribeToResize().catch(() => undefined);

    return () => {
      isDisposed = true;
      unlistenResize?.();
    };
  }, [appWindow]);

  async function toggleMaximize() {
    await appWindow.toggleMaximize();
    setIsMaximized(await appWindow.isMaximized());
  }

  return { isMaximized, toggleMaximize };
}

function WindowsMaximizeIcon(props: { isMaximized: boolean }) {
  if (props.isMaximized) {
    return (
      <svg viewBox="0 0 14 14" aria-hidden="true">
        <path d="M4.5 4.5V3H11v6.5H9.5" />
        <rect x="3" y="4.5" width="6.5" height="6.5" rx="0.5" />
      </svg>
    );
  }

  return (
    <svg viewBox="0 0 14 14" aria-hidden="true">
      <rect x="3" y="3" width="8" height="8" rx="0.5" />
    </svg>
  );
}

function WindowsMaximizeButton() {
  const { isMaximized, toggleMaximize } = useWindowsMaximizedState();
  const label = isMaximized ? t("app.window.restore") : t("app.window.maximize");

  return (
    <button
      className="window-control-button"
      type="button"
      aria-label={label}
      title={label}
      onClick={() => void toggleMaximize()}
    >
      <WindowsMaximizeIcon isMaximized={isMaximized} />
    </button>
  );
}

function WindowsWindowControls() {
  const { window: appWindow } = useTransport();

  return (
    <div className="window-controls" aria-label={t("app.window.controls")}>
      <button
        className="window-control-button"
        type="button"
        aria-label={t("app.window.minimize")}
        title={t("app.window.minimize")}
        onClick={() => void appWindow.minimize()}
      >
        <svg viewBox="0 0 14 14" aria-hidden="true">
          <path d="M3 7h8" />
        </svg>
      </button>
      <WindowsMaximizeButton />
      <button
        className="window-control-button window-control-button--close"
        type="button"
        aria-label={t("app.window.close")}
        title={t("app.window.close")}
        onClick={() => void appWindow.close()}
      >
        <svg viewBox="0 0 14 14" aria-hidden="true">
          <path d="m3 3 8 8m0-8-8 8" />
        </svg>
      </button>
    </div>
  );
}

function SettingsPage(props: {
  theme: ThemePreference;
  onThemeChange: (theme: ThemePreference) => void;
  onOpenPending: () => void;
  onOpenImport: () => void;
}) {
  return (
    <div className="placeholder-grid settings-page">
      <SettingsGroup title={t("settings.group.preferences")}>
        <SettingsItem label={t("settings.theme.label")} description={t("settings.theme.description")}>
          <ThemePicker value={props.theme} onChange={props.onThemeChange} />
        </SettingsItem>
        <UnmanagedNoticeSetting />
      </SettingsGroup>
      <VersioningSection onOpenPending={props.onOpenPending} />
      <MigrateSection onOpenImport={props.onOpenImport} />
      <SecretsSection onOpenPending={props.onOpenPending} />
      <SourcesSection />
      <CatalogsSection />
      <AgentSection />
      <UpdatesSection />
    </div>
  );
}

function AboutPage() {
  const appInfo = useAppInfo();
  const dataHome = useDataHome();

  const version = appInfo.data
    ? t("about.version.description", {
        product: PRODUCT_NAME,
        version: appInfo.data.version,
        platform: platformLabels[appInfo.data.platform],
      })
    : t("about.loading");
  const dataHomeText = dataHome.data
    ? t("about.dataHome.description", {
        path: dataHome.data.path,
        source: dataHomeSourceLabels[dataHome.data.source],
      })
    : dataHome.error instanceof IpcFailure
      ? errorMessage(dataHome.error.code)
      : dataHome.isError
        ? t("app.error.unknown")
        : t("about.loading");

  return (
    <div className="placeholder-grid settings-page">
      <SettingsGroup title={t("about.group.app")}>
        <SettingsItem readonly label={t("about.version.label")} description={version} />
        <SettingsItem readonly label={t("about.identifier.label")} description={APP_ID} />
        <SettingsItem
          readonly
          label={t("about.repository.label")}
          description={`https://github.com/${REPO}`}
        />
        <SettingsItem readonly label={t("about.dataHome.label")} description={dataHomeText} />
      </SettingsGroup>
    </div>
  );
}

function RoutePage(props: {
  route: RouteDefinition;
  onNavigate: (route: RouteKey) => void;
  onOpenPending: () => void;
  /** Opens the Import page, with an entry picked when `target` is given. */
  onOpenImport: (target?: ImportTarget) => void;
  importTarget: ImportTarget | null;
  theme: ThemePreference;
  onThemeChange: (theme: ThemePreference) => void;
}) {
  const { route } = props;
  if (route.key === "settings") {
    return (
      <SettingsPage
        theme={props.theme}
        onThemeChange={props.onThemeChange}
        onOpenPending={props.onOpenPending}
        onOpenImport={() => props.onOpenImport()}
      />
    );
  }
  if (route.key === "about") {
    return <AboutPage />;
  }
  if (route.key === "mcp") {
    return (
      <McpPage
        onOpenPending={props.onOpenPending}
        onOpenInstall={() => props.onNavigate("install")}
        onOpenImport={props.onOpenImport}
      />
    );
  }
  if (route.key === "tools") {
    return <ToolsPage onOpenMcp={() => props.onNavigate("mcp")} onOpenImport={props.onOpenImport} />;
  }
  if (route.key === "history") {
    return (
      <HistoryPage
        onOpenPending={props.onOpenPending}
        onOpenSettings={() => props.onNavigate("settings")}
      />
    );
  }
  if (route.key === "projects") {
    return <ProjectsPage />;
  }
  if (route.key === "skills") {
    return <SkillsPage onOpenInstall={() => props.onNavigate("install")} />;
  }
  if (route.key === "install") {
    return <InstallPage onOpenSkills={() => props.onNavigate("skills")} onOpenSettings={() => props.onNavigate("settings")} />;
  }
  if (route.key === "import") {
    return <ImportPage target={props.importTarget} onOpenPending={props.onOpenPending} />;
  }
  return <EmptyPage message={t(route.emptyKey)} />;
}

declare module "@tanstack/react-query" {
  interface Register {
    /** `inlineError`: the query's view shows its failure itself, so no toast repeats it (M28b finding 10). */
    queryMeta: { inlineError?: boolean };
  }
}

/** Shows every failed query as an error toast, localized from its code, except those shown inline. */
function useQueryErrorToasts() {
  const queryClient = useQueryClient();
  const { notify } = useNotifications();

  useEffect(
    () =>
      queryClient.getQueryCache().subscribe((event) => {
        if (event.type !== "updated" || event.action.type !== "error" || event.query.meta?.inlineError) {
          return;
        }
        const error = event.action.error;
        notify({
          tone: "error",
          message: error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown"),
        });
      }),
    [notify, queryClient],
  );
}

function AppContent(props: { features: FeatureFlags }) {
  const appInfo = useAppInfo();
  const [theme, setTheme] = useThemePreference();
  const platform = appInfo.data?.platform;
  const isMacOS = platform === "macos";
  const isWindows = platform === "windows";
  const usesExternalSidebarToggle = isMacOS || isWindows;
  const macOSDragRegion = isMacOS ? "" : undefined;
  const brandIconRef = useRef<HTMLDivElement | null>(null);
  const pageContentRef = useRef<HTMLElement | null>(null);
  const [activeRoute, setActiveRoute] = useState<RouteKey>("skills");
  const [importTarget, setImportTarget] = useState<ImportTarget | null>(null);
  const [isSidebarCollapsed, setIsSidebarCollapsed] = useState(false);
  const [sidebarHandleTop, setSidebarHandleTop] = useState<number | null>(null);
  const [toolbarHost, setToolbarHost] = useState<HTMLElement | null>(null);
  const [isDrawerOpen, setIsDrawerOpen] = useState(false);
  // Stable: the memoized MCP cards take it, so opening the drawer does not re-render them.
  const openPending = useCallback(() => setIsDrawerOpen(true), []);
  useQueryErrorToasts();
  useDriftTriggers();
  useStartupUpdateCheck();
  useAgentSkillStartup();

  const visibleRoutes = routes.filter((route) => !route.flag || props.features[route.flag]);
  const activeDefinition =
    visibleRoutes.find((route) => route.key === activeRoute) ?? visibleRoutes[0];

  function navigate(route: RouteKey) {
    if (pageContentRef.current) {
      pageContentRef.current.scrollTop = 0;
    }
    setImportTarget(null);
    setActiveRoute(route);
  }

  function openImport(target?: ImportTarget) {
    navigate("import");
    setImportTarget(target ?? null);
  }

  useLayoutEffect(() => {
    if (!usesExternalSidebarToggle) {
      setSidebarHandleTop(null);
      return;
    }

    const updateSidebarHandleTop = () => {
      const rect = brandIconRef.current?.getBoundingClientRect();
      if (!rect) {
        return;
      }
      setSidebarHandleTop(rect.top + rect.height / 2);
    };

    updateSidebarHandleTop();
    const frameId = window.requestAnimationFrame(updateSidebarHandleTop);
    const resizeObserver =
      typeof ResizeObserver !== "undefined" && brandIconRef.current
        ? new ResizeObserver(updateSidebarHandleTop)
        : null;

    if (resizeObserver && brandIconRef.current) {
      resizeObserver.observe(brandIconRef.current);
    }

    window.addEventListener("resize", updateSidebarHandleTop);

    return () => {
      window.cancelAnimationFrame(frameId);
      window.removeEventListener("resize", updateSidebarHandleTop);
      resizeObserver?.disconnect();
    };
  }, [isSidebarCollapsed, usesExternalSidebarToggle]);

  return (
    <div
      className={`app-shell${isSidebarCollapsed ? " is-sidebar-collapsed" : ""}${isMacOS ? " is-macos-window" : ""}${isWindows ? " is-windows-window" : ""}`}
    >
      {usesExternalSidebarToggle ? (
        <SidebarToggleButton
          className="sidebar-toggle--edge"
          isSidebarCollapsed={isSidebarCollapsed}
          onToggle={() => setIsSidebarCollapsed((current) => !current)}
          style={sidebarHandleTop == null ? undefined : { top: `${sidebarHandleTop}px` }}
        />
      ) : null}
      <aside className="sidebar">
        {isMacOS || isWindows ? (
          <div className="window-topbar window-topbar--sidebar">
            <div className="window-topbar__drag-region" data-tauri-drag-region aria-hidden="true" />
          </div>
        ) : null}
        <div className="brand-block">
          <div ref={brandIconRef} className="brand-icon" aria-hidden="true">
            <BrandIcon />
          </div>
          <p className="brand-title">{PRODUCT_NAME}</p>
          {!usesExternalSidebarToggle ? (
            <SidebarToggleButton
              isSidebarCollapsed={isSidebarCollapsed}
              onToggle={() => setIsSidebarCollapsed((current) => !current)}
            />
          ) : null}
        </div>
        <ScopeSwitcher onExpandSidebar={() => setIsSidebarCollapsed(false)} />
        <div className="sidebar-divider" aria-hidden="true" />
        <nav aria-label={t("app.nav.primary")} className="nav-list">
          {visibleRoutes.map((route) => {
            const label = t(route.labelKey);
            const selected = route.key === activeDefinition.key;
            return (
              <div key={route.key} className="nav-group">
                <button
                  className={`nav-item${route.isSubItem ? " nav-sub-item" : ""}${selected ? " is-selected" : ""}`}
                  type="button"
                  aria-current={selected ? "page" : undefined}
                  aria-label={isSidebarCollapsed ? label : undefined}
                  data-tooltip={isSidebarCollapsed ? label : undefined}
                  onClick={() => navigate(route.key)}
                >
                  <span className="nav-icon" aria-hidden="true">
                    <NavRouteIcon route={route.key} />
                  </span>
                  <span className="nav-label">{label}</span>
                </button>
              </div>
            );
          })}
        </nav>
        <UnmanagedNotice collapsed={isSidebarCollapsed} onReview={() => openImport()} />
      </aside>
      <main className="main-panel" data-active-route={activeDefinition.key}>
        {isMacOS ? (
          <div className="window-topbar window-topbar--main" data-tauri-drag-region aria-hidden="true" />
        ) : isWindows ? (
          <div className="window-topbar window-topbar--main window-topbar--windows">
            {/* Tauri's drag-region script maximizes on double-click
                (`core:window:allow-internal-toggle-maximize`). */}
            <div className="window-topbar__drag-region" data-tauri-drag-region aria-hidden="true" />
          </div>
        ) : null}
        <header className="page-header">
          <div
            key={activeDefinition.key}
            className="page-header--split"
            data-tauri-drag-region={macOSDragRegion}
          >
            <div className="page-header__row" data-tauri-drag-region={macOSDragRegion}>
              {/* Where focus goes when a closed dialog's opener is gone (M29). */}
              <h1 tabIndex={-1} data-focus-fallback>
                {t(activeDefinition.labelKey)}
              </h1>
              <PendingChangesPill onOpen={openPending} />
              <div className="mcp-header-toolbar-slot" ref={setToolbarHost} />
            </div>
            <p>{t(activeDefinition.descriptionKey)}</p>
          </div>
        </header>
        <div className="page-header-divider" aria-hidden="true" />
        <section ref={pageContentRef} className="page-content">
          <HeaderToolbarProvider value={toolbarHost}>
            <RouteErrorBoundary route={activeDefinition.key}>
              <Suspense fallback={<RouteLoading />}>
                <RoutePage
                  route={activeDefinition}
                  onNavigate={navigate}
                  onOpenPending={openPending}
                  onOpenImport={openImport}
                  importTarget={importTarget}
                  theme={theme}
                  onThemeChange={setTheme}
                />
              </Suspense>
            </RouteErrorBoundary>
          </HeaderToolbarProvider>
        </section>
        {/* Last in the panel so Tab reaches them after the page, not between
            the sidebar and the page (M28b finding 19); drawn top right. */}
        {isWindows ? <WindowsWindowControls /> : null}
      </main>
      <ApplyDrawer open={isDrawerOpen} onClose={() => setIsDrawerOpen(false)} />
      <FirstRunBackupRemote />
    </div>
  );
}

export type AppProps = {
  transport: Transport;
  /** Defaults to a fresh client; tests pass their own. */
  queryClient?: QueryClient;
  features?: FeatureFlags;
};

export function App({ transport, queryClient, features = DEFAULT_FEATURES }: AppProps) {
  const [client] = useState(() => queryClient ?? createQueryClient());

  return (
    <TransportContext.Provider value={transport}>
      <QueryClientProvider client={client}>
        <NotificationProvider>
          <ActiveScopeProvider>
            <AppContent features={features} />
          </ActiveScopeProvider>
          <AppTooltip />
        </NotificationProvider>
      </QueryClientProvider>
    </TransportContext.Provider>
  );
}
