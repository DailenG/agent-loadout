import { EditorView } from "@codemirror/view";
import { screen, waitFor, within } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { errorMessage } from "@/i18n/en";
import type { ClientPreview, ConvertedServer, ServerDraft } from "@/ipc/bindings";
import { MOCK_CLIENTS, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { ServerEditDialog, type ServerEditDialogProps } from "./ServerEditDialog";

// jsdom has no layout; CodeMirror measures text ranges.
beforeAll(() => {
  const emptyRects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= emptyRects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect();
});

const DRAFT: ServerDraft = {
  id: "demo",
  displayName: "Demo",
  description: null,
  transport: "stdio",
  command: "demo-server",
  args: ["--flag"],
  url: null,
  cwd: null,
  env: [],
  headers: [],
  clients: { allDetected: false, clients: ["claude-code"] },
  delivery: null,
  toolsAllow: [],
  toolsDeny: [],
};

const PREVIEWS: ClientPreview[] = [
  {
    client: "claude-code",
    name: "demo",
    format: "json",
    text: '"demo": { "env": { "API_KEY": "********" } }',
    delivery: "inline",
    problem: null,
    warnings: [],
  },
  {
    client: "codex",
    name: "demo",
    format: "toml",
    text: null,
    delivery: null,
    problem: { code: "missing_secret", message: "detail" },
    warnings: [],
  },
];

function renderDialog(
  props: Partial<ServerEditDialogProps> = {},
  commands: MockTransportOptions["commands"] = {},
) {
  const onClose = vi.fn();
  const view = renderWithProviders(
    <ServerEditDialog mode="create" clients={MOCK_CLIENTS} onClose={onClose} {...props} />,
    { commands },
  );
  return { onClose, ...view };
}

function lastArgs(transport: { invocations: { command: string; args: unknown[] }[] }, command: string) {
  return transport.invocations.filter((item) => item.command === command).at(-1)?.args;
}

describe("ServerEditDialog", () => {
  it("switches between the Form, TOML and Preview tabs", async () => {
    const { user, transport } = renderDialog({}, { previewServer: async () => ok(PREVIEWS) });
    expect(screen.getByRole("tablist")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Form" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByLabelText("ID")).toBeInTheDocument();
    // New servers start with no client; the preview needs one.
    await user.click(screen.getByLabelText("Claude Code"));

    await user.click(screen.getByRole("tab", { name: "TOML" }));
    expect(screen.getByRole("tab", { name: "TOML" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText(/exists once the server is saved/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();

    await user.click(screen.getByRole("tab", { name: "Preview" }));
    const claude = await screen.findByRole("region", { name: "Claude Code" });
    expect(within(claude).getByText(/\*\*\*\*\*\*\*\*/)).toBeInTheDocument();
    const codex = screen.getByRole("region", { name: "Codex" });
    expect(within(codex).getByText(errorMessage("missing_secret"))).toBeInTheDocument();
    expect(transport.calls).toContain("previewServer");

    await user.click(screen.getByRole("tab", { name: "Form" }));
    expect(screen.getByLabelText("ID")).toBeInTheDocument();
  });

  it("refetches the preview each time the tab opens", async () => {
    const { user, transport } = renderDialog({}, { previewServer: async () => ok(PREVIEWS) });
    await user.click(screen.getByLabelText("Claude Code"));
    await user.click(screen.getByRole("tab", { name: "Preview" }));
    await screen.findByRole("region", { name: "Codex" });
    await user.click(screen.getByRole("tab", { name: "Form" }));
    await user.click(screen.getByRole("tab", { name: "Preview" }));
    await screen.findByRole("region", { name: "Codex" });
    expect(transport.calls.filter((call) => call === "previewServer")).toHaveLength(2);
  });

  it("saves the edited TOML with saveServerToml", async () => {
    const original = 'display_name = "Demo"\n';
    const edited = 'display_name = "Demo edited"\n';
    const { user, transport, onClose } = renderDialog(
      { mode: "edit", serverId: "demo", initialTab: "toml" },
      { getServer: async () => ok({ draft: DRAFT, invalid: null, toml: original }) },
    );
    await waitFor(() => expect(document.querySelector(".cm-editor")).not.toBeNull());
    const editorRoot = document.querySelector<HTMLElement>(".cm-editor");
    const view = editorRoot ? EditorView.findFromDOM(editorRoot) : null;
    expect(view?.state.doc.toString()).toBe(original);
    view?.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: edited } });

    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(lastArgs(transport, "saveServerToml")?.slice(0, 2)).toEqual(["demo", edited]));
    expect(transport.calls).not.toContain("saveServer");
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("opens a file that does not load on the TOML tab only", async () => {
    const broken = "schema = 1\ntransport = 7\n";
    renderDialog(
      { mode: "edit", serverId: "demo" },
      {
        getServer: async () =>
          ok({ draft: null, invalid: { code: "invalid_value", message: "detail" }, toml: broken }),
      },
    );
    await waitFor(() => expect(document.querySelector(".cm-editor")).not.toBeNull());
    expect(screen.getByRole("alert")).toHaveTextContent(errorMessage("invalid_value"));
    expect(screen.getByRole("tab", { name: "Form" })).toBeDisabled();
    expect(screen.getByRole("tab", { name: "Preview" })).toBeDisabled();
    expect(screen.getByRole("tab", { name: "TOML" })).toHaveAttribute("aria-selected", "true");
  });

  it("fills the form from a pasted snippet and keeps the client selection", async () => {
    const snippet = '{"mcpServers":{"demo":{"command":"npx"}}}';
    const converted: ConvertedServer = {
      name: "demo",
      sourceClient: "claude-code",
      problem: null,
      draft: {
        ...DRAFT,
        displayName: null,
        command: "npx",
        args: ["-y", "demo-server"],
        env: [{ key: "API_KEY", value: "${secret:demo-key}" }],
        clients: { allDetected: true, clients: [] },
      },
    };
    const { user, transport } = renderDialog({ focusPaste: true }, { convertSnippet: async () => ok([converted]) });
    const pasteBox = screen.getByLabelText("Paste a config snippet");
    expect(pasteBox).toHaveFocus();

    // New servers start with no client; pick one before pasting.
    expect(screen.getByLabelText("All detected clients")).not.toBeChecked();
    await user.click(screen.getByLabelText("Claude Code"));

    await user.click(pasteBox);
    await user.paste(snippet);
    await waitFor(() => expect(lastArgs(transport, "convertSnippet")?.[0]).toBe(snippet));

    await waitFor(() => expect(screen.getByLabelText("Command")).toHaveValue("npx"));
    expect(screen.getByText("-y")).toBeInTheDocument();
    expect(screen.getByText("demo-server")).toBeInTheDocument();
    expect(screen.getByLabelText("Name 1")).toHaveValue("API_KEY");
    expect(screen.getByLabelText("Value 1")).toHaveValue("${secret:demo-key}");
    expect(screen.getByRole("status")).toHaveTextContent("Filled the form from demo (Claude Code).");

    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(transport.calls).toContain("saveServer"));
    const [draft, create] = lastArgs(transport, "saveServer") as [ServerDraft, boolean];
    expect(create).toBe(true);
    expect(draft.clients).toEqual({ allDetected: false, clients: ["claude-code"] });
    expect(draft.command).toBe("npx");
  });

  it("shows a needs_secret_extraction result and fills nothing", async () => {
    const converted: ConvertedServer = {
      name: "leaky",
      sourceClient: "cursor",
      draft: null,
      problem: { code: "needs_secret_extraction", message: "detail" },
    };
    const { user } = renderDialog({}, { convertSnippet: async () => ok([converted]) });
    await user.type(screen.getByLabelText("Paste a config snippet"), "leaky snippet");
    await user.click(screen.getByRole("button", { name: "Convert" }));

    expect(await screen.findByText(errorMessage("needs_secret_extraction"))).toBeInTheDocument();
    expect(screen.getByText("From Cursor")).toBeInTheDocument();
    expect(screen.getByLabelText("Command")).toHaveValue("");
    expect(screen.getByLabelText("ID")).toHaveValue("");
    expect(screen.queryByRole("button", { name: "Use leaky" })).toBeNull();
  });

  it("lets the user pick one of several converted servers", async () => {
    const results: ConvertedServer[] = [
      { name: "one", sourceClient: "codex", problem: null, draft: { ...DRAFT, id: "one", command: "one-cmd" } },
      { name: "two", sourceClient: "codex", problem: null, draft: { ...DRAFT, id: "two", command: "two-cmd" } },
    ];
    const { user } = renderDialog({}, { convertSnippet: async () => ok(results) });
    await user.type(screen.getByLabelText("Paste a config snippet"), "two servers");
    await user.click(screen.getByRole("button", { name: "Convert" }));
    expect(screen.getByLabelText("Command")).toHaveValue("");
    await user.click(await screen.findByRole("button", { name: "Use two" }));
    expect(screen.getByLabelText("Command")).toHaveValue("two-cmd");
    expect(screen.getByLabelText("ID")).toHaveValue("two");
  });

  it("saves the form with saveServer and the create flag", async () => {
    const { user, transport, onClose } = renderDialog();
    await user.type(screen.getByLabelText("ID"), "demo");
    await user.type(screen.getByLabelText("Display name"), "Demo");
    await user.type(screen.getByLabelText("Command"), "demo-server");
    await user.type(screen.getByLabelText("Arguments"), "--flag{Enter}");
    await user.click(screen.getByRole("button", { name: "Add variable" }));
    await user.type(screen.getByLabelText("Name 1"), "LOG_LEVEL");
    await user.type(screen.getByLabelText("Value 1"), "debug");

    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(transport.calls).toContain("saveServer"));
    expect(lastArgs(transport, "saveServer")?.slice(0, 2)).toEqual([
      {
        id: "demo",
        displayName: "Demo",
        description: null,
        transport: "stdio",
        command: "demo-server",
        args: ["--flag"],
        url: null,
        cwd: null,
        env: [{ key: "LOG_LEVEL", value: "debug" }],
        headers: [],
        clients: { allDetected: false, clients: [] },
        delivery: null,
        toolsAllow: [],
        toolsDeny: [],
      },
      true,
    ]);
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("inserts an env reference into a value", async () => {
    const { user } = renderDialog();
    await user.click(screen.getByRole("button", { name: "Add variable" }));
    await user.click(screen.getByRole("combobox", { name: "Insert a reference into value 1" }));
    await user.click(screen.getByRole("option", { name: "Environment variable..." }));
    await user.type(screen.getByLabelText("Variable name"), "HOME_DIR");
    await user.click(screen.getByRole("button", { name: "Insert" }));
    expect(screen.getByLabelText("Value 1")).toHaveValue("${env:HOME_DIR}");
  });

  it("shows a save failure inline from its error code", async () => {
    const { user, onClose } = renderDialog({}, {
      saveServer: async () => ({ status: "error", error: { code: "already_exists", message: "detail" } }),
    });
    await user.type(screen.getByLabelText("ID"), "demo");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(errorMessage("already_exists"));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("closes on Escape and on the backdrop", async () => {
    const { user, onClose } = renderDialog();
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
    const backdrop = document.querySelector<HTMLElement>(".dialog-backdrop");
    if (backdrop) {
      await user.click(backdrop);
    }
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});
