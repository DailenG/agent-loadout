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
