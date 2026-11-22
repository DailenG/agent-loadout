// Explicit mock transport (SPEC 4.3, 19 item 8): typed against the generated
// bindings, used only by tests and by `npm run dev:mock`. Values are
// synthetic; nothing here reads the real machine. Every command answers with
// a fixed default; a test overrides the commands it cares about through
// `commands`.

import type {
  AppInfo,
  ApplyRun,
  ChangeResult,
  ClientInfo,
  DataHome,
  IpcError,
  PendingPlan,
  ProviderEntry,
  RemotesOverview,
  Scope,
  ServerList,
  SkillChange,
  SkillClient,
  SkillInstalled,
  SkillList,
  ToolInfo,
} from "@/ipc/bindings";
import type { CommandResult, Commands, Transport } from "@/ipc/transport";

export type MockTransportOptions = {
  appInfo?: AppInfo;
  scopes?: Scope[];
  dataHome?: CommandResult<DataHome>;
  isMaximized?: boolean;
  /** Replaces the default answer of these commands. */
  commands?: Partial<Commands>;
};

export type MockInvocation = {
  command: keyof Commands;
  args: unknown[];
};

export type MockTransport = Transport & {
  /** Command accessors called, in order. */
  readonly calls: (keyof Commands)[];
  /** Commands called with their arguments, in order. */
  readonly invocations: MockInvocation[];
  /** Window controls called, in order. */
  readonly windowCalls: string[];
  /** Fires the resize listeners, as a real window resize would. */
  resize(): void;
};

export const MOCK_APP_INFO: AppInfo = { version: "0.0.0-mock", platform: "linux" };

// Paths arrive under their placeholder roots, as the backend sends them.
export const MOCK_DATA_HOME: DataHome = { path: "%LOCALAPPDATA%/Loadout", source: "env_var" };

export const MOCK_CLIENTS: ClientInfo[] = [
  { id: "claude-code", name: "Claude Code", detected: true, supported: true },
  { id: "codex", name: "Codex", detected: true, supported: true },
  { id: "cursor", name: "Cursor", detected: false, supported: true },
  { id: "claude-desktop", name: "Claude Desktop", detected: false, supported: false },
];

export const MOCK_SERVER_LIST: ServerList = { servers: [], clients: MOCK_CLIENTS, warnings: [] };

export const MOCK_APPLY_RUN: ApplyRun = {
  applyId: "01MOCKAPPLY0000000000000000",
  outcome: "ok",
  status: "done",
  recorded: true,
  filesWritten: 1,
  unresolved: 0,
  files: [],
  warnings: [],
};

export const MOCK_CHANGE: ChangeResult = { commit: "0000000000000000000000000000000000000001", apply: MOCK_APPLY_RUN };

export const MOCK_PENDING_PLAN: PendingPlan = {
  applyMode: "instant",
  files: [],
  pending: 0,
  writes: 0,
  unresolved: 0,
  warnings: [],
};

export const MOCK_TOOLS: ToolInfo[] = [
  {
    id: "claude-code",
    name: "Claude Code",
    tier: "a",
    detected: true,
    supported: true,
    readOnly: false,
    userFiles: [{ path: "~/.claude.json", exists: true }],
    projectFiles: ["<P>/.mcp.json"],
    transports: { stdio: true, http: true, sse: true },
    envRefs: true,
    restartRequired: false,
    warnings: [],
  },
];

/** No remote and the first run's step answered: Settings shows "No backup remote". */
export const MOCK_REMOTES: RemotesOverview = {
  remotes: [],
  offerBackupRemote: false,
  gitFound: true,
  ghFound: false,
  autoTagRetentionDays: 90,
};

export const MOCK_PROVIDERS: ProviderEntry[] = [
  { kind: "env", state: "available", version: null, backend: null, features: [] },
  { kind: "keyring", state: "available", version: null, backend: "Mock keyring", features: [] },
  {
    kind: "onepassword",
    state: "unavailable",
    version: null,
    backend: null,
    features: [
      { feature: "item_read", available: false },
      { feature: "environments", available: false },
      { feature: "environment_listing", available: false },
    ],
  },
];

export function ok<T>(data: T): CommandResult<T> {
  return { status: "ok", data };
}

export function mockIpcError(error: IpcError): CommandResult<never> {
  return { status: "error", error };
}

/** Six clients with skill folders (SPEC 7.6), Claude Code and Codex detected. */
export const MOCK_SKILL_CLIENTS: SkillClient[] = [
  { id: "claude-code", name: "Claude Code", detected: true },
  { id: "omp", name: "omp", detected: false },
  { id: "codex", name: "Codex", detected: true },
  { id: "pi", name: "Pi", detected: false },
  { id: "copilot", name: "GitHub Copilot", detected: false },
  { id: "cursor", name: "Cursor", detected: false },
];

