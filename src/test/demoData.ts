// A realistic demo dataset for `npm run dev:mock` (the default
// `?dataset=demo`): two projects (one trusted, one changed since trusted),
// store servers in mixed pill states, skills, tools with omp detected and a
// history with applies and tags. It gives every page and dialog content for
// screenshots and design review. Values are synthetic: paths stay under
// their placeholder roots and no secret value appears anywhere.

import type {
  AliasEntry,
  AppliedRun,
  BackupRunEntry,
  ClientInfo,
  ClientPill,
  CommitDetail,
  EffectiveMatrix,
  HistoryEntry,
  IpcError,
  MigrationReport,
  PendingPlan,
  PillState,
  ProjectRow,
  ProjectServerList,
  ProjectServerRow,
  RemotesOverview,
  ServerDetail,
  ServerRow,
  SkillClient,
  SkillInfo,
  SkillList,
  TagEntry,
  ToolInfo,
  ToolSkillDir,
  TrustReview,
} from "@/ipc/bindings";
import { MOCK_CHANGE, mockIpcError, ok, type MockTransportOptions } from "@/test/mockTransport";

const problem = (code: IpcError["code"], message: string): IpcError => ({ code, message });

// --- Clients -------------------------------------------------------------

const CLIENTS: ClientInfo[] = [
  { id: "claude-code", name: "Claude Code", detected: true, supported: true },
  { id: "omp", name: "omp", detected: true, supported: true },
  { id: "codex", name: "Codex", detected: true, supported: true },
  { id: "cursor", name: "Cursor", detected: true, supported: true },
  { id: "windsurf", name: "Windsurf", detected: false, supported: true },
  { id: "claude-desktop", name: "Claude Desktop", detected: true, supported: false },
];

const SKILL_CLIENTS: SkillClient[] = [
  { id: "claude-code", name: "Claude Code", detected: true },
  { id: "omp", name: "omp", detected: true },
  { id: "codex", name: "Codex", detected: true },
  { id: "pi", name: "Pi", detected: false },
  { id: "copilot", name: "GitHub Copilot", detected: true },
  { id: "cursor", name: "Cursor", detected: true },
];

/** One pill per client in `CLIENTS` order; `states` maps client id to state. */
function pills(states: Partial<Record<string, PillState>>, reasons: Partial<Record<string, IpcError>> = {}): ClientPill[] {
  return CLIENTS.map((client) => {
    const state = states[client.id] ?? (client.supported ? "disabled" : "not_supported");
    return {
      client: client.id,
      selected: state !== "disabled" && state !== "not_supported",
      state,
      reason: reasons[client.id] ?? null,
    };
  });
}

const selected = (row: ClientPill[]) => row.filter((pill) => pill.selected).map((pill) => pill.client);

// --- Store servers ---------------------------------------------------------

function server(
  id: string,
  fields: Partial<ServerRow> & Pick<ServerRow, "launch" | "transport" | "pills">,
): ServerRow {
  return {
    id,
    displayName: null,
    description: null,
    origin: null,
    tags: [],
    clients: { allDetected: false, clients: selected(fields.pills) },
    problems: [],
    invalid: null,
    ...fields,
  };
}

const SERVERS: ServerRow[] = [
  server("github", {
    displayName: "GitHub",
    description: "Issues, pull requests and code search on github.com.",
    transport: "stdio",
    launch: "npx -y @modelcontextprotocol/server-github",
    origin: "imported:claude-code",
    tags: ["git", "work"],
    pills: pills({ "claude-code": "enabled", omp: "enabled", codex: "enabled", cursor: "enabled" }),
  }),
  server("context7", {
    displayName: "Context7",
    description: "Up-to-date library documentation for prompts.",
    transport: "http",
    launch: "https://mcp.context7.com/mcp",
    tags: ["docs"],
    pills: pills({ "claude-code": "enabled", omp: "pending", codex: "pending" }),
  }),
  server("filesystem", {
    displayName: "Filesystem",
    description: "Read and write files under the allowed folders.",
    transport: "stdio",
    launch: "npx -y @modelcontextprotocol/server-filesystem ~/Code",
    pills: pills({ "claude-code": "enabled", omp: "enabled", cursor: "drift" }),
  }),
  server("sentry", {
    displayName: "Sentry",
    description: "Errors and performance issues from Sentry.",
    transport: "http",
    launch: "https://mcp.sentry.dev/mcp",
    tags: ["work"],
    pills: pills(
      { "claude-code": "blocked", omp: "blocked" },
      {
        "claude-code": problem("missing_secret", "alias SENTRY_TOKEN has no value"),
        omp: problem("missing_secret", "alias SENTRY_TOKEN has no value"),
      },
    ),
    problems: [problem("missing_secret", "alias SENTRY_TOKEN has no value")],
  }),
  server("playwright", {
    displayName: "Playwright",
    description: "Drive a browser for end-to-end checks.",
    transport: "stdio",
    launch: "npx -y @playwright/mcp@latest",
    pills: pills({ omp: "enabled", codex: "enabled" }),
  }),
  server("linear", {
    displayName: "Linear",
    description: null,
    transport: "sse",
    launch: "https://mcp.linear.app/sse",
    pills: pills({}),
  }),
  server("postgres-local", {
    displayName: null,
    description: null,
    transport: null,
    launch: "",
    pills: pills({}),
    invalid: problem("parse_error", "store/servers/postgres-local.toml: expected `=` at line 4"),
  }),
];

