// Header toolbar slot (SPEC 4.3): pages render their toolbar into the shell
// header through React context and a portal, instead of looking up slot
// elements with `document.getElementById`.

import { createContext, useContext, type ReactNode } from "react";
import { createPortal } from "react-dom";

const HeaderToolbarContext = createContext<HTMLElement | null>(null);

export const HeaderToolbarProvider = HeaderToolbarContext.Provider;

/** Renders `children` in the current page's header toolbar. */
export function HeaderToolbar({ children }: { children: ReactNode }) {
  const host = useContext(HeaderToolbarContext);
  return host ? createPortal(children, host) : null;
}

/** The class of a collapsible header action's text label. */
export const HEADER_ACTION_LABEL = "header-action__label";

/**
 * Props for a labeled header action that shows only its icon when the
 * header toolbar is narrow (#13, container query on the toolbar host):
 * `early` actions (Refresh, Install, Remotes) collapse first, `late` ones
 * (Check updates, Paste config, Tag current) in the narrowest headers.
 * The label (`HEADER_ACTION_LABEL`) stays in the DOM, visually hidden, so it
 * remains the accessible name; the tooltip shows it only while it is hidden.
 */
export function collapsibleAction(label: string, stage: "early" | "late") {
  return {
    "data-collapse": stage,
    "data-tooltip": label,
    "data-tooltip-icon-only": "",
  } as const;
}