export const MOCK_SKILL_LIST: SkillList = {
  skills: [],
  clients: MOCK_SKILL_CLIENTS,
  project: null,
  agentsCli: null,
  sizeWarning: null,
};

export const MOCK_SKILL_CHANGE: SkillChange = {
  commit: MOCK_CHANGE.commit,
  links: { notes: [], warnings: [], needsAttention: false },
};

export const MOCK_SKILL_INSTALLED: SkillInstalled = {
  id: "repo-notes",
  name: "repo-notes",
  commit: MOCK_CHANGE.commit,
  sourceCommit: null,
  renamedFrom: null,
  links: null,
  warnings: [],
};

export function createMockTransport(options: MockTransportOptions = {}): MockTransport {
  const calls: (keyof Commands)[] = [];
  const invocations: MockInvocation[] = [];
  const windowCalls: string[] = [];
  const resizeListeners = new Set<() => void>();
  let isMaximized = options.isMaximized ?? false;

  const defaults: Commands = {
    appInfo: async () => options.appInfo ?? MOCK_APP_INFO,
    listScopes: async () => options.scopes ?? [{ kind: "system" }],
    dataHome: async () => options.dataHome ?? ok(MOCK_DATA_HOME),
    listServers: async () => ok(MOCK_SERVER_LIST),
    getServer: async (id) => mockIpcError({ code: "not_found", message: `server ${id} does not exist` }),
    saveServer: async () => ok(MOCK_CHANGE),
    saveServerToml: async () => ok(MOCK_CHANGE),
    serverDraftToToml: async (_draft, text) => ok(text),
    serverTomlToDraft: async () => mockIpcError({ code: "parse_error", message: "not mocked" }),
    removeServers: async () => ok(MOCK_CHANGE),
    setServerClients: async () => ok(MOCK_CHANGE),
    previewServer: async () => ok([]),
    convertSnippet: async () => ok([]),
    listSecretAliases: async () => ok([]),
    pendingPlan: async () => ok(MOCK_PENDING_PLAN),
    runApply: async () => ok(MOCK_CHANGE),
    listAppliedRuns: async () => ok([]),
    undoApply: async (applyId) =>
      ok({ applyId, restored: [], tag: "auto/pre-rollback/mock", commit: null, apply: MOCK_APPLY_RUN }),
    listTools: async () => ok(MOCK_TOOLS),
    revealToolFolder: async () => ok(null),
    listProjects: async () => ok([]),
    pickProjectFolder: async () => ok(null),
    addProject: async () => ok({ id: "mock-a1b2", trust: "untrusted", commit: MOCK_CHANGE.commit, apply: null }),
    initProject: async () => ok({ id: "mock-a1b2", trust: "trusted", ...MOCK_CHANGE }),
    reviewProject: async (id) => mockIpcError({ code: "not_found", message: `project ${id} is not mocked` }),
    trustProject: async (id) => ok({ id, trust: "trusted", ...MOCK_CHANGE }),
    removeProject: async () => ok(MOCK_CHANGE),
    applyProjectFiles: async () => ok(MOCK_CHANGE),
    revealProjectFolder: async () => ok(null),
    effectiveMatrix: async (id) => mockIpcError({ code: "not_found", message: `project ${id} is not mocked` }),
    resolverDetail: async () => mockIpcError({ code: "not_found", message: "entry not mocked" }),
    resolveEntry: async () => ok(MOCK_CHANGE),
    listHistory: async () => ok([]),
    commitDetail: async (id) => mockIpcError({ code: "not_found", message: `commit ${id} is not mocked` }),
    listTags: async () => ok([]),
    createTag: async (name, commit) =>
      ok({ name, kind: "user", commit: commit ?? MOCK_CHANGE.commit!, message: null, time: null }),
    deleteTag: async () => ok(null),
    restorePreview: async (rev) =>
      ok({ target: rev, shortId: rev.slice(0, 7), added: [], changed: [], removed: [], files: [], sharedManifests: [], unplaced: [] }),
    restoreStoreVersion: async () =>
      ok({ tag: "auto/pre-restore/mock", commit: MOCK_CHANGE.commit, files: [], manifests: [] }),
    listFileBackups: async () => ok([]),
    restoreClientFiles: async () => ok([]),
    listMergeConflicts: async () => ok([]),
    saveVersionZip: async () => ok(false),
    remotesOverview: async () => ok(MOCK_REMOTES),
    dismissBackupRemoteOffer: async () => ok(null),
    pickRemoteFolder: async () => ok(null),
    addFolderRemote: async () => mockIpcError({ code: "not_found", message: "not mocked" }),
    addGitRemote: async () => mockIpcError({ code: "not_found", message: "not mocked" }),
    removeRemote: async (name) => ok({ name, tokenDeleted: false }),
    setRemoteToken: async () => ok(null),
    clearRemoteToken: async () => ok(false),
    pushRemotes: async () => ok([]),
    pullRemotes: async () => ok([]),
    listRestoreMachines: async () => ok({ direct: false, machines: [], artifactsIgnored: 0 }),
    restoreFromRemote: async () => mockIpcError({ code: "not_found", message: "not mocked" }),
    secretProviders: async () => ok(MOCK_PROVIDERS),
    secretsOverview: async () =>
      ok({ aliases: [], undefined: [], environment: null, opAccount: null, serviceAccountSet: false }),
    checkSecrets: async () => ok([]),
    setSecretReference: async () => ok(MOCK_CHANGE),
    setSecretKeyringValue: async () => ok(MOCK_CHANGE),
    clearSecretKeyringValue: async () => ok(false),
    setOpServiceAccount: async () => ok(null),
    clearOpServiceAccount: async () => ok(false),
    listOpEnvironment: async () => ok({ names: null }),
    mapOpEnvironment: async () => ok({ mapped: [], kept: [], replaced: null, change: MOCK_CHANGE }),
    setOpAccount: async () => ok(MOCK_CHANGE),
    listSkills: async () => ok(MOCK_SKILL_LIST),
    listSkillToolDirs: async () => ok([]),
    pickSkillSource: async () => ok(null),
    installLocalSkill: async () => ok(MOCK_SKILL_INSTALLED),
    installGitSkill: async () => ok(MOCK_SKILL_INSTALLED),
    setSkillClients: async () => ok(MOCK_SKILL_CHANGE),
    setSkillMethod: async () => ok(MOCK_SKILL_CHANGE),
    planSkilldockMigration: async () => mockIpcError({ code: "not_found", message: "no ~/.skilldock" }),
    applySkilldockMigration: async () => mockIpcError({ code: "not_found", message: "no ~/.skilldock" }),
    replaceSkilldockWrapper: async () => ok(MOCK_CHANGE),
    setProjectSkill: async () => ok(MOCK_SKILL_CHANGE),
    removeSkill: async () => ok(MOCK_SKILL_CHANGE),
    setSkillTags: async () => ok(MOCK_CHANGE.commit),
    checkSkillUpdates: async () => ok([]),
    skillUpdateDiff: async (_id, file) => ok({ path: file, before: "", after: "", binary: false }),
    updateSkill: async (id) => ok({ id, from: null, to: "0000000", commit: MOCK_CHANGE.commit, changes: [], links: MOCK_SKILL_CHANGE.links }),
    listSkillFiles: async () => ok([]),
    readSkillFile: async (_id, file) => ok({ path: file, text: "", bytes: 0 }),
    revealSkillFolder: async () => ok(null),
    revealSkillToolFolder: async () => ok(null),
    importToolSkill: async () => ok(MOCK_SKILL_INSTALLED),
    trashToolSkill: async () => ok("%LOCALAPPDATA%/Loadout/backups/trash/mock"),
    vendorAgentsSkill: async () => ok(MOCK_SKILL_INSTALLED),
    runAgentsSkillCli: async () => ok(""),
    // No updater by default: the mock build is like a package-manager build.
    updaterStatus: async () => ok({ available: false, enabled: true }),
    setCheckForUpdates: async (enabled) => ok({ available: false, enabled }),
    checkForUpdate: async () => mockIpcError({ code: "update_failed", message: "no updater in the mock" }),
    installUpdate: async () => mockIpcError({ code: "update_failed", message: "no updater in the mock" }),
  };
  const merged = { ...defaults, ...options.commands } as Commands;

  // Records every call, then answers with the default or the override.
  const commands = Object.fromEntries(
    Object.entries(merged).map(([name, handler]) => [
      name,
      (...args: unknown[]) => {
        calls.push(name as keyof Commands);
        invocations.push({ command: name as keyof Commands, args });
        return (handler as (...rest: unknown[]) => unknown)(...args);
      },
    ]),
  ) as Commands;

  return {
    calls,
    invocations,
    windowCalls,
    commands,
    resize() {
      resizeListeners.forEach((listener) => listener());
    },
    window: {
      minimize: async () => {
        windowCalls.push("minimize");
      },
      toggleMaximize: async () => {
        windowCalls.push("toggleMaximize");
        isMaximized = !isMaximized;
      },
      close: async () => {
        windowCalls.push("close");
      },
      isMaximized: async () => isMaximized,
      onResized: async (handler) => {
        resizeListeners.add(handler);
        return () => {
          resizeListeners.delete(handler);
        };
      },
    },
  };
}
