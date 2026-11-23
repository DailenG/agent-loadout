// Renders a page or component with the app's providers over a mock
// transport: query client, notifications, tooltips, the active scope and a
// header toolbar host (pages portal their toolbar into it).

import { QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ReactElement } from "react";
import { createQueryClient } from "@/app/App";
import { AppTooltip } from "@/app/components/AppTooltip";
import { HeaderToolbarProvider } from "@/app/components/HeaderSlots";
import { NotificationProvider } from "@/app/components/notifications";
import { ActiveScopeProvider, type ActiveScope } from "@/app/scope/ActiveScope";
import { TransportContext } from "@/ipc/transport";
import { createMockTransport, type MockTransportOptions } from "@/test/mockTransport";

export type RenderOptions = MockTransportOptions & {
  /** The active scope the shell starts with (default User). */
  scope?: ActiveScope;
};

function ToolbarHost({ children }: { children: ReactElement }) {
  const [host, setHost] = useState<HTMLElement | null>(null);
  return (
    <>
      <div data-testid="header-toolbar" ref={setHost} />
      <HeaderToolbarProvider value={host}>{children}</HeaderToolbarProvider>
    </>
  );
}

/**
 * `delay: null`: user-event runs each key and pointer step without a
 * `setTimeout(0)` between them. With the default, every typed character
 * waited for a timer turn, and under the full suite's parallel workers a
 * typed URL could take seconds (the M28c timeouts). Shared by App-level
 * tests that set up user-event themselves.
 */
export const USER_EVENT_OPTIONS = { delay: null } as const;

export function renderWithProviders(ui: ReactElement, options: RenderOptions = {}) {
  const { scope, ...transportOptions } = options;
  const transport = createMockTransport(transportOptions);
  const queryClient = createQueryClient();
  const user = userEvent.setup(USER_EVENT_OPTIONS);
  const view = render(
    <TransportContext.Provider value={transport}>
      <QueryClientProvider client={queryClient}>
        <NotificationProvider>
          <ActiveScopeProvider initial={scope}>
            <ToolbarHost>{ui}</ToolbarHost>
          </ActiveScopeProvider>
          <AppTooltip />
        </NotificationProvider>
      </QueryClientProvider>
    </TransportContext.Provider>,
  );
  return { transport, queryClient, user, ...view };
}
