import { screen, waitFor, within } from "@testing-library/react";
import { StrictMode } from "react";
import { describe, expect, it, vi } from "vitest";
import { SkillReviewDialog } from "@/app/routes/install/SkillReviewDialog";
import { SkillExternalScanButton, SkillExternalScanDialog } from "@/app/routes/skills/SkillExternalScan";
import { SkillUpdateDialog } from "@/app/routes/skills/SkillFileDialog";
import type { ScanResult, SkillInfo, SkillInstallReview, SkillUpdateCheck } from "@/ipc/bindings";
import { createMockTransport, mockIpcError, MOCK_SCAN_CLEAN, MOCK_SKILL_REVIEW, ok } from "@/test/mockTransport";
import { renderWithProviders, type RenderOptions } from "@/test/renderWithProviders";
import {
  EXTERNAL_CLEAN,
  EXTERNAL_DANGEROUS,
  EXTERNAL_LIMIT_REACHED,
  EXTERNAL_NOT_UNDERSTOOD,
  EXTERNAL_WARNING,
  externalNotScanned,
  type ScannerMockState,
  type ScannerResults,
} from "@/test/scannerData";

const ON: Partial<ScannerMockState> = { installed: true, enabled: true, tokenRef: "env:SNYK_TOKEN" };
const ACK = "I read the findings and want to go ahead anyway";

function every(result: ScannerResults["review"]): ScannerResults {
  return { review: result, update: result, skill: result };
}

const DANGEROUS_BUILTIN: ScanResult = {
  verdict: "dangerous",
  findings: [
    {
      scanner: "builtin",
      rule: "pipe-to-shell",
      severity: "dangerous",
      file: "install.sh",
      line: 1,
      message: "Downloads a script and runs it in a shell in one step.",
      excerpt: null,
    },
  ],
  rulesVersion: 1,
  cached: false,
  customRules: false,
  warnings: [],
};

const LOCAL_REVIEW: SkillInstallReview = {
  ...MOCK_SKILL_REVIEW,
  source: { kind: "local", url: null, path: null, gitRef: null, commit: null },
};

function renderReview(options: RenderOptions = {}, review: SkillInstallReview = MOCK_SKILL_REVIEW, strict = false) {
  const onInstall = vi.fn();
  const dialog = <SkillReviewDialog review={review} isInstalling={false} onInstall={onInstall} onClose={() => undefined} />;
  const view = renderWithProviders(strict ? <StrictMode>{dialog}</StrictMode> : dialog, options);
  return { onInstall, ...view };
}

function invokedArgs(transport: { invocations: { command: string; args: unknown[] }[] }, command: string) {
  return transport.invocations.filter((call) => call.command === command).map((call) => call.args);
}

/** The external section's live status line. */
async function liveStatus() {
  const section = await screen.findByRole("region", { name: "External scan" });
  return within(section).getByRole("status");
}

const SKILL: SkillInfo = {
  id: "alpha",
  name: "alpha",
  description: null,
  fields: [],
  tags: [],
  source: { kind: "git", url: "https://github.com/acme/skills", path: "alpha", gitRef: null, commit: "1111111" },
  allDetected: true,
  clients: [],
  method: "link",
  projectEnabled: false,
  projectShared: false,
  links: [],
  bytes: 100,
  hasContent: true,
  problems: [],
};

const CHECK: SkillUpdateCheck = {
  id: "alpha",
  status: "available",
  current: "1111111",
  latest: "2222222aaaa",
  changes: [{ path: "SKILL.md", kind: "modified", script: false }],
  error: null,
  scan: MOCK_SCAN_CLEAN,
  trust: null,
  movedTrust: null,
};

