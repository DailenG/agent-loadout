// Synthetic catalogs for the mock transport (M45): the built-ins (the MCP
// Registry, anthropics/skills, skills.sh as a link) and two user-added ones,
// with an entry or answer for every state Browse shows: fresh, saved
// (cached), stale with its age, off, a query too short for a registry, a
// timeout, a deprecated server, an unpinned one, a plugin from a source
// Loadout refuses, and a plugin with several skills to pick from. No value
// here comes from the real catalogs or the machine.

import type {
  CatalogEntry,
  CatalogEntryDetail,
  CatalogInfo,
  CatalogResults,
  CatalogsOverview,
  McpSourceReview,
  SkillInstallReview,
} from "@/ipc/bindings";
import type { CommandResult, Commands } from "@/ipc/transport";

const REGISTRY_ATTRIBUTION = "Data from the Official MCP Registry";

export const MOCK_CATALOGS: CatalogInfo[] = [
  {
    id: "mcp-registry",
    kind: "mcp_registry",
    title: "Official MCP Registry",
    url: "https://registry.modelcontextprotocol.io",
    gitRef: null,
    enabled: true,
    builtin: true,
    official: false,
    attribution: REGISTRY_ATTRIBUTION,
  },
  {
    id: "anthropic-skills",
    kind: "git_tap",
    title: "Anthropic skills",
    url: "https://github.com/anthropics/skills",
    gitRef: null,
    enabled: true,
    builtin: true,
    official: true,
    attribution: null,
  },
  {
    id: "skills-sh",
    kind: "link",
    title: "skills.sh",
    url: "https://skills.sh",
    gitRef: null,
    enabled: true,
    builtin: true,
    official: false,
    attribution: null,
  },
];

/** User-added catalogs of the demo: a tap whose refresh fails, a registry that times out. */
export const DEMO_USER_CATALOGS: CatalogInfo[] = [
  {
    id: "team-skills",
    kind: "git_tap",
    title: "Team skills",
    url: "https://github.com/acme/skills",
    gitRef: "main",
    enabled: true,
    builtin: false,
    official: false,
    attribution: null,
  },
  {
    id: "acme",
    kind: "mcp_registry",
    title: "Acme registry",
    url: "https://registry.acme.example",
    gitRef: null,
    enabled: true,
    builtin: false,
    official: false,
    attribution: null,
  },
];

function entry(fields: Partial<CatalogEntry> & Pick<CatalogEntry, "catalog" | "id" | "kind" | "name">): CatalogEntry {
  return {
    ref: `catalog:${fields.catalog}/${fields.id}`,
    description: null,
    version: null,
    updatedAt: null,
    deprecated: false,
    statusMessage: null,
    publisher: null,
    official: false,
    warning: null,
    repository: null,
    website: null,
    license: null,
    transports: [],
    packages: [],
    remotes: [],
    requiredSecrets: [],
    pinned: false,
    gitRef: null,
    commit: null,
    path: null,
    skills: [],
    servers: [],
    notImported: [],
    source: null,
    installMcp: false,
    installSkill: false,
    problem: null,
    ...fields,
  };
}

const CHECKED = "namespace ownership checked by the registry";

