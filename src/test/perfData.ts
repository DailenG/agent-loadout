// Large synthetic dataset for the performance baseline (#10, docs/perf):
// ~200 MCP servers, ~300 skills, 20 projects, every client Loadout has an
// adapter for, and a long history. `npm run dev:mock` serves it under
// `?dataset=perf`; the perf measurement tests use it directly. Values are
// generated and deterministic; nothing here reads the real machine, and
// paths stay under their placeholder roots.

import type {
  AppliedRun,
  ClientInfo,
  ClientPill,
  EffectiveMatrix,
  HistoryEntry,
  PendingPlan,
  PillState,
  PlannedFile,
  ProjectRow,
  ProjectServerList,
  ServerRow,
  SkillClient,
  SkillInfo,
  TagEntry,
  ToolInfo,
  ToolSkillDir,
} from "@/ipc/bindings";
import type { Commands } from "@/ipc/transport";
import { MOCK_CHANGE, MOCK_SKILL_CHANGE, ok, type MockTransportOptions } from "@/test/mockTransport";

export const PERF_COUNTS = {
  servers: 200,
  skills: 300,
  projects: 20,
  history: 1000,
  tags: 60,
  appliedRuns: 200,
} as const;

/** Every adapter in core table order (`adapters/mod.rs` ADAPTERS, 38). */
const CLIENTS: [id: string, name: string][] = [
  ["claude-code", "Claude Code"],
  ["omp", "oh-my-pi"],
  ["pi", "Pi"],
  ["codex", "Codex"],
  ["claude-desktop", "Claude Desktop"],
  ["copilot", "GitHub Copilot"],
  ["cursor", "Cursor"],
  ["gemini", "Gemini CLI"],
  ["antigravity", "Antigravity"],
  ["opencode", "OpenCode"],
  ["grok", "Grok Build"],
  ["vscode-legacy", "VS Code (legacy mcp.json)"],
  ["windsurf", "Windsurf"],
  ["devin", "Devin Desktop"],
  ["augment", "Augment"],
  ["cline", "Cline"],
  ["roo-code", "Roo Code"],
  ["kilo-code", "Kilo Code"],
  ["kiro", "Kiro"],
  ["junie", "Junie"],
  ["droid", "Droid"],
  ["commandcode", "Command Code"],
  ["crush", "Crush"],
  ["qoder", "Qoder"],
  ["trae", "Trae"],
  ["trae-cn", "Trae CN"],
  ["codebuddy", "CodeBuddy"],
  ["iflow", "iFlow"],
  ["qwen-code", "Qwen Code"],
  ["zencoder", "Zencoder"],
  ["openclaw", "OpenClaw"],
  ["mimo-code", "MiMo Code"],
  ["workbuddy", "WorkBuddy"],
  ["zcode", "ZCode"],
  ["continue", "Continue"],
  ["goose", "Goose"],
  ["hermes", "Hermes Agent"],
  ["intellij", "IntelliJ IDEA"],
];

/** Deterministic: every third client is installed, plus the first four. */
const detected = (index: number) => index < 4 || index % 3 === 0;

const PILL_CYCLE: PillState[] = ["enabled", "enabled", "disabled", "pending", "enabled", "drift", "disabled", "blocked"];

const iso = (minutesAgo: number) => new Date(Date.UTC(2026, 10, 22, 12, 0) - minutesAgo * 60_000).toISOString();
const hex = (n: number, width = 40) => n.toString(16).padStart(width, "0");

function buildClients(): ClientInfo[] {
  return CLIENTS.filter(([id]) => id !== "intellij").map(([id, name], index) => ({
    id,
    name,
    detected: detected(index),
    supported: id !== "claude-desktop" || index % 2 === 0,
  }));
}

function buildServers(clients: ClientInfo[]): ServerRow[] {
  return Array.from({ length: PERF_COUNTS.servers }, (_, i) => {
    const id = `server-${String(i).padStart(3, "0")}`;
    const enabled = clients.filter((_, c) => (i + c) % 4 !== 0).map((c) => c.id);
    const pills: ClientPill[] = clients.map((client, c) => ({
      client: client.id,
      selected: enabled.includes(client.id),
      state: client.supported ? PILL_CYCLE[(i + c) % PILL_CYCLE.length] : "not_supported",
      reason: null,
      readOnly: null,
    }));
    const stdio = i % 3 !== 0;
    return {
      id,
      displayName: i % 2 === 0 ? `Server ${i}` : null,
      description: i % 5 === 0 ? `Synthetic server ${i} for the performance baseline` : null,
      transport: stdio ? "stdio" : "http",
      launch: stdio ? `npx -y @perf/server-${i} --token \${secret:perf_${i % 20}}` : `https://mcp.example.test/${id}`,
      origin: i % 7 === 0 ? "imported:claude-code" : null,
      tags: i % 4 === 0 ? ["perf", `group-${i % 10}`] : [],
      clients: { allDetected: i % 6 === 0, clients: enabled },
      pills,
      problems: i % 25 === 0 ? [{ code: "missing_secret", message: `perf_${i % 20} has no value` }] : [],
      invalid: null,
    };
  });
}

