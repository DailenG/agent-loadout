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
  Scope,
  ServerList,
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

export function ok<T>(data: T): CommandResult<T> {
  return { status: "ok", data };
}

export function mockIpcError(error: IpcError): CommandResult<never> {
  return { status: "error", error };
}

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