export const MOCK_CATALOG_ENTRIES: Record<string, CatalogEntry[]> = {
  "mcp-registry": [
    entry({
      catalog: "mcp-registry",
      id: "io.github.acme/weather",
      kind: "server",
      name: "Weather",
      description: "Forecasts and current conditions for any city.",
      version: "1.2.0",
      updatedAt: "2026-11-05T23:35:33Z",
      publisher: { name: "io.github.acme", checked: CHECKED },
      repository: "https://github.com/acme/weather-mcp",
      website: "https://acme.example/weather",
      license: "MIT",
      transports: ["stdio"],
      packages: [{ registryType: "npm", identifier: "@acme/weather-mcp", version: "1.2.0", transport: "stdio", pinned: true }],
      requiredSecrets: ["WEATHER_API_KEY"],
      pinned: true,
      installMcp: true,
    }),
    entry({
      catalog: "mcp-registry",
      id: "com.example/notes",
      kind: "server",
      name: "Team notes",
      description: "Search and write notes in the Example workspace.",
      version: "0.9.1",
      updatedAt: "2026-10-12T08:00:00Z",
      publisher: { name: "com.example", checked: CHECKED },
      website: "https://example.com/mcp",
      transports: ["streamable-http"],
      remotes: [{ transport: "streamable-http", url: "https://{tenant}.example.com/mcp" }],
      requiredSecrets: ["EXAMPLE_TOKEN"],
      pinned: true,
      installMcp: true,
    }),
    entry({
      catalog: "mcp-registry",
      id: "io.github.octo/weather-legacy",
      kind: "server",
      name: "Weather (legacy)",
      description: "The old weather server; use io.github.acme/weather.",
      version: "0.3.0",
      updatedAt: "2025-12-01T10:00:00Z",
      deprecated: true,
      statusMessage: "Replaced by io.github.acme/weather.",
      publisher: { name: "io.github.octo", checked: CHECKED },
      repository: "https://github.com/octo/weather-legacy",
      transports: ["stdio"],
      packages: [{ registryType: "oci", identifier: "ghcr.io/octo/weather", version: null, transport: "stdio", pinned: false }],
      pinned: false,
      installMcp: true,
    }),
  ],
  "anthropic-skills": [
    entry({
      catalog: "anthropic-skills",
      id: "pdf",
      kind: "skill",
      name: "pdf",
      description: "Read, fill and create PDF files.",
      official: true,
      repository: "https://github.com/anthropics/skills",
      license: "Apache-2.0",
      path: "skills/pdf",
      commit: "8c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d",
      installSkill: true,
    }),
    entry({
      catalog: "anthropic-skills",
      id: "webapp-testing",
      kind: "skill",
      name: "webapp-testing",
      description: "Test web apps with a headless browser.",
      official: true,
      repository: "https://github.com/anthropics/skills",
      license: "Apache-2.0",
      path: "skills/webapp-testing",
      commit: "8c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d",
      installSkill: true,
    }),
    // The plugin's other skills are entries of their own, as the core indexes them.
    ...(["docx", "pptx", "xlsx"] as const).map((name) =>
      entry({
        catalog: "anthropic-skills",
        id: name,
        kind: "skill",
        name,
        description: `Create and edit ${name} files.`,
        official: true,
        repository: "https://github.com/anthropics/skills",
        license: "Apache-2.0",
        path: `skills/${name}`,
        commit: "8c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d",
        installSkill: true,
      }),
    ),
    entry({
      catalog: "anthropic-skills",
      id: "plugin/document-skills",
      kind: "plugin",
      name: "document-skills",
      description: "Skills for Word, Excel, PowerPoint and PDF files.",
      official: true,
      repository: "https://github.com/anthropics/skills",
      path: ".",
      commit: "8c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d",
      skills: ["docx", "pdf", "pptx", "xlsx"],
      source: { kind: "relative", url: "https://github.com/anthropics/skills", path: ".", gitRef: null, sha: null },
      installSkill: true,
    }),
  ],
  "team-skills": [
    entry({
      catalog: "team-skills",
      id: "repo-conventions",
      kind: "skill",
      name: "repo-conventions",
      description: "How this team names branches, writes commits and reviews code.",
      repository: "https://github.com/acme/skills",
      path: "skills/repo-conventions",
      gitRef: "main",
      commit: "3f2a9c1b7e5d4c3b2a1f0e9d8c7b6a5f4e3d2c1b",
      installSkill: true,
    }),
    entry({
      catalog: "team-skills",
      id: "plugin/ops-tools",
      kind: "plugin",
      name: "ops-tools",
      description: "On-call helpers, published to npm.",
      source: { kind: "npm", url: null, path: null, gitRef: null, sha: null },
      notImported: ["hooks", "commands"],
      problem: "plugins from npm/archive/command sources are not supported",
    }),
    entry({
      catalog: "team-skills",
      id: "plugin/incident",
      kind: "plugin",
      name: "incident",
      description: "Incident response skills and a status page server.",
      repository: "https://github.com/acme/incident-plugin",
      // A plugin from another repository: its skills and servers are known after the review.
      skills: [],
      servers: ["statuspage"],
      notImported: ["hooks"],
      pinned: true,
      source: {
        kind: "github",
        url: "https://github.com/acme/incident-plugin",
        path: null,
        gitRef: null,
        sha: "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
      },
      installMcp: true,
      installSkill: false,
    }),
  ],
  acme: [],
};