function buildSkillClients(): SkillClient[] {
  return CLIENTS.map(([id, name], index) => ({ id, name, detected: detected(index), readOnly: null }));
}

function buildSkills(clients: SkillClient[]): SkillInfo[] {
  return Array.from({ length: PERF_COUNTS.skills }, (_, i) => {
    const id = `skill-${String(i).padStart(3, "0")}`;
    const enabled = clients.filter((c, n) => c.detected && (i + n) % 3 !== 0).map((c) => c.id);
    return {
      id,
      name: id,
      description: `Synthetic skill ${i}: does one narrow thing for the performance baseline.`,
      fields: [
        { key: "name", value: id },
        { key: "description", value: `Synthetic skill ${i}` },
      ],
      tags: i % 3 === 0 ? ["perf"] : [],
      source:
        i % 2 === 0
          ? { kind: "git", url: `https://example.test/skills-${i % 15}.git`, path: `skills/${id}`, gitRef: "main", commit: hex(i) }
          : { kind: "local", url: null, path: `~/skills/${id}`, gitRef: null, commit: null },
      allDetected: i % 5 === 0,
      clients: enabled,
      method: i % 10 === 0 ? "copy" : "link",
      projectEnabled: false,
      projectShared: false,
      links: enabled.map((client) => ({
        client,
        scope: "user",
        projectId: null,
        path: `~/.${client}/skills/${id}`,
        method: "junction",
      })),
      bytes: 4096 + i * 37,
      hasContent: true,
      problems: [],
    };
  });
}

function buildTools(): ToolInfo[] {
  return CLIENTS.map(([id, name], index) => ({
    id,
    name,
    tier: (["a", "b", "c", "d"] as const)[Math.min(3, Math.floor(index / 10))],
    detected: detected(index),
    supported: id !== "intellij",
    readOnly: id === "vscode-legacy",
    userFiles: [
      { path: `~/.${id}/mcp.json`, exists: detected(index), readOnly: null },
      { path: `~/.config/${id}/settings.json`, exists: false, readOnly: null },
    ],
    projectFiles: [`<P>/.${id}/mcp.json`],
    transports: { stdio: true, http: index % 2 === 0, sse: index % 4 === 0 },
    envRefs: index % 2 === 1,
    restartRequired: index % 3 === 1,
    warnings: [],
    homepage: `https://example.test/${id}`,
  }));
}

function buildProjects(): ProjectRow[] {
  const trust = ["trusted", "trusted", "trusted", "changed", "untrusted"] as const;
  return Array.from({ length: PERF_COUNTS.projects }, (_, i) => ({
    id: `proj-${String(i).padStart(2, "0")}${hex(i, 4)}`,
    name: `project-${i}`,
    path: `~/Code/project-${i}`,
    layout: (["private", "shared", "shared_and_local"] as const)[i % 3],
    trust: trust[i % trust.length],
    error: null,
    servers: 3 + (i % 8),
    clients: 2 + (i % 5),
    pending: i % 4 === 0 ? 2 : 0,
    readOnly: null,
    readOnlyFiles: [],
  }));
}

function buildHistory(projects: ProjectRow[]): HistoryEntry[] {
  return Array.from({ length: PERF_COUNTS.history }, (_, i) => {
    const id = hex(PERF_COUNTS.history - i);
    const project = i % 4 === 0 ? projects[i % projects.length] : null;
    const isApply = i % 5 === 0;
    return {
      id,
      shortId: id.slice(0, 7),
      time: iso(i * 37),
      summary: isApply ? `apply: ${1 + (i % 6)} files` : `server server-${String(i % 200).padStart(3, "0")}: set clients`,
      op: isApply ? "apply" : "set_clients",
      entities: [`server/server-${String(i % 200).padStart(3, "0")}`],
      scope: project ? `project/${project.id}` : "system",
      machine: i % 9 === 0 ? "laptop" : "desktop",
      thisMachine: i % 9 !== 0,
      apply: isApply ? { applyId: `01PERF${hex(i, 20)}`, outcome: "ok", undoable: i < 50, latest: i === 0 } : null,
      tags: i % 50 === 0 ? [`perf-tag-${i / 50}`] : [],
    };
  });
}

