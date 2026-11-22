// Drift detection triggers (SPEC 8.5): the backend's file watcher, window
// focus and the Refresh buttons recheck what the client files hold now. Each
// refetches the views drift shows up in; the planner finds the drift, and
// nothing is reverted.

import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { useTransport } from "@/ipc/transport";

/** Query keys of the views that read client files, manifests or skill folders. */
export const driftQueryKeys = [
  storeQueryKeys.pendingPlan,
  storeQueryKeys.servers,
  storeQueryKeys.projects,
  storeQueryKeys.effective,
  storeQueryKeys.tools,
  storeQueryKeys.skills,
  storeQueryKeys.skillToolDirs,
] as const;

/** Refetches every view drift shows up in (manual Refresh). */
export function useRefreshDrift() {
  const queryClient = useQueryClient();
  return useCallback(async () => {
    await Promise.all(
      driftQueryKeys.map((queryKey) => queryClient.invalidateQueries({ queryKey })),
    );
  }, [queryClient]);
}

/**
 * Rechecks on every watcher event and each time the window gains focus.
 * TanStack's own focus refetch listens for `visibilitychange`, which a
 * desktop window switching focus never fires, so the shell listens to the
 * window instead.
 */
export function useDriftTriggers() {
  const { window: appWindow, events } = useTransport();
  const refresh = useRefreshDrift();

  useEffect(() => {
    let disposed = false;
    const unsubscribers: (() => void)[] = [];
    const recheck = () => void refresh();
    for (const subscribe of [events.onTargetsChanged, appWindow.onFocused]) {
      void subscribe(recheck).then((unsubscribe) => {
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
  }, [appWindow, events, refresh]);
}