function matches(item: CatalogEntry, query: string): boolean {
  const needle = query.trim().toLowerCase();
  return (
    needle === "" ||
    [item.id, item.name, item.description ?? ""].some((text) => text.toLowerCase().includes(needle))
  );
}

/** How a catalog answers while online in the mock: fresh, saved, stale or failing. */
export type MockCatalogBehavior =
  | { kind: "fresh" }
  | { kind: "cached"; ageSecs: number }
  | { kind: "stale"; ageSecs: number; message: string }
  | { kind: "error"; message: string };

export type CatalogMockState = {
  online: boolean;
  decided: boolean;
  catalogs: CatalogInfo[];
  /** Default fresh. */
  behavior: Record<string, MockCatalogBehavior>;
  /** Saved results' age while online catalogs are off (default 3 days). */
  offAgeSecs: number;
  /** Catalogs with saved results while online catalogs are off (default the Anthropic tap). */
  offSaved: string[];
};

function ok<T>(data: T): CommandResult<T> {
  return { status: "ok", data };
}

function fail(code: "catalogs_off" | "not_found" | "invalid_value" | "already_exists", message: string): CommandResult<never> {
  return { status: "error", error: { code, message } };
}

/** The commands of the catalog mock, over a shared state the tests and the demo can seed. */
export function catalogCommands(
  state: CatalogMockState,
  reviews: { mcp: McpSourceReview; skill: SkillInstallReview },
): Pick<
  Commands,
  | "catalogsOverview"
  | "setOnlineCatalogs"
  | "addCatalog"
  | "removeCatalog"
  | "setCatalogEnabled"
  | "searchCatalog"
  | "showCatalogEntry"
  | "refreshCatalog"
  | "reviewCatalogMcp"
  | "reviewCatalogSkill"
  | "openCatalogLink"
