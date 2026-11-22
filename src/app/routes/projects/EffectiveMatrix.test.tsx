import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { EffectiveMatrixView } from "@/app/routes/projects/EffectiveMatrix";
import { errorMessage, t } from "@/i18n/en";
import type { EffectiveMatrix, MatrixCell } from "@/ipc/bindings";
import { ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const GITHUB_CLAUDE: MatrixCell = {
  client: "claude-code",
  winner: { layer: "project", file: "<P>/.mcp.json" },
  source: "store:github+project",
  loaded: true,
  shadowed: [{ layer: "system", file: "~/.claude.json" }],
  alsoVisibleTo: ["copilot", "grok"],
  status: "synced",
  warnings: [],
  snippet: '{\n  "mcpServers": {\n    "github": { "command": "github-mcp", "env": { "TOKEN": "********" } }\n  }\n}\n',
};

const GITHUB_GROK: MatrixCell = {
  ...GITHUB_CLAUDE,
  client: "grok",
  winner: { layer: "client_compat", file: "<P>/.mcp.json" },
  alsoVisibleTo: ["claude-code", "copilot"],
  warnings: [{ code: "duplicate_via_compat", message: "Grok loads \"github\" from ~/.grok/config.toml and from <P>/.mcp.json" }],
};

const SLACK_CLAUDE: MatrixCell = {
  client: "claude-code",
  winner: { layer: "system", file: "~/.claude.json" },
  source: "store:slack+project",
  loaded: false,
  shadowed: [],
  alsoVisibleTo: [],
  status: "synced",
  warnings: [],
  snippet: null,
};

const MATRIX: EffectiveMatrix = {
  projectId: "app-a1b2",
  projectName: "app",
  path: "~/code/app",
  clients: [
    { id: "claude-code", name: "Claude Code", detected: true },
    { id: "codex", name: "Codex", detected: true },
    { id: "copilot", name: "VS Code (Copilot)", detected: true },
    { id: "grok", name: "Grok", detected: false },
  ],
  rows: [
    { name: "github", cells: [GITHUB_CLAUDE, GITHUB_GROK] },
    { name: "slack", cells: [SLACK_CLAUDE] },
  ],
  warnings: [],
};

function renderMatrix() {
  return renderWithProviders(<EffectiveMatrixView projectId="app-a1b2" onBack={() => undefined} />, {
    commands: { effectiveMatrix: async () => ok(MATRIX) },
  });
}

describe("EffectiveMatrixView", () => {
  it("shows servers by client with the layer letter and load state", async () => {
    const { transport } = renderMatrix();
    const table = await screen.findByRole("table", { name: t("matrix.title", { name: "app" }) });
    expect(transport.invocations.find((call) => call.command === "effectiveMatrix")?.args).toEqual(["app-a1b2"]);
    const headers = within(table).getAllByRole("columnheader").map((cell) => cell.textContent);
    expect(headers).toEqual([t("matrix.server"), "Claude Code", "Codex", "VS Code (Copilot)", "Grok"]);

    const github = within(table).getByRole("button", {
      name: t("matrix.cell.aria", {
        server: "github",
        client: "Claude Code",
        layer: t("matrix.layer.project"),
        state: t("apply.state.synced"),
      }),
    });
    expect(github).toHaveTextContent("P✓");
    const compat = within(table).getByRole("button", { name: /github in Grok/ });
    expect(compat).toHaveTextContent("C⚠");
    const slack = within(table).getByRole("button", { name: /slack in Claude Code/ });
    expect(slack).toHaveTextContent("S⊘");
    // A client without an entry shows a dash.
    expect(within(table).getAllByLabelText(t("matrix.cell.none")).length).toBeGreaterThan(0);
    expect(screen.getByText(t("matrix.legend"))).toBeInTheDocument();
  });

  it("shows the provenance chain of a cell", async () => {
    const { user } = renderMatrix();
    await user.click(await screen.findByRole("button", { name: /github in Claude Code/ }));
    const detail = screen.getByRole("complementary", {
      name: t("matrix.detail.title", { server: "github", client: "Claude Code" }),
    });
    expect(within(detail).getAllByText("<P>/.mcp.json").length).toBeGreaterThan(0);
    expect(within(detail).getByText("store:github+project")).toBeInTheDocument();
    const shadowed = within(detail).getByRole("list", { name: t("matrix.detail.shadowed") });
    expect(shadowed).toHaveTextContent(t("matrix.layer.system"));
    expect(shadowed).toHaveTextContent("~/.claude.json");
    expect(within(detail).getByText("VS Code (Copilot), Grok")).toBeInTheDocument();
    const snippet = within(detail).getByLabelText(t("matrix.detail.snippet"));
    expect(snippet).toHaveTextContent('"command": "github-mcp"');
    expect(snippet).toHaveTextContent("********");

    await user.click(screen.getByRole("button", { name: /github in Grok/ }));
    const grok = screen.getByRole("complementary", {
      name: t("matrix.detail.title", { server: "github", client: "Grok" }),
    });
    const warnings = within(grok).getByRole("list", { name: t("matrix.detail.warnings") });
    expect(warnings).toHaveTextContent(errorMessage("duplicate_via_compat"));
    expect(warnings).toHaveTextContent("from <P>/.mcp.json");
    expect(within(grok).getByText(t("matrix.layer.clientCompat"))).toBeInTheDocument();
  });
});