const SERVER_DETAILS: Record<string, ServerDetail> = {
  github: {
    draft: {
      id: "github",
      displayName: "GitHub",
      description: "Issues, pull requests and code search on github.com.",
      transport: "stdio",
      command: "npx",
      args: ["-y", "@modelcontextprotocol/server-github"],
      url: null,
      cwd: null,
      env: [{ key: "GITHUB_PERSONAL_ACCESS_TOKEN", value: "${secret:GITHUB_TOKEN}" }],
      headers: [],
      clients: { allDetected: false, clients: ["claude-code", "omp", "codex", "cursor"] },
      delivery: null,
      toolsAllow: [],
      toolsDeny: ["delete_repository"],
    },
    invalid: null,
    toml: [
      'id = "github"',
      'display_name = "GitHub"',
      'description = "Issues, pull requests and code search on github.com."',
      'transport = "stdio"',
      'command = "npx"',
      'args = ["-y", "@modelcontextprotocol/server-github"]',
      "",
      "[env]",
      'GITHUB_PERSONAL_ACCESS_TOKEN = "${secret:GITHUB_TOKEN}"',
      "",
      "[enable]",
      'clients = ["claude-code", "omp", "codex", "cursor"]',
      "",
      "[tools]",
      'deny = ["delete_repository"]',
      "",
    ].join("\n"),
  },
  "postgres-local": {
    draft: null,
    invalid: problem("parse_error", "store/servers/postgres-local.toml: expected `=` at line 4"),
    toml: 'id = "postgres-local"\ntransport = "stdio"\ncommand = "npx"\nargs [\n',
  },
};

// --- Projects ----------------------------------------------------------------

const PROJECTS: ProjectRow[] = [
  {
    id: "web-shop-3f9a",
    name: "web-shop",
    path: "~/Code/web-shop",
    layout: "shared_and_local",
    trust: "trusted",
    error: null,
    servers: 3,
    clients: 3,
    pending: 1,
  },
  {
    id: "api-gateway-81c2",
    name: "api-gateway",
    path: "~/Code/api-gateway",
    layout: "shared",
    trust: "changed",
    error: null,
    servers: 2,
    clients: 2,
    pending: 0,
  },
];

function projectServer(fields: Partial<ProjectServerRow> & Pick<ProjectServerRow, "name" | "pills">): ProjectServerRow {
  return {
    kind: "define",
    from: null,
    layers: ["shared"],
    editLayer: "shared",
    transport: "stdio",
    launch: "",
    clients: { allDetected: false, clients: selected(fields.pills) },
    usesDefaults: false,
    problems: [],
    invalid: null,
    ...fields,
  };
}

const INHERITED = SERVERS.filter((row) => row.invalid === null && row.pills.some((pill) => pill.selected)).map(
  (row) => ({ id: row.id, displayName: row.displayName, transport: row.transport, launch: row.launch }),
);

