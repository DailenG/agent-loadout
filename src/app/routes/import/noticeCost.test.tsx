// Perf assertion for the not-managed scans (#9 review): how many full scans
// a store change and a window focus cost on the MCP page, with the notice
// on and off. The counts must not creep back up.

import { act, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { STORE_COALESCE_MS } from "@/app/apply/changeNotice";
import { useDriftTriggers } from "@/app/apply/driftTriggers";
import { McpPage } from "@/app/routes/mcp/McpPage";
import type { ServerList, ServerRow } from "@/ipc/bindings";
import { MOCK_CLIENTS, MOCK_TOOLS, ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { NOTICE_STORAGE_KEY } from "./unmanaged";
import { UnmanagedNotice } from "./UnmanagedNotice";

function row(id: string): ServerRow {
  return {
    id,
    displayName: null,
    description: null,
    transport: "stdio",
    launch: `${id}-server`,
    origin: null,
    tags: [],
    clients: { allDetected: false, clients: ["claude-code"] },
    pills: [
      { client: "claude-code", selected: true, state: "enabled", reason: null },
      { client: "codex", selected: false, state: "disabled", reason: null },
    ],
    problems: [],
    invalid: null,
  };
}

const LIST: ServerList = { clients: MOCK_CLIENTS, warnings: [], servers: [row("alpha"), row("beta")] };

const SCANS = ["unmanagedSummary", "newUnmanaged", "scanUnmanagedSkills"] as const;

/** The shell's sidebar notice and drift triggers next to the MCP page. */
function Shell() {
  useDriftTriggers();
  return (
    <>
      <UnmanagedNotice collapsed={false} onReview={vi.fn()} />
      <McpPage onOpenPending={vi.fn()} onOpenInstall={vi.fn()} onOpenImport={vi.fn()} />
    </>
  );
}

async function renderShell() {
  const view = renderWithProviders(<Shell />, {
    commands: { listServers: async () => ok(LIST), listTools: async () => ok(MOCK_TOOLS) },
  });
  await screen.findByText("alpha");
  await waitFor(() => expect(view.transport.calls).toContain("unmanagedSummary"));
  const counts = () =>
    Object.fromEntries(SCANS.map((command) => [command, view.transport.calls.filter((call) => call === command).length]));
  return { ...view, counts };
}

/** Two pill toggles in quick succession: two store changes. */
async function toggleTwoPills(user: Awaited<ReturnType<typeof renderShell>>["user"]) {
  for (const id of ["alpha", "beta"]) {
    await user.click(screen.getByRole("button", { name: `Expand ${id}` }));
    await user.click(await screen.findByRole("button", { name: "Codex: disabled" }));
  }
}

describe("not-managed scan cost", () => {
  it("with the notice on: two store changes scan once, a focus within the minute not at all", async () => {
    const { user, transport, counts } = await renderShell();
    await waitFor(() => expect(transport.calls).toContain("scanUnmanagedSkills"));
    const before = counts();
    expect(before).toEqual({ unmanagedSummary: 1, newUnmanaged: 0, scanUnmanagedSkills: 1 });

    await toggleTwoPills(user);
    await waitFor(() => expect(transport.calls.filter((call) => call === "setServerClients")).toHaveLength(2));
    await new Promise((resolve) => setTimeout(resolve, STORE_COALESCE_MS + 300));
    expect(counts()).toEqual({ unmanagedSummary: 2, newUnmanaged: 0, scanUnmanagedSkills: 1 });

    act(() => transport.focus());
    act(() => transport.focus());
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(counts()).toEqual({ unmanagedSummary: 2, newUnmanaged: 0, scanUnmanagedSkills: 1 });
  });

  it("with the notice off: the page's own scope rescans per focus, nothing else", async () => {
    window.localStorage.setItem(NOTICE_STORAGE_KEY, "off");
    const { transport, counts } = await renderShell();
    expect(counts()).toEqual({ unmanagedSummary: 1, newUnmanaged: 0, scanUnmanagedSkills: 0 });
    expect(transport.invocations.find((call) => call.command === "unmanagedSummary")?.args[0]).toEqual({ kind: "user" });

    act(() => transport.focus());
    await waitFor(() => expect(counts().unmanagedSummary).toBe(2));
    expect(counts()).toEqual({ unmanagedSummary: 2, newUnmanaged: 0, scanUnmanagedSkills: 0 });
  });
});
