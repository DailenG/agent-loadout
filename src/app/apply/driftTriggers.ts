// Drift detection triggers (SPEC 8.5): the backend's file watcher, window
// focus and the Refresh buttons recheck what the client files hold now. Each
// refetches the views drift shows up in; the planner finds the drift, and
// nothing is reverted. Refresh (and focus every few minutes) also asks the
// backend to forget its cached plan and write probes first, so a permission
// change that left the content alone shows up (#23).

import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useCallback, useEffect } from "react";
import { importQueryKeys, NOTICE_STALE_MS, storeQueryKeys } from "@/app/apply/changeNotice";
import { useTransport } from "@/ipc/transport";

/** How old the last backend recheck may be before a window focus runs one again. */
export const FOCUS_RECHECK_MS = 5 * 60 * 1000;

/** When the backend last rechecked, per app (query client): Refresh and focus share it. */
const lastRechecks = new WeakMap<QueryClient, number>();

/**
 * Asks the backend to drop its cached plan and write probes (`recheck_targets`),
 * so the next fetches plan again and probe every file's permissions afresh.
 * A failure only means the cached state stays; the refetch still runs.
 */
export function useRecheckTargets() {
  const { commands } = useTransport();
  const queryClient = useQueryClient();
  return useCallback(async () => {
    lastRechecks.set(queryClient, Date.now());
    try {
      await commands.recheckTargets();
    } catch {
      // Not reachable: the refetch shows what the backend has.
    }
  }, [commands, queryClient]);
}

/** Query keys of the views that read client files, manifests or skill folders. */
export const driftQueryKeys = [
  storeQueryKeys.pendingPlan,
  storeQueryKeys.servers,
  storeQueryKeys.projectServers,
  storeQueryKeys.storeChanges,
  storeQueryKeys.projects,
  storeQueryKeys.effective,
  storeQueryKeys.tools,
  storeQueryKeys.skills,
  storeQueryKeys.skillToolDirs,
  importQueryKeys.unmanaged,
] as const;

/**
 * The notice's full scans (every client file, every skill folder): a watcher
 * event or window focus refetches them only when older than
 * `NOTICE_STALE_MS` (their stale time), so at most once a minute; a Refresh
 * button always does.
 */
export const throttledDriftQueryKeys = [importQueryKeys.unmanagedAll, importQueryKeys.unmanagedSkills] as const;

/**
 * Refetches every view drift shows up in. `throttled` (watcher, focus):
 * the notice's full scans only when stale; otherwise (Refresh) all of them,
 * after the backend's recheck.
 */
export function useRefreshDrift() {
  const queryClient = useQueryClient();
  const recheckTargets = useRecheckTargets();
  return useCallback(
    async (throttled = false) => {
      if (!throttled) {
        await recheckTargets();
      }
      await Promise.all([
        ...driftQueryKeys.map((queryKey) => queryClient.invalidateQueries({ queryKey })),
        ...throttledDriftQueryKeys.map((queryKey) =>
          throttled
            ? // By the data's age (`Date.now()`), not the observers' stale timers.
              queryClient.refetchQueries({
                queryKey,
                type: "active",
                predicate: (query) => query.isStaleByTime(NOTICE_STALE_MS),
              })
            : queryClient.invalidateQueries({ queryKey }),
        ),
      ]);
    },
    [queryClient, recheckTargets],
  );
}

/**
 * Rechecks on every watcher event and each time the window gains focus;
 * a focus more than `FOCUS_RECHECK_MS` after the last backend recheck runs
 * one first. TanStack's own focus refetch listens for `visibilitychange`,
 * which a desktop window switching focus never fires, so the shell listens
 * to the window instead.
 */
export function useDriftTriggers() {
  const { window: appWindow, events } = useTransport();
  const queryClient = useQueryClient();
  const refresh = useRefreshDrift();
  const recheckTargets = useRecheckTargets();

  useEffect(() => {
    let disposed = false;
    const unsubscribers: (() => void)[] = [];
    if (!lastRechecks.has(queryClient)) {
      // The backend planned afresh at start.
      lastRechecks.set(queryClient, Date.now());
    }
    const onFocus = async () => {
      if (Date.now() - (lastRechecks.get(queryClient) ?? 0) >= FOCUS_RECHECK_MS) {
        await recheckTargets();
      }
      await refresh(true);
    };
    const subscriptions: [(handler: () => void) => Promise<() => void>, () => void][] = [
      [events.onTargetsChanged, () => void refresh(true)],
      [appWindow.onFocused, () => void onFocus()],
    ];
    for (const [subscribe, handler] of subscriptions) {
      void subscribe(handler).then((unsubscribe) => {
        if (disposed) {
          unsubscribe();
        } else {
          unsubscribers.push(unsubscribe);
        }
      });
    }
    return () => {
      disposed = true;
      unsubscribers.forEach((unsubscribe) => unsubscribe());
    };
  }, [appWindow, events, queryClient, refresh, recheckTargets]);
}
