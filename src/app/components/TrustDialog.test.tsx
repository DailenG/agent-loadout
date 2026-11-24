import { act, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { SourceTrustInfo } from "@/ipc/bindings";
import { MOCK_TRUST_ASK } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { TrustDialog, useTrustPrompt, type TrustAnswer, type TrustSources } from "./TrustDialog";

const MOVED_TO: SourceTrustInfo = {
  ...MOCK_TRUST_ASK,
  display: "github.com/evil/skills",
  scopes: [
    { scope: "repo", display: "github.com/evil/skills" },
    { scope: "owner", display: "github.com/evil/*" },
  ],
};

/** Runs `resolveTrust(sources)` on click and shows what it resolved to. */
function Harness({ sources, onBlocked }: { sources: TrustSources; onBlocked?: (info: SourceTrustInfo) => void }) {
  const { resolveTrust, dialog } = useTrustPrompt();
  const [result, setResult] = useState<TrustAnswer | null | undefined>(undefined);
  return (
    <>
      <button type="button" onClick={() => void resolveTrust(sources, onBlocked).then(setResult)}>
        Ask
      </button>
      <output aria-label="result">{result === undefined ? "pending" : JSON.stringify(result)}</output>
      {dialog}
    </>
  );
}

describe("TrustDialog", () => {
  it("starts on Cancel, so a held or doubled Enter grants nothing", async () => {
    const onAnswer = vi.fn();
    const { user } = renderWithProviders(<TrustDialog info={MOCK_TRUST_ASK} onAnswer={onAnswer} />);
    const dialog = screen.getByRole("alertdialog", { name: "Trust this source?" });
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(onAnswer).toHaveBeenCalledWith(null);
  });

  it("answers null on Escape", async () => {
    const onAnswer = vi.fn();
    const { user } = renderWithProviders(<TrustDialog info={MOCK_TRUST_ASK} onAnswer={onAnswer} />);
    await user.keyboard("{Escape}");
    expect(onAnswer).toHaveBeenCalledWith(null);
  });

  it("offers only the scopes the backend gives and says what it knows about the source", async () => {
    const onAnswer = vi.fn();
    const info: SourceTrustInfo = {
      ...MOCK_TRUST_ASK,
      display: "http://git.acme.corp/team/tools",
      scopes: [
        { scope: "repo", display: "http://git.acme.corp/team/tools" },
        { scope: "owner", display: "http://git.acme.corp/team/*" },
        { scope: "host", display: "http://git.acme.corp/*" },
      ],
      insecure: true,
      installedSkills: ["notes", "lint"],
    };
    const { user } = renderWithProviders(<TrustDialog info={info} onAnswer={onAnswer} />);
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getAllByRole("radio")).toHaveLength(4);
    expect(within(dialog).getByRole("radio", { name: "Always trust everything on http://git.acme.corp/*" })).toBeInTheDocument();
    expect(within(dialog).getByText(/Unencrypted connection/)).toBeInTheDocument();
    expect(within(dialog).getByText("Skills already installed from here: notes, lint.")).toBeInTheDocument();
    expect(within(dialog).queryByText("Official")).toBeNull();
    await user.click(within(dialog).getByRole("radio", { name: "Always trust everything on http://git.acme.corp/*" }));
    await user.click(within(dialog).getByRole("button", { name: "Always trust and continue" }));
    expect(onAnswer).toHaveBeenCalledWith({ kind: "always", scope: "host" });
  });
});

describe("useTrustPrompt", () => {
  it("asks about the requested source, then about where it moved, naming both addresses", async () => {
    const { user } = renderWithProviders(<Harness sources={{ trust: MOCK_TRUST_ASK, movedTrust: MOVED_TO }} />);
    await user.click(screen.getByRole("button", { name: "Ask" }));
    let dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("radio", { name: "Always trust github.com/acme/skills" }));
    await user.click(within(dialog).getByRole("button", { name: "Always trust and continue" }));

    // A fresh dialog for the second question: Cancel focused, "this time only" selected again.
    dialog = await screen.findByRole("alertdialog");
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus());
    expect(within(dialog).getByText(/github\.com\/acme\/skills now redirects to github\.com\/evil\/skills\./)).toBeInTheDocument();
    expect(within(dialog).getByRole("radio", { name: "Trust this time only" })).toBeChecked();
    await user.click(within(dialog).getByRole("button", { name: "Trust once and continue" }));
    await waitFor(() =>
      expect(screen.getByLabelText("result")).toHaveTextContent(
        JSON.stringify({ trust: { kind: "always", scope: "repo" }, movedTrust: { kind: "once" } }),
      ),
    );
  });

  it("sends nothing when the moved prompt is canceled", async () => {
    const { user } = renderWithProviders(<Harness sources={{ trust: MOCK_TRUST_ASK, movedTrust: MOVED_TO }} />);
    await user.click(screen.getByRole("button", { name: "Ask" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Trust once and continue" }));
    const moved = await screen.findByRole("alertdialog");
    await user.click(within(moved).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.getByLabelText("result")).toHaveTextContent("null"));
  });

  it("refuses a blocked redirect target before asking anything", async () => {
    const onBlocked = vi.fn();
    const blocked: SourceTrustInfo = {
      ...MOVED_TO,
      state: { kind: "blocked", scope: "owner", display: "github.com/evil/*", decidedAt: "2026-11-20T09:00:00Z" },
    };
    const { user } = renderWithProviders(
      <Harness sources={{ trust: MOCK_TRUST_ASK, movedTrust: blocked }} onBlocked={onBlocked} />,
    );
    await act(() => user.click(screen.getByRole("button", { name: "Ask" })));
    await waitFor(() => expect(screen.getByLabelText("result")).toHaveTextContent("null"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(onBlocked).toHaveBeenCalledWith(blocked);
  });
});
