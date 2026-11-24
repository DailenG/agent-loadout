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
  ProjectRow,
  ProviderEntry,
  RemotesOverview,
  ScanResult,
  ServerList,
  SkillAdoptChoice,
  SkillAdoptResult,
  SkillAdoptedFolder,
  SkillChange,
  SkillClient,
  SkillInstalled,
  SkillInstallReview,
  SkillList,
  ToolInfo,
  McpDraft,
  McpSourceReview,
  SourceTrustInfo,
  UnmanagedSkills,
  SkillUpdateCheck,
  McpUpdateCheck,
  TrustGrantArg,
  McpCheckoutReview,
} from "@/ipc/bindings";
import type { CommandResult, Commands, Transport } from "@/ipc/transport";

export type MockTransportOptions = {
  appInfo?: AppInfo;
  /** What `listProjects` answers (default none). */
  projects?: ProjectRow[];
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
  /** Fires the focus listeners, as the window gaining focus would. */
  focus(): void;
  /** Fires the drift watcher's listeners, as an outside file change would. */
  changeTargets(): void;
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
    userFiles: [{ path: "~/.claude.json", exists: true, readOnly: null }],
    projectFiles: ["<P>/.mcp.json"],
    privateProjectTarget: true,
    transports: { stdio: true, http: true, sse: true },
    envRefs: true,
    restartRequired: false,
    warnings: [],
    homepage: null,
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
  { id: "claude-code", name: "Claude Code", detected: true, readOnly: null },
  { id: "omp", name: "omp", detected: false, readOnly: null },
  { id: "codex", name: "Codex", detected: true, readOnly: null },
  { id: "pi", name: "Pi", detected: false, readOnly: null },
  { id: "copilot", name: "GitHub Copilot", detected: false, readOnly: null },
  { id: "cursor", name: "Cursor", detected: false, readOnly: null },
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

/** A scan without findings (M40). */
export const MOCK_SCAN_CLEAN: ScanResult = {
  verdict: "clean",
  findings: [],
  rulesVersion: 1,
  cached: false,
  customRules: false,
  warnings: [],
};

export const MOCK_SKILL_INSTALLED: SkillInstalled = {
  id: "repo-notes",
  name: "repo-notes",
  commit: MOCK_CHANGE.commit,
  sourceCommit: null,
  renamedFrom: null,
  links: null,
  warnings: [],
  scan: MOCK_SCAN_CLEAN,
};

/** What `review_git_skill` / `review_local_skill` answer by default. */
export const MOCK_SKILL_REVIEW: SkillInstallReview = {
  token: "review-1",
  name: "repo-notes",
  files: [{ path: "SKILL.md", bytes: 120 }],
  source: { kind: "git", url: "https://github.com/acme/skills", path: null, gitRef: "main", commit: "3f2a9c1" },
  scan: MOCK_SCAN_CLEAN,
  trust: null,
  movedTrust: null,
};

/** A first-time git source the trust dialog asks about (M39, M42). */
export const MOCK_TRUST_ASK: SourceTrustInfo = {
  display: "github.com/acme/skills",
  official: false,
  scopes: [
    { scope: "repo", display: "github.com/acme/skills" },
    { scope: "owner", display: "github.com/acme/*" },
  ],
  state: { kind: "ask", reason: "first_time", lapse: null },
  installedSkills: [],
  change: null,
  insecure: false,
};

/** One stdio server drafted from a registry `server.json` (M41, M42). */
export const MOCK_MCP_DRAFT: McpDraft = {
  key: "draft-1",
  kind: "registry",
  file: "server.json",
  converter: "registry",
  entry: "packages[0]",
  draft: {
    id: "weather",
    displayName: "Weather",
    description: null,
    transport: "stdio",
    command: "npx",
    args: ["-y", "@acme/weather-mcp@1.2.0"],
    url: null,
    cwd: null,
    env: [{ key: "WEATHER_API_KEY", value: "${secret:WEATHER_API_KEY}" }],
    headers: [],
    clients: { allDetected: false, clients: [] },
    delivery: null,
    toolsAllow: [],
    toolsDeny: [],
  },
  launch: "npx -y @acme/weather-mcp@1.2.0",
  launchLines: [{ os: null, line: "npx -y @acme/weather-mcp@1.2.0" }],
  pinned: true,
  inputs: [
    { field: "env.WEATHER_API_KEY", description: null, secret: true, required: true, alias: "WEATHER_API_KEY", valueDropped: false },
  ],
  runsRepoCode: false,
  checkout: false,
  setup: [],
  alsoIn: [],
  scan: MOCK_SCAN_CLEAN,
};

/** What `review_mcp_git_source` / `review_mcp_local_source` answer by default. */
export const MOCK_MCP_REVIEW: McpSourceReview = {
  token: "mcp-review-1",
  source: { kind: "git", url: "https://github.com/acme/weather-mcp", folder: null, path: null, gitRef: null, commit: "3f2a9c1" },
  drafts: [MOCK_MCP_DRAFT],
  skipped: [],
  trust: null,
  movedTrust: null,
  skippedFolders: [],
  scan: MOCK_SCAN_CLEAN,
};

/**
 * What `adopt_skills` answers for `scan` and `choices`, by the core's rules
 * (M32): a picked content is adopted and other contents differ; "both"
 * adopts every content, the others as `name-<client>`; a skip is skipped;
 * a name with several contents and no choice is a conflict, which (like a
 * kept folder) needs attention. Every adopted folder goes to the trash.
 */
export function mockAdoptResult(scan: UnmanagedSkills, choices: SkillAdoptChoice[]): SkillAdoptResult {
  const folders: SkillAdoptedFolder[] = [];
  const added: string[] = [];
  for (const candidate of scan.candidates) {
    const action = choices.find((choice) => choice.name === candidate.name)?.action ?? null;
    const primary = action?.kind === "pick" ? action.variant : action?.kind === "both" ? action.primary : null;
    for (const variant of candidate.variants) {
      const renamed = action?.kind === "both" && variant.hash !== primary && variant.folders[0]
        ? `${candidate.name}-${variant.folders[0].client}`
        : null;
      const skillId = primary === null ? null : variant.hash === primary ? candidate.name : renamed;
      if (skillId && !variant.inStore && !added.includes(skillId)) {
        added.push(skillId);
      }
      for (const folder of variant.folders) {
        const status: SkillAdoptedFolder["status"] = primary === null
          ? action === null && candidate.conflict ? "conflict" : "skipped"
          : skillId ? "adopted" : "differs";
        folders.push({
          client: folder.client,
          name: candidate.name,
          path: folder.path,
          skillId,
          status,
          trash: status === "adopted" ? `%LOCALAPPDATA%/Loadout/backups/trash/20261123T101500Z-${folder.client}-${candidate.name}` : null,
          error: null,
        });
      }
    }
  }
  return {
    commit: added.length > 0 ? MOCK_CHANGE.commit : null,
    added,
    reused: [],
    folders,
    problems: scan.problems,
    links: MOCK_SKILL_CHANGE.links,
    needsAttention: folders.some((folder) => folder.status === "conflict" || folder.status === "kept"),
  };
}

/** What a review or an update check asks of the install, save or update that follows it. */
type MockGate = { trust: SourceTrustInfo | null; movedTrust: SourceTrustInfo | null; scans: (ScanResult | null)[] };

/** The commands whose answers the mock remembers for the gates. */
const RECORDED: Record<string, true> = {
  reviewGitSkill: true,
  reviewLocalSkill: true,
  reviewMcpGitSource: true,
  reviewMcpLocalSource: true,
  checkSkillUpdates: true,
  checkMcpSourceUpdate: true,
  reviewMcpCheckout: true,
};

/**
 * The backend's gates (M39 trust, then M40 scan): a block refuses, an
 * untrusted source needs its own grant, a dangerous verdict needs
 * `allowDangerous`. Null: let through (also for content no review or check
 * of this transport described).
 */
function gateRefusal(
  gate: MockGate | undefined,
  trust: TrustGrantArg | null,
  movedTrust: TrustGrantArg | null,
  allowDangerous: boolean,
): CommandResult<never> | null {
  if (!gate) {
    return null;
  }
  for (const [info, grant] of [
    [gate.trust, trust],
    [gate.movedTrust, movedTrust],
  ] as const) {
    if (info?.state.kind === "blocked") {
      return mockIpcError({ code: "source_blocked", message: `${info.display} is blocked` });
    }
    if (info?.state.kind === "ask" && grant === null) {
      return mockIpcError({ code: "source_trust_required", message: `${info.display} is not trusted` });
    }
  }
  if (!allowDangerous && gate.scans.some((scan) => scan?.verdict === "dangerous")) {
    return mockIpcError({ code: "scan_blocked", message: "found dangerous findings" });
  }
  return null;
}

export function createMockTransport(options: MockTransportOptions = {}): MockTransport {
  const calls: (keyof Commands)[] = [];
  const invocations: MockInvocation[] = [];
  const windowCalls: string[] = [];
  const resizeListeners = new Set<() => void>();
  const focusListeners = new Set<() => void>();
  const targetListeners = new Set<() => void>();
  const subscribe = (listeners: Set<() => void>) => async (handler: () => void) => {
    listeners.add(handler);
    return () => {
      listeners.delete(handler);
    };
  };
  let isMaximized = options.isMaximized ?? false;
  // What the reviews and update checks this transport answered asked for (whoever answered them: a
  // default or an override), so the default installs, saves and updates refuse like the backend.
  const skillReviews = new Map<string, MockGate>();
  const mcpReviews = new Map<string, McpSourceReview>();
  const skillChecks = new Map<string, MockGate>();
  const mcpChecks = new Map<string, MockGate>();
  const mcpCheckouts = new Map<string, MockGate>();

  const defaults: Commands = {
    appInfo: async () => options.appInfo ?? MOCK_APP_INFO,
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
      ok({
        applyId,
        restored: [],
        tag: "auto/pre-rollback/mock",
        commit: null,
        apply: MOCK_APPLY_RUN,
        unavailableOriginals: [],
      }),
    pendingStoreChanges: async () => ok([]),
    discardPending: async (entities) =>
      ok({ discarded: entities ?? [], commit: MOCK_CHANGE.commit, tag: "auto/pre-restore/mock" }),
    recheckTargets: async () => ok(null),
    listTools: async () => ok(MOCK_TOOLS),
    revealToolFolder: async () => ok(null),
    openToolHomepage: async () => ok(null),
    scanImport: async () => ok({ candidates: [], warnings: [] }),
    applyImport: async () =>
      ok({
        imported: [],
        definitionOnly: [],
        skipped: [],
        secrets: [],
        change: MOCK_CHANGE,
        backupsWithOldValues: [],
        oldValuesInFiles: [],
      }),
    purgeImportBackups: async (applyId) => ok({ applyId, removed: [], notDeleted: [], failed: [] }),
    unmanagedSummary: async () => ok({ groups: [], total: 0, new: 0 }),
    newUnmanaged: async () => ok({ groups: [], total: 0, new: 0 }),
    acknowledgeUnmanaged: async () => ok(0),
    scanUnmanagedSkills: async () => ok({ candidates: [], problems: [] }),
    adoptSkills: async () =>
      ok({
        commit: null,
        added: [],
        reused: [],
        folders: [],
        problems: [],
        links: { notes: [], warnings: [], needsAttention: false },
        needsAttention: false,
      }),
    discoverProjects: async () => ok({ candidates: [], warnings: [] }),
    addDiscoveredProjects: async () => ok({ commit: null, added: [], failed: [] }),
    listProjects: async () => ok(options.projects ?? []),
    pickProjectFolder: async () => ok(null),
    addProject: async () => ok({ id: "mock-a1b2", trust: "untrusted", commit: MOCK_CHANGE.commit, apply: null }),
    initProject: async () => ok({ id: "mock-a1b2", trust: "trusted", ...MOCK_CHANGE }),
    reviewProject: async (id) => mockIpcError({ code: "not_found", message: `project ${id} is not mocked` }),
    trustProject: async (id) => ok({ id, trust: "trusted", ...MOCK_CHANGE }),
    removeProject: async () => ok(MOCK_CHANGE),
    applyProjectFiles: async () => ok(MOCK_CHANGE),
    revealProjectFolder: async () => ok(null),
    listProjectServers: async (id) => mockIpcError({ code: "not_found", message: `project ${id} is not mocked` }),
    getProjectServer: async (_id, name) => mockIpcError({ code: "not_found", message: `server ${name} is not mocked` }),
    saveProjectServer: async () => ok(MOCK_CHANGE),
    removeProjectServer: async () => ok(MOCK_CHANGE),
    setProjectServerClients: async () => ok(MOCK_CHANGE),
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
    reviewGitSkill: async () => ok(MOCK_SKILL_REVIEW),
    reviewLocalSkill: async () => ok({ ...MOCK_SKILL_REVIEW, source: { kind: "local", url: null, path: null, gitRef: null, commit: null } }),
    installReviewedSkill: async (token, _id, _renameIfTaken, allowDangerous, trust, movedTrust) =>
      gateRefusal(skillReviews.get(token), trust, movedTrust, allowDangerous) ?? ok(MOCK_SKILL_INSTALLED),
    discardSkillReview: async () => ok(null),
    listSources: async () => ok([]),
    checkSourceTrust: async () => ok(MOCK_TRUST_ASK),
    revokeSource: async () => ok(MOCK_CHANGE.commit),
    blockSource: async () => ok(MOCK_CHANGE.commit),
    blockSourceUrl: async () => ok(MOCK_CHANGE.commit),
    reviewMcpGitSource: async () => ok(MOCK_MCP_REVIEW),
    reviewMcpLocalSource: async () =>
      ok({ ...MOCK_MCP_REVIEW, source: { kind: "local", url: null, folder: "~/code/weather", path: null, gitRef: null, commit: null } }),
    saveMcpSourceDrafts: async (token, picks, trust, movedTrust, allowDangerous) => {
      const review = mcpReviews.get(token);
      const gate = review && {
        trust: review.trust,
        movedTrust: review.movedTrust,
        scans: [review.scan, ...review.drafts.filter((draft) => picks.some((pick) => pick.key === draft.key)).map((draft) => draft.scan)],
      };
      return (
        gateRefusal(gate, trust, movedTrust, allowDangerous) ??
        ok({ ids: picks.map((pick) => pick.draft.id), change: MOCK_CHANGE, scan: MOCK_SCAN_CLEAN })
      );
    },
    discardMcpSourceReview: async () => ok(null),
    checkMcpSourceUpdate: async (id) =>
      ok({
        id,
        status: "up_to_date",
        current: "3f2a9c1",
        latest: "3f2a9c1",
        changes: [],
        files: [],
        launch: null,
        launchLines: [],
        runsRepoCode: false,
        setup: [],
        setupAgain: false,
        checkoutOutOfDate: false,
        scan: null,
        trust: null,
        movedTrust: null,
        error: null,
      }),
    updateMcpSourceServer: async (id, _latest, trust, movedTrust, allowDangerous) =>
      gateRefusal(mcpChecks.get(id), trust, movedTrust, allowDangerous) ?? ok(MOCK_CHANGE),
    // Already up to date by default; a test that restores answers with a review.
    reviewMcpCheckout: async () => ok(null),
    restoreMcpCheckout: async (token, trust, movedTrust, allowDangerous) =>
      gateRefusal(mcpCheckouts.get(token), trust, movedTrust, allowDangerous) ?? ok(MOCK_CHANGE),
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
    updateSkill: async (id, _latest, allowDangerous, trust, movedTrust) =>
      gateRefusal(skillChecks.get(id), trust, movedTrust, allowDangerous) ??
      ok({ id, from: null, to: "0000000", commit: MOCK_CHANGE.commit, changes: [], links: MOCK_SKILL_CHANGE.links, scan: MOCK_SCAN_CLEAN }),
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
        const answer = (handler as (...rest: unknown[]) => unknown)(...args);
        return name in RECORDED ? Promise.resolve(answer).then((result) => record(name, result)) : answer;
      },
    ]),
  ) as Commands;

  /** Remembers what a review or check asked for; answers with the result unchanged. */
  function record(name: string, result: unknown): unknown {
    const answered = result as CommandResult<unknown>;
    if (answered.status !== "ok") {
      return result;
    }
    if (name === "reviewGitSkill" || name === "reviewLocalSkill") {
      const review = answered.data as SkillInstallReview;
      skillReviews.set(review.token, { trust: review.trust, movedTrust: review.movedTrust, scans: [review.scan] });
    } else if (name === "reviewMcpGitSource" || name === "reviewMcpLocalSource") {
      const review = answered.data as McpSourceReview;
      mcpReviews.set(review.token, review);
    } else if (name === "checkSkillUpdates") {
      for (const check of answered.data as SkillUpdateCheck[]) {
        skillChecks.set(check.id, { trust: check.trust, movedTrust: check.movedTrust, scans: [check.scan] });
      }
    } else if (name === "checkMcpSourceUpdate") {
      const check = answered.data as McpUpdateCheck;
      mcpChecks.set(check.id, { trust: check.trust, movedTrust: check.movedTrust, scans: [check.scan] });
    } else if (name === "reviewMcpCheckout" && answered.data) {
      const review = answered.data as McpCheckoutReview;
      mcpCheckouts.set(review.token, { trust: review.trust, movedTrust: review.movedTrust, scans: [review.scan] });
    }
    return result;
  }

  return {
    calls,
    invocations,
    windowCalls,
    commands,
    resize() {
      resizeListeners.forEach((listener) => listener());
    },
    focus() {
      focusListeners.forEach((listener) => listener());
    },
    changeTargets() {
      targetListeners.forEach((listener) => listener());
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
      onResized: subscribe(resizeListeners),
      onFocused: subscribe(focusListeners),
    },
    events: {
      onTargetsChanged: subscribe(targetListeners),
    },
  };
}
