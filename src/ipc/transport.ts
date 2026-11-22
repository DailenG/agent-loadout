// Typed IPC transport (SPEC 4.3). The app talks to the backend only through
// a `Transport`: the Tauri one in the app, the explicit mock
// (`src/test/mockTransport.ts`) in tests and in `npm run dev:mock`. There is
// no silent fixture fallback: outside Tauri, without the mock, calls fail.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { createContext, useContext } from "react";
import { commands, events, type ErrorCode, type IpcError } from "@/ipc/bindings";

/** The generated command accessors; a mock must match them exactly. */
export type Commands = typeof commands;

/** Window chrome actions for the custom Windows title bar (SPEC 13.1). */
export type WindowControls = {
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
  close(): Promise<void>;
  isMaximized(): Promise<boolean>;
  /** Calls `handler` after every resize; resolves to the unsubscribe. */
  onResized(handler: () => void): Promise<() => void>;
  /** Calls `handler` each time the window gains focus; resolves to the unsubscribe. */
  onFocused(handler: () => void): Promise<() => void>;
};

/** What the backend reports on its own (typed events). */
export type TransportEvents = {
  /**
   * Calls `handler` when the drift watcher saw a client file, manifest or
   * skill folder change (SPEC 8.5); resolves to the unsubscribe.
   */
  onTargetsChanged(handler: () => void): Promise<() => void>;
};

export type Transport = {
  commands: Commands;
  window: WindowControls;
  events: TransportEvents;
};

export const tauriTransport: Transport = {
  commands,
  window: {
    minimize: () => getCurrentWindow().minimize(),
    toggleMaximize: () => getCurrentWindow().toggleMaximize(),
    close: () => getCurrentWindow().close(),
    isMaximized: () => getCurrentWindow().isMaximized(),
    onResized: (handler) => getCurrentWindow().onResized(() => handler()),
    onFocused: (handler) =>
      getCurrentWindow().onFocusChanged(({ payload: focused }) => {
        if (focused) {
          handler();
        }
      }),
  },
  events: {
    onTargetsChanged: (handler) => events.targetsChanged.listen(() => handler()),
  },
};

/** A command's typed failure, thrown so TanStack Query sees an error. */
export class IpcFailure extends Error {
  readonly code: ErrorCode;

  constructor(error: IpcError) {
    super(error.message);
    this.name = "IpcFailure";
    this.code = error.code;
  }
}

export type CommandResult<T> =
  | { status: "ok"; data: T }
  | { status: "error"; error: IpcError };

/** The data of a fallible command, or an [`IpcFailure`] thrown. */
export function unwrap<T>(result: CommandResult<T>): T {
  if (result.status === "error") {
    throw new IpcFailure(result.error);
  }
  return result.data;
}

export const TransportContext = createContext<Transport | null>(null);

export function useTransport(): Transport {
  const transport = useContext(TransportContext);
  if (!transport) {
    throw new Error("useTransport must be used inside TransportContext");
  }
  return transport;
}
