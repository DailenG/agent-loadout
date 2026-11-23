import { useCallback, useLayoutEffect, useRef } from "react";

/**
 * A callback with a stable identity that always runs the latest `fn`. Memoized
 * cards take it, so a page render does not re-render every card.
 */
export function useEventCallback<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => R {
  const latest = useRef(fn);
  useLayoutEffect(() => {
    latest.current = fn;
  });
  return useCallback((...args: A) => latest.current(...args), []);
}