const PROJECT_SERVERS: Record<string, ProjectServerList> = {
  "web-shop-3f9a": {
    project: {
      id: "web-shop-3f9a",
      name: "web-shop",
      path: "~/Code/web-shop",
      trust: "trusted",
      hash: "b3a1c0de",
      layers: ["shared", "local"],
      defaultClients: { allDetected: true, clients: [] },
    },
    servers: [
      projectServer({
        name: "storybook",
        transport: "stdio",
        launch: "npx -y @storybook/mcp",
        usesDefaults: true,
        pills: pills({ "claude-code": "enabled", omp: "enabled", codex: "enabled", cursor: "pending" }),
      }),
      projectServer({
        name: "github",
        kind: "override",
        from: "github",
        layers: ["shared", "local"],
        editLayer: "local",
        launch: "npx -y @modelcontextprotocol/server-github",
        pills: pills({ "claude-code": "enabled", omp: "enabled" }),
      }),
      projectServer({
        name: "linear",
        kind: "suppress",
        from: "linear",
        layers: ["local"],
        editLayer: "local",
        transport: null,
        launch: "",
        pills: pills({ cursor: "enabled" }),
      }),
    ],
    inherited: INHERITED,
    clients: CLIENTS,
    storeServers: SERVERS.map((row) => row.id),
    warnings: [],
  },
  "api-gateway-81c2": {
    project: {
      id: "api-gateway-81c2",
      name: "api-gateway",
      path: "~/Code/api-gateway",
      trust: "changed",
      hash: "77e0aa12",
      layers: ["shared", "local"],
      defaultClients: { allDetected: false, clients: ["omp", "codex"] },
    },
    servers: [
      projectServer({
        name: "k6",
        transport: "stdio",
        launch: "k6 mcp",
        usesDefaults: true,
        pills: pills({ omp: "enabled", codex: "enabled" }),
      }),
      projectServer({
        name: "openapi",
        transport: "http",
        launch: "http://localhost:8088/mcp",
        pills: pills({ omp: "blocked" }, { omp: problem("missing_secret", "alias GATEWAY_KEY has no value") }),
        problems: [problem("missing_secret", "alias GATEWAY_KEY has no value")],
      }),
    ],
    inherited: INHERITED,
    clients: CLIENTS,
    storeServers: SERVERS.map((row) => row.id),
    warnings: [],
  },
};

const TRUST_REVIEWS: Record<string, TrustReview> = {
  "api-gateway-81c2": {
    id: "api-gateway-81c2",
    name: "api-gateway",
    path: "~/Code/api-gateway",
    trust: "changed",
    hash: "77e0aa12",
    files: [
      {
        file: "project.toml",
        path: "<P>/.loadout/project.toml",
        change: "changed",
        content: null,
        diff: [
          "--- trusted",
          "+++ now",
          "@@ -6,3 +6,8 @@",
          " [servers.k6]",
          ' command = "k6"',
          ' args = ["mcp"]',
          "+",
          "+[servers.openapi]",
          '+transport = "http"',
          '+url = "http://localhost:8088/mcp"',
          '+headers = { Authorization = "Bearer ${secret:GATEWAY_KEY}" }',
        ].join("\n"),
      },
    ],
    warnings: [],
  },
  "web-shop-3f9a": {
    id: "web-shop-3f9a",
    name: "web-shop",
    path: "~/Code/web-shop",
    trust: "trusted",
    hash: "b3a1c0de",
    files: [
      {
        file: "project.toml",
        path: "<P>/.loadout/project.toml",
        change: "unchanged",
        content: '[defaults]\nclients = "all-detected"\n\n[servers.storybook]\ncommand = "npx"\nargs = ["-y", "@storybook/mcp"]\n',
        diff: null,
      },
    ],
    warnings: [],
  },
};

function matrix(projectId: string): EffectiveMatrix | null {
  const project = PROJECTS.find((row) => row.id === projectId);
  if (!project) {
    return null;
  }
  const cell = (client: string, layer: "system" | "project" | "project_local" | "foreign", source: string, loaded = true) => ({
    client,
    winner: {
      layer,
      file: layer === "system" ? "~/.claude.json" : layer === "foreign" ? "<P>/.cursor/mcp.json" : "<P>/.mcp.json",
    },
    source,
    loaded,
    shadowed: [],
    alsoVisibleTo: [],
    status: loaded ? ("synced" as const) : ("pending_create" as const),
    warnings: [],
    snippet: `"${source.split(":")[1] ?? source}": { "command": "npx" }`,
  });
  return {
    projectId,
    projectName: project.name,
    path: project.path,
    clients: CLIENTS.filter((client) => client.detected).map(({ id, name, detected }) => ({ id, name, detected })),
    rows: [
      {
        name: "github",
        cells: [
          { ...cell("claude-code", "project_local", "store:github+project"), shadowed: [{ layer: "system", file: "~/.claude.json" }] },
          cell("omp", "project_local", "store:github+project"),
          cell("codex", "system", "store:github"),
        ],
      },
      { name: "storybook", cells: [cell("claude-code", "project", "project:storybook"), cell("omp", "project", "project:storybook"), cell("cursor", "project", "project:storybook", false)] },
      { name: "context7", cells: [cell("claude-code", "system", "store:context7")] },
      { name: "figma", cells: [cell("cursor", "foreign", "foreign")] },
    ],
    warnings: [],
  };
}

