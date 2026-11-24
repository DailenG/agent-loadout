import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { McpCheckoutReview, McpUpdateCheck } from "@/ipc/bindings";
import { MOCK_TRUST_ASK, mockIpcError, ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { McpSourceUpdate } from "./McpSourceUpdate";

const CHECK: McpUpdateCheck = {
  id: "weather",
  status: "available",
  current: "3f2a9c1",
  latest: "e7a9f13c0000",
  changes: [{ field: "args", old: "-y @acme/weather-mcp@1.2.0", new: "-y @acme/weather-mcp@1.3.0" }],
  files: [{ path: "bin/start.sh", kind: "modified", script: true }],
  launch: "npx -y @acme/weather-mcp@1.3.0",
  launchLines: [
    { os: null, line: "npx -y @acme/weather-mcp@1.3.0" },
    { os: "windows", line: "npx -y @acme/weather-mcp@1.3.0 --log-file ~/weather.log" },
  ],
  runsRepoCode: true,
  setup: ["docker build -t loadout-local/weather:0123456789ab ~/.loadout/servers/weather"],
  setupAgain: true,
  checkoutOutOfDate: true,
  scan: { verdict: "clean", findings: [], rulesVersion: 1, cached: false, customRules: false, warnings: [] },
  trust: {
    ...MOCK_TRUST_ASK,
    state: { kind: "ask", reason: "lapsed", lapse: { reason: "moved", at: "2026-11-22T08:00:00Z", detail: "github.com/new/weather" } },
  },
  movedTrust: null,
  error: null,
};

describe("McpSourceUpdate", () => {
  it("checks, reviews the field changes and updates with the trust answer", async () => {
    const { user, transport } = renderWithProviders(<McpSourceUpdate serverId="weather" name="Weather" />, {
      commands: { checkMcpSourceUpdate: async () => ok(CHECK) },
    });
    await user.click(screen.getByRole("button", { name: "Check for update" }));
    expect(await screen.findByText(/An update is available at e7a9f13\./)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restore copy" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Review update" }));
    const dialog = await screen.findByRole("dialog", { name: "Update Weather" });
    const row = within(dialog).getByRole("row", { name: /args/ });
    expect(within(row).getByText("-y @acme/weather-mcp@1.2.0")).toBeInTheDocument();
    expect(within(row).getByText("-y @acme/weather-mcp@1.3.0")).toBeInTheDocument();
    expect(within(dialog).getByText(/runs code from this repository every time a client starts it/)).toBeInTheDocument();
    expect(within(dialog).getByText("Script")).toBeInTheDocument();
    expect(within(dialog).getByText("The repository moved to github.com/new/weather since the last fetch.")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Update" }));
    const trust = await screen.findByRole("alertdialog", { name: "Trust this source?" });
    await user.click(within(trust).getByRole("button", { name: "Trust once and continue" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({
        command: "updateMcpSourceServer",
        args: ["weather", "e7a9f13c0000", { kind: "once" }, null, false],
      }),
    );
  });

  it("announces each check in one live region that is there from the start", async () => {
    const { user } = renderWithProviders(<McpSourceUpdate serverId="weather" name="Weather" />, {
      commands: { checkMcpSourceUpdate: async () => ok({ ...CHECK, status: "up_to_date", checkoutMissing: false }) },
    });
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent(/came from a Git repository or folder/);
    await user.click(screen.getByRole("button", { name: "Check for update" }));
    await waitFor(() => expect(status).toHaveTextContent("Up to date with its source."));
    expect(screen.getAllByRole("status")).toEqual([status]);
  });

  it("asks about the redirect target too and sends both answers", async () => {
    const moved = { ...MOCK_TRUST_ASK, display: "github.com/new/weather", scopes: [{ scope: "repo" as const, display: "github.com/new/weather" }] };
    const { user, transport } = renderWithProviders(<McpSourceUpdate serverId="weather" name="Weather" />, {
      commands: { checkMcpSourceUpdate: async () => ok({ ...CHECK, movedTrust: moved }) },
    });
    await user.click(screen.getByRole("button", { name: "Check for update" }));
    await user.click(await screen.findByRole("button", { name: "Review update" }));
    const dialog = await screen.findByRole("dialog", { name: "Update Weather" });
    expect(dialog).toHaveFocus();
    await user.click(within(dialog).getByRole("button", { name: "Update" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Trust once and continue" }));
    const second = await screen.findByRole("alertdialog");
    expect(within(second).getByText(/github\.com\/acme\/skills now redirects to github\.com\/new\/weather\./)).toBeInTheDocument();
    await user.click(within(second).getByRole("button", { name: "Trust once and continue" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({
        command: "updateMcpSourceServer",
        args: ["weather", "e7a9f13c0000", { kind: "once" }, { kind: "once" }, false],
      }),
    );
  });

  it("words a dangerous refusal as an update, not an install", async () => {
    const { user } = renderWithProviders(<McpSourceUpdate serverId="weather" name="Weather" />, {
      commands: {
        checkMcpSourceUpdate: async () => ok({ ...CHECK, trust: null }),
        updateMcpSourceServer: async () => mockIpcError({ code: "scan_blocked", message: "dangerous" }),
      },
    });
    await user.click(screen.getByRole("button", { name: "Check for update" }));
    await user.click(await screen.findByRole("button", { name: "Review update" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Update Weather" })).getByRole("button", { name: "Update" }));
    expect(await screen.findByText(/Nothing was updated; review the findings, and update anyway only if you trust it\./)).toBeInTheDocument();
    expect(screen.queryByText(/install anyway/i)).toBeNull();
  });

  it("shows every new launch line and the setup steps to run again", async () => {
    const { user } = renderWithProviders(<McpSourceUpdate serverId="weather" name="Weather" />, {
      commands: { checkMcpSourceUpdate: async () => ok(CHECK) },
    });
    await user.click(screen.getByRole("button", { name: "Check for update" }));
    await user.click(await screen.findByRole("button", { name: "Review update" }));
    const dialog = await screen.findByRole("dialog", { name: "Update Weather" });
    expect(within(dialog).getByText("npx -y @acme/weather-mcp@1.3.0")).toBeInTheDocument();
    expect(within(dialog).getByText("On Windows")).toBeInTheDocument();
    expect(within(dialog).getByText("npx -y @acme/weather-mcp@1.3.0 --log-file ~/weather.log")).toBeInTheDocument();
    expect(within(dialog).getByText("docker build -t loadout-local/weather:0123456789ab ~/.loadout/servers/weather")).toBeInTheDocument();
    expect(within(dialog).getByText(/run the setup steps again after updating/)).toBeInTheDocument();
  });

  it("restores an out-of-date checkout through its review, trust answer and scan gate", async () => {
    const review: McpCheckoutReview = {
      token: "checkout-1",
      id: "weather",
      commit: "e7a9f13c00000000000000000000000000000000",
      trust: MOCK_TRUST_ASK,
      movedTrust: null,
      scan: {
        verdict: "dangerous",
        findings: [
          {
            scanner: "builtin",
            rule: "reverse-shell",
            severity: "dangerous",
            file: "bin/start.sh",
            line: 4,
            message: "Opens a shell for a remote machine.",
            excerpt: null,
          },
        ],
        rulesVersion: 1,
        cached: false,
        customRules: false,
        warnings: [],
      },
    };
    const { user, transport } = renderWithProviders(<McpSourceUpdate serverId="weather" name="Weather" />, {
      commands: { checkMcpSourceUpdate: async () => ok(CHECK), reviewMcpCheckout: async () => ok(review) },
    });
    await user.click(screen.getByRole("button", { name: "Check for update" }));
    expect(await screen.findByText(/missing or out of date/)).toBeInTheDocument();

    // Closed without restoring: the review is discarded.
    await user.click(screen.getByRole("button", { name: "Restore copy" }));
    let dialog = await screen.findByRole("dialog", { name: "Restore the copy of Weather" });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(transport.invocations).toContainEqual({ command: "discardMcpSourceReview", args: ["checkout-1"] });

    await user.click(screen.getByRole("button", { name: "Restore copy" }));
    dialog = await screen.findByRole("dialog", { name: "Restore the copy of Weather" });
    expect(within(dialog).getByText("reverse-shell")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Restore" })).toBeNull();
    await user.click(within(dialog).getByRole("checkbox", { name: "I read the findings and want to go ahead anyway" }));
    await user.click(within(dialog).getByRole("button", { name: "Restore anyway" }));
    // A canceled trust question restores nothing; the review stays open.
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    expect(transport.calls).not.toContain("restoreMcpCheckout");
    await user.click(within(dialog).getByRole("button", { name: "Restore anyway" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Trust once and continue" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({
        command: "restoreMcpCheckout",
        args: ["checkout-1", { kind: "once" }, null, true],
      }),
    );
    expect(await screen.findByText(/Restored the copy of Weather on this computer\./)).toBeInTheDocument();
  });

  it("says when the copy is already up to date", async () => {
    const { user } = renderWithProviders(<McpSourceUpdate serverId="weather" name="Weather" />, {
      commands: { checkMcpSourceUpdate: async () => ok(CHECK) },
    });
    await user.click(screen.getByRole("button", { name: "Check for update" }));
    await user.click(await screen.findByRole("button", { name: "Restore copy" }));
    expect(await screen.findByText("The copy of Weather on this computer is already up to date.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Restore copy" })).toBeNull();
  });
});