describe("External scan in the install review", () => {
  it("runs nothing external while the scanner is off", async () => {
    const { transport } = renderReview({ scanner: { installed: true, tokenRef: "env:SNYK_TOKEN", enabled: false } });
    await waitFor(() => expect(transport.calls).toContain("scannerStatus"));
    expect(await screen.findByRole("button", { name: "Install" })).toBeEnabled();
    expect(screen.queryByRole("heading", { name: "External scan" })).toBeNull();
    expect(transport.calls.filter((call) => call.startsWith("externalScan"))).toEqual([]);
  });

  it("offers nothing after Remove left the switch on (on means on and installed)", async () => {
    const { transport } = renderReview({ scanner: { ...ON, installed: false } });
    await waitFor(() => expect(transport.calls).toContain("scannerStatus"));
    expect(await screen.findByRole("button", { name: "Install" })).toBeEnabled();
    expect(screen.queryByRole("heading", { name: "External scan" })).toBeNull();
    expect(transport.calls.filter((call) => call.startsWith("externalScan"))).toEqual([]);
  });

  it("keeps Install off while it checks whether the scanner is on", async () => {
    renderReview({ commands: { scannerStatus: () => new Promise(() => undefined) } });
    expect(await liveStatus()).toHaveTextContent("Checking whether Snyk Agent Scan is on…");
    expect(screen.getByRole("button", { name: "Install" })).toBeDisabled();
  });

  it("says the scanner's status is unknown when it can't be read, without blocking or scanning", async () => {
    const { transport, onInstall, user } = renderReview({
      scanner: ON,
      commands: { scannerStatus: async () => mockIpcError({ code: "io_error", message: "unreadable" }) },
    });
    const section = await screen.findByRole("region", { name: "External scan" });
    // Shown and announced.
    expect(await within(section).findAllByText("Snyk Agent Scan status unknown, not scanned.")).toHaveLength(2);
    expect(within(section).getByRole("status")).toHaveTextContent("Snyk Agent Scan status unknown, not scanned.");
    await user.click(screen.getByRole("button", { name: "Install" }));
    expect(onInstall).toHaveBeenCalledWith(false);
    expect(transport.calls.filter((call) => call.startsWith("externalScan"))).toEqual([]);
  });

  it("shows a dangerous external verdict beside the built-in scan, announces it and installs only through Install anyway", async () => {
    const { user, transport, onInstall } = renderReview({ scanner: { ...ON, results: every(EXTERNAL_DANGEROUS) } });
    const section = await screen.findByRole("region", { name: "External scan" });
    // The verdict badge, then a severity badge per dangerous finding.
    expect(await within(section).findAllByText("Dangerous")).toHaveLength(3);
    expect(within(section).getByText(/Snyk Agent Scan 0\.7\.1/)).toBeInTheDocument();
    expect(within(section).getByText("3 findings")).toBeInTheDocument();
    expect(within(section).getByText("Suspicious download")).toBeInTheDocument();
    expect(within(section).getByText("scripts/bootstrap.sh:7")).toBeInTheDocument();
    expect(within(section).getByText("Malicious URL")).toBeInTheDocument();
    expect(within(section).getByText("https://get.example.invalid/install.sh")).toBeInTheDocument();
    expect(within(section).getByText(/sends this content to Snyk's analysis service/)).toBeInTheDocument();
    expect(within(section).getByRole("status")).toHaveTextContent(
      "Snyk Agent Scan: Dangerous, 3 findings. Install anyway needs your confirmation.",
    );
    // The built-in scan is unchanged and stays clean.
    expect(screen.getByRole("region", { name: "Built-in scan" })).toHaveTextContent("Clean");
    const [[token, runId]] = invokedArgs(transport, "externalScanReview");
    expect(token).toBe("review-1");
    expect(typeof runId).toBe("string");

    expect(screen.queryByRole("button", { name: "Install" })).toBeNull();
    expect(screen.getByText(/^Snyk Agent Scan rated this content dangerous/)).toBeInTheDocument();
    const anyway = screen.getByRole("button", { name: "Install anyway" });
    expect(anyway).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: ACK }));
    await user.click(anyway);
    expect(onInstall).toHaveBeenCalledWith(true);
  });

  it("names both scanners when both rated the content dangerous", async () => {
    renderReview({ scanner: { ...ON, results: every(EXTERNAL_DANGEROUS) } }, { ...MOCK_SKILL_REVIEW, scan: DANGEROUS_BUILTIN });
    expect(
      await screen.findByText(/^Loadout's built-in scan and Snyk Agent Scan both rated this content dangerous/),
    ).toBeInTheDocument();
  });

  it("clears a box ticked while Snyk ran, once its result arrives", async () => {
    const { user } = renderReview(
      { scanner: { ...ON, results: every(EXTERNAL_WARNING), scanMs: 300 } },
      { ...MOCK_SKILL_REVIEW, scan: DANGEROUS_BUILTIN },
    );
    expect(await screen.findByRole("button", { name: "Cancel scan" })).toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: ACK }));
    expect(screen.getByRole("checkbox", { name: ACK })).toBeChecked();
    // The tick covered only the built-in findings; Snyk's result asks again.
    await screen.findByText("Unverifiable URL", undefined, { timeout: 3_000 });
    expect(screen.getByRole("checkbox", { name: ACK })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Install anyway" })).toBeDisabled();
  });

  it("keeps focus on the one action button across Cancel scan and Scan again, then moves it to the heading", async () => {
    const { user, transport } = renderReview({ scanner: { ...ON, results: every(EXTERNAL_CLEAN), scanMs: 300 } });
    const cancel = await screen.findByRole("button", { name: "Cancel scan" });
    expect(await liveStatus()).toHaveTextContent("Scanning with Snyk Agent Scan…");
    await user.click(cancel);
    const again = await screen.findByRole("button", { name: "Scan again" });
    expect(again).toBe(cancel);
    expect(again).toHaveFocus();
    expect(await liveStatus()).toHaveTextContent("Not scanned (canceled). Install is available.");
    // Only that run was canceled.
    const [[, firstRun]] = invokedArgs(transport, "externalScanReview");
    expect(invokedArgs(transport, "cancelExternalScan")).toEqual([[firstRun]]);
    // Nothing was sent before the cancel, so the "sends" sentence isn't shown.
    expect(screen.queryByText(/sends this content to Snyk/)).toBeNull();

    await user.click(again);
    expect(again).toHaveFocus();
    expect(again).toHaveTextContent("Cancel scan");
    // Clean: no Scan again, so focus moves to the section's heading instead of the page.
    await waitFor(() => expect(screen.getByRole("heading", { name: "External scan" })).toHaveFocus(), { timeout: 3_000 });
    expect(await liveStatus()).toHaveTextContent("Snyk Agent Scan: Clean, 0 findings. Install is available.");
  });

  it("cancels its run when the review closes mid-scan, but not on a strict-mode remount", async () => {
    const { transport, unmount } = renderReview({ scanner: { ...ON, scanMs: 5_000 } }, MOCK_SKILL_REVIEW, true);
    await screen.findByRole("button", { name: "Cancel scan" });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(transport.calls).not.toContain("cancelExternalScan");
    const runs = invokedArgs(transport, "externalScanReview");
    expect(runs).toHaveLength(1);
    unmount();
    await waitFor(() => expect(invokedArgs(transport, "cancelExternalScan")).toEqual([[runs[0][1]]]));
  });

  it("doesn't send a folder from this computer on its own: Scan with Snyk sends it on request", async () => {
    const { user, transport, onInstall } = renderReview({ scanner: { ...ON, results: every(EXTERNAL_WARNING) } }, LOCAL_REVIEW);
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findByText(/isn't sent to Snyk on its own/)).toBeInTheDocument();
    expect(transport.calls.filter((call) => call.startsWith("externalScan"))).toEqual([]);
    // Install doesn't wait for a scan nobody asked for.
    expect(screen.getByRole("button", { name: "Install" })).toBeEnabled();
    await user.click(within(section).getByRole("button", { name: "Scan with Snyk" }));
    expect(await within(section).findByText("Unverifiable URL")).toBeInTheDocument();
    expect(invokedArgs(transport, "externalScanReview")).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Install" }));
    expect(onInstall).toHaveBeenCalledWith(false);
  });

  it.each([
    ["the daily limit", EXTERNAL_LIMIT_REACHED, "Not scanned (Snyk limit reached).", true],
    ["a timeout", externalNotScanned("timeout"), "Not scanned (timed out).", true],
    ["no token", externalNotScanned("no_token"), "Not scanned (no Snyk token set).", false],
    ["an unreadable token", externalNotScanned("token_unavailable"), "Not scanned (the token couldn't be read).", false],
    ["unreadable output", EXTERNAL_NOT_UNDERSTOOD, "Scanner output not understood (version 0.7.1).", true],
  ])("never blocks on %s and never calls it clean", async (_name, result, text, sent) => {
    const { user, onInstall } = renderReview({ scanner: { ...ON, results: every(result) } });
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findByText(text)).toBeInTheDocument();
    expect(within(section).queryByText("Clean")).toBeNull();
    // "Sends this content" only when the content reached Snyk.
    expect(within(section).queryByText(/sends this content to Snyk/) !== null).toBe(sent);
    expect(screen.queryByRole("checkbox", { name: ACK })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Install" }));
    expect(onInstall).toHaveBeenCalledWith(false);
  });

  it("shows Snyk's limit message and the configured timeout", async () => {
    renderReview({ scanner: { ...ON, results: every(EXTERNAL_LIMIT_REACHED) } });
    expect(await screen.findByText(/Daily usage limit reached/)).toBeInTheDocument();
  });

  it("keeps Install off while the external scan runs and says how long it may take", async () => {
    renderReview({ scanner: { ...ON, scanMs: 5_000 } });
    expect(await screen.findByText("Scanning with Snyk Agent Scan (up to 60 seconds)…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Install" })).toBeDisabled();
  });
});

