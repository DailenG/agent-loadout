import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SkillInfo, SkillUpdateCheck } from "@/ipc/bindings";
import { MOCK_TRUST_ASK } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { SkillUpdateDialog } from "./SkillFileDialog";

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
  changes: [
    { path: "SKILL.md", kind: "modified", script: false },
    { path: "scripts/run.sh", kind: "added", script: true },
  ],
  error: null,
  scan: {
    verdict: "dangerous",
    findings: [
      {
        scanner: "builtin",
        rule: "reverse-shell",
        severity: "dangerous",
        file: "scripts/run.sh",
        line: 2,
        message: "Opens a shell for a remote machine.",
        excerpt: null,
      },
    ],
    rulesVersion: 1,
    cached: false,
    customRules: false,
    warnings: [],
  },
  trust: null,
  movedTrust: null,
};

function renderDialog(check: SkillUpdateCheck) {
  const onUpdate = vi.fn();
  const view = renderWithProviders(
    <SkillUpdateDialog skill={SKILL} check={check} isUpdating={false} onUpdate={onUpdate} onClose={() => undefined} />,
  );
  return { onUpdate, ...view };
}

describe("SkillUpdateDialog", () => {
  it("marks scripts, shows the findings and updates a dangerous version only through Update anyway", async () => {
    const { user, onUpdate } = renderDialog(CHECK);
    const dialog = screen.getByRole("dialog", { name: "Update alpha" });
    expect(within(dialog).getByRole("button", { name: /run\.sh.*Script/ })).toBeInTheDocument();
    expect(within(dialog).getByText("reverse-shell")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Update" })).toBeNull();
    const anyway = within(dialog).getByRole("button", { name: "Update anyway" });
    expect(anyway).toBeDisabled();
    await user.click(within(dialog).getByRole("checkbox", { name: "I read the findings and want to go ahead anyway" }));
    await user.click(anyway);
    expect(onUpdate).toHaveBeenCalledWith(true, { trust: null, movedTrust: null });
  });

  it("asks about a lapsed source and passes the grant", async () => {
    const { user, onUpdate } = renderDialog({
      ...CHECK,
      scan: { verdict: "clean", findings: [], rulesVersion: 1, cached: false, customRules: false, warnings: [] },
      trust: {
        ...MOCK_TRUST_ASK,
        state: {
          kind: "ask",
          reason: "lapsed",
          lapse: { reason: "history_rewritten", at: "2026-11-22T08:00:00Z", detail: null },
        },
      },
    });
    expect(screen.getByText("Its history was rewritten (a force-push) since the last fetch.")).toBeInTheDocument();
    // Update waits until Loadout knows whether Snyk Agent Scan is on (M49 review F6).
    await waitFor(() => expect(screen.getByRole("button", { name: "Update" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Update" }));
    const trust = await screen.findByRole("alertdialog", { name: "Trust this source?" });
    await user.click(within(trust).getByRole("radio", { name: "Always trust github.com/acme/skills" }));
    await user.click(within(trust).getByRole("button", { name: "Always trust and continue" }));
    await waitFor(() =>
      expect(onUpdate).toHaveBeenCalledWith(false, { trust: { kind: "always", scope: "repo" }, movedTrust: null }),
    );
  });
});