// --- Pending plan ------------------------------------------------------------

const PENDING_PLAN: PendingPlan = {
  applyMode: "review",
  files: [
    {
      file: "~/.omp/agent/mcp.json",
      clients: ["omp"],
      scope: "user",
      projectId: null,
      project: null,
      counts: { create: 1, update: 0, remove: 0, disable: 0 },
      diff: '--- ~/.omp/agent/mcp.json\n+++ ~/.omp/agent/mcp.json\n@@ -3,0 +4,3 @@\n+    "context7": {\n+      "url": "https://mcp.context7.com/mcp"\n+    },\n',
      entries: [{ action: "create", name: "context7", client: "omp", server: "context7", state: "pending_create", unresolved: false, problem: null }],
      problem: null,
      warnings: [],
    },
    {
      file: "~/.codex/config.toml",
      clients: ["codex"],
      scope: "user",
      projectId: null,
      project: null,
      counts: { create: 1, update: 0, remove: 0, disable: 0 },
      diff: '--- ~/.codex/config.toml\n+++ ~/.codex/config.toml\n@@ -8,0 +9,2 @@\n+[mcp_servers.context7]\n+url = "https://mcp.context7.com/mcp"\n',
      entries: [{ action: "create", name: "context7", client: "codex", server: "context7", state: "pending_create", unresolved: false, problem: null }],
      problem: null,
      warnings: [],
    },
    {
      file: "~/.cursor/mcp.json",
      clients: ["cursor"],
      scope: "user",
      projectId: null,
      project: null,
      counts: { create: 0, update: 0, remove: 0, disable: 0 },
      diff: "",
      entries: [{ action: "none", name: "filesystem", client: "cursor", server: "filesystem", state: "drifted", unresolved: true, problem: null }],
      problem: null,
      warnings: [],
    },
  ],
  pending: 3,
  writes: 2,
  unresolved: 1,
  warnings: [],
};

// --- Skills -------------------------------------------------------------------

function skill(fields: Partial<SkillInfo> & Pick<SkillInfo, "id" | "description">): SkillInfo {
  return {
    name: fields.id,
    fields: [],
    tags: [],
    source: null,
    allDetected: false,
    clients: [],
    method: "link",
    projectEnabled: false,
    projectShared: false,
    links: [],
    bytes: 4_812,
    hasContent: true,
    problems: [],
    ...fields,
  };
}

const SKILLS: SkillInfo[] = [
  skill({
    id: "code-review",
    description: "Review a diff for correctness, security and style before it lands.",
    tags: ["review", "quality"],
    source: { kind: "git", url: "https://github.com/example/agent-skills", path: "skills/code-review", gitRef: "main", commit: "4c1d9e2" },
    allDetected: true,
    clients: ["claude-code", "omp", "codex", "copilot", "cursor"],
    bytes: 12_408,
    projectEnabled: true,
    projectShared: true,
  }),
  skill({
    id: "release-notes",
    description: "Draft release notes from merged pull requests and the changelog.",
    tags: ["writing"],
    source: { kind: "local", url: null, path: "~/Skills/release-notes", gitRef: null, commit: null },
    clients: ["omp", "claude-code"],
    bytes: 3_150,
    projectEnabled: true,
  }),
  skill({
    id: "sql-migrations",
    description: "Write reversible SQL migrations and check them against the schema.",
    tags: ["database"],
    source: { kind: "catalog", url: "https://skills.example.dev/sql-migrations", path: null, gitRef: null, commit: null },
    clients: ["codex"],
    method: "copy",
    bytes: 7_904,
  }),
  skill({
    id: "frontend-design",
    description: "Build accessible, responsive interfaces with the project's design tokens.",
    tags: ["ui", "accessibility"],
    source: { kind: "agents_cli", url: "https://github.com/example/frontend-skills", path: null, gitRef: null, commit: null },
    clients: ["claude-code", "omp", "cursor"],
    bytes: 21_337,
  }),
  skill({
    id: "incident-triage",
    description: "Summarize an incident from logs and alerts, and propose next steps.",
    source: { kind: "authored", url: null, path: null, gitRef: null, commit: null },
    clients: [],
    bytes: 1_204,
    problems: [problem("collision", "~/.codex/skills/incident-triage exists and is not Loadout's")],
  }),
  skill({
    id: "empty-draft",
    description: null,
    source: { kind: "authored", url: null, path: null, gitRef: null, commit: null },
    bytes: 0,
    hasContent: false,
  }),
];

