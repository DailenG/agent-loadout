import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SourceRecordInfo } from "@/ipc/bindings";
import { MOCK_TRUST_ASK, ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { SourcesSection } from "./SourcesSection";

const RECORDS: SourceRecordInfo[] = [
  {
    id: "id-official",
    display: "github.com/anthropics/*",
    scope: "owner",
    decision: "trust",
    decidedAt: "2026-11-20T09:12:00Z",
    via: "app",
    machine: "studio-desk",
    lapse: null,
    official: true,
  },
  {
    id: "id-lapsed",
    display: "github.com/acme/skills",
    scope: "repo",
    decision: "trust",
    decidedAt: "2026-11-01T10:00:00Z",
    via: "cli-prompt",
    machine: null,
    lapse: { reason: "history_rewritten", at: "2026-11-22T08:00:00Z", detail: null },
    official: false,
  },
  {
    id: "id-blocked",
    display: "github.com/shady/*",
    scope: "owner",
    decision: "block",
    decidedAt: "2026-11-15T12:00:00Z",
    via: "cli-command",
    machine: null,
    lapse: null,
    official: false,
  },
];

function renderSection() {
  return renderWithProviders(<SourcesSection />, { commands: { listSources: async () => ok(RECORDS) } });
}

describe("SourcesSection", () => {
  it("lists every decision with what it means", async () => {
    renderSection();
    const list = await screen.findByRole("list", { name: "Source decisions" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(3);
    expect(within(items[0]).getByText("Official")).toBeInTheDocument();
    expect(within(items[0]).getByText(/Trusted for everything from this owner\./)).toBeInTheDocument();
    expect(within(items[1]).getByText("Asks again")).toBeInTheDocument();
    expect(within(items[1]).getByText(/force-push/)).toBeInTheDocument();
    expect(within(items[2]).getByText(/Blocked for everything from this owner\./)).toBeInTheDocument();
    expect(screen.getByText(/Blocked: nothing is fetched, installed or updated from it/)).toBeInTheDocument();
  });

  it("revokes, unblocks and blocks a listed record after confirmation", async () => {
    const { user, transport } = renderSection();
    await user.click(await screen.findByRole("button", { name: "Revoke trust in github.com/acme/skills" }));
    let confirm = screen.getByRole("alertdialog", { name: "Revoke trust in github.com/acme/skills?" });
    await user.click(within(confirm).getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(transport.invocations).toContainEqual({ command: "revokeSource", args: ["id-lapsed"] }));

    await user.click(screen.getByRole("button", { name: "Unblock github.com/shady/*" }));
    confirm = screen.getByRole("alertdialog", { name: "Unblock github.com/shady/*?" });
    await user.click(within(confirm).getByRole("button", { name: "Unblock" }));
    await waitFor(() => expect(transport.invocations).toContainEqual({ command: "revokeSource", args: ["id-blocked"] }));

    await user.click(screen.getByRole("button", { name: "Block github.com/anthropics/*" }));
    confirm = screen.getByRole("alertdialog", { name: "Block github.com/anthropics/*?" });
    await user.click(within(confirm).getByRole("button", { name: "Block" }));
    await waitFor(() => expect(transport.invocations).toContainEqual({ command: "blockSource", args: ["id-official"] }));
  });

  it("blocks a source by URL at the chosen scope", async () => {
    const { user, transport } = renderWithProviders(<SourcesSection />, {
      commands: { checkSourceTrust: async () => ok(MOCK_TRUST_ASK) },
    });
    await user.click(await screen.findByRole("button", { name: "Block a source…" }));
    const dialog = screen.getByRole("dialog", { name: "Block a source" });
    await user.type(within(dialog).getByRole("textbox", { name: "Git repository URL" }), "https://github.com/acme/skills");
    await user.click(within(dialog).getByRole("button", { name: "Next" }));
    await user.click(await within(dialog).findByRole("radio", { name: /everything from this owner/ }));
    await user.click(within(dialog).getByRole("button", { name: "Block" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({
        command: "blockSourceUrl",
        args: ["https://github.com/acme/skills", "owner"],
      }),
    );
  });
});
