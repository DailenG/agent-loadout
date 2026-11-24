import { screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { McpDraft, McpSourceReview, McpSourceSaved } from "@/ipc/bindings";
import type { CommandResult } from "@/ipc/transport";
import {
  MOCK_MCP_DRAFT,
  MOCK_MCP_REVIEW,
  MOCK_SERVER_LIST,
  MOCK_TRUST_ASK,
  mockIpcError,
  ok,
  type MockTransportOptions,
} from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { McpSourcePanel } from "./McpSourcePanel";

const REPO_DRAFT: McpDraft = {
  ...MOCK_MCP_DRAFT,
  key: "draft-2",
  kind: "package",
  file: "package.json",
  converter: "npm",
  entry: null,
  draft: { ...MOCK_MCP_DRAFT.draft, id: "weather-dev", displayName: "Weather dev", env: [] },
  launch: "npx -y github:acme/weather-mcp#3f2a9c1",
  inputs: [],
  runsRepoCode: true,
  scan: {
    verdict: "dangerous",
    findings: [
      {
        scanner: "builtin",
        rule: "shell-runs-download",
        severity: "dangerous",
        file: "scripts/postinstall.js",
        line: 9,
        message: "Runs a downloaded file with a shell.",
        excerpt: null,
      },
    ],
    rulesVersion: 1,
    cached: false,
    customRules: false,
    warnings: [],
  },
};

function renderPanel(commands: MockTransportOptions["commands"] = {}) {
  return renderWithProviders(<McpSourcePanel />, {
    commands: { listServers: async () => ok(MOCK_SERVER_LIST), ...commands },
  });
}

async function findServers(user: UserEvent) {
  await user.type(screen.getByRole("textbox", { name: "Git repository URL" }), "https://github.com/acme/weather-mcp");
  await user.type(screen.getByRole("textbox", { name: "Folder in the repository (optional)" }), "servers/weather");
  await user.type(screen.getByRole("textbox", { name: "Branch, tag or commit (optional)" }), "v1.2.0");
  await user.click(screen.getByRole("button", { name: "Find servers" }));
  return screen.findByRole("region", { name: "Proposed servers" });
}

function saveCalls(transport: { invocations: { command: string; args: unknown[] }[] }) {
  return transport.invocations.filter((call) => call.command === "saveMcpSourceDrafts").map((call) => call.args);
}

async function editCommand(user: UserEvent, name: string, command: string) {
  await user.click(screen.getByRole("button", { name: `Review and edit ${name}` }));
  const editor = await screen.findByRole("dialog", { name: `Review ${name}` });
  const field = within(editor).getByRole("textbox", { name: "Command" });
  await user.clear(field);
  await user.type(field, command);
  await user.click(within(editor).getByRole("button", { name: "Keep changes" }));
}

describe("McpSourcePanel", () => {
  it("reviews a git source and saves the picked draft for the chosen clients", async () => {
    const { user, transport } = renderPanel();
    const review = await findServers(user);
    expect(transport.invocations).toContainEqual({
      command: "reviewMcpGitSource",
      args: ["https://github.com/acme/weather-mcp", "servers/weather", "v1.2.0"],
    });
    expect(within(review).getByText("From server.json, packages[0] (registry converter)")).toBeInTheDocument();
    expect(within(review).getByText("npx -y @acme/weather-mcp@1.2.0")).toBeInTheDocument();
    expect(within(review).getByText(/becomes the secret WEATHER_API_KEY/)).toBeInTheDocument();
    // A single draft is picked already.
    expect(within(review).getByRole("checkbox", { name: "Add Weather" })).toBeChecked();
    await user.click(within(review).getByRole("checkbox", { name: "Claude Code" }));
    await user.click(within(review).getByRole("button", { name: "Save server" }));
    await waitFor(() => expect(saveCalls(transport)).toHaveLength(1));
    const [token, picks, trust, movedTrust, allowDangerous] = saveCalls(transport)[0];
    expect([token, trust, movedTrust, allowDangerous]).toEqual(["mcp-review-1", null, null, false]);
    expect(picks).toEqual([
      { key: "draft-1", draft: { ...MOCK_MCP_DRAFT.draft, clients: { allDetected: false, clients: ["claude-code"] } } },
    ]);
    expect(await screen.findByText(/Added weather/)).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Proposed servers" })).toBeNull();
  });

  it("says plainly that a repository-as-server draft runs the repository's code", async () => {
    const review: McpSourceReview = { ...MOCK_MCP_REVIEW, drafts: [MOCK_MCP_DRAFT, REPO_DRAFT] };
    const { user } = renderPanel({ reviewMcpGitSource: async () => ok(review) });
    const region = await findServers(user);
    const items = within(region).getAllByRole("listitem").filter((item) => item.classList.contains("mcp-draft"));
    expect(within(items[0]).queryByText(/runs code from this repository every time a client starts it/)).toBeNull();
    expect(
      within(items[1]).getByText(/This server runs code from this repository every time a client starts it\./),
    ).toBeInTheDocument();
    // Several drafts: none picked until the user picks.
    expect(within(region).getByRole("button", { name: "Save server" })).toBeDisabled();
  });

  it("gates a dangerous draft behind Save anyway", async () => {
    const review: McpSourceReview = { ...MOCK_MCP_REVIEW, drafts: [REPO_DRAFT] };
    const { user, transport } = renderPanel({ reviewMcpGitSource: async () => ok(review) });
    const region = await findServers(user);
    expect(within(region).getByText("shell-runs-download")).toBeInTheDocument();
    expect(within(region).getByText("scripts/postinstall.js:9")).toBeInTheDocument();
    expect(within(region).queryByRole("button", { name: "Save server" })).toBeNull();
    const anyway = within(region).getByRole("button", { name: "Save anyway" });
    expect(anyway).toBeDisabled();
    await user.click(within(region).getByRole("checkbox", { name: "I read the findings and want to go ahead anyway" }));
    await user.click(anyway);
    await waitFor(() => expect(saveCalls(transport).map((args) => args[4])).toEqual([true]));
  });

  it("turns a scan_blocked save (edited command) into the Save anyway gate", async () => {
    const { user, transport } = renderPanel({
      saveMcpSourceDrafts: async (_token, _picks, _trust, _moved, allowDangerous) =>
        allowDangerous
          ? ok({ ids: ["weather"], change: { commit: null, apply: null }, scan: MOCK_MCP_REVIEW.scan })
          : mockIpcError({ code: "scan_blocked", message: "dangerous" }),
    });
    const region = await findServers(user);
    await user.click(within(region).getByRole("button", { name: "Save server" }));
    const anyway = await within(region).findByRole("button", { name: "Save anyway" });
    await user.click(within(region).getByRole("checkbox", { name: "I read the findings and want to go ahead anyway" }));
    await user.click(anyway);
    await waitFor(() => expect(saveCalls(transport).map((args) => args[4])).toEqual([false, true]));
  });

  it("asks the trust question before saving and sends the grant", async () => {
    const { user, transport } = renderPanel({
      reviewMcpGitSource: async () => ok({ ...MOCK_MCP_REVIEW, trust: MOCK_TRUST_ASK }),
    });
    const region = await findServers(user);
    await user.click(within(region).getByRole("button", { name: "Save server" }));
    const trust = await screen.findByRole("alertdialog", { name: "Trust this source?" });
    await user.click(within(trust).getByRole("radio", { name: "Always trust github.com/acme/skills" }));
    await user.click(within(trust).getByRole("button", { name: "Always trust and continue" }));
    await waitFor(() =>
      expect(saveCalls(transport).map((args) => args[2])).toEqual([{ kind: "always", scope: "repo" }]),
    );
  });

  it("saves a draft as edited in the server form", async () => {
    const { user, transport } = renderPanel();
    const region = await findServers(user);
    await user.click(within(region).getByRole("button", { name: "Review and edit Weather" }));
    const editor = await screen.findByRole("dialog", { name: "Review Weather" });
    expect(within(editor).getByText("From server.json, packages[0] (registry converter)")).toBeInTheDocument();
    const id = within(editor).getByRole("textbox", { name: "ID" });
    await user.clear(id);
    await user.type(id, "acme-weather");
    await user.click(within(editor).getByRole("button", { name: "Keep changes" }));
    await user.click(within(region).getByRole("button", { name: "Save server" }));
    await waitFor(() => expect(saveCalls(transport)).toHaveLength(1));
    const picks = saveCalls(transport)[0][1] as { draft: { id: string } }[];
    expect(picks.map((pick) => pick.draft.id)).toEqual(["acme-weather"]);
  });

  it("discards the review", async () => {
    const { user, transport } = renderPanel();
    const region = await findServers(user);
    await user.click(within(region).getByRole("button", { name: "Discard" }));
    expect(transport.invocations).toContainEqual({ command: "discardMcpSourceReview", args: ["mcp-review-1"] });
    expect(saveCalls(transport)).toEqual([]);
  });

  it("reviews a picked folder by its token", async () => {
    const { user, transport } = renderPanel({
      pickSkillSource: async () => ok({ token: "folder-9", path: "~/code/weather", name: "weather" }),
    });
    await user.click(screen.getByRole("radio", { name: "Folder" }));
    await user.click(screen.getByRole("button", { name: "Choose folder…" }));
    expect(await screen.findByText("~/code/weather")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Find servers" }));
    await screen.findByRole("region", { name: "Proposed servers" });
    expect(transport.invocations).toContainEqual({ command: "reviewMcpLocalSource", args: ["folder-9"] });
  });

  it("names the edited server and the reported findings when the edited command scans dangerous", async () => {
    const { user, transport } = renderPanel({
      saveMcpSourceDrafts: async (_token, _picks, _trust, _moved, allowDangerous) =>
        allowDangerous
          ? ok({ ids: ["weather"], change: { commit: null, apply: null }, scan: MOCK_MCP_REVIEW.scan })
          : mockIpcError({ code: "scan_blocked", message: "found 1 dangerous finding: pipe-to-shell (command line)" }),
    });
    const region = await findServers(user);
    await editCommand(user, "Weather", "sh");
    // The row shows the command as it will be saved, and that its scan on screen no longer applies.
    expect(within(region).getByText("sh -y @acme/weather-mcp@1.2.0")).toBeInTheDocument();
    expect(within(region).getByText("Edited; the command is scanned again when you save.")).toBeInTheDocument();
    await user.click(within(region).getByRole("button", { name: "Save server" }));
    expect(await within(region).findByText(/dangerous content in weather as edited\. Nothing was saved/)).toBeInTheDocument();
    expect(within(region).getByText("What the scan reported: found 1 dangerous finding: pipe-to-shell (command line)")).toBeInTheDocument();
    expect(within(region).queryByText(/install anyway/i)).toBeNull();
    expect(within(region).getByRole("button", { name: "Save anyway" })).toBeDisabled();

    // Another edit: the refusal and its "Save anyway" were for the content before it.
    await editCommand(user, "Weather", "npx");
    expect(within(region).queryByRole("button", { name: "Save anyway" })).toBeNull();
    expect(within(region).getByRole("button", { name: "Save server" })).toBeEnabled();
    expect(saveCalls(transport).map((args) => args[4])).toEqual([false]);
  });

  it("clears the acknowledgement when the picks change", async () => {
    const review: McpSourceReview = { ...MOCK_MCP_REVIEW, drafts: [MOCK_MCP_DRAFT, REPO_DRAFT] };
    const { user } = renderPanel({ reviewMcpGitSource: async () => ok(review) });
    const region = await findServers(user);
    await user.click(within(region).getByRole("checkbox", { name: "Add Weather dev" }));
    const ack = within(region).getByRole("checkbox", { name: "I read the findings and want to go ahead anyway" });
    await user.click(ack);
    expect(within(region).getByRole("button", { name: "Save anyway" })).toBeEnabled();
    await user.click(within(region).getByRole("checkbox", { name: "Add Weather" }));
    expect(within(region).getByRole("checkbox", { name: "I read the findings and want to go ahead anyway" })).not.toBeChecked();
    expect(within(region).getByRole("button", { name: "Save anyway" })).toBeDisabled();
  });

  it("keeps Discard and Find servers off while a save is in flight", async () => {
    let finish: (value: CommandResult<McpSourceSaved>) => void = () => undefined;
    const { user, transport } = renderPanel({ saveMcpSourceDrafts: () => new Promise((resolve) => (finish = resolve)) });
    const region = await findServers(user);
    await user.click(within(region).getByRole("button", { name: "Save server" }));
    expect(within(region).getByRole("button", { name: "Discard" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Find servers" })).toBeDisabled();
    finish(ok({ ids: ["weather"], change: { commit: null, apply: null }, scan: MOCK_MCP_REVIEW.scan }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Proposed servers" })).toBeNull());
    expect(transport.calls).not.toContain("discardMcpSourceReview");
  });

  it("shows every launch line, warns when not pinned and names folders it did not read", async () => {
    const draft: McpDraft = {
      ...MOCK_MCP_DRAFT,
      launch: "npx -y @acme/weather-mcp",
      launchLines: [
        { os: null, line: "npx -y @acme/weather-mcp" },
        { os: "linux", line: "npx -y @acme/weather-mcp --linux" },
      ],
      pinned: false,
    };
    const { user } = renderPanel({
      reviewMcpGitSource: async () => ok({ ...MOCK_MCP_REVIEW, drafts: [draft], skippedFolders: ["node_modules", "dist"] }),
    });
    const region = await findServers(user);
    expect(within(region).getByText("npx -y @acme/weather-mcp")).toBeInTheDocument();
    expect(within(region).getByText("On Linux")).toBeInTheDocument();
    expect(within(region).getByText("npx -y @acme/weather-mcp --linux")).toBeInTheDocument();
    expect(within(region).getByText(/^Not pinned:/)).toBeInTheDocument();
    expect(within(region).getByText("Not read or scanned (dependency and build folders): node_modules, dist.")).toBeInTheDocument();
  });
});