function skillList(scope: { kind: "user" } | { kind: "project"; id: string }): SkillList {
  if (scope.kind === "user") {
    return { skills: SKILLS, clients: SKILL_CLIENTS, project: null, agentsCli: null, sizeWarning: null };
  }
  const project = PROJECTS.find((row) => row.id === scope.id) ?? PROJECTS[0];
  return {
    skills: SKILLS,
    clients: SKILL_CLIENTS,
    project: {
      id: project.id,
      name: project.name,
      path: project.path,
      trust: project.trust,
      hash: PROJECT_SERVERS[project.id]?.project.hash ?? "00000000",
      clients: ["claude-code", "omp", "codex"],
      share: false,
    },
    agentsCli: null,
    sizeWarning: null,
  };
}

const SKILL_TOOL_DIRS: ToolSkillDir[] = [
  {
    client: "claude-code",
    name: "Claude Code",
    dir: "~/.claude/skills",
    exists: true,
    entries: [
      { name: "code-review", path: "~/.claude/skills/code-review", class: "managed", linkTarget: "%LOCALAPPDATA%/Loadout/store/skills/code-review", skillId: "code-review", description: "Review a diff for correctness, security and style before it lands." },
      { name: "pdf-tools", path: "~/.claude/skills/pdf-tools", class: "unmanaged", linkTarget: null, skillId: null, description: "Fill and split PDF files." },
    ],
  },
  {
    client: "omp",
    name: "omp",
    dir: "~/.omp/agent/skills",
    exists: true,
    entries: [
      { name: "code-review", path: "~/.omp/agent/skills/code-review", class: "managed", linkTarget: "%LOCALAPPDATA%/Loadout/store/skills/code-review", skillId: "code-review", description: "Review a diff for correctness, security and style before it lands." },
    ],
  },
  {
    client: "codex",
    name: "Codex",
    dir: "~/.codex/skills",
    exists: true,
    entries: [
      { name: "incident-triage", path: "~/.codex/skills/incident-triage", class: "mismatch", linkTarget: null, skillId: "incident-triage", description: null },
    ],
  },
];

// --- Tools --------------------------------------------------------------------

function tool(fields: Partial<ToolInfo> & Pick<ToolInfo, "id" | "name">): ToolInfo {
  return {
    tier: "a",
    detected: true,
    supported: true,
    readOnly: false,
    userFiles: [],
    projectFiles: [],
    transports: { stdio: true, http: true, sse: true },
    envRefs: true,
    restartRequired: false,
    warnings: [],
    homepage: null,
    ...fields,
  };
}

const TOOLS: ToolInfo[] = [
  tool({
    id: "claude-code",
    name: "Claude Code",
    userFiles: [{ path: "~/.claude.json", exists: true }],
    projectFiles: ["<P>/.mcp.json"],
  }),
  tool({
    id: "omp",
    name: "omp",
    userFiles: [{ path: "~/.omp/agent/mcp.json", exists: true }],
    projectFiles: ["<P>/.omp/mcp.json"],
    homepage: "https://github.com/can1357/oh-my-pi",
  }),
  tool({
    id: "codex",
    name: "Codex",
    userFiles: [{ path: "~/.codex/config.toml", exists: true }],
    projectFiles: ["<P>/.codex/config.toml"],
    envRefs: false,
    transports: { stdio: true, http: true, sse: false },
  }),
  tool({
    id: "cursor",
    name: "Cursor",
    tier: "b",
    userFiles: [{ path: "~/.cursor/mcp.json", exists: true }],
    projectFiles: ["<P>/.cursor/mcp.json"],
    restartRequired: true,
  }),
  tool({
    id: "windsurf",
    name: "Windsurf",
    tier: "b",
    detected: false,
    userFiles: [{ path: "~/.codeium/windsurf/mcp_config.json", exists: false }],
  }),
  tool({
    id: "claude-desktop",
    name: "Claude Desktop",
    tier: "c",
    supported: false,
    userFiles: [],
    warnings: [problem("client_mcp_replaced_by_extension", "Claude Desktop manages MCP servers through extensions")],
  }),
  tool({
    id: "vscode-legacy",
    name: "VS Code (legacy settings)",
    tier: "d",
    readOnly: true,
    userFiles: [{ path: "%APPDATA%/Code/User/settings.json", exists: true }],
  }),
];

