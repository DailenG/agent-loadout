import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { MigrationApplied, MigrationReport } from "@/ipc/bindings";
import { MOCK_CHANGE, mockIpcError, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { MigrateWizard } from "./MigrateWizard";

const REPORT: MigrationReport = {
  source: "~/.skilldock",
  servers: ["context7", "notes"],
  skills: ["repo-notes"],
  counts: { migrated: 2, needsReview: 1, skipped: 1 },
  files: ["servers/context7.toml", "servers/notes.toml", "skills/repo-notes/SKILL.md"],
  extractions: [
    { server: "notes", field: "env.NOTES_API_KEY", alias: "NOTES_NOTES_API_KEY", reason: "key_name", suggestedRef: "keyring://loadout/NOTES_NOTES_API_KEY" },
    { server: "notes", field: "args[2]", alias: "NOTES_TOKEN", reason: "argument_name", suggestedRef: "keyring://loadout/NOTES_TOKEN" },
    { server: "issues", field: "headers.Authorization", alias: "ISSUES_AUTHORIZATION", reason: "key_name", suggestedRef: "keyring://loadout/ISSUES_AUTHORIZATION" },
  ],
  items: [
    {
      kind: "server",
      name: "context7",
      status: "migrated",
      detail: "unwrapped from mcp-filter",
      code: null,
      id: "context7",
      offers: [{ kind: "replace_wrapper", client: "codex", name: "context7" }],
    },
    { kind: "skill", name: "repo-notes", status: "migrated", detail: "copied", code: null, id: "repo-notes", offers: [] },
    { kind: "server", name: "broken", status: "needs_review", detail: "no command", code: "missing_field", id: null, offers: [] },
    { kind: "plugin", name: "~/.skilldock/plugins", status: "skipped", detail: "plugins are not migrated", code: null, id: null, offers: [] },
  ],
};

const APPLIED: MigrationApplied = {
  report: { ...REPORT, files: [] },
  tag: "auto/pre-migrate/20261122T000000Z",
  commit: "0000000000000000000000000000000000000002",
  postTag: "auto/post-migrate/20261122T000000Z",
  backupImported: false,
};

function renderWizard(commands: MockTransportOptions["commands"] = {}) {
  const onClose = vi.fn();
  const onOpenPending = vi.fn();
  const view = renderWithProviders(<MigrateWizard onClose={onClose} onOpenPending={onOpenPending} />, {
    commands: {
      planSkilldockMigration: async () => ok(REPORT),
      applySkilldockMigration: async () => ok(APPLIED),
      ...commands,
    },
  });
  return { onClose, onOpenPending, ...view };
}

describe("MigrateWizard", () => {
  it("shows the dry run with every item, its status and offers, filtered by status", async () => {
    const { user, transport } = renderWizard();
    const report = await screen.findByRole("list", { name: "Migration report" });
    expect(screen.getByText(/A dry run of ~\/\.skilldock/)).toBeInTheDocument();
    expect(within(report).getAllByRole("listitem", { name: undefined }).length).toBeGreaterThanOrEqual(4);
    expect(within(report).getByText("Replace SkillDock wrapper for codex/context7 after the migration.")).toBeInTheDocument();
    expect(within(report).getByText(/A required field is missing\. no command/)).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Skipped (1)" }));
    expect(within(report).getByText("~/.skilldock/plugins")).toBeInTheDocument();
    expect(within(report).queryByText("context7")).toBeNull();
    expect(transport.calls).not.toContain("applySkilldockMigration");
  });

  it("routes each flagged value, migrates once, then sets the secrets one way", async () => {
    const { user, transport } = renderWizard();
    await screen.findByRole("list", { name: "Migration report" });
    await user.click(screen.getByRole("button", { name: "Next" }));

    // Keyring (default): a password field.
    const keyValue = screen.getByLabelText("Value for notes env.NOTES_API_KEY");
    expect(keyValue).toHaveAttribute("type", "password");
    const next = screen.getByRole("button", { name: "Next" });
    expect(next).toBeDisabled();
    await user.type(keyValue, "typed-value");

    await user.click(screen.getByRole("combobox", { name: "Destination for notes args[2]" }));
    await user.click(await screen.findByRole("option", { name: /Environment variable/ }));
    await user.type(screen.getByLabelText("Environment variable for notes args[2]"), "NOTES_TOKEN");

    await user.click(screen.getByRole("combobox", { name: "Destination for issues headers.Authorization" }));
    await user.click(await screen.findByRole("option", { name: /1Password reference/ }));
    await user.type(screen.getByLabelText("1Password reference for issues headers.Authorization"), "op://Dev/Issues/token");
    expect(next).toBeEnabled();
    await user.click(next);

    expect(screen.getByText("Migrate 2 servers and 1 skills into the store?")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Migrate" }));
    expect(await screen.findByText(/Migrated 2 servers and 1 skills in one commit/)).toBeInTheDocument();
    expect(transport.calls.filter((call) => call === "applySkilldockMigration")).toHaveLength(1);
    expect(transport.invocations).toContainEqual({
      command: "setSecretKeyringValue",
      args: ["NOTES_NOTES_API_KEY", "typed-value", "store"],
    });
    expect(transport.invocations).toContainEqual({
      command: "setSecretReference",
      args: ["NOTES_TOKEN", "env:NOTES_TOKEN", "store", null],
    });
    expect(transport.invocations).toContainEqual({
      command: "setSecretReference",
      args: ["ISSUES_AUTHORIZATION", "op://Dev/Issues/token", "store", null],
    });
    // The value is not on screen after the migration.
    expect(screen.queryByDisplayValue("typed-value")).toBeNull();
  });

  it("offers Replace SkillDock wrapper after the migration and opens Pending changes", async () => {
    const { user, transport, onOpenPending } = renderWizard({
      planSkilldockMigration: async () => ok({ ...REPORT, extractions: [] }),
      applySkilldockMigration: async () => ok({ ...APPLIED, report: { ...APPLIED.report, extractions: [] } }),
      replaceSkilldockWrapper: async () => ok(MOCK_CHANGE),
    });
    await screen.findByRole("list", { name: "Migration report" });
    await user.click(screen.getByRole("button", { name: "Next" }));
    await user.click(screen.getByRole("button", { name: "Migrate" }));
    const wrappers = await screen.findByRole("list", { name: "SkillDock wrappers" });
    await user.click(within(wrappers).getByRole("button", { name: "Replace SkillDock wrapper" }));
    await waitFor(() =>
      expect(transport.invocations).toContainEqual({ command: "replaceSkilldockWrapper", args: ["codex", "context7"] }),
    );
    expect(within(wrappers).getByRole("button", { name: "Replaced" })).toBeDisabled();
    expect(transport.calls).not.toContain("setSecretKeyringValue");
    await user.click(screen.getByRole("button", { name: "Review the first apply" }));
    expect(onOpenPending).toHaveBeenCalled();
  });

  it("says when there is no SkillDock data", async () => {
    renderWizard({ planSkilldockMigration: async () => mockIpcError({ code: "not_found", message: "none" }) });
    expect(await screen.findByText("No SkillDock data was found in ~/.skilldock.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
  });
});