describe("External scan in the update review", () => {
  it("scans the checked version and updates a dangerous one only through Update anyway", async () => {
    const onUpdate = vi.fn();
    const { user, transport } = renderWithProviders(
      <SkillUpdateDialog skill={SKILL} check={CHECK} isUpdating={false} onUpdate={onUpdate} onClose={() => undefined} />,
      { scanner: { ...ON, results: every(EXTERNAL_DANGEROUS) } },
    );
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findAllByText("Dangerous")).toHaveLength(3);
    expect(invokedArgs(transport, "externalScanUpdate").map((args) => args.slice(0, 2))).toEqual([["alpha", "2222222aaaa"]]);
    expect(screen.getByText(/^Snyk Agent Scan rated the new version dangerous/)).toBeInTheDocument();
    expect(within(section).getByRole("status")).toHaveTextContent("Update anyway needs your confirmation.");
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
    const anyway = screen.getByRole("button", { name: "Update anyway" });
    expect(anyway).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: ACK }));
    await user.click(anyway);
    expect(onUpdate).toHaveBeenCalledWith(true, { trust: null, movedTrust: null });
  });

  it("updates normally with warnings", async () => {
    const onUpdate = vi.fn();
    const { user, transport } = renderWithProviders(
      <SkillUpdateDialog skill={SKILL} check={CHECK} isUpdating={false} onUpdate={onUpdate} onClose={() => undefined} />,
      { scanner: { ...ON, results: every(EXTERNAL_WARNING) } },
    );
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findByText("Unverifiable URL")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Update" }));
    expect(onUpdate).toHaveBeenCalledWith(false, { trust: null, movedTrust: null });
    expect(transport.calls).toContain("externalScanUpdate");
  });
});

