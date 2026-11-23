import { screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { t } from "@/i18n/en";
import type {
  ImportApplied,
  ImportCandidate,
  ImportChoices,
  ImportSource,
  ImportVariant,
  ProjectRow,
  ProjectServerList,
  ServerDraft,
} from "@/ipc/bindings";
import { MOCK_CHANGE, MOCK_CLIENTS, MOCK_SERVER_LIST, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";
import { McpImportSection } from "./McpImportSection";
import type { ImportTarget } from "./model";

const PROJECT: ProjectRow = {
  id: "web-shop-3f9a",
  name: "web-shop",
  path: "~/Code/web-shop",
  layout: "shared",
  trust: "untrusted",
  error: null,
  servers: 1,
  clients: 1,
  pending: 0,
};

function draft(id: string, fields: Partial<ServerDraft>): ServerDraft {
  return {
    id,
    displayName: null,
    description: null,
    transport: "stdio",
    command: null,
    args: [],
    url: null,
    cwd: null,
    env: [],
    headers: [],
    clients: { allDetected: false, clients: [] },
    delivery: null,
    toolsAllow: [],
    toolsDeny: [],
    ...fields,
  };
}

function source(client: string, name: string, fields: Partial<ImportSource> = {}): ImportSource {
  return {
    key: client,
    id: `${client}/${name}`,
    client,
    scope: "user",
    projectId: null,
    trusted: null,
    file: `~/.${client}/mcp.json`,
    ...fields,
  };
}

function variant(fields: Partial<ImportVariant> & Pick<ImportVariant, "key" | "suggestedId" | "sources">): ImportVariant {
  return { draft: null, error: null, enables: true, secrets: [], ...fields };
}

const GITHUB: ImportCandidate = {
  name: "github",
  inStore: false,
  conflict: false,
  variants: [
    variant({
      key: "claude-code",
      suggestedId: "github",
      draft: draft("github", {
        command: "npx",
        args: ["-y", "@modelcontextprotocol/server-github"],
        env: [{ key: "GITHUB_TOKEN", value: "${secret:GITHUB_GITHUB_TOKEN}" }],
      }),
      secrets: [{ field: "env.GITHUB_TOKEN", suggestedAlias: "GITHUB_GITHUB_TOKEN", reason: "sensitive_key" }],
      sources: [source("claude-code", "github"), source("codex", "github")],
    }),
  ],
};

const DOCS: ImportCandidate = {
  name: "docs",
  inStore: false,
  conflict: true,
  variants: [
    variant({
      key: "claude-code",
      suggestedId: "docs",
      draft: draft("docs", { transport: "http", url: "https://docs.example.dev/mcp" }),
      sources: [source("claude-code", "docs")],
    }),
    variant({
      key: "codex",
      suggestedId: "docs-codex",
      draft: draft("docs-codex", { command: "docs-mcp", args: ["--stdio"] }),
      sources: [source("codex", "docs")],
    }),
  ],
};

const LINEAR: ImportCandidate = {
  name: "linear",
  inStore: false,
  conflict: false,
  variants: [
    variant({
      key: `claude-code@${PROJECT.id}`,
      suggestedId: "linear",
      enables: false,
      draft: draft("linear", { transport: "http", url: "https://mcp.linear.app/sse" }),
      sources: [
        source("claude-code", "linear", {
          key: `claude-code@${PROJECT.id}`,
          id: `claude-code@${PROJECT.id}/linear`,
          scope: "project",
          projectId: PROJECT.id,
          trusted: false,
          file: "<P>/.mcp.json",
        }),
      ],
    }),
  ],
};

const FETCH: ImportCandidate = {
  name: "fetch",
  inStore: true,
  conflict: false,
  variants: [
    variant({
      key: "codex",
      suggestedId: "fetch",
      draft: draft("fetch", { command: "uvx", args: ["mcp-server-fetch"] }),
      sources: [source("codex", "fetch")],
    }),
  ],
};

const APPLIED: ImportApplied = {
  imported: ["github"],
  definitionOnly: [],
  skipped: [],
  secrets: [{ alias: "GITHUB_GITHUB_TOKEN", server: "github", field: "env.GITHUB_TOKEN", destination: "keyring" }],
  change: MOCK_CHANGE,
};

const STORE = {
  ...MOCK_SERVER_LIST,
  servers: [
    {
      id: "fetch",
      displayName: null,
      description: null,
      transport: "stdio" as const,
      launch: "uvx mcp-server-fetch",
      origin: null,
      tags: [],
      clients: { allDetected: true, clients: [] },
      pills: [],
      problems: [],
      invalid: null,
    },
  ],
};

function renderSection(
  candidates: ImportCandidate[],
  commands: MockTransportOptions["commands"] = {},
  target: ImportTarget | null = null,
) {
  const onOpenPending = vi.fn();
  const view = renderWithProviders(<McpImportSection target={target} onOpenPending={onOpenPending} />, {
    projects: [PROJECT],
    commands: {
      scanImport: async () => ok({ candidates, warnings: [] }),
      listServers: async () => ok({ ...STORE, clients: MOCK_CLIENTS }),
      applyImport: async () => ok(APPLIED),
      ...commands,
    },
  });
  return { onOpenPending, ...view };
}

function sentChoices(transport: { invocations: { command: string; args: unknown[] }[] }): ImportChoices {
  const call = transport.invocations.find((entry) => entry.command === "applyImport");
  return call?.args[0] as ImportChoices;
}

async function enter(user: UserEvent, field: HTMLElement, value: string) {
  await user.clear(field);
  if (value) {
    await user.paste(value);
  }
}

async function review(user: UserEvent) {
  await user.click(screen.getByRole("button", { name: t("import.review.open") }));
  return screen.findByRole("dialog", { name: t("import.review.title") });
}

describe("MCP import", () => {
  it("collapses identical entries into one row and imports them with one apply", async () => {
    const { user, transport } = renderSection([GITHUB]);
    const card = await screen.findByRole("article", { name: "github" });
    expect(within(card).getByText("Identical in 2 files")).toBeInTheDocument();
    const sources = within(card).getByRole("list", { name: t("import.variant.sourcesAria") });
    expect(within(sources).getAllByRole("listitem")).toHaveLength(2);
    expect(within(card).getByRole("checkbox", { name: "Import github" })).toBeChecked();

    const dialog = await review(user);
    expect(within(dialog).getByText("Enabled for Claude Code, Codex")).toBeInTheDocument();
    expect(within(dialog).getByText("env.GITHUB_TOKEN goes to the OS keyring")).toBeInTheDocument();
    expect(transport.calls).not.toContain("applyImport");
    await user.click(within(dialog).getByRole("button", { name: "Import (1)" }));

    const result = await screen.findByRole("dialog", { name: t("import.result.title") });
    // Focus lands on the result's title, and the persistent live region announces it.
    expect(within(result).getByRole("heading", { name: t("import.result.title") })).toHaveFocus();
    expect(within(result).getByRole("status")).toHaveTextContent("Imported: 1. Definitions only: 0. Skipped: 0.");
    expect(within(result).getByText(/GITHUB_GITHUB_TOKEN \(github env\.GITHUB_TOKEN\) is in the OS keyring/)).toBeInTheDocument();
    expect(transport.calls.filter((call) => call === "applyImport")).toHaveLength(1);
    // A keyring choice carries no value: core reads it from the client file.
    expect(sentChoices(transport)).toEqual({
      picks: [{ name: "github", source: "claude-code", id: "github" }],
      secrets: [{ name: "github", field: "env.GITHUB_TOKEN", target: { kind: "keyring" } }],
      only: false,
    });
  });

  it("offers Pending changes after an import in review mode", async () => {
    const { user, onOpenPending } = renderSection([GITHUB], {
      applyImport: async () => ok({ ...APPLIED, change: { commit: MOCK_CHANGE.commit, apply: null } }),
    });
    await screen.findByRole("article", { name: "github" });
    const dialog = await review(user);
    await user.click(within(dialog).getByRole("button", { name: "Import (1)" }));
    const result = await screen.findByRole("dialog", { name: t("import.result.title") });
    expect(within(result).getByText(t("import.result.reviewMode"))).toBeInTheDocument();
    await user.click(within(result).getByRole("button", { name: t("import.result.openPending") }));
    expect(onOpenPending).toHaveBeenCalledOnce();
  });

  it("waits for a choice on a conflict, then imports both under checked ids", async () => {
    const { user, transport } = renderSection([DOCS]);
    const card = await screen.findByRole("article", { name: "docs" });
    expect(within(card).getByText("2 different definitions")).toBeInTheDocument();
    const choice = within(card).getByRole("group", { name: "How to import docs" });
    expect(within(choice).getByRole("radio", { name: "Skip for now" })).toBeChecked();
    expect(screen.getByRole("button", { name: t("import.review.open") })).toBeDisabled();

    await user.click(within(choice).getByRole("radio", { name: t("import.choice.both") }));
    const first = within(card).getByRole("textbox", { name: "Server ID for Claude Code" });
    const second = within(card).getByRole("textbox", { name: "Server ID for Codex" });
    expect(first).toHaveValue("docs");
    expect(second).toHaveValue("docs-codex");

    await enter(user, second, "fetch");
    expect(within(card).getByText(t("import.id.taken"))).toBeInTheDocument();
    expect(second).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("button", { name: t("import.review.open") })).toBeDisabled();
    await enter(user, second, "docs");
    expect(within(card).getAllByText(t("import.id.duplicate"))).toHaveLength(2);
    await enter(user, second, "Docs Local");
    expect(within(card).getByText(t("import.id.invalid"))).toBeInTheDocument();
    await enter(user, second, "docs-local");

    const dialog = await review(user);
    await user.click(within(dialog).getByRole("button", { name: "Import (2)" }));
    await screen.findByRole("dialog", { name: t("import.result.title") });
    expect(sentChoices(transport).picks).toEqual([
      { name: "docs", source: "claude-code", id: "docs" },
      { name: "docs", source: "codex", id: "docs-local" },
    ]);
  });

  it("picks one definition of a conflict under the plain name", async () => {
    const { user, transport } = renderSection([DOCS]);
    const card = await screen.findByRole("article", { name: "docs" });
    await user.click(within(card).getByRole("radio", { name: "Use the definition from Codex" }));
    expect(within(card).getByRole("textbox", { name: t("import.id.label") })).toHaveValue("docs");
    const dialog = await review(user);
    await user.click(within(dialog).getByRole("button", { name: "Import (1)" }));
    await screen.findByRole("dialog", { name: t("import.result.title") });
    expect(sentChoices(transport).picks).toEqual([{ name: "docs", source: "codex", id: "docs" }]);
  });

  it("routes a secret to 1Password or a variable, checked first, or skips the server", async () => {
    const { user, transport } = renderSection([GITHUB, DOCS]);
    const card = await screen.findByRole("article", { name: "github" });
    const destination = within(card).getByRole("combobox", { name: "Destination of github env.GITHUB_TOKEN" });

    await user.click(destination);
    await user.click(await screen.findByRole("option", { name: t("import.secret.onepassword") }));
    const reference = within(card).getByLabelText("1Password reference for github env.GITHUB_TOKEN");
    expect(within(card).getByText(t("import.secret.missing"))).toBeInTheDocument();
    await enter(user, reference, "op://Private");
    expect(within(card).getByText(t("import.secret.badOp"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("import.review.open") })).toBeDisabled();
    await enter(user, reference, "op://Private/GitHub/token");
    expect(within(card).queryByText(t("import.secret.badOp"))).toBeNull();

    let dialog = await review(user);
    expect(within(dialog).getByText("env.GITHUB_TOKEN goes to 1Password (op://Private/GitHub/token)")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("import.review.cancel") }));

    await user.click(destination);
    await user.click(await screen.findByRole("option", { name: t("import.secret.env") }));
    const variable = within(card).getByLabelText("Environment variable for github env.GITHUB_TOKEN");
    await enter(user, variable, "1TOKEN");
    expect(within(card).getByText(t("import.secret.badEnv"))).toBeInTheDocument();
    await enter(user, variable, "GITHUB_TOKEN");

    dialog = await review(user);
    await user.click(within(dialog).getByRole("button", { name: "Import (1)" }));
    await screen.findByRole("dialog", { name: t("import.result.title") });
    expect(sentChoices(transport).secrets).toEqual([
      { name: "github", field: "env.GITHUB_TOKEN", target: { kind: "env", name: "GITHUB_TOKEN" } },
    ]);
  });

  it("leaves a server out when its secret is set to Skip this server", async () => {
    const { user } = renderSection([GITHUB]);
    const card = await screen.findByRole("article", { name: "github" });
    await user.click(within(card).getByRole("combobox", { name: "Destination of github env.GITHUB_TOKEN" }));
    await user.click(await screen.findByRole("option", { name: t("import.secret.skip") }));
    expect(screen.getByRole("status")).toHaveTextContent("Selected for import: 0");
    expect(screen.getByRole("button", { name: t("import.review.open") })).toBeDisabled();
  });

  it("marks a name the store has and a project-only definition", async () => {
    renderSection([FETCH, LINEAR]);
    const fetch = await screen.findByRole("article", { name: "fetch" });
    expect(within(fetch).getByText(t("import.candidate.inStore"))).toBeInTheDocument();
    expect(within(fetch).getByRole("checkbox", { name: "Import fetch" })).not.toBeChecked();

    const linear = screen.getByRole("article", { name: "linear" });
    expect(within(linear).getByText(t("import.variant.definitionOnly"))).toBeInTheDocument();
    expect(within(linear).getByText(t("import.source.untrusted"))).toBeInTheDocument();
    // The project file is named under the project's folder.
    expect(within(linear).getByText("~/Code/web-shop/.mcp.json")).toBeInTheDocument();
  });

  it("filters by where entries were found", async () => {
    const { user } = renderSection([GITHUB, LINEAR]);
    await screen.findByRole("article", { name: "github" });
    await user.click(screen.getByRole("button", { name: "Projects (1)" }));
    expect(screen.queryByRole("article", { name: "github" })).toBeNull();
    expect(screen.getByRole("article", { name: "linear" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "User files (1)" }));
    expect(screen.getByRole("article", { name: "github" })).toBeInTheDocument();
  });

  async function importLinear(layer: "shared" | "local" | null) {
    const projectServers = (trust: ProjectRow["trust"]): ProjectServerList => ({
      project: {
        id: PROJECT.id,
        name: PROJECT.name,
        path: PROJECT.path,
        trust,
        hash: "hash-1",
        layers: ["shared", "local"],
        defaultClients: { allDetected: true, clients: [] },
      },
      servers: [],
      inherited: [],
      clients: MOCK_CLIENTS,
      storeServers: ["linear"],
      warnings: [],
    });
    let trusted = false;
    const view = renderSection([LINEAR], {
      applyImport: async () => ok({ ...APPLIED, imported: [], definitionOnly: ["linear"], secrets: [] }),
      listProjectServers: async () => ok(projectServers(trusted ? "trusted" : "untrusted")),
      reviewProject: async () =>
        ok({ id: PROJECT.id, name: PROJECT.name, path: PROJECT.path, trust: "untrusted", hash: "hash-1", files: [], warnings: [] }),
      trustProject: async (id) => {
        trusted = true;
        return ok({ id, trust: "trusted", ...MOCK_CHANGE });
      },
    });
    const { user, transport } = view;
    await screen.findByRole("article", { name: "linear" });
    const dialog = await review(user);
    expect(within(dialog).getByText(t("import.review.definitionOnly"))).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Import (1)" }));

    const result = await screen.findByRole("dialog", { name: t("import.result.title") });
    // A Claude Code entry in the shared .mcp.json: shared (default) or private.
    const layers = within(result).getByRole("group", { name: "Which manifest of web-shop" });
    expect(within(layers).getByRole("radio", { name: t("import.manifest.shared") })).toBeChecked();
    if (layer === "local") {
      await user.click(within(layers).getByRole("radio", { name: t("import.manifest.private") }));
    }
    await user.click(within(result).getByRole("button", { name: "Add linear to the manifest of web-shop" }));
    const trustDialog = await screen.findByRole("dialog", { name: /web-shop/ });
    expect(transport.calls).not.toContain("saveProjectServer");
    await user.click(within(trustDialog).getByRole("button", { name: t("projects.review.trust") }));

    await waitFor(() => expect(transport.calls).toContain("saveProjectServer"));
    const save = transport.invocations.find((call) => call.command === "saveProjectServer");
    expect(save?.args.slice(0, 2)).toEqual([PROJECT.id, "hash-1"]);
    expect(save?.args[3]).toBe(true);
    expect(await within(result).findByRole("button", { name: "Add linear to the manifest of web-shop" })).toHaveTextContent(
      t("import.manifest.done"),
    );
    return save?.args[2];
  }

  it("adds a shared-file definition to the shared manifest by default, after a trust review, for its client only", async () => {
    expect(await importLinear(null)).toEqual({
      name: "linear",
      layer: "shared",
      kind: "override",
      from: "linear",
      transport: null,
      command: null,
      args: [],
      url: null,
      env: [],
      headers: [],
      clients: { allDetected: false, clients: ["claude-code"] },
    });
  });

  it("adds a private entry next to the shared one when Private is chosen", async () => {
    expect(await importLinear("local")).toMatchObject({
      name: "linear",
      layer: "local",
      from: "linear",
      clients: { allDetected: false, clients: ["claude-code"] },
    });
  });

  it("suggests a lowercase id for an uppercase name", async () => {
    const upper: ImportCandidate = {
      ...GITHUB,
      name: "GitHub",
      variants: [{ ...GITHUB.variants[0], suggestedId: "github" }],
    };
    const { user, transport } = renderSection([upper]);
    const card = await screen.findByRole("article", { name: "GitHub" });
    expect(within(card).getByRole("textbox", { name: t("import.id.label") })).toHaveValue("github");
    expect(within(card).queryByText(t("import.id.invalid"))).toBeNull();
    const dialog = await review(user);
    await user.click(within(dialog).getByRole("button", { name: "Import (1)" }));
    await screen.findByRole("dialog", { name: t("import.result.title") });
    expect(sentChoices(transport).picks).toEqual([{ name: "GitHub", source: "claude-code", id: "github" }]);
  });

  it("checks only the entries the filter shows, so a hidden field never blocks", async () => {
    const { user, transport } = renderSection([GITHUB, LINEAR]);
    const github = await screen.findByRole("article", { name: "github" });
    await user.click(within(github).getByRole("combobox", { name: "Destination of github env.GITHUB_TOKEN" }));
    await user.click(await screen.findByRole("option", { name: t("import.secret.onepassword") }));
    expect(screen.getByRole("button", { name: t("import.review.open") })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Projects (1)" }));
    expect(screen.getByRole("button", { name: t("import.review.open") })).toBeEnabled();
    const dialog = await review(user);
    await user.click(within(dialog).getByRole("button", { name: "Import (1)" }));
    await screen.findByRole("dialog", { name: t("import.result.title") });
    expect(sentChoices(transport).picks).toEqual([{ name: "linear", source: LINEAR.variants[0].key, id: "linear" }]);
  });

  it("accepts 1Password Environment references only as core parses them", async () => {
    const { user } = renderSection([GITHUB]);
    const card = await screen.findByRole("article", { name: "github" });
    await user.click(within(card).getByRole("combobox", { name: "Destination of github env.GITHUB_TOKEN" }));
    await user.click(await screen.findByRole("option", { name: t("import.secret.onepassword") }));
    const reference = within(card).getByLabelText("1Password reference for github env.GITHUB_TOKEN");
    for (const bad of ["op-env://Env-1/GITHUB_TOKEN", "op-env://env_1/GITHUB_TOKEN", "op-env://abc123/1TOKEN"]) {
      await enter(user, reference, bad);
      expect(within(card).getByText(t("import.secret.badOp")), bad).toBeInTheDocument();
    }
    await enter(user, reference, "op-env://abc123/GITHUB_TOKEN");
    expect(within(card).queryByText(t("import.secret.badOp"))).toBeNull();
  });

  it("allows only the keyring or skipping for a field both imported definitions hold", async () => {
    const token = { field: "env.TOKEN", suggestedAlias: "DOCS_TOKEN", reason: "sensitive_key" };
    const docs: ImportCandidate = {
      ...DOCS,
      variants: DOCS.variants.map((item, index) => ({
        ...item,
        secrets: [{ ...token, suggestedAlias: index === 0 ? "DOCS_TOKEN" : "DOCS_CODEX_TOKEN" }],
      })),
    };
    const { user, transport } = renderSection([docs]);
    const card = await screen.findByRole("article", { name: "docs" });
    // One definition: every destination; a variable chosen.
    await user.click(within(card).getByRole("radio", { name: "Use the definition from Codex" }));
    await user.click(within(card).getByRole("combobox", { name: "Destination of docs env.TOKEN" }));
    await user.click(await screen.findByRole("option", { name: t("import.secret.env") }));
    await enter(user, within(card).getByLabelText("Environment variable for docs env.TOKEN"), "DOCS_TOKEN");

    // Both: the field falls back to the keyring, with the reason, and only keyring or skip is offered.
    await user.click(within(card).getByRole("radio", { name: t("import.choice.both") }));
    const destination = within(card).getByRole("combobox", { name: "Destination of docs env.TOKEN" });
    expect(destination).toHaveTextContent(t("import.secret.keyring"));
    expect(within(card).getByText(t("import.secret.sharedNote"))).toBeInTheDocument();
    await user.click(destination);
    const options = (await screen.findAllByRole("option")).map((option) => option.textContent?.replace("✓", ""));
    expect(options).toEqual([t("import.secret.keyring"), t("import.secret.skip")]);
    await user.keyboard("{Escape}");

    const dialog = await review(user);
    await user.click(within(dialog).getByRole("button", { name: "Import (2)" }));
    await screen.findByRole("dialog", { name: t("import.result.title") });
    expect(sentChoices(transport).secrets).toEqual([{ name: "docs", field: "env.TOKEN", target: { kind: "keyring" } }]);
  });

  it("starts with only the pointed-at entry picked, in the clicked file's definition", async () => {
    const { user, transport } = renderSection([GITHUB, DOCS], {}, { name: "docs", findingId: "codex/docs" });
    const docs = await screen.findByRole("article", { name: "docs" });
    await waitFor(() => expect(docs).toHaveFocus());
    expect(within(docs).getByRole("radio", { name: "Use the definition from Codex" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Import github" })).not.toBeChecked();
    const dialog = await review(user);
    await user.click(within(dialog).getByRole("button", { name: "Import (1)" }));
    await screen.findByRole("dialog", { name: t("import.result.title") });
    expect(sentChoices(transport).picks).toEqual([{ name: "docs", source: "codex", id: "docs" }]);
  });
});