function buildTags(): TagEntry[] {
  return Array.from({ length: PERF_COUNTS.tags }, (_, i) => ({
    name: i % 2 === 0 ? `perf-tag-${i}` : `auto/pre-apply/${hex(i, 8)}`,
    kind: i % 2 === 0 ? "user" : "auto",
    commit: hex(PERF_COUNTS.history - i * 10),
    message: null,
    time: iso(i * 370),
  }));
}

function buildAppliedRuns(): AppliedRun[] {
  return Array.from({ length: PERF_COUNTS.appliedRuns }, (_, i) => ({
    applyId: `01PERF${hex(i * 5, 20)}`,
    commit: hex(PERF_COUNTS.history - i * 5),
    time: iso(i * 5 * 37),
    summary: `apply: ${1 + (i % 6)} files`,
    latest: i === 0,
  }));
}

/** The drawer's plan: one pending file per detected client and per project with pending entries. */
function buildPlan(clients: ClientInfo[], servers: ServerRow[], projects: ProjectRow[]): PendingPlan {
  const pendingServers = servers.filter((_, i) => i % 8 === 3).slice(0, 25);
  const diff = (client: string) =>
    pendingServers
      .map((s) => `   "${s.id}": {\n-    "command": "npx",\n+    "command": "npx",\n+    "args": ["-y", "@perf/${s.id}", "--client", "${client}"]\n   },`)
      .join("\n");
  const userFiles: PlannedFile[] = clients
    .filter((c) => c.detected && c.supported)
    .map((client) => ({
      file: `~/.${client.id}/mcp.json`,
      clients: [client.id],
      scope: "user",
      projectId: null,
      project: null,
      counts: { create: pendingServers.length, update: 2, remove: 1, disable: 0 },
      diff: diff(client.id),
      entries: pendingServers.map((s) => ({
        action: "create",
        name: s.id,
        client: client.id,
        server: s.id,
        state: "pending_create",
        unresolved: false,
        problem: null,
      })),
      problem: null,
      warnings: [],
      readOnly: null,
    }));
  const projectFiles: PlannedFile[] = projects
    .filter((p) => p.pending > 0)
    .map((p) => ({
      file: `${p.path}/.mcp.json`,
      clients: ["claude-code"],
      scope: "project",
      projectId: p.id,
      project: p.name,
      counts: { create: 2, update: 0, remove: 0, disable: 0 },
      diff: diff("claude-code"),
      entries: [],
      problem: null,
      warnings: [],
      readOnly: null,
    }));
  const files = [...userFiles, ...projectFiles];
  const pending = files.reduce((sum, f) => sum + f.counts.create + f.counts.update + f.counts.remove, 0);
  return { applyMode: "review", files, pending, writes: files.length, unresolved: 0, warnings: [] };
}

function buildProjectServers(project: ProjectRow, clients: ClientInfo[], servers: ServerRow[]): ProjectServerList {
  const defaults = clients.filter((c) => c.detected).map((c) => c.id);
  return {
    project: {
      id: project.id,
      name: project.name,
      path: project.path,
      trust: project.trust,
      hash: hex(project.servers, 64),
      layers: project.layout === "shared_and_local" ? ["shared", "local"] : ["shared"],
      defaultClients: { allDetected: true, clients: defaults },
      readOnly: null,
      readOnlyLayers: [],
    },
    servers: Array.from({ length: project.servers }, (_, i) => ({
      name: `proj-server-${i}`,
      kind: i % 3 === 0 ? "override" : "define",
      from: i % 3 === 0 ? servers[i].id : null,
      layers: ["shared"],
      editLayer: "shared",
      transport: "stdio",
      launch: `npx -y @perf/project-server-${i}`,
      clients: { allDetected: true, clients: defaults },
      usesDefaults: true,
      pills: clients.map((client, c) => ({
        client: client.id,
        selected: client.detected,
        state: client.supported ? PILL_CYCLE[(i + c) % PILL_CYCLE.length] : "not_supported",
        reason: null,
        readOnly: null,
      })),
      problems: [],
      invalid: null,
    })),
    inherited: servers.slice(0, 40).map((s) => ({ id: s.id, displayName: s.displayName, transport: s.transport, launch: s.launch })),
    clients,
    storeServers: servers.map((s) => s.id),
    warnings: [],
  };
}

