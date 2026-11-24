import { screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import type { AliasEntry } from "@/ipc/bindings";
import { ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { SecuritySection } from "./SecuritySection";

function invoked(transport: { invocations: { command: string; args: unknown[] }[] }, command: string) {
  return transport.invocations.filter((call) => call.command === command).map((call) => call.args);
}

function alias(name: string, reference: string): AliasEntry {
  return { alias: name, reference, source: "store", storeReference: null, description: null, envName: name, servers: [] };
}

/** Settings with a way to leave it and come back, as navigating does (the app's caches stay). */
function Leavable() {
  const [shown, setShown] = useState(true);
  return (
    <>
      <button type="button" onClick={() => setShown((value) => !value)}>
        {shown ? "Leave Settings" : "Back to Settings"}
      </button>
      {shown ? <SecuritySection /> : null}
    </>
  );
}

describe("SecuritySection (Snyk Agent Scan)", () => {
  it("discloses what is sent to Snyk's cloud before anything is on, and sends nothing itself", async () => {
    const { transport, user } = renderWithProviders(<SecuritySection />);
    expect(await screen.findByText("Snyk Agent Scan (optional)")).toBeInTheDocument();
    expect(screen.getByText(/It scans skills only/)).toBeInTheDocument();
    expect(screen.getByText(/as soon as an install or update review opens, before you choose Install or Update/)).toBeInTheDocument();
    expect(screen.getByText(/Snyk's analysis service in the cloud \(api\.snyk\.io\)/)).toBeInTheDocument();
    const sent = screen.getByRole("list", { name: "What is sent to Snyk for each scan:" });
    expect(within(sent).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "every file of the skill (Snyk redacts secrets it detects in them)",
      "the skill's name (from SKILL.md)",
      "the full path of the folder it scans (a temporary copy in Loadout's data folder)",
      "your user name on this computer",
      "this machine's host name, which the scanner adds itself where its platform reports one",
      "the scanner's version",
    ]);
    expect(screen.getByText(/needs a Snyk API token \(SNYK_TOKEN\)\. The free tier has a daily limit/)).toBeInTheDocument();
    expect(screen.getByText(/never sends anything to Snyk while it's off/)).toBeInTheDocument();
    // Snyk's terms and privacy policy through the fixed help topics only.
    await user.click(screen.getByRole("button", { name: "Snyk Agent Scan terms" }));
    await user.click(screen.getByRole("button", { name: "Snyk privacy policy" }));
    expect(invoked(transport, "openHelp")).toEqual([["snyk_terms"], ["snyk_privacy"]]);
    await screen.findByText("Not installed");
    expect(screen.getByRole("checkbox", { name: "Use Snyk Agent Scan" })).toBeDisabled();
    expect(transport.calls.filter((call) => call.startsWith("externalScan") || call === "installScanner")).toEqual([]);
  });

  it("installs with announced progress, shows the verified version, sets a token reference and only then enables", async () => {
    const { user, transport } = renderWithProviders(<SecuritySection />, { scanner: { installStepMs: 40 } });
    expect(await screen.findByText(/Install downloads Snyk Agent Scan 0\.7\.1 \(25\.5 MB\)/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Install" }));
    const bar = await screen.findByRole("progressbar", { name: "Download progress" });
    await waitFor(() => expect(bar.getAttribute("aria-valuetext")).toMatch(/^\d+\.\d MB of 25\.5 MB$/));
    expect(screen.getByText("Downloading Snyk Agent Scan.")).toHaveAttribute("role", "status");
    expect(
      await screen.findByText("Snyk Agent Scan 0.7.1, installed and verified: its SHA-256 matches the build pinned in Loadout."),
    ).toBeInTheDocument();
    expect(screen.getByText("Snyk Agent Scan 0.7.1 is installed and verified. Turn it on when you're ready.")).toBeInTheDocument();
    // Installed, no token: still off and can't be turned on.
    expect(screen.getByText("Needs a token")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Use Snyk Agent Scan" })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Set token…" }));
    const dialog = await screen.findByRole("dialog", { name: "Snyk token" });
    await user.click(within(dialog).getByRole("radio", { name: "Environment variable" }));
    await user.type(within(dialog).getByRole("textbox", { name: "Variable name" }), "SNYK_TOKEN");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(invoked(transport, "setScannerToken")).toEqual([["env:SNYK_TOKEN"]]));
    expect(await screen.findByText("env:SNYK_TOKEN")).toBeInTheDocument();

    const enable = screen.getByRole("checkbox", { name: "Use Snyk Agent Scan" });
    expect(enable).toBeEnabled();
    expect(enable).not.toBeChecked();
    await user.click(enable);
    await waitFor(() => expect(invoked(transport, "setScannerEnabled")).toEqual([[true]]));
    expect(await screen.findByText("On")).toBeInTheDocument();
  });

  it("keeps an install's progress and Cancel across leaving Settings, and never starts a second one", async () => {
    const { user, transport } = renderWithProviders(<Leavable />, { scanner: { installStepMs: 150 } });
    await user.click(await screen.findByRole("button", { name: "Install" }));
    await screen.findByRole("progressbar", { name: "Download progress" });
    await user.click(screen.getByRole("button", { name: "Leave Settings" }));
    await user.click(screen.getByRole("button", { name: "Back to Settings" }));
    expect(await screen.findByRole("progressbar", { name: "Download progress" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Install" })).toBeNull();
    expect(invoked(transport, "installScanner")).toHaveLength(1);
    expect(
      await screen.findByText(
        "Snyk Agent Scan 0.7.1, installed and verified: its SHA-256 matches the build pinned in Loadout.",
        undefined,
        { timeout: 3_000 },
      ),
    ).toBeInTheDocument();
  });

  it("cancels an install in progress and installs nothing", async () => {
    const { user, transport } = renderWithProviders(<SecuritySection />, { scanner: { installStepMs: 200 } });
    await user.click(await screen.findByRole("button", { name: "Install" }));
    await screen.findByRole("progressbar", { name: "Download progress" });
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(invoked(transport, "cancelExternalScan")).toEqual([[null]]);
    expect(await screen.findByText("Install canceled; nothing was installed.")).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Install" })).toBeInTheDocument();
    expect(screen.getByText("Not installed")).toBeInTheDocument();
  });

  it("says why an install was refused", async () => {
    const { user } = renderWithProviders(<SecuritySection />, {
      scanner: { installError: { code: "scanner_verification_failed", message: "sha256 mismatch" } },
    });
    await user.click(await screen.findByRole("button", { name: "Install" }));
    expect(await screen.findByText(/didn't match the build pinned in Loadout, so it was deleted/)).toBeInTheDocument();
    expect(screen.getByText("Not installed")).toBeInTheDocument();
  });

  it("removes the scanner after a confirmation that starts on Cancel, and turns it off", async () => {
    const { user, transport } = renderWithProviders(<SecuritySection />, {
      scanner: { installed: true, enabled: true, tokenRef: "env:SNYK_TOKEN" },
    });
    await user.click(await screen.findByRole("button", { name: "Remove" }));
    const confirm = await screen.findByRole("alertdialog", { name: "Remove Snyk Agent Scan?" });
    expect(within(confirm).getByText(/turns Snyk Agent Scan off/)).toBeInTheDocument();
    await waitFor(() => expect(within(confirm).getByRole("button", { name: "Cancel" })).toHaveFocus());
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(transport.calls).not.toContain("removeScanner");

    await user.click(screen.getByRole("button", { name: "Remove" }));
    const again = await screen.findByRole("alertdialog", { name: "Remove Snyk Agent Scan?" });
    await user.click(within(again).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(transport.calls).toContain("removeScanner"));
    // The backend turned it off already: no second call.
    expect(invoked(transport, "setScannerEnabled")).toEqual([]);
    expect(await screen.findByRole("button", { name: "Install" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Use Snyk Agent Scan" })).not.toBeChecked();
    // Reinstalling doesn't silently start sending skills again.
    await user.click(screen.getByRole("button", { name: "Install" }));
    expect(
      await screen.findByText("Snyk Agent Scan 0.7.1 is installed and verified. Turn it on when you're ready."),
    ).toBeInTheDocument();
  });

  it("turns the switch off itself when Remove left it on", async () => {
    const { user, transport } = renderWithProviders(<SecuritySection />, {
      scanner: { installed: true, enabled: true, tokenRef: "env:SNYK_TOKEN" },
      commands: {
        removeScanner: async () =>
          ok({
            scanner: "snyk-agent-scan",
            version: "0.7.1",
            supported: true,
            installed: false,
            enabled: true,
            tokenRef: "env:SNYK_TOKEN",
            tokenKind: "env",
            sizeBytes: 25_480_568,
            timeoutSecs: 60,
          }),
      },
    });
    await user.click(await screen.findByRole("button", { name: "Remove" }));
    const confirm = await screen.findByRole("alertdialog", { name: "Remove Snyk Agent Scan?" });
    await user.click(within(confirm).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(invoked(transport, "setScannerEnabled")).toEqual([[false]]));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "Use Snyk Agent Scan" })).not.toBeChecked());
  });

  it("handles the token only as a reference and announces the check", async () => {
    const { user, transport, container } = renderWithProviders(<SecuritySection />, {
      scanner: { installed: true, tokenRef: "op://Private/Snyk/token", tokenCheck: "token_unavailable" },
      commands: {
        secretsOverview: async () =>
          ok({
            aliases: [alias("SNYK_TOKEN", "keyring://loadout/SNYK_TOKEN"), alias("TEAM_ENV", "op-env://Team/ci/SNYK_TOKEN")],
            undefined: [],
            environment: null,
            opAccount: null,
            serviceAccountSet: false,
          }),
      },
    });
    expect(await screen.findByText("op://Private/Snyk/token")).toBeInTheDocument();
    expect(screen.getByText(/1Password reference:/)).toBeInTheDocument();

    // The status line is there before the check, so its answer is announced.
    const token = screen.getByText("Snyk token").closest(".settings-form-item") as HTMLElement;
    const status = within(token).getByRole("status");
    expect(status).toHaveTextContent("");
    await user.click(screen.getByRole("button", { name: "Check" }));
    await waitFor(() => expect(status).toHaveTextContent("Loadout can't use the token: the token couldn't be read."));
    expect(transport.calls).not.toContain("externalScanSkill");

    await user.click(screen.getByRole("button", { name: "Change…" }));
    const dialog = await screen.findByRole("dialog", { name: "Snyk token" });
    // No field takes the token itself.
    expect(container.ownerDocument.querySelectorAll('input[type="password"]')).toHaveLength(0);
    const field = within(dialog).getByRole("textbox", { name: /1Password reference/ });
    expect(field).toHaveValue("op://Private/Snyk/token");
    // A 1Password Environment reference can't hold the token: Save is off and says why.
    await user.clear(field);
    await user.type(field, "op-env://Private/env/SNYK_TOKEN");
    expect(within(dialog).getByRole("button", { name: "Save" })).toBeDisabled();
    expect(field).toHaveAccessibleDescription(/1Password Environment \(op-env:\/\/\) can't hold the scanner token/);
    // A pasted token isn't a reference either.
    await user.click(within(dialog).getByRole("radio", { name: "Environment variable" }));
    const variable = within(dialog).getByRole("textbox", { name: "Variable name" });
    await user.type(variable, "snyk_uat.0123-4567");
    expect(within(dialog).getByRole("button", { name: "Save" })).toBeDisabled();
    expect(variable).toHaveAccessibleDescription("Enter the variable's name (letters, digits and _), not the token.");

    // Aliases that point to a 1Password Environment aren't offered.
    await user.click(within(dialog).getByRole("radio", { name: "Secret from the Secrets page" }));
    expect(within(dialog).getByText(/Secrets that point to a 1Password Environment aren't listed/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("combobox", { name: "Secret" }));
    expect(screen.queryByRole("option", { name: "TEAM_ENV" })).toBeNull();
    await user.click(await screen.findByRole("option", { name: "SNYK_TOKEN" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(invoked(transport, "setScannerToken")).toEqual([["SNYK_TOKEN"]]));

    await user.click(await screen.findByRole("button", { name: "Clear" }));
    await waitFor(() => expect(invoked(transport, "setScannerToken")).toEqual([["SNYK_TOKEN"], [null]]));
    expect(await screen.findByText("Not set.")).toBeInTheDocument();
  });

  it("says when no build exists for this platform and offers no Install", async () => {
    renderWithProviders(<SecuritySection />, { scanner: { supported: false } });
    expect(await screen.findByText("Not available")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Install" })).toBeNull();
    expect(screen.getByRole("checkbox", { name: "Use Snyk Agent Scan" })).toBeDisabled();
  });
});