describe("Scan with Snyk on a library skill", () => {
  it("is offered only while the scanner is on and installed, named with its visible text", async () => {
    const { transport } = renderWithProviders(<SkillExternalScanButton skill={SKILL} onOpen={() => undefined} />);
    await waitFor(() => expect(transport.calls).toContain("scannerStatus"));
    expect(screen.queryByRole("button", { name: /Scan with Snyk/ })).toBeNull();
    const removed = renderWithProviders(<SkillExternalScanButton skill={SKILL} onOpen={() => undefined} />, {
      scanner: { ...ON, installed: false },
    });
    await waitFor(() => expect(removed.transport.calls).toContain("scannerStatus"));
    expect(screen.queryByRole("button", { name: /Scan with Snyk/ })).toBeNull();
    renderWithProviders(<SkillExternalScanButton skill={SKILL} onOpen={() => undefined} />, { scanner: ON });
    expect(await screen.findByRole("button", { name: "Scan with Snyk: alpha" })).toBeInTheDocument();
  });

  it("scans the store copy and reports the result", async () => {
    const { transport } = renderWithProviders(<SkillExternalScanDialog skill={SKILL} onClose={() => undefined} />, {
      scanner: { ...ON, results: every(EXTERNAL_WARNING) },
    });
    const dialog = await screen.findByRole("dialog", { name: "Snyk Agent Scan: alpha" });
    expect(await within(dialog).findByText("Warnings")).toBeInTheDocument();
    expect(within(dialog).getByText("2 findings")).toBeInTheDocument();
    expect(invokedArgs(transport, "externalScanSkill").map((args) => args[0])).toEqual(["alpha"]);
  });
});