// --- History ------------------------------------------------------------------

const commitId = (n: number) => `${n.toString(16).padStart(2, "0")}c3f1a9e2b47d6058e1f2a3b4c5d6e7f8091a2b3`.slice(0, 40);

function entry(n: number, fields: Partial<HistoryEntry> & Pick<HistoryEntry, "time" | "summary">): HistoryEntry {
  const id = commitId(n);
  return {
    id,
    shortId: id.slice(0, 7),
    op: null,
    entities: [],
    scope: "system",
    machine: "desktop",
    thisMachine: true,
    apply: null,
    tags: [],
    ...fields,
  };
}

const HISTORY: HistoryEntry[] = [
  entry(12, { time: "2026-11-21T16:42:10Z", summary: "Enable context7 for omp and Codex", op: "set-clients", entities: ["server/context7"] }),
  entry(11, {
    time: "2026-11-21T09:15:33Z",
    summary: "Apply: 3 files written",
    op: "apply",
    apply: { applyId: "01JDEMOAPPLY000000000000B", outcome: "ok", undoable: true, latest: true },
  }),
  entry(10, { time: "2026-11-20T18:03:51Z", summary: "Add project server storybook", op: "project-server", entities: ["project/web-shop-3f9a"], scope: "project/web-shop-3f9a" }),
  entry(9, { time: "2026-11-20T11:27:02Z", summary: "Install skill frontend-design", op: "skill-install", entities: ["skill/frontend-design"], tags: ["before-redesign"] }),
  entry(8, {
    time: "2026-11-19T22:40:19Z",
    summary: "Apply: 1 file written, 1 blocked",
    op: "apply",
    apply: { applyId: "01JDEMOAPPLY000000000000A", outcome: "partial", undoable: true, latest: false },
  }),
  entry(7, { time: "2026-11-19T08:12:44Z", summary: "Edit server github", op: "save-server", entities: ["server/github"], machine: "laptop", thisMachine: false }),
  entry(6, { time: "2026-11-18T14:55:00Z", summary: "Trust project api-gateway", op: "trust", entities: ["project/api-gateway-81c2"], scope: "project/api-gateway-81c2" }),
  entry(5, { time: "2026-11-17T10:00:00Z", summary: "Import 4 servers from Claude Code", op: "import", entities: ["server/github", "server/filesystem", "server/sentry", "server/playwright"], tags: ["v-baseline", "auto/pre-restore/20261117"] }),
];

const TAGS: TagEntry[] = [
  { name: "before-redesign", kind: "user", commit: commitId(9), message: "Known good before the UI work", time: "2026-11-20T11:30:00Z" },
  { name: "v-baseline", kind: "user", commit: commitId(5), message: null, time: "2026-11-17T10:05:00Z" },
  { name: "auto/pre-restore/20261117", kind: "auto", commit: commitId(5), message: null, time: "2026-11-17T10:01:00Z" },
];

function commitDetail(id: string): CommitDetail | null {
  const found = HISTORY.find((row) => row.id === id || row.shortId === id);
  if (!found) {
    return null;
  }
  const isApply = found.apply !== null;
  return {
    entry: found,
    body: isApply ? "" : `${found.summary}.`,
    changes: isApply
      ? []
      : [
          {
            entity: found.entities[0] ?? "store",
            file: `store/servers/${(found.entities[0] ?? "server/github").split("/")[1]}.toml`,
            change: "changed",
            fields: [{ field: "enable.clients", before: '["claude-code"]', after: '["claude-code", "omp", "codex"]' }],
            text: null,
          },
        ],
    raw: '--- a/store/servers/context7.toml\n+++ b/store/servers/context7.toml\n@@ -6 +6 @@\n-clients = ["claude-code"]\n+clients = ["claude-code", "omp", "codex"]\n',
    applied: isApply
      ? {
          trigger: "review",
          outcome: found.apply!.outcome,
          targets: [
            { client: "omp", scope: "user", projectId: null, file: "~/.omp/agent/mcp.json", counts: { create: 1, update: 1, remove: 0, disable: 0 }, outcome: "ok" },
            { client: "claude-code", scope: "project", projectId: "web-shop-3f9a", file: "<P>/.mcp.json", counts: { create: 1, update: 0, remove: 0, disable: 0 }, outcome: found.apply!.outcome },
          ],
        }
      : null,
  };
}

