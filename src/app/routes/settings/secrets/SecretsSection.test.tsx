import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { SecretsSection } from "@/app/routes/settings/secrets/SecretsSection";
import { t } from "@/i18n/en";
import type { AliasCheck, ProviderEntry, SecretsOverview } from "@/ipc/bindings";
import { MOCK_PROVIDERS, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const OVERVIEW: SecretsOverview = {
  aliases: [
    {
      alias: "GITHUB_PAT",
      reference: "keyring://loadout/GITHUB_PAT",
      source: "machine",
      storeReference: "op://Dev/GitHub/token",
      description: null,
      envName: "GITHUB_PAT",
      servers: ["github"],
    },
    {
      alias: "SLACK_TOKEN",
      reference: "env:SLACK_TOKEN",
      source: "store",
      storeReference: null,
      description: null,
      envName: "SLACK_TOKEN",
      servers: [],
    },
  ],
  undefined: ["LINEAR_KEY"],
  environment: null,
  opAccount: null,
  serviceAccountSet: false,
};

const CHECK: AliasCheck = {
  alias: "GITHUB_PAT",
  reference: "keyring://loadout/GITHUB_PAT",
  source: "machine",
  status: "missing",
  message: null,
  neededBy: [{ server: "github", clients: ["claude-code"] }],
  blocking: true,
};

function renderSection(options: MockTransportOptions = {}) {
  return renderWithProviders(<SecretsSection />, {
    commands: { secretsOverview: async () => ok(OVERVIEW), ...options.commands },
  });
}

describe("SecretsSection", () => {
  it('shows "1Password CLI not found" with install steps', async () => {
    renderWithProviders(<SecretsSection />, { commands: { secretProviders: async () => ok(MOCK_PROVIDERS) } });
    expect(await screen.findByText("1Password CLI not found")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Secrets" })).toBeInTheDocument();
    const steps = screen.getByRole("list", { name: t("settings.secrets.op.installSteps") });
    expect(within(steps).getByText(/yay -S 1password-cli/)).toBeInTheDocument();
    expect(within(steps).getByText(/winget/)).toBeInTheDocument();
    expect(screen.getByText("Mock keyring")).toBeInTheDocument();
  });

  it("shows the 1Password CLI version and features when it is installed", async () => {
    const providers: ProviderEntry[] = [
      {
        kind: "onepassword",
        state: "available",
        version: "2.41.1",
        backend: null,
        features: [
          { feature: "item_read", available: true },
          { feature: "environments", available: true },
          { feature: "environment_listing", available: false },
        ],
      },
    ];
    renderWithProviders(<SecretsSection />, { commands: { secretProviders: async () => ok(providers) } });
    expect(await screen.findByText(t("settings.secrets.op.version", { version: "2.41.1" }))).toBeInTheDocument();
    expect(screen.queryByText("1Password CLI not found")).toBeNull();
    expect(
      screen.getByText(
        t("settings.secrets.op.feature", {
          feature: t("settings.secrets.op.feature.environmentListing"),
          available: t("settings.secrets.no"),
        }),
      ),
    ).toBeInTheDocument();
  });

  it("lists aliases with the machine badge and the undefined ones", async () => {
    renderSection();
    const table = await screen.findByRole("table", { name: t("settings.secrets.aliases.label") });
    const github = within(table).getByRole("row", { name: /GITHUB_PAT/ });
    expect(within(github).getByText(t("settings.secrets.thisMachine"))).toBeInTheDocument();
    expect(within(github).getByText("keyring://loadout/GITHUB_PAT")).toBeInTheDocument();
    expect(within(github).getByText("github")).toBeInTheDocument();
    const slack = within(table).getByRole("row", { name: /SLACK_TOKEN/ });
    expect(within(slack).queryByText(t("settings.secrets.thisMachine"))).toBeNull();
    const undefinedList = screen.getByRole("list", { name: "Used but not defined" });
    expect(within(undefinedList).getByText("LINEAR_KEY")).toBeInTheDocument();
  });

  it("checks secrets only when Check all is clicked", async () => {
    const { user, transport } = renderSection({ commands: { checkSecrets: async () => ok([CHECK]) } });
    const checkAll = await screen.findByRole("button", { name: "Check all" });
    await waitFor(() => expect(transport.calls).toContain("secretProviders"));
    expect(transport.calls).not.toContain("checkSecrets");

    await user.click(checkAll);
    const row = await screen.findByRole("article", { name: "GITHUB_PAT" });
    expect(within(row).getByText(t("settings.secrets.status.missing"))).toBeInTheDocument();
    expect(transport.invocations.filter((call) => call.command === "checkSecrets").map((call) => call.args)).toEqual([
      [[]],
    ]);
  });

  it("sets the service account token through a one-way input", async () => {
    const { user, transport } = renderSection();
    await user.click(await screen.findByRole("button", { name: t("settings.secrets.serviceAccount.setAction") }));
    const dialog = await screen.findByRole("dialog", { name: t("settings.secrets.serviceAccount.label") });
    const input = within(dialog).getByLabelText(t("settings.secrets.serviceAccount.input"));
    expect(input).toHaveAttribute("type", "password");
    await user.type(input, "ops_token");
    await user.click(within(dialog).getByRole("button", { name: t("settings.secrets.save") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "setOpServiceAccount")?.args).toEqual([
        "ops_token",
      ]),
    );
    // Clear is offered only once a token is stored.
    expect(screen.getByRole("button", { name: t("settings.secrets.serviceAccount.clearAction") })).toBeDisabled();
  });

  it("maps the picked names of a 1Password Environment", async () => {
    const { user, transport } = renderSection({
      commands: {
        listOpEnvironment: async () => ok({ names: ["GITHUB_PAT", "SLACK_TOKEN"] }),
        mapOpEnvironment: async (_id, names) =>
          ok({
            mapped: names.filter((name) => name !== "GITHUB_PAT"),
            kept: ["GITHUB_PAT"],
            replaced: null,
            change: { commit: null, apply: null },
          }),
      },
    });
    await user.click(await screen.findByRole("button", { name: "Map a 1Password Environment..." }));
    const dialog = await screen.findByRole("dialog", { name: t("settings.secrets.map.title") });
    await user.type(within(dialog).getByRole("textbox", { name: t("settings.secrets.map.id") }), "env-1");
    await user.click(within(dialog).getByRole("button", { name: t("settings.secrets.map.load") }));
    const slack = await within(dialog).findByRole("checkbox", { name: "SLACK_TOKEN" });
    expect(slack).toBeChecked();
    await user.click(within(dialog).getByRole("checkbox", { name: "GITHUB_PAT" }));
    await user.click(within(dialog).getByRole("button", { name: t("settings.secrets.map.submit") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "mapOpEnvironment")?.args).toEqual([
        "env-1",
        ["SLACK_TOKEN"],
        null,
      ]),
    );
    expect(
      await screen.findByText(t("settings.secrets.map.kept", { aliases: "GITHUB_PAT" })),
    ).toBeInTheDocument();
  });

  it("takes typed names when the CLI cannot list them", async () => {
    const { user, transport } = renderSection({
      commands: { listOpEnvironment: async () => ok({ names: null }) },
    });
    await user.click(await screen.findByRole("button", { name: "Map a 1Password Environment..." }));
    const dialog = await screen.findByRole("dialog", { name: t("settings.secrets.map.title") });
    await user.type(within(dialog).getByRole("textbox", { name: t("settings.secrets.map.id") }), "env-2");
    await user.click(within(dialog).getByRole("button", { name: t("settings.secrets.map.load") }));
    const names = await within(dialog).findByRole("textbox", { name: t("settings.secrets.map.typeNames") });
    await user.type(names, "A_KEY, B_KEY{enter}C_KEY");
    await user.click(within(dialog).getByRole("button", { name: t("settings.secrets.map.submit") }));
    await waitFor(() =>
      expect(transport.invocations.find((call) => call.command === "mapOpEnvironment")?.args).toEqual([
        "env-2",
        ["A_KEY", "B_KEY", "C_KEY"],
        null,
      ]),
    );
  });
});
