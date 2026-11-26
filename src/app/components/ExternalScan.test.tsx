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
  EXTERNAL_DO_NOT_INSTALL,
  EXTERNAL_FAILED,
  EXTERNAL_INCOMPLETE,
  EXTERNAL_INCOMPLETE_DANGEROUS,
  EXTERNAL_NOT_UNDERSTOOD,
  EXTERNAL_WARNING,
  externalNotScanned,
  SCANNER_NEEDS_PREPARE,
  SCANNER_ON,
  SCANNER_READY,
  type ScannerMockState,
  type ScannerResults,
} from "@/test/scannerData";

const ACK = "I read the findings and want to go ahead anyway";

function every(result: ScannerResults["review"]): ScannerResults {
  return { review: result, update: result, skill: result };
}

function on(result: ScannerResults["review"], extra: Partial<ScannerMockState> = {}): Partial<ScannerMockState> {
  return { ...SCANNER_ON, results: every(result), ...extra };
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

/** The external scan commands the UI called (Prepare and the switch are Settings' own). */
function scanCalls(transport: { calls: string[] }) {
  return transport.calls.filter((call) => call.startsWith("externalScan") || call === "prepareScanner");
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

describe("SkillSpector in the install review", () => {
  it.each([
    ["off (never set up)", {}],
    ["off while ready", SCANNER_READY],
  ])("runs nothing and shows nothing while it is %s", async (_name, scanner) => {
    const { transport, onInstall, user } = renderReview({ scanner: { ...scanner, results: every(EXTERNAL_DANGEROUS) } });
    await waitFor(() => expect(transport.calls).toContain("scannerStatus"));
    const install = await screen.findByRole("button", { name: "Install" });
    await waitFor(() => expect(install).toBeEnabled());
    expect(screen.queryByRole("region", { name: "External scan" })).toBeNull();
    expect(screen.queryByText(/SkillSpector/)).toBeNull();
    await user.click(install);
    expect(onInstall).toHaveBeenCalledWith(false);
    expect(scanCalls(transport)).toEqual([]);
  });

  it.each([
    ["isn't prepared", SCANNER_NEEDS_PREPARE, "Not scanned (SkillSpector isn't prepared yet; choose Prepare in Settings > Security)."],
    ["isn't available", {}, "Not scanned (SkillSpector isn't available on this computer; see Settings > Security)."],
  ])("runs nothing when it's on but %s, says so and never blocks", async (_name, scanner, text) => {
    const { transport, onInstall, user } = renderReview({
      scanner: { ...scanner, enabled: true, results: every(EXTERNAL_DANGEROUS) },
    });
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findByText(text)).toBeInTheDocument();
    expect(within(section).getByText("Not scanned")).toBeInTheDocument();
    expect(within(section).getByRole("status")).toHaveTextContent(`${text} Install is available.`);
    expect(screen.queryByRole("checkbox", { name: ACK })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Install" }));
    expect(onInstall).toHaveBeenCalledWith(false);
    expect(scanCalls(transport)).toEqual([]);
  });

  it("keeps Install off while it checks whether SkillSpector is on", async () => {
    renderReview({ commands: { scannerStatus: () => new Promise(() => undefined) } });
    expect(await liveStatus()).toHaveTextContent("Checking whether SkillSpector is on…");
    expect(screen.getByRole("button", { name: "Install" })).toBeDisabled();
  });

  it("says the status is unknown when it can't be read, without blocking or scanning", async () => {
    const { transport, onInstall, user } = renderReview({
      scanner: SCANNER_ON,
      commands: { scannerStatus: async () => mockIpcError({ code: "io_error", message: "unreadable" }) },
    });
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findAllByText("SkillSpector status unknown, not scanned.")).toHaveLength(2);
    await user.click(screen.getByRole("button", { name: "Install" }));
    expect(onInstall).toHaveBeenCalledWith(false);
    expect(scanCalls(transport)).toEqual([]);
  });

  it("shows a dangerous verdict with each finding's severity, rule id, file:line, message and excerpt, and installs only through Install anyway", async () => {
    const { user, transport, onInstall } = renderReview({ scanner: on(EXTERNAL_DANGEROUS) });
    const section = await screen.findByRole("region", { name: "External scan" });
    // The verdict badge, then the dangerous finding's severity badge.
    expect(await within(section).findAllByText("Dangerous")).toHaveLength(2);
    expect(within(section).getByText("SkillSpector 2.13.7")).toBeInTheDocument();
    expect(within(section).getByText("2 findings")).toBeInTheDocument();
    expect(within(section).getByText("E2")).toBeInTheDocument();
    expect(within(section).getByText("scripts/sync.py:22")).toBeInTheDocument();
    expect(within(section).getByText("Collects environment variables and sends them to a remote host.")).toBeInTheDocument();
    expect(within(section).getByText("requests.post(URL, json=dict(os.environ))")).toBeInTheDocument();
    expect(within(section).getByText("SC4")).toBeInTheDocument();
    expect(within(section).getByText("requirements.txt:2")).toBeInTheDocument();
    expect(within(section).getByText(/^SkillSpector runs on this computer; only dependency names and versions/)).toBeInTheDocument();
    expect(within(section).getByRole("status")).toHaveTextContent(
      "SkillSpector: Dangerous, 2 findings. Install anyway needs your confirmation.",
    );
    // The built-in scan is unchanged and stays clean.
    expect(screen.getByRole("region", { name: "Built-in scan" })).toHaveTextContent("Clean");
    const [[token, runId]] = invokedArgs(transport, "externalScanReview");
    expect(token).toBe("review-1");
    expect(typeof runId).toBe("string");

    expect(screen.queryByRole("button", { name: "Install" })).toBeNull();
    expect(screen.getByText(/^SkillSpector rated this content dangerous/)).toBeInTheDocument();
    const anyway = screen.getByRole("button", { name: "Install anyway" });
    expect(anyway).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: ACK }));
    await user.click(anyway);
    expect(onInstall).toHaveBeenCalledWith(true);
  });

  it("gates SkillSpector's DO_NOT_INSTALL recommendation like a dangerous finding", async () => {
    renderReview({ scanner: on(EXTERNAL_DO_NOT_INSTALL) });
    expect(await screen.findByText(/recommends not installing this skill \(risk score 72 of 100\)/)).toBeInTheDocument();
    expect(screen.getByText("DO_NOT_INSTALL")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Install anyway" })).toBeDisabled();
  });

  it("scans a folder from this computer as soon as the review opens: nothing is uploaded", async () => {
    const { transport } = renderReview({ scanner: on(EXTERNAL_WARNING) }, LOCAL_REVIEW);
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findByText("scripts/env_report.py:14")).toBeInTheDocument();
    expect(invokedArgs(transport, "externalScanReview")).toHaveLength(1);
    // Warnings don't gate.
    expect(screen.getByRole("button", { name: "Install" })).toBeEnabled();
    // SkillSpector's own "OSV.dev unreachable" finding is shown as is.
    expect(within(section).getByText("OSV.dev unreachable, using static fallback. Results may be incomplete.")).toBeInTheDocument();
  });

  it("names both scanners when both rated the content dangerous", async () => {
    renderReview({ scanner: on(EXTERNAL_DANGEROUS) }, { ...MOCK_SKILL_REVIEW, scan: DANGEROUS_BUILTIN });
    expect(
      await screen.findByText(/^Loadout's built-in scan and SkillSpector both rated this content dangerous/),
    ).toBeInTheDocument();
  });

  it("clears a box ticked while SkillSpector ran, once its result arrives", async () => {
    const { user } = renderReview({ scanner: on(EXTERNAL_WARNING, { scanMs: 300 }) }, { ...MOCK_SKILL_REVIEW, scan: DANGEROUS_BUILTIN });
    expect(await screen.findByRole("button", { name: "Cancel scan" })).toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: ACK }));
    expect(screen.getByRole("checkbox", { name: ACK })).toBeChecked();
    // The tick covered only the built-in findings; SkillSpector's result asks again.
    await screen.findByText("scripts/env_report.py:14", undefined, { timeout: 3_000 });
    expect(screen.getByRole("checkbox", { name: ACK })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Install anyway" })).toBeDisabled();
  });

  it("keeps Install off while it scans, says how long it may take and keeps focus on Cancel scan / Scan again", async () => {
    const { user, transport } = renderReview({ scanner: on(EXTERNAL_CLEAN, { scanMs: 300 }) });
    const cancel = await screen.findByRole("button", { name: "Cancel scan" });
    expect(screen.getByText("Scanning with SkillSpector on this computer (up to 120 seconds)…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Install" })).toBeDisabled();
    expect(await liveStatus()).toHaveTextContent("Scanning with SkillSpector…");
    await user.click(cancel);
    const again = await screen.findByRole("button", { name: "Scan again" });
    expect(again).toBe(cancel);
    expect(again).toHaveFocus();
    expect(await liveStatus()).toHaveTextContent("Not scanned (canceled). Install is available.");
    // Only that run was canceled.
    const [[, firstRun]] = invokedArgs(transport, "externalScanReview");
    expect(invokedArgs(transport, "cancelExternalScan")).toEqual([[firstRun]]);
    expect(screen.getByRole("button", { name: "Install" })).toBeEnabled();

    await user.click(again);
    expect(again).toHaveFocus();
    expect(again).toHaveTextContent("Cancel scan");
    // Clean: no Scan again, so focus moves to the section's heading instead of the page.
    await waitFor(() => expect(screen.getByRole("heading", { name: "External scan" })).toHaveFocus(), { timeout: 3_000 });
    expect(await liveStatus()).toHaveTextContent("SkillSpector: Clean, 0 findings. Install is available.");
    expect(screen.getByText(/^SkillSpector found nothing to point out/)).toBeInTheDocument();
  });

  it("cancels its run when the review closes mid-scan, but not on a strict-mode remount", async () => {
    const { transport, unmount } = renderReview({ scanner: on(EXTERNAL_CLEAN, { scanMs: 5_000 }) }, MOCK_SKILL_REVIEW, true);
    await screen.findByRole("button", { name: "Cancel scan" });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(transport.calls).not.toContain("cancelExternalScan");
    const runs = invokedArgs(transport, "externalScanReview");
    expect(runs).toHaveLength(1);
    unmount();
    await waitFor(() => expect(invokedArgs(transport, "cancelExternalScan")).toEqual([[runs[0][1]]]));
  });

  it.each([
    ["a timeout", externalNotScanned("timeout"), "Not scanned (timed out)."],
    ["a failure", EXTERNAL_FAILED, "Not scanned (the scanner failed)."],
    ["a missing uv cache", externalNotScanned("not_prepared"), "Not scanned (SkillSpector isn't prepared yet; choose Prepare in Settings > Security)."],
    ["unreadable output", EXTERNAL_NOT_UNDERSTOOD, "Scanner output not understood (version 2.13.7)."],
  ])("never blocks on %s and never calls it clean", async (_name, result, text) => {
    const { user, onInstall } = renderReview({ scanner: on(result) });
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findByText(text)).toBeInTheDocument();
    expect(within(section).queryByText("Clean")).toBeNull();
    expect(within(section).getByRole("button", { name: "Scan again" })).toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: ACK })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Install" }));
    expect(onInstall).toHaveBeenCalledWith(false);
  });

  it("shows the scanner's one-line detail of a failure", async () => {
    renderReview({ scanner: on(EXTERNAL_FAILED) });
    expect(await screen.findByText("Error: could not read <skill>/SKILL.md")).toBeInTheDocument();
  });

  it("never makes a scan that wasn't complete look clean, and doesn't gate it", async () => {
    const { user, onInstall } = renderReview({ scanner: on(EXTERNAL_INCOMPLETE) });
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findByText("Not fully scanned")).toBeInTheDocument();
    expect(within(section).queryByText("Clean")).toBeNull();
    expect(within(section).queryByText(/found nothing to point out/)).toBeNull();
    expect(within(section).getByText(/couldn't inspect all of this content, so this result doesn't count as clean/)).toBeInTheDocument();
    expect(within(section).getByText("not fully scanned (partial, 62% coverage)")).toBeInTheDocument();
    expect(within(section).getByRole("status")).toHaveTextContent(
      "SkillSpector: not fully scanned, 1 finding. Install is available.",
    );
    expect(within(section).getByRole("status")).not.toHaveTextContent(/Clean/);
    expect(screen.queryByRole("checkbox", { name: ACK })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Install" }));
    expect(onInstall).toHaveBeenCalledWith(false);
  });

  it("still gates a dangerous scan that wasn't complete", async () => {
    renderReview({ scanner: on(EXTERNAL_INCOMPLETE_DANGEROUS) });
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findByText("Not fully scanned")).toBeInTheDocument();
    expect(within(section).getByRole("status")).toHaveTextContent(
      "SkillSpector: Dangerous, not fully scanned, 3 findings. Install anyway needs your confirmation.",
    );
    expect(screen.getByRole("button", { name: "Install anyway" })).toBeDisabled();
  });

  it("names the version that ran", async () => {
    renderReview({ scanner: on(EXTERNAL_CLEAN, { runtime: "path", version: "2.14.0" }) });
    expect(await screen.findByText("SkillSpector 2.14.0")).toBeInTheDocument();
  });
});