const APPLIED_RUNS: AppliedRun[] = HISTORY.filter((row) => row.apply).map((row) => ({
  applyId: row.apply!.applyId,
  commit: row.id,
  time: row.time,
  summary: row.summary,
  latest: row.apply!.latest,
}));

const BACKUPS: BackupRunEntry[] = [
  {
    run: "20261121T091533-01JDEMOAPPLY000000000000B",
    applyId: "01JDEMOAPPLY000000000000B",
    time: "2026-11-21T09:15:33Z",
    undoOfRestore: false,
    files: ["~/.omp/agent/mcp.json", "~/.claude.json", "<P>/.mcp.json"],
  },
];

// --- Settings -----------------------------------------------------------------

const REMOTES: RemotesOverview = {
  remotes: [
    {
      name: "sync",
      kind: "folder",
      location: "~/Sync/loadout",
      mode: "repo_per_machine",
      auth: null,
      username: null,
      push: "on-commit",
      pull: "interval:15m",
      tokenStored: null,
      syncTool: "syncthing",
      status: { reachable: true, pushedAt: "2026-11-21T16:42:12Z", pulledAt: "2026-11-21T16:30:00Z", ahead: 0, behind: 1, lastError: null },
    },
    {
      name: "origin",
      kind: "git",
      location: "https://github.com/example/loadout-store.git",
      mode: null,
      auth: "gh",
      username: null,
      push: "manual",
      pull: "manual",
      tokenStored: null,
      syncTool: null,
      status: {
        reachable: null,
        pushedAt: "2026-11-19T08:00:00Z",
        pulledAt: null,
        ahead: 4,
        behind: null,
        lastError: { error: problem("git_error", "remote rejected: fetch first"), at: "2026-11-20T12:00:00Z" },
      },
    },
  ],
  offerBackupRemote: false,
  gitFound: true,
  ghFound: true,
  autoTagRetentionDays: 90,
};

const ALIASES: AliasEntry[] = [
  { alias: "GITHUB_TOKEN", reference: "keyring://loadout/GITHUB_TOKEN", source: "store", storeReference: null, description: "Fine-grained token, repo read", envName: "GITHUB_TOKEN", servers: ["github"] },
  { alias: "SENTRY_TOKEN", reference: "op://Work/Sentry/token", source: "store", storeReference: null, description: null, envName: "SENTRY_TOKEN", servers: ["sentry"] },
  { alias: "GATEWAY_KEY", reference: "env:GATEWAY_KEY", source: "machine", storeReference: "op://Work/Gateway/key", description: null, envName: "GATEWAY_KEY", servers: [] },
];

// What the Migrate from SkillDock wizard finds: migrated, review and skipped
// items, plus secrets to extract.
const MIGRATION: MigrationReport = {
  source: "~/.skilldock",
  servers: ["context7", "notes"],
  skills: ["repo-notes"],
  counts: { migrated: 2, needsReview: 1, skipped: 1 },
  files: ["servers/context7.toml", "servers/notes.toml", "skills/repo-notes/SKILL.md"],
  extractions: [
    { server: "notes", field: "env.NOTES_API_KEY", alias: "NOTES_API_KEY", reason: "key_name", suggestedRef: "keyring://loadout/NOTES_API_KEY" },
    { server: "issues", field: "headers.Authorization", alias: "ISSUES_AUTHORIZATION", reason: "key_name", suggestedRef: "keyring://loadout/ISSUES_AUTHORIZATION" },
  ],
  items: [
    { kind: "server", name: "context7", status: "migrated", detail: "unwrapped from mcp-filter", code: null, id: "context7", offers: [{ kind: "replace_wrapper", client: "codex", name: "context7" }] },
    { kind: "skill", name: "repo-notes", status: "migrated", detail: "copied", code: null, id: "repo-notes", offers: [] },
    { kind: "server", name: "broken", status: "needs_review", detail: "no command", code: "missing_field", id: null, offers: [] },
    { kind: "plugin", name: "~/.skilldock/plugins", status: "skipped", detail: "plugins are not migrated", code: null, id: null, offers: [] },
  ],
};

