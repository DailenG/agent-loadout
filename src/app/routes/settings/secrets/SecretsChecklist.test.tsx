import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SecretsChecklist } from "@/app/routes/settings/secrets/SecretsChecklist";
import { t } from "@/i18n/en";
import type { AliasCheck } from "@/ipc/bindings";
import { renderWithProviders } from "@/test/renderWithProviders";

const NO_OP_CLI: AliasCheck = {
  alias: "GITHUB_PAT",
  reference: "op://Dev/GitHub/token",
  source: "store",
  status: "provider_unavailable",
  message: null,
  neededBy: [
    { server: "github", clients: ["claude-code", "codex"] },
    { server: "fetch", clients: [] },
  ],
  blocking: true,
};

const MISSING: AliasCheck = {
  alias: "SLACK_TOKEN",
  reference: "keyring://loadout/SLACK_TOKEN",
  source: "store",
  status: "missing",
  message: null,
  neededBy: [{ server: "slack", clients: ["all-detected"] }],
  blocking: true,
};

const RESOLVED: AliasCheck = {
  ...MISSING,
  alias: "LINEAR_KEY",
  reference: "env:LINEAR_KEY",
  status: "resolved",
  neededBy: [],
  blocking: false,
};

function renderChecklist() {
  const onChanged = vi.fn();
  const view = renderWithProviders(<SecretsChecklist checks={[NO_OP_CLI, MISSING, RESOLVED]} onChanged={onChanged} />);
  return { ...view, onChanged };
}

const SET_COMMANDS = ["setSecretKeyringValue", "setSecretReference"];

describe("SecretsChecklist", () => {
  it("shows status and who needs each alias", () => {
    renderChecklist();
    const github = screen.getByRole("article", { name: "GITHUB_PAT" });
    expect(within(github).getByText("1Password CLI not found")).toBeInTheDocument();
    expect(
      within(github).getByText("needed by github (claude-code, codex); fetch (not enabled)"),
    ).toBeInTheDocument();
    const slack = screen.getByRole("article", { name: "SLACK_TOKEN" });
    expect(within(slack).getByText(t("settings.secrets.status.missing"))).toBeInTheDocument();
    expect(within(slack).getByText("needed by slack (all detected clients)")).toBeInTheDocument();
    // A resolved alias has nothing to fix.
    const linear = screen.getByRole("article", { name: "LINEAR_KEY" });
    expect(within(linear).getByText(t("settings.secrets.status.resolved"))).toBeInTheDocument();
    expect(within(linear).queryByRole("button")).toBeNull();
  });

  it("enters a keyring value on this machine by default when the provider is missing", async () => {
    const { user, transport, onChanged } = renderChecklist();
    const github = screen.getByRole("article", { name: "GITHUB_PAT" });
    await user.click(within(github).getByRole("button", { name: "Enter value into keyring" }));
    expect(within(github).getByRole("checkbox", { name: "This machine only" })).toBeChecked();
    const input = within(github).getByLabelText(t("settings.secrets.fix.keyringInput"));
    expect(input).toHaveAttribute("type", "password");
    await user.type(input, "s3cret");
    await user.click(within(github).getByRole("button", { name: t("settings.secrets.fix.save") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "setSecretKeyringValue")?.args).toEqual([
        "GITHUB_PAT",
        "s3cret",
        "this_machine",
      ]),
    );
    await waitFor(() => expect(onChanged).toHaveBeenCalledTimes(1));
    expect(screen.queryByDisplayValue("s3cret")).toBeNull();
  });

  it("points an alias at an environment variable or a 1Password item for every machine", async () => {
    const { user, transport } = renderChecklist();
    const slack = screen.getByRole("article", { name: "SLACK_TOKEN" });
    await user.click(within(slack).getByRole("button", { name: "Use environment variable" }));
    expect(within(slack).getByRole("checkbox", { name: "This machine only" })).not.toBeChecked();
    await user.type(within(slack).getByRole("textbox", { name: t("settings.secrets.fix.envInput") }), "SLACK_BOT_TOKEN");
    await user.click(within(slack).getByRole("button", { name: t("settings.secrets.fix.save") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "setSecretReference")?.args).toEqual([
        "SLACK_TOKEN",
        "env:SLACK_BOT_TOKEN",
        "store",
        null,
      ]),
    );

    await user.click(within(slack).getByRole("button", { name: "Choose a 1Password item" }));
    await user.type(
      within(slack).getByRole("textbox", { name: t("settings.secrets.fix.opInput") }),
      "op://Dev/Slack/token",
    );
    await user.click(within(slack).getByRole("button", { name: t("settings.secrets.fix.save") }));
    await waitFor(() =>
      expect(
        transport.invocations.filter((call) => call.command === "setSecretReference").map((call) => call.args),
      ).toEqual([
        ["SLACK_TOKEN", "env:SLACK_BOT_TOKEN", "store", null],
        ["SLACK_TOKEN", "op://Dev/Slack/token", "store", null],
      ]),
    );
  });

  it("Skip for now hides the fixes and sends nothing", async () => {
    const { user, transport, onChanged } = renderChecklist();
    const slack = screen.getByRole("article", { name: "SLACK_TOKEN" });
    await user.click(within(slack).getByRole("button", { name: "Skip for now" }));
    expect(within(slack).queryByRole("button", { name: "Enter value into keyring" })).toBeNull();
    expect(within(slack).getByText(t("settings.secrets.fix.skipped"))).toBeInTheDocument();
    expect(transport.calls.filter((command) => SET_COMMANDS.includes(command))).toEqual([]);
    expect(onChanged).not.toHaveBeenCalled();
  });
});