describe("SkillSpector in the update review", () => {
  it("scans the checked version and updates a dangerous one only through Update anyway", async () => {
    const onUpdate = vi.fn();
    const { user, transport } = renderWithProviders(
      <SkillUpdateDialog skill={SKILL} check={CHECK} isUpdating={false} onUpdate={onUpdate} onClose={() => undefined} />,
      { scanner: on(EXTERNAL_DANGEROUS) },
    );
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findAllByText("Dangerous")).toHaveLength(2);
    expect(invokedArgs(transport, "externalScanUpdate").map((args) => args.slice(0, 2))).toEqual([["alpha", "2222222aaaa"]]);
    expect(screen.getByText(/^SkillSpector rated the new version dangerous/)).toBeInTheDocument();
    expect(within(section).getByRole("status")).toHaveTextContent("Update anyway needs your confirmation.");
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
    const anyway = screen.getByRole("button", { name: "Update anyway" });
    expect(anyway).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: ACK }));
    await user.click(anyway);
    expect(onUpdate).toHaveBeenCalledWith(true, { trust: null, movedTrust: null });
  });

  it("updates normally with warnings or a scan that didn't run", async () => {
    for (const result of [EXTERNAL_WARNING, externalNotScanned("timeout")]) {
      const onUpdate = vi.fn();
      const { user, unmount } = renderWithProviders(
        <SkillUpdateDialog skill={SKILL} check={CHECK} isUpdating={false} onUpdate={onUpdate} onClose={() => undefined} />,
        { scanner: on(result) },
      );
      const section = await screen.findByRole("region", { name: "External scan" });
      await waitFor(() => expect(within(section).getByRole("status")).toHaveTextContent("Update is available."));
      await user.click(screen.getByRole("button", { name: "Update" }));
      expect(onUpdate).toHaveBeenCalledWith(false, { trust: null, movedTrust: null });
      unmount();
    }
  });

  it("runs nothing while SkillSpector is off", async () => {
    const { transport } = renderWithProviders(
      <SkillUpdateDialog skill={SKILL} check={CHECK} isUpdating={false} onUpdate={() => undefined} onClose={() => undefined} />,
      { scanner: { ...SCANNER_READY, results: every(EXTERNAL_DANGEROUS) } },
    );
    await waitFor(() => expect(screen.getByRole("button", { name: "Update" })).toBeEnabled());
    expect(screen.queryByRole("region", { name: "External scan" })).toBeNull();
    expect(scanCalls(transport)).toEqual([]);
  });

  it("drops the last result when Scan again fails, and holds Update until a scan finishes", async () => {
    let runs = 0;
    const onUpdate = vi.fn();
    const { user, transport } = renderWithProviders(
      <SkillUpdateDialog skill={SKILL} check={CHECK} isUpdating={false} onUpdate={onUpdate} onClose={() => undefined} />,
      {
        scanner: on(EXTERNAL_CLEAN),
        commands: {
          externalScanUpdate: async () => {
            runs += 1;
            return runs === 1
              ? ok(externalNotScanned("canceled"))
              : mockIpcError({ code: "io_error", message: "the cached clone could not be read" });
          },
        },
      },
    );
    const section = await screen.findByRole("region", { name: "External scan" });
    expect(await within(section).findByText("Not scanned")).toBeInTheDocument();
    await user.click(within(section).getByRole("button", { name: "Scan again" }));
    expect(await within(section).findByText("A file could not be read or written.")).toBeInTheDocument();
    expect(within(section).queryByText("Not scanned")).toBeNull();
    expect(invokedArgs(transport, "externalScanUpdate")).toHaveLength(2);
    expect(screen.getByRole("button", { name: "Update" })).toBeDisabled();
    expect(within(section).getByRole("status")).toHaveTextContent("Update waits for a finished SkillSpector scan: choose Scan again.");
    expect(within(section).getByRole("button", { name: "Scan again" })).toBeInTheDocument();
    expect(onUpdate).not.toHaveBeenCalled();
  });
});

