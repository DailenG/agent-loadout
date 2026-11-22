import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { PendingChangesPill } from "@/app/apply/PendingChangesPill";
import { t } from "@/i18n/en";
import type { PendingPlan } from "@/ipc/bindings";
import { ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const PLAN: PendingPlan = {
  applyMode: "instant",
  files: [],
  pending: 3,
  writes: 2,
  unresolved: 0,
  warnings: [],
};

describe("PendingChangesPill", () => {
  it("is hidden when nothing is pending", async () => {
    const { transport } = renderWithProviders(<PendingChangesPill onOpen={() => undefined} />, {
      commands: { pendingPlan: async () => ok({ ...PLAN, pending: 0 }) },
    });
    await waitFor(() => expect(transport.calls).toContain("pendingPlan"));
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("shows the count and opens the drawer on click", async () => {
    const onOpen = vi.fn();
    const { user } = renderWithProviders(<PendingChangesPill onOpen={onOpen} />, {
      commands: { pendingPlan: async () => ok(PLAN) },
    });
    const pill = await screen.findByRole("button", { name: t("apply.pill.aria", { count: 3 }) });
    expect(pill).toHaveTextContent(t("apply.pill.count", { count: 3 }));
    expect(pill).not.toHaveTextContent(t("apply.pill.review"));
    await user.click(pill);
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("hints at review mode", async () => {
    renderWithProviders(<PendingChangesPill onOpen={() => undefined} />, {
      commands: { pendingPlan: async () => ok({ ...PLAN, applyMode: "review" }) },
    });
    const pill = await screen.findByRole("button", { name: t("apply.pill.aria", { count: 3 }) });
    expect(pill).toHaveTextContent(t("apply.pill.review"));
  });
});