> {
  const overview = (): CatalogsOverview => ({
    online: state.online,
    decided: state.decided,
    contacts: [
      ...new Set(
        state.catalogs
          .filter((catalog) => catalog.enabled && catalog.kind !== "link")
          .map((catalog) => new URL(catalog.url).host),
      ),
    ],
    catalogs: state.catalogs,
  });
  const find = (id: string) => state.catalogs.find((catalog) => catalog.id === id);

  function results(id: string, query: string): CatalogResults {
    const catalog = find(id);
    const base: CatalogResults = {
      catalog: id,
      status: "fresh",
      ageSecs: null,
      fetchedAt: null,
      error: null,
      entries: [],
      nextCursor: null,
      link: null,
      attribution: catalog?.attribution ?? null,
    };
    if (!catalog || !catalog.enabled) {
      return { ...base, status: "disabled" };
    }
    if (catalog.kind === "link") {
      return { ...base, status: "link", link: catalog.url };
    }
    const entries = (MOCK_CATALOG_ENTRIES[id] ?? []).filter((item) => matches(item, query));
    // As the core: a registry checks the query length before anything else.
    if (catalog.kind === "mcp_registry" && query.trim().length < 3) {
      return { ...base, status: "query_too_short" };
    }
    if (!state.online) {
      // Off: whatever is saved, of any age.
      return state.offSaved.includes(id)
        ? { ...base, status: "off", ageSecs: state.offAgeSecs, entries }
        : { ...base, status: "off" };
    }
    const behavior = state.behavior[id] ?? { kind: "fresh" };
    switch (behavior.kind) {
      case "fresh":
        return { ...base, ageSecs: 0, entries };
      case "cached":
        return { ...base, status: "cached", ageSecs: behavior.ageSecs, entries };
      case "stale":
        return {
          ...base,
          status: "stale",
          ageSecs: behavior.ageSecs,
          entries,
          error: { code: "catalog_unreachable", message: behavior.message },
        };
      case "error":
        return { ...base, status: "error", error: { code: "catalog_unreachable", message: behavior.message } };
    }
  }

  return {
    catalogsOverview: async () => ok(overview()),
    setOnlineCatalogs: async (on) => {
      state.online = on;
      state.decided = true;
      return ok(overview());
    },
    addCatalog: async (kind, url, id, gitRef, title) => {
      const newId = id ?? new URL(url).host.split(".")[0];
      if (find(newId)) {
        return fail("already_exists", `catalog ${newId} exists`);
      }
      state.catalogs = [
        ...state.catalogs,
        { id: newId, kind, title: title ?? url, url, gitRef, enabled: true, builtin: false, official: false, attribution: null },
      ];
      return ok(overview());
    },
    removeCatalog: async (id) => {
      if (find(id)?.builtin) {
        return fail("invalid_value", `${id} is built in; disable it instead`);
      }
      state.catalogs = state.catalogs.filter((catalog) => catalog.id !== id);
      return ok(overview());
    },
    setCatalogEnabled: async (id, enabled) => {
      state.catalogs = state.catalogs.map((catalog) => (catalog.id === id ? { ...catalog, enabled } : catalog));
      return ok(overview());
    },
    searchCatalog: async (id, query) => ok(results(id, query)),
    refreshCatalog: async (id) => (state.online ? ok(results(id, "")) : fail("catalogs_off", "online catalogs are off")),
    showCatalogEntry: async (id, entryId) => {
      const found = (MOCK_CATALOG_ENTRIES[id] ?? []).find((item) => item.id === entryId);
      if (!found) {
        return fail("not_found", `${id}/${entryId} is not in the mock catalogs`);
      }
      const listed = results(id, "");
      const detail: CatalogEntryDetail = {
        entry: found,
        status: listed.status === "query_too_short" ? "fresh" : listed.status,
        ageSecs: listed.ageSecs,
        error: listed.error,
      };
      return ok(detail);
    },
    reviewCatalogMcp: async (id, entryId, version) => {
      if (!state.online) {
        return fail("catalogs_off", "online catalogs are off");
      }
      const found = (MOCK_CATALOG_ENTRIES[id] ?? []).find((item) => item.id === entryId);
      if (!found || found.problem || !found.installMcp) {
        return fail("invalid_value", found?.problem ?? `${id}/${entryId} has no MCP server`);
      }
      const catalog = find(id);
      const registry = catalog?.kind === "mcp_registry";
      return ok({
        ...reviews.mcp,
        token: `catalog-mcp-${id}-${entryId}`,
        source: {
          kind: registry ? "registry" : "git",
          url: registry ? (catalog?.url ?? null) : (found.source?.url ?? found.repository),
          folder: null,
          path: registry ? null : found.source?.path ?? null,
          gitRef: null,
          commit: registry ? null : found.source?.sha ?? found.commit,
          catalog: `${id}/${entryId}`,
          version: registry ? (version ?? found.version) : null,
        },
      });
    },
    reviewCatalogSkill: async (id, entryId, skill) => {
      if (!state.online) {
        return fail("catalogs_off", "online catalogs are off");
      }
      // As the app command: a picked skill replaces the entry (the plugin itself is not consulted).
      const found = (MOCK_CATALOG_ENTRIES[id] ?? []).find((item) => item.id === (skill ?? entryId));
      if (!found || found.problem || !found.installSkill) {
        return fail(found ? "invalid_value" : "not_found", found?.problem ?? `${id}/${skill ?? entryId} has no skill`);
      }
      return ok({
        ...reviews.skill,
        token: `catalog-skill-${id}-${found.id}`,
        name: found.id,
        source: {
          ...reviews.skill.source,
          url: found.source?.url ?? found.repository,
          path: found.path,
          gitRef: found.gitRef,
          commit: found.commit?.slice(0, 7) ?? null,
        },
      });
    },
    openCatalogLink: async () => ok(null),
  };
}

/** A fresh state: nothing decided yet, the built-ins on. */
export function defaultCatalogState(): CatalogMockState {
  return {
    online: false,
    decided: false,
    catalogs: [...MOCK_CATALOGS],
    behavior: {},
    offAgeSecs: 3 * 86_400,
    offSaved: ["anthropic-skills"],
  };
}