describe("Scan with SkillSpector on a library skill", () => {
  it("is offered only while SkillSpector is on and ready, named with its visible text", async () => {
    for (const scanner of [SCANNER_READY, { ...SCANNER_NEEDS_PREPARE, enabled: true }, { enabled: true }]) {
      const { transport, unmount } = renderWithProviders(<SkillExternalScanButton skill={SKILL} onOpen={() => undefined} />, {
        scanner,
      });
      await waitFor(() => expect(transport.calls).toContain("scannerStatus"));
      expect(screen.queryByRole("button")).toBeNull();
      expect(scanCalls(transport)).toEqual([]);
      unmount();
    }
    renderWithProviders(<SkillExternalScanButton skill={SKILL} onOpen={() => undefined} />, { scanner: SCANNER_ON });
    expect(await screen.findByRole("button", { name: "Scan with SkillSpector: alpha" })).toHaveTextContent("Scan with SkillSpector");
  });

  it("scans the store copy and reports the result, partial scans included", async () => {
    const { transport } = renderWithProviders(<SkillExternalScanDialog skill={SKILL} onClose={() => undefined} />, {
      scanner: on(EXTERNAL_INCOMPLETE),
    });
    const dialog = await screen.findByRole("dialog", { name: "SkillSpector: alpha" });
    expect(await within(dialog).findByText("Not fully scanned")).toBeInTheDocument();
    expect(within(dialog).queryByText("Clean")).toBeNull();
    expect(within(dialog).getByText(/scans the skill's store copy on this computer/)).toBeInTheDocument();
    expect(invokedArgs(transport, "externalScanSkill").map((args) => args[0])).toEqual(["alpha"]);
  });
});

describe("The mock backend (M50)", () => {
  it("refuses an install SkillSpector rated dangerous without allowDangerous, never one it didn't scan or a partial clean one", async () => {
    for (const [result, blocked] of [
      [EXTERNAL_DANGEROUS, true],
      [EXTERNAL_INCOMPLETE_DANGEROUS, true],
      [EXTERNAL_INCOMPLETE, false],
      [externalNotScanned("timeout"), false],
      [EXTERNAL_NOT_UNDERSTOOD, false],
    ] as const) {
      const { commands } = createMockTransport({ scanner: on(result) });
      const review = await commands.reviewGitSkill("https://github.com/acme/skills", null, null);
      expect(review.status).toBe("ok");
      await commands.externalScanReview("review-1", "run-1");
      const refused = await commands.installReviewedSkill("review-1", null, false, false, null, null);
      expect(refused.status === "error" && refused.error.code === "scan_blocked").toBe(blocked);
      expect((await commands.installReviewedSkill("review-1", null, false, true, null, null)).status).toBe("ok");
    }
  });

  it("refuses a scan while off and runs nothing while not ready", async () => {
    const off = createMockTransport({ scanner: SCANNER_READY });
    const refused = await off.commands.externalScanSkill("alpha", "run-1");
    expect(refused.status === "error" && refused.error.code).toBe("scanner_unavailable");
    const unprepared = createMockTransport({ scanner: { ...SCANNER_NEEDS_PREPARE, enabled: true } });
    const answer = await unprepared.commands.externalScanSkill("alpha", "run-1");
    expect(answer.status === "ok" && answer.data.reason).toBe("not_prepared");
  });

  it("forgets a dangerous update report once SkillSpector is turned off", async () => {
    const { commands } = createMockTransport({
      scanner: on(EXTERNAL_DANGEROUS),
      commands: { checkSkillUpdates: async () => ok([CHECK]) },
    });
    await commands.checkSkillUpdates(["alpha"]);
    await commands.externalScanUpdate("alpha", "2222222aaaa", "run-1");
    const blocked = await commands.updateSkill("alpha", "2222222aaaa", false, null, null);
    expect(blocked.status === "error" && blocked.error.code).toBe("scan_blocked");
    await commands.setScannerEnabled(false);
    expect((await commands.updateSkill("alpha", "2222222aaaa", false, null, null)).status).toBe("ok");
  });

  it("cancels one run by id without touching the next one", async () => {
    const { commands } = createMockTransport({ scanner: on(EXTERNAL_WARNING, { scanMs: 200 }) });
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