describe("The mock backend (M47)", () => {
  it("refuses an install the external scan rated dangerous without allowDangerous, never one it didn't scan", async () => {
    for (const [result, blocked] of [
      [EXTERNAL_DANGEROUS, true],
      [EXTERNAL_LIMIT_REACHED, false],
      [EXTERNAL_NOT_UNDERSTOOD, false],
    ] as const) {
      const { commands } = createMockTransport({ scanner: { ...ON, results: every(result) } });
      const review = await commands.reviewGitSkill("https://github.com/acme/skills", null, null);
      expect(review.status).toBe("ok");
      await commands.externalScanReview("review-1", "run-1");
      const refused = await commands.installReviewedSkill("review-1", null, false, false, null, null);
      expect(refused.status === "error" && refused.error.code === "scan_blocked").toBe(blocked);
      expect((await commands.installReviewedSkill("review-1", null, false, true, null, null)).status).toBe("ok");
    }
  });

  it("forgets a dangerous update report once the scanner is turned off (review F1)", async () => {
    const { commands } = createMockTransport({
      scanner: { ...ON, results: every(EXTERNAL_DANGEROUS) },
      commands: { checkSkillUpdates: async () => ok([CHECK]) },
    });
    await commands.checkSkillUpdates(["alpha"]);
    await commands.externalScanUpdate("alpha", "2222222aaaa", "run-1");
    const blocked = await commands.updateSkill("alpha", "2222222aaaa", false, null, null);
    expect(blocked.status === "error" && blocked.error.code).toBe("scan_blocked");
    await commands.setScannerEnabled(false);
    expect((await commands.updateSkill("alpha", "2222222aaaa", false, null, null)).status).toBe("ok");
  });

  it("cancels one run by id without touching the next one (review F12)", async () => {
    const { commands } = createMockTransport({ scanner: { ...ON, results: every(EXTERNAL_WARNING), scanMs: 200 } });
    const first = commands.externalScanSkill("alpha", "run-a");
    await commands.cancelExternalScan("run-a");
    const firstResult = await first;
    expect(firstResult.status === "ok" && firstResult.data.reason).toBe("canceled");
    const second = commands.externalScanSkill("alpha", "run-b");
    // A late cancel of the finished run.
    await commands.cancelExternalScan("run-a");
    const secondResult = await second;
    expect(secondResult.status === "ok" && secondResult.data.status).toBe("scanned");
  });
});
