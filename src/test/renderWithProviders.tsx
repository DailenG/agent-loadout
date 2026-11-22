// Renders a page or component with the app's providers over a mock
// transport: query client, notifications, tooltips and a header toolbar
// host (pages portal their toolbar into it).

import { QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ReactElement } from "react";
import { createQueryClient } from "@/app/App";
import { AppTooltip } from "@/app/components/AppTooltip";
import { HeaderToolbarProvider } from "@/app/components/HeaderSlots";
import { NotificationProvider } from "@/app/components/notifications";
import { TransportContext } from "@/ipc/transport";
import { createMockTransport, type MockTransportOptions } from "@/test/mockTransport";

function ToolbarHost({ children }: { children: ReactElement }) {
  const [host, setHost] = useState<HTMLElement | null>(null);
  return (
    <>
      <div data-testid="header-toolbar" ref={setHost} />
      <HeaderToolbarProvider value={host}>{children}</HeaderToolbarProvider>
    </>
  );
}

export function renderWithProviders(ui: ReactElement, options: MockTransportOptions = {}) {
  const transport = createMockTransport(options);
  const queryClient = createQueryClient();
  const user = userEvent.setup();
  const view = render(
    <TransportContext.Provider value={transport}>
      <QueryClientProvider client={queryClient}>
        <NotificationProvider>
          <ToolbarHost>{ui}</ToolbarHost>
          <AppTooltip />
        </NotificationProvider>
      </QueryClientProvider>
    </TransportContext.Provider>,
  );
  return { transport, queryClient, user, ...view };
}
