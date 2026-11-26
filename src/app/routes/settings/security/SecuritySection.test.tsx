import { screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { renderWithProviders } from "@/test/renderWithProviders";
import { SCANNER_NEEDS_PREPARE, SCANNER_ON, SCANNER_READY } from "@/test/scannerData";
import { SecuritySection } from "./SecuritySection";

function invoked(transport: { invocations: { command: string; args: unknown[] }[] }, command: string) {
  return transport.invocations.filter((call) => call.command === command).map((call) => call.args);
}

/** Nothing that runs SkillSpector or downloads it. */
function runCalls(transport: { calls: string[] }) {
  return transport.calls.filter((call) => call.startsWith("externalScan") || call === "prepareScanner");
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

const SWITCH = "Use SkillSpector";

describe("SecuritySection (SkillSpector)", () => {
  it("discloses that it runs here and exactly what can leave the machine, before anything is on, and runs nothing", async () => {
    const { transport, user } = renderWithProviders(<SecuritySection />);
    expect(await screen.findByText("SkillSpector (optional)")).toBeInTheDocument();
    expect(screen.getByText(/runs it in every install and update review as soon as the review opens/)).toBeInTheDocument();
    expect(
      screen.getByText(
        "SkillSpector runs on this computer and never uploads the skill. Nothing leaves this machine except the names and versions of the skill's dependencies, sent to OSV.dev (api.osv.dev) to look up known vulnerabilities, when the skill has a dependency manifest (requirements.txt, package.json, pyproject.toml or Pipfile).",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "Preparing the uvx form once downloads SkillSpector 2.13.7 and its dependencies (a few hundred MB) from PyPI, and a Python 3.13 build when uv has none; scans themselves never download anything.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "No account, no token, no upload of skill content. Loadout runs it only on skills, never on MCP servers (SkillSpector does not read MCP client configs), and only when you turn it on (or pass --external).",
      ),
    ).toBeInTheDocument();
    // The project through the fixed help topic only.
    await user.click(screen.getByRole("button", { name: "SkillSpector 2.13.7 on GitHub" }));
    expect(invoked(transport, "openHelp")).toEqual([["skillspector"]]);
    expect(await screen.findByText("Not available")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: SWITCH })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: SWITCH })).not.toBeChecked();
    expect(runCalls(transport)).toEqual([]);
  });

  it("gives setup instructions when neither uv nor SkillSpector is found, Python 3.14 included", async () => {
    const { transport, user } = renderWithProviders(<SecuritySection />);
    expect(await screen.findByText("Neither uv nor SkillSpector was found on PATH.")).toBeInTheDocument();
    expect(screen.getByText(/Loadout never installs SkillSpector or uv itself/)).toBeInTheDocument();
    expect(screen.getByText("pipx install skillspector==2.13.7 --python 3.13")).toBeInTheDocument();
    expect(
      screen.getByText("Python 3.14 is not supported yet: SkillSpector's YARA dependency has no build for it. Use Python 3.13."),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "How to install uv" }));
    expect(invoked(transport, "openHelp")).toEqual([["uv_install"]]);
    expect(screen.queryByRole("button", { name: "Prepare" })).toBeNull();
    expect(screen.getByText("Available once SkillSpector is ready (see Status).")).toBeInTheDocument();
    // Check again asks for the detection again.
    const before = invoked(transport, "scannerStatus").length;
    await user.click(screen.getByRole("button", { name: "Check again" }));
    await waitFor(() => expect(invoked(transport, "scannerStatus").length).toBe(before + 1));
  });

  it("shows the uvx runtime when ready, the timeout, and turns it on and off", async () => {
    const { transport, user } = renderWithProviders(<SecuritySection />, { scanner: SCANNER_READY });
    expect(await screen.findByText("SkillSpector 2.13.7 runs through uvx with Python 3.13.")).toBeInTheDocument();
    expect(screen.getByText("Ready")).toBeInTheDocument();
    expect(screen.getByText("120 s")).toBeInTheDocument();
    expect(screen.getByText(/Each scan stops after 120 seconds and then says Not scanned \(timed out\)/)).toBeInTheDocument();
    expect(screen.queryByText(/is not tested with this Loadout/)).toBeNull();
    expect(screen.queryByText(/Set up SkillSpector/)).toBeNull();
    const enable = screen.getByRole("checkbox", { name: SWITCH });
    expect(enable).toBeEnabled();
    await user.click(enable);
    await waitFor(() => expect(screen.getByRole("checkbox", { name: SWITCH })).toBeChecked());
    await user.click(screen.getByRole("checkbox", { name: SWITCH }));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: SWITCH })).not.toBeChecked());
    expect(invoked(transport, "setScannerEnabled")).toEqual([[true], [false]]);
    expect(runCalls(transport)).toEqual([]);
  });

  it("shows a skillspector on PATH with its version and the untested-version note", async () => {
    renderWithProviders(<SecuritySection />, { scanner: { ...SCANNER_ON, runtime: "path", version: "2.14.0" } });
    expect(await screen.findByText("SkillSpector 2.14.0 on PATH.")).toBeInTheDocument();
    expect(screen.getByText(/^SkillSpector 2\.14\.0 is not tested with this Loadout \(tested: 2\.13\.7\)/)).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: SWITCH })).toBeChecked();
  });

  it("prepares with uv's lines as announced progress, then can be turned on, without turning itself on", async () => {
    const { user, transport } = renderWithProviders(<SecuritySection />, {
      scanner: { ...SCANNER_NEEDS_PREPARE, prepareStepMs: 60 },
    });
    expect(await screen.findByText("Needs Prepare")).toBeInTheDocument();
    expect(screen.getByText(/uv is on PATH, but SkillSpector 2\.13\.7 isn't downloaded yet/)).toBeInTheDocument();
    expect(screen.getByText(/^Downloads SkillSpector 2\.13\.7 and its dependencies \(a few hundred MB\) from PyPI once/)).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: SWITCH })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Prepare" }));
    const bar = await screen.findByRole("progressbar", { name: "Prepare progress" });
    await waitFor(() => expect(bar.getAttribute("aria-valuetext")).toMatch(/^Downloading /));
    expect(screen.getByText("Preparing SkillSpector. This can take several minutes.")).toHaveAttribute("role", "status");
    expect(await screen.findByText("SkillSpector 2.13.7 is ready. Turn it on when you want it in reviews.", undefined, { timeout: 3_000 })).toBeInTheDocument();
    expect(screen.getByText("SkillSpector 2.13.7 runs through uvx with Python 3.13.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Prepare" })).toBeNull();
    const enable = screen.getByRole("checkbox", { name: SWITCH });
    expect(enable).toBeEnabled();
    expect(enable).not.toBeChecked();
    expect(invoked(transport, "prepareScanner")).toHaveLength(1);
    expect(transport.calls.filter((call) => call.startsWith("externalScan"))).toEqual([]);
  });

  it("keeps Prepare's progress and Cancel across leaving Settings, and never starts a second one", async () => {
    const { user, transport } = renderWithProviders(<Leavable />, { scanner: { ...SCANNER_NEEDS_PREPARE, prepareStepMs: 120 } });
    await user.click(await screen.findByRole("button", { name: "Prepare" }));
    await screen.findByRole("progressbar", { name: "Prepare progress" });
    await user.click(screen.getByRole("button", { name: "Leave Settings" }));
    await user.click(screen.getByRole("button", { name: "Back to Settings" }));
    expect(await screen.findByRole("progressbar", { name: "Prepare progress" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel Prepare" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Prepare" })).toBeNull();
    expect(invoked(transport, "prepareScanner")).toHaveLength(1);
    expect(
      await screen.findByText("SkillSpector 2.13.7 runs through uvx with Python 3.13.", undefined, {
        timeout: 3_000,
      }),
    ).toBeInTheDocument();
  });

  it("cancels a Prepare in progress, keeping focus on the one Prepare / Cancel Prepare button", async () => {
    const { user, transport } = renderWithProviders(<SecuritySection />, { scanner: { ...SCANNER_NEEDS_PREPARE, prepareStepMs: 200 } });
    await user.click(await screen.findByRole("button", { name: "Prepare" }));
    await screen.findByRole("progressbar", { name: "Prepare progress" });
    expect(screen.getByRole("button", { name: "Cancel Prepare" })).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Cancel Prepare" }));
    expect(invoked(transport, "cancelExternalScan")).toEqual([[null]]);
    expect(await screen.findByText("Prepare canceled. SkillSpector isn't ready yet.")).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Prepare" })).toHaveFocus();
    expect(screen.getByText("Needs Prepare")).toBeInTheDocument();
  });

  it("moves focus to the Status badge once Prepare succeeds and its row goes away", async () => {
    const { user } = renderWithProviders(<SecuritySection />, { scanner: { ...SCANNER_NEEDS_PREPARE, prepareStepMs: 60 } });
    await user.click(await screen.findByRole("button", { name: "Prepare" }));
    expect(await screen.findByText("SkillSpector 2.13.7 runs through uvx with Python 3.13.", undefined, { timeout: 3_000 })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Prepare/ })).toBeNull();
    expect(screen.getByText("Ready")).toHaveFocus();
  });

  it("cancels a Prepare whose Cancel reached the backend before Prepare itself did", async () => {
    const { user, transport } = renderWithProviders(<SecuritySection />, { scanner: { ...SCANNER_NEEDS_PREPARE, prepareStepMs: 60 } });
    // The progress subscription takes a while, so Cancel is clicked before `prepareScanner` is called.
    const subscribe = transport.events.onScannerPrepareProgress;
    let release: () => void = () => undefined;
    const subscribed = new Promise<void>((resolve) => {
      release = resolve;
    });
    transport.events.onScannerPrepareProgress = async (handler) => {
      await subscribed;
      return subscribe(handler);
    };
    await user.click(await screen.findByRole("button", { name: "Prepare" }));
    await user.click(await screen.findByRole("button", { name: "Cancel Prepare" }));
    expect(invoked(transport, "prepareScanner")).toHaveLength(0);
    release();
    expect(await screen.findByText("Prepare canceled. SkillSpector isn't ready yet.")).toBeInTheDocument();
    expect(invoked(transport, "prepareScanner")).toHaveLength(1);
    expect(screen.getByText("Needs Prepare")).toBeInTheDocument();
  });

  it("runs Prepare once more when the backend answers with a cancel the user didn't ask for this time", async () => {
    const { user, transport } = renderWithProviders(<SecuritySection />, { scanner: { ...SCANNER_NEEDS_PREPARE, prepareStepMs: 30 } });
    // A cancel that arrived after an earlier Prepare ended: the backend keeps it for the next one.
    await transport.commands.cancelExternalScan(null);
    await user.click(await screen.findByRole("button", { name: "Prepare" }));
    expect(await screen.findByText("SkillSpector 2.13.7 runs through uvx with Python 3.13.", undefined, { timeout: 3_000 })).toBeInTheDocument();
    expect(invoked(transport, "prepareScanner")).toHaveLength(2);
    expect(screen.queryByText("Prepare canceled. SkillSpector isn't ready yet.")).toBeNull();
  });

  it("can be turned off while Prepare runs", async () => {
    const { user, transport } = renderWithProviders(<SecuritySection />, {
      scanner: { ...SCANNER_NEEDS_PREPARE, enabled: true, prepareStepMs: 200 },
    });
    await user.click(await screen.findByRole("button", { name: "Prepare" }));
    await screen.findByRole("progressbar", { name: "Prepare progress" });
    const enable = screen.getByRole("checkbox", { name: SWITCH });
    expect(enable).toBeChecked();
    expect(enable).toBeEnabled();
    await user.click(enable);
    await waitFor(() => expect(screen.getByRole("checkbox", { name: SWITCH })).not.toBeChecked());
    expect(invoked(transport, "setScannerEnabled")).toEqual([[false]]);
    // Off, it can't be turned back on until Prepare ends.
    expect(screen.getByRole("checkbox", { name: SWITCH })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Cancel Prepare" }));
  });

  it("says why a Prepare failed, with uv's last line", async () => {
    const { user } = renderWithProviders(<SecuritySection />, {
      scanner: {
        ...SCANNER_NEEDS_PREPARE,
        prepareError: { code: "scanner_download_failed", message: "error: Failed to fetch: `https://pypi.org/simple/skillspector/`" },
      },
    });
    await user.click(await screen.findByRole("button", { name: "Prepare" }));
    expect(
      await screen.findByText("SkillSpector could not be downloaded. Nothing was turned on; check the connection and try Prepare again."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Last message from uv: error: Failed to fetch: `https://pypi.org/simple/skillspector/`"),
    ).toBeInTheDocument();
    expect(screen.getByText("Needs Prepare")).toBeInTheDocument();
  });

  it("lets a switch left on be turned off when SkillSpector can't run here, and says so", async () => {
    const { user, transport } = renderWithProviders(<SecuritySection />, { scanner: { enabled: true } });
    expect(await screen.findByText(/^SkillSpector is on but can't run here yet/)).toBeInTheDocument();
    const enable = screen.getByRole("checkbox", { name: SWITCH });
    expect(enable).toBeChecked();
    expect(enable).toBeEnabled();
    await user.click(enable);
    await waitFor(() => expect(screen.getByRole("checkbox", { name: SWITCH })).toBeDisabled());
    expect(invoked(transport, "setScannerEnabled")).toEqual([[false]]);
  });

  it("points a 0.7.0 Snyk Agent Scan leftover to the CLI only", async () => {
    renderWithProviders(<SecuritySection />, { scanner: { snykInstalled: true } });
    const label = await screen.findByText("Snyk Agent Scan (from Loadout 0.7.0)");
    const row = label.closest(".settings-form-item") as HTMLElement;
    expect(within(row).getByText(/It's disabled and never runs/)).toBeInTheDocument();
    expect(within(row).getByText("loadout scanner remove")).toBeInTheDocument();
    expect(within(row).queryByRole("button")).toBeNull();
  });
});