function buildMatrix(project: ProjectRow, clients: ClientInfo[], servers: ServerRow[]): EffectiveMatrix {
  const columns = clients.filter((c) => c.detected).map((c) => ({ id: c.id, name: c.name, detected: true }));
  return {
    projectId: project.id,
    projectName: project.name,
    path: project.path,
    clients: columns,
    rows: servers.slice(0, 60).map((s, i) => ({
      name: s.id,
      cells: columns.map((c) => ({
        client: c.id,
        winner: { layer: i % 4 === 0 ? "project" : "system", file: `~/.${c.id}/mcp.json`, readOnly: null },
        source: "loadout",
        loaded: true,
        shadowed: [],
        alsoVisibleTo: [],
        status: "synced",
        warnings: [],
        snippet: null,
      })),
    })),
    warnings: [],
  };
}

function buildToolSkillDirs(clients: SkillClient[], skills: SkillInfo[]): ToolSkillDir[] {
  return clients
    .filter((c) => c.detected)
    .map((c) => ({
      client: c.id,
      name: c.name,
      dir: `~/.${c.id}/skills`,
      exists: true,
      entries: skills.slice(0, 40).map((s, i) => ({
        name: s.id,
        path: `~/.${c.id}/skills/${s.id}`,
        class: i % 10 === 9 ? "unmanaged" : "managed",
        linkTarget: null,
        skillId: i % 10 === 9 ? null : s.id,
        description: s.description,
      })),
      readOnly: null,
    }));
}

/**
 * Mock transport options for the perf dataset. Server pill toggles change
 * the answer of `listServers`, so a toggle re-renders as it would against
 * the real backend.
 */
export function buildPerfDataset(): MockTransportOptions {
  const clients = buildClients();
  const servers = buildServers(clients);
  const skillClients = buildSkillClients();
  const skills = buildSkills(skillClients);
  const tools = buildTools();
  const projects = buildProjects();
  const history = buildHistory(projects);
  const tags = buildTags();
  const appliedRuns = buildAppliedRuns();
  const plan = buildPlan(clients, servers, projects);
  const toolSkillDirs = buildToolSkillDirs(skillClients, skills);
  const byProject = new Map(projects.map((p) => [p.id, p]));
  const notFound = (id: string) => ({ status: "error" as const, error: { code: "not_found" as const, message: `project ${id} is not in the perf dataset` } });

  const commands: Partial<Commands> = {
    listServers: async () => ok({ servers, clients, warnings: [] }),
    setServerClients: async (id, choice) => {
      const index = servers.findIndex((s) => s.id === id);
      if (index >= 0) {
        const row = servers[index];
        servers[index] = {
          ...row,
          clients: choice,
          pills: row.pills.map((pill) => ({
            ...pill,
            selected: choice.clients.includes(pill.client),
            state: pill.state === "not_supported" ? pill.state : "pending",
          })),
        };
      }
      return ok(MOCK_CHANGE);
    },
    pendingPlan: async () => ok(plan),
    pendingStoreChanges: async () => ok(servers.slice(0, 12).map((s) => ({ entity: `server/${s.id}`, kind: "changed" as const }))),
    listAppliedRuns: async () => ok(appliedRuns),
    listTools: async () => ok(tools),
    listProjectServers: async (id) => {
      const project = byProject.get(id);
      return project ? ok(buildProjectServers(project, clients, servers)) : notFound(id);
    },
    effectiveMatrix: async (id) => {
      const project = byProject.get(id);
      return project ? ok(buildMatrix(project, clients, servers)) : notFound(id);
    },
    listHistory: async (filter) => ok(filter.limit == null ? history : history.slice(0, filter.limit)),
    listTags: async () => ok(tags),
    listSkills: async (scope) => {
      const project = scope.kind === "project" ? byProject.get(scope.id) : undefined;
      return ok({
        skills: project ? skills.map((s, i) => ({ ...s, projectEnabled: i % 4 === 0 })) : skills,
        clients: skillClients,
        project: project
          ? { id: project.id, name: project.name, path: project.path, trust: project.trust, hash: hex(1, 64), clients: ["claude-code"], share: false, readOnly: null }
          : null,
        agentsCli: null,
        sizeWarning: null,
      });
    },
    listSkillToolDirs: async () => ok(toolSkillDirs),
    setSkillClients: async () => ok(MOCK_SKILL_CHANGE),
    listSecretAliases: async () => ok(Array.from({ length: 20 }, (_, i) => `perf_${i}`)),
  };

  return { projects, commands };
}
