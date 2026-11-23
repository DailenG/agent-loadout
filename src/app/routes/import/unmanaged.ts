// The not-managed entries (#3, #9) as the app reads them: the notice's
// on/off preference and the `unmanaged_summary` queries. With the notice on,
// one cached summary of every scope (`importQueryKeys.unmanagedAll`) feeds
// the notice and the MCP and Tools lists, so a trigger scans once; with it
// off, the lists read their own scope.

import { useQuery } from "@tanstack/react-query";
import { useCallback, useMemo, useSyncExternalStore } from "react";
import { importQueryKeys } from "@/app/apply/changeNotice";
import { BIN_NAME, type ImportScope, type UnmanagedGroup, type UnmanagedSummary } from "@/ipc/bindings";
import { unwrap, useTransport, type Commands } from "@/ipc/transport";

export const NOTICE_STORAGE_KEY = `${BIN_NAME}.settings.unmanagedNotice`;

const listeners = new Set<() => void>();

function readEnabled(): boolean {
  return window.localStorage.getItem(NOTICE_STORAGE_KEY) !== "off";
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The notice preference (on by default), shared by Settings, the sidebar and the lists. */
export function useUnmanagedNoticeEnabled(): [boolean, (enabled: boolean) => void] {
  const enabled = useSyncExternalStore(subscribe, readEnabled);
  const setEnabled = useCallback((next: boolean) => {
    if (next) {
      window.localStorage.removeItem(NOTICE_STORAGE_KEY);
    } else {
      window.localStorage.setItem(NOTICE_STORAGE_KEY, "off");
    }
    listeners.forEach((listener) => listener());
  }, []);
  return [enabled, setEnabled];
}

/** `unmanaged_summary({kind:"all"})`, shared by the notice and the lists. */
export function unmanagedAllQuery(commands: Commands) {
  return {
    queryKey: importQueryKeys.unmanagedAll,
    queryFn: async () => unwrap(await commands.unmanagedSummary({ kind: "all" })),
    // Also the notice's scan, rechecked on focus: a failure stays quiet and the next trigger retries.
    meta: { inlineError: true },
  };
}

function inScope(group: UnmanagedGroup, scope: ImportScope): boolean {
  switch (scope.kind) {
    case "all":
      return true;
    case "user":
      return group.scope === "user";
    case "projects":
      return group.scope !== "user";
    case "project":
      return group.projectId === scope.id;
  }
}

/** The part of an every-scope summary one scope would have returned. */
export function summaryFor(all: UnmanagedSummary, scope: ImportScope): UnmanagedSummary {
  const groups = all.groups.filter((group) => inScope(group, scope));
  const entries = groups.flatMap((group) => group.entries);
  return { groups, total: entries.length, new: entries.filter((entry) => entry.new).length };
}

/**
 * The not-managed entries of one scope: cut from the shared every-scope
 * summary while the notice is on, else `unmanaged_summary(scope)`.
 */
export function useUnmanagedSummary(scope: ImportScope) {
  const { commands } = useTransport();
  const [noticeOn] = useUnmanagedNoticeEnabled();
  const all = useQuery({ ...unmanagedAllQuery(commands), enabled: noticeOn });
  const scoped = useQuery({
    queryKey: [...importQueryKeys.unmanaged, scope.kind, scope.kind === "project" ? scope.id : ""],
    queryFn: async () => unwrap(await commands.unmanagedSummary(scope)),
    enabled: !noticeOn,
  });
  const scopeKey = scope.kind === "project" ? `project:${scope.id}` : scope.kind;
  // `scope` is a fresh object each render; `scopeKey` names it.
  const data = useMemo(
    () => (noticeOn ? (all.data ? summaryFor(all.data, scope) : undefined) : scoped.data),
    [noticeOn, all.data, scoped.data, scopeKey],
  );
  return { data };
}
