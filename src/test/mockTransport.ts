// Explicit mock transport (SPEC 4.3, 19 item 8): typed against the generated
// bindings, used only by tests and by `npm run dev:mock`. Values are
// synthetic; nothing here reads the real machine.

import type { AppInfo, DataHome, IpcError, Scope } from "@/ipc/bindings";
import type { CommandResult, Commands, Transport } from "@/ipc/transport";

export type MockTransportOptions = {
  appInfo?: AppInfo;
  scopes?: Scope[];
  dataHome?: CommandResult<DataHome>;
  isMaximized?: boolean;
};

export type MockTransport = Transport & {
  /** Command accessors called, in order. */
  readonly calls: (keyof Commands)[];
  /** Window controls called, in order. */
  readonly windowCalls: string[];
  /** Fires the resize listeners, as a real window resize would. */
  resize(): void;
};

export const MOCK_APP_INFO: AppInfo = { version: "0.0.0-mock", platform: "linux" };

export const MOCK_DATA_HOME: DataHome = { path: "/mock/loadout-home", source: "env_var" };

export function mockIpcError(error: IpcError): CommandResult<never> {
  return { status: "error", error };
}

export function createMockTransport(options: MockTransportOptions = {}): MockTransport {
  const calls: (keyof Commands)[] = [];
  const windowCalls: string[] = [];
  const resizeListeners = new Set<() => void>();
  let isMaximized = options.isMaximized ?? false;

  const commands: Commands = {
    appInfo: async () => {
      calls.push("appInfo");
      return options.appInfo ?? MOCK_APP_INFO;
    },
    listScopes: async () => {
      calls.push("listScopes");
      return options.scopes ?? [{ kind: "system" }];
    },
    dataHome: async () => {
      calls.push("dataHome");
      return options.dataHome ?? { status: "ok", data: MOCK_DATA_HOME };
    },
  };

  return {
    calls,
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
