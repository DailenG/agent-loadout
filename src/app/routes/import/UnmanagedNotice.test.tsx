import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App, createQueryClient } from "@/app/App";
import { McpPage } from "@/app/routes/mcp/McpPage";
import { t } from "@/i18n/en";
import type { ImportScope, UnmanagedSkills, UnmanagedSummary } from "@/ipc/bindings";
import { createMockTransport, MOCK_SERVER_LIST, MOCK_TOOLS, ok, type MockTransportOptions } from "@/test/mockTransport";
import { renderWithProviders, USER_EVENT_OPTIONS } from "@/test/renderWithProviders";
import { NOTICE_STORAGE_KEY } from "./unmanaged";
import { UnmanagedNotice, UnmanagedNoticeSetting } from "./UnmanagedNotice";

function summary(client: string, names: string[], isNew = true): UnmanagedSummary {
  return {
    groups: [
      {
        client,
        scope: "user",
        projectId: null,
        files: [`~/.${client}/mcp.json`],
        entries: names.map((name) => ({ id: `${client}/${name}`, name, new: isNew })),
      },
    ],
    total: names.length,
    new: isNew ? names.length : 0,
  };
}

const SKILLS: UnmanagedSkills = {
  candidates: [
    {
      name: "pdf-tools",
      conflict: false,
      variants: [
        {
          hash: "a1b2c3",
          folders: [{ client: "claude-code", path: "~/.claude/skills/pdf-tools" }],
          inStore: false,
          description: null,
          bytes: 1200,
        },
      ],
    },
  ],
  problems: [],
};

function count(transport: { calls: string[] }, command: string) {
  return transport.calls.filter((call) => call === command).length;
}

function renderNotice(commands: MockTransportOptions["commands"] = {}) {
  const onReview = vi.fn();
  const view = renderWithProviders(<UnmanagedNotice collapsed={false} onReview={onReview} />, {
    commands: {
      listTools: async () => ok(MOCK_TOOLS),
      unmanagedSummary: async () => ok(summary("claude-code", ["docs", "github"])),
      ...commands,
    },
  });
  return { onReview, ...view };
}

/** Finds the one-line notice and expands it into the message with Review and Dismiss. */
async function openNotice(user: UserEvent) {
  const notice = await screen.findByRole("region", { name: t("import.notice.aria") });
  const toggle = within(notice).getByRole("button", { expanded: false });
  await user.click(toggle);
  expect(toggle).toHaveAttribute("aria-expanded", "true");
  return notice;
}

afterEach(() => {
  vi.useRealTimers();
});

