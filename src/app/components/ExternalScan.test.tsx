import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { EXTERNAL_SCANNER_AVAILABLE } from "@/app/components/ExternalScan";
import { SkillReviewDialog } from "@/app/routes/install/SkillReviewDialog";
import { SkillExternalScanButton } from "@/app/routes/skills/SkillExternalScan";
import { SkillUpdateDialog } from "@/app/routes/skills/SkillFileDialog";
import type { SkillInfo, SkillInstallReview, SkillUpdateCheck } from "@/ipc/bindings";
import { createMockTransport, MOCK_SCAN_CLEAN, MOCK_SKILL_REVIEW, ok } from "@/test/mockTransport";
import { renderWithProviders, type RenderOptions } from "@/test/renderWithProviders";
import {
  EXTERNAL_DANGEROUS,
  EXTERNAL_LIMIT_REACHED,
  EXTERNAL_NOT_UNDERSTOOD,
  EXTERNAL_WARNING,
  type ScannerMockState,
  type ScannerResults,
} from "@/test/scannerData";

// Snyk Agent Scan is disabled in this build (0.7.1): even with a 0.7.0 setting that turned an installed scanner
// on, the reviews show no External scan section, offer no Scan with Snyk and call no scanner command.

const ON: Partial<ScannerMockState> = { installed: true, enabled: true, tokenRef: "env:SNYK_TOKEN" };

function every(result: ScannerResults["review"]): ScannerResults {
  return { review: result, update: result, skill: result };
}

const LOCAL_REVIEW: SkillInstallReview = {
  ...MOCK_SKILL_REVIEW,
  source: { kind: "local", url: null, path: null, gitRef: null, commit: null },
};

function renderReview(options: RenderOptions = {}, review: SkillInstallReview = MOCK_SKILL_REVIEW) {
  const onInstall = vi.fn();
  const view = renderWithProviders(
    <SkillReviewDialog review={review} isInstalling={false} onInstall={onInstall} onClose={() => undefined} />,
    options,
  );
  return { onInstall, ...view };
}

/** Any scanner command the UI called. */
function scannerCalls(transport: { calls: string[] }) {
  return transport.calls.filter((call) => call.startsWith("externalScan") || call.toLowerCase().includes("scanner"));
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

describe("Snyk Agent Scan disabled in this build", () => {
  it("is switched off", () => {
    expect(EXTERNAL_SCANNER_AVAILABLE).toBe(false);
  });

  it("shows no External scan in an install review and installs without asking the scanner", async () => {
    const { transport, onInstall, user } = renderReview({ scanner: { ...ON, results: every(EXTERNAL_DANGEROUS) } });
    const install = await screen.findByRole("button", { name: "Install" });
    expect(install).toBeEnabled();
    expect(screen.queryByRole("region", { name: "External scan" })).toBeNull();
    expect(screen.queryByText(/Snyk/)).toBeNull();
    await user.click(install);
    expect(onInstall).toHaveBeenCalledWith(false);
    expect(scannerCalls(transport)).toEqual([]);
  });

  it("offers no Scan with Snyk for a folder from this computer", async () => {
    const { transport } = renderReview({ scanner: ON }, LOCAL_REVIEW);
    expect(await screen.findByRole("button", { name: "Install" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: /Scan with Snyk/ })).toBeNull();
    expect(scannerCalls(transport)).toEqual([]);
  });

  it("shows no External scan in an update review and updates without asking the scanner", async () => {
    const onUpdate = vi.fn();
    const { user, transport } = renderWithProviders(
      <SkillUpdateDialog skill={SKILL} check={CHECK} isUpdating={false} onUpdate={onUpdate} onClose={() => undefined} />,
      { scanner: { ...ON, results: every(EXTERNAL_DANGEROUS) } },
    );
    const update = await screen.findByRole("button", { name: "Update" });
    expect(screen.queryByRole("region", { name: "External scan" })).toBeNull();
    await user.click(update);
    expect(onUpdate).toHaveBeenCalledWith(false, { trust: null, movedTrust: null });
    expect(scannerCalls(transport)).toEqual([]);
  });

  it("offers no Scan with Snyk on a library skill", async () => {
    const { transport } = renderWithProviders(<SkillExternalScanButton skill={SKILL} onOpen={() => undefined} />, {
      scanner: ON,
    });
    expect(screen.queryByRole("button")).toBeNull();
    expect(scannerCalls(transport)).toEqual([]);
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