// --- The dataset --------------------------------------------------------------

const notMocked = (what: string) => mockIpcError({ code: "not_found", message: `${what} is not in the demo dataset` });

/** The demo dataset `npm run dev:mock` loads by default (`?dataset=demo`). */
export function buildDemoDataset(): MockTransportOptions {
  return {
    projects: PROJECTS,
    commands: {
      listServers: async () => ok({ servers: SERVERS, clients: CLIENTS, warnings: [] }),
      getServer: async (id) => {
        const stored = SERVER_DETAILS[id];
        if (stored) {
          return ok(stored);
        }
        const row = SERVERS.find((candidate) => candidate.id === id);
        if (!row) {
          return notMocked(`server ${id}`);
        }
        const isUrl = row.transport !== "stdio";
        const [command = null, ...args] = isUrl ? [] : row.launch.split(" ");
        const detail: ServerDetail = {
          draft: {
            id: row.id,
            displayName: row.displayName,
            description: row.description,
            transport: row.transport ?? "stdio",
            command,
            args,
            url: isUrl ? row.launch : null,
            cwd: null,
            env: [],
            headers: [],
            clients: row.clients,
            delivery: null,
            toolsAllow: [],
            toolsDeny: [],
          },
          invalid: null,
          toml: `id = "${row.id}"\ntransport = "${row.transport}"\n`,
        };
        return ok(detail);
      },
      planSkilldockMigration: async () => ok(MIGRATION),
      pendingPlan: async () => ok(PENDING_PLAN),
      pendingStoreChanges: async () => ok([{ entity: "server/context7", kind: "changed" }]),
      listAppliedRuns: async () => ok(APPLIED_RUNS),
      listTools: async () => ok(TOOLS),
      reviewProject: async (id) => (TRUST_REVIEWS[id] ? ok(TRUST_REVIEWS[id]) : notMocked(`project ${id}`)),
      listProjectServers: async (id) => (PROJECT_SERVERS[id] ? ok(PROJECT_SERVERS[id]) : notMocked(`project ${id}`)),
      effectiveMatrix: async (id) => {
        const found = matrix(id);
        return found ? ok(found) : notMocked(`project ${id}`);
      },
      listHistory: async (filter) =>
        ok(
          HISTORY.filter((row) =>
            filter.scope === "all" ? true : filter.scope === "projects" ? row.scope?.startsWith("project/") : !row.scope?.startsWith("project/"),
          ),
        ),
      commitDetail: async (id) => {
        const found = commitDetail(id);
        return found ? ok(found) : notMocked(`commit ${id}`);
      },
      listTags: async () => ok(TAGS),
      listFileBackups: async () => ok(BACKUPS),
      remotesOverview: async () => ok(REMOTES),
      secretsOverview: async () =>
        ok({ aliases: ALIASES, undefined: ["LINEAR_API_KEY"], environment: null, opAccount: "work.1password.com", serviceAccountSet: false }),
      listSkills: async (scope) => ok(skillList(scope)),
      listSkillToolDirs: async () => ok(SKILL_TOOL_DIRS),
      listSkillFiles: async () =>
        ok([
          { path: "SKILL.md", bytes: 3_412 },
          { path: "references/checklist.md", bytes: 1_870 },
          { path: "scripts/diff-stats.py", bytes: 2_096 },
        ]),
      readSkillFile: async (_id, file) =>
        ok({
          path: file,
          text: "---\nname: code-review\ndescription: Review a diff for correctness, security and style.\n---\n\n# Code review\n\n1. Read the diff.\n2. Check the tests.\n",
          bytes: 3_412,
        }),
      checkSkillUpdates: async () =>
        ok([{ id: "code-review", status: "available", current: "4c1d9e2", latest: "9a7f310", changes: [{ path: "SKILL.md", kind: "modified" }], error: null }]),
      restorePreview: async (rev) =>
        ok({
          target: rev,
          shortId: rev.slice(0, 7),
          added: [],
          changed: ["server/github", "server/context7"],
          removed: ["skill/frontend-design"],
          files: [],
          sharedManifests: [],
          unplaced: [],
        }),
      runApply: async () => ok(MOCK_CHANGE),
    },
  };
}