describe("not-managed notice (#9)", () => {
  it("names the client and count from the every-scope summary, and Dismiss acknowledges what it showed", async () => {
    const { user, transport } = renderNotice();
    const notice = await openNotice(user);
    expect(await within(notice).findByText("Claude Code has 2 MCP servers not managed by Loadout.")).toBeInTheDocument();
    const scan = transport.invocations.find((call) => call.command === "unmanagedSummary");
    expect(scan?.args[0]).toEqual({ kind: "all" });
    expect(transport.calls).not.toContain("newUnmanaged");

    await user.click(within(notice).getByRole("button", { name: t("import.notice.dismiss") }));
    expect(screen.queryByRole("region", { name: t("import.notice.aria") })).toBeNull();
    const ack = transport.invocations.find((call) => call.command === "acknowledgeUnmanaged");
    expect(ack?.args[0]).toEqual(["claude-code/docs", "claude-code/github"]);
  });

  it("announces through a live region mounted before there is anything to say", async () => {
    let answer: (value: ReturnType<typeof ok<UnmanagedSummary>>) => void = () => undefined;
    renderNotice({ unmanagedSummary: () => new Promise((resolve) => (answer = resolve)) });
    const live = screen.getByTestId("unmanaged-notice-live");
    expect(live).toHaveAttribute("role", "status");
    expect(live).toBeEmptyDOMElement();
    act(() => answer(ok(summary("claude-code", ["docs"]))));
    await waitFor(() => expect(live).toHaveTextContent("Claude Code has 1 MCP server not managed by Loadout."));
    // The same element, now with text: screen readers announce the change.
    expect(screen.getByTestId("unmanaged-notice-live")).toBe(live);
  });

  it("opens the Import page from Review and acknowledges", async () => {
    const { user, transport, onReview } = renderNotice();
    const notice = await openNotice(user);
    await user.click(within(notice).getByRole("button", { name: t("import.notice.review") }));
    expect(onReview).toHaveBeenCalledOnce();
    expect(transport.calls).toContain("acknowledgeUnmanaged");
  });

  it("clears the New badges of the MCP page's list after Dismiss", async () => {
    let acknowledged = false;
    const transportCommands: MockTransportOptions["commands"] = {
      listTools: async () => ok(MOCK_TOOLS),
      listServers: async () => ok(MOCK_SERVER_LIST),
      unmanagedSummary: async () => ok(summary("claude-code", ["docs"], !acknowledged)),
      acknowledgeUnmanaged: async (ids) => {
        acknowledged = true;
        return ok(ids.length);
      },
    };
    const { user } = renderWithProviders(
      <>
        <UnmanagedNotice collapsed={false} onReview={vi.fn()} />
        <McpPage onOpenPending={vi.fn()} onOpenInstall={vi.fn()} onOpenImport={vi.fn()} />
      </>,
      { commands: transportCommands },
    );
    const section = await screen.findByRole("region", { name: "Not managed by Loadout (1)" });
    expect(within(section).getByText(t("import.unmanaged.new"))).toBeInTheDocument();
    const notice = await openNotice(user);
    await user.click(within(notice).getByRole("button", { name: t("import.notice.dismiss") }));
    await waitFor(() => expect(within(section).queryByText(t("import.unmanaged.new"))).toBeNull());
  });

  it("stays quiet when every finding was acknowledged", async () => {
    const { transport } = renderNotice({ unmanagedSummary: async () => ok(summary("claude-code", ["docs"], false)) });
    await waitFor(() => expect(transport.calls).toContain("unmanagedSummary"));
    await waitFor(() => expect(transport.calls).toContain("scanUnmanagedSkills"));
    expect(screen.queryByRole("region", { name: t("import.notice.aria") })).toBeNull();
  });

  it("counts skill folders and does not show the same folders again", async () => {
    const commands: MockTransportOptions["commands"] = {
      unmanagedSummary: async () => ok({ groups: [], total: 0, new: 0 }),
      scanUnmanagedSkills: async () => ok(SKILLS),
    };
    const first = renderNotice(commands);
    const notice = await openNotice(first.user);
    expect(within(notice).getByText("1 skill folder is not managed by Loadout.")).toBeInTheDocument();
    await first.user.click(within(notice).getByRole("button", { name: t("import.notice.dismiss") }));
    first.unmount();

    // A restart: the folder was seen, so nothing shows.
    const second = renderNotice(commands);
    await waitFor(() => expect(second.transport.calls).toContain("scanUnmanagedSkills"));
    expect(screen.queryByRole("region", { name: t("import.notice.aria") })).toBeNull();
  });

  it("is off when Settings turns it off", async () => {
    window.localStorage.setItem(NOTICE_STORAGE_KEY, "off");
    const { transport } = renderNotice();
    expect(screen.queryByRole("region", { name: t("import.notice.aria") })).toBeNull();
    expect(transport.calls).not.toContain("unmanagedSummary");
    expect(transport.calls).not.toContain("scanUnmanagedSkills");
  });

  it("turns the notice off from Settings", async () => {
    const { user } = renderWithProviders(
      <>
        <UnmanagedNoticeSetting />
        <UnmanagedNotice collapsed={false} onReview={vi.fn()} />
      </>,
      {
        commands: {
          listTools: async () => ok(MOCK_TOOLS),
          unmanagedSummary: async () => ok(summary("claude-code", ["docs"])),
        },
      },
    );
    const notice = await openNotice(user);
    expect(await within(notice).findByText("Claude Code has 1 MCP server not managed by Loadout.")).toBeInTheDocument();
    const toggle = screen.getByRole("checkbox", { name: t("import.notice.setting.label") });
    expect(toggle).toBeChecked();
    await user.click(toggle);
    expect(window.localStorage.getItem(NOTICE_STORAGE_KEY)).toBe("off");
    expect(screen.queryByRole("region", { name: t("import.notice.aria") })).toBeNull();
  });

  it("rechecks on focus and watcher events at most once a minute, and opens Import from the sidebar", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const transport = createMockTransport({
      commands: {
        unmanagedSummary: async (scope: ImportScope | null) =>
          ok(scope?.kind === "all" ? summary("codex", ["docs", "fetch"]) : { groups: [], total: 0, new: 0 }),
      },
    });
    render(<App transport={transport} queryClient={createQueryClient()} />);
    const user = userEvent.setup(USER_EVENT_OPTIONS);
    const notice = await screen.findByRole("region", { name: t("import.notice.aria") });
    await waitFor(() => expect(count(transport, "scanUnmanagedSkills")).toBe(1));
    const plan = count(transport, "pendingPlan");
    expect(count(transport, "unmanagedSummary")).toBe(1);

    // Within the minute: the plan rechecks, the full scans do not.
    act(() => transport.focus());
    act(() => transport.changeTargets());
    await waitFor(() => expect(count(transport, "pendingPlan")).toBe(plan + 2));
    expect(count(transport, "unmanagedSummary")).toBe(1);
    expect(count(transport, "scanUnmanagedSkills")).toBe(1);

    // A minute later: one more of each.
    vi.setSystemTime(Date.now() + 61_000);
    act(() => transport.focus());
    await waitFor(() => expect(count(transport, "unmanagedSummary")).toBe(2));
    await waitFor(() => expect(count(transport, "scanUnmanagedSkills")).toBe(2));
    expect(transport.calls).not.toContain("newUnmanaged");

    await openNotice(user);
    await user.click(within(notice).getByRole("button", { name: t("import.notice.review") }));
    const nav = screen.getByRole("navigation", { name: t("app.nav.primary") });
    expect(within(nav).getByRole("button", { name: "Import" })).toHaveAttribute("aria-current", "page");
  });

  it("shows one line with the count and expands on click into the message and buttons (M36)", async () => {
    const { user } = renderNotice({ scanUnmanagedSkills: async () => ok(SKILLS) });
    const notice = await screen.findByRole("region", { name: t("import.notice.aria") });
    const toggle = await within(notice).findByRole("button", { name: "3 not managed" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    const panel = document.getElementById(toggle.getAttribute("aria-controls") ?? "");
    expect(panel).not.toBeNull();
    expect(panel).not.toBeVisible();
    expect(within(notice).queryByRole("button", { name: t("import.notice.review") })).toBeNull();
    // The live region still says the whole message while the notice is one line.
    expect(screen.getByTestId("unmanaged-notice-live")).toHaveTextContent(
      "Claude Code has 2 MCP servers not managed by Loadout. 1 skill folder is not managed by Loadout.",
    );

    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(panel).toBeVisible();
    expect(within(panel as HTMLElement).getByText(/Claude Code has 2 MCP servers/)).toBeInTheDocument();

    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(panel).not.toBeVisible();
  });

  it("opens from the keyboard, closes on Escape with focus back on the line, and on a press elsewhere", async () => {
    const { user, transport } = renderNotice();
    const notice = await screen.findByRole("region", { name: t("import.notice.aria") });
    const toggle = await within(notice).findByRole("button", { name: "2 not managed" });
    toggle.focus();
    await user.keyboard("{Enter}");
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    await user.tab();
    expect(within(notice).getByRole("button", { name: t("import.notice.review") })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveFocus();

    await user.keyboard(" ");
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    fireEvent.pointerDown(document.body);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    // Opening and closing acknowledges nothing.
    expect(transport.calls).not.toContain("acknowledgeUnmanaged");
  });

  it("shows only the count badge in the collapsed sidebar, which reviews", async () => {
    const onReview = vi.fn();
    const { user, transport } = renderWithProviders(<UnmanagedNotice collapsed onReview={onReview} />, {
      commands: {
        listTools: async () => ok(MOCK_TOOLS),
        unmanagedSummary: async () => ok(summary("claude-code", ["docs", "github"])),
      },
    });
    const badge = await screen.findByRole("button", {
      name: "Claude Code has 2 MCP servers not managed by Loadout. Review",
    });
    expect(badge).toHaveTextContent(/^2$/);
    expect(screen.queryByRole("region", { name: t("import.notice.aria") })).toBeNull();
    expect(screen.queryByText("2 not managed")).toBeNull();
    await user.click(badge);
    expect(onReview).toHaveBeenCalledOnce();
    expect(transport.calls).toContain("acknowledgeUnmanaged");
  });
});
