// GUI baseline (#10): renders the whole app over the perf dataset and
// records, per interaction, the IPC calls the mock transport saw, the React
// commits, the components that rendered and React's render time. jsdom
// times are only comparable with each other, not with WebView2.
// Run: npx vitest run --config scripts/perf/vitest.config.ts
// Output: a table on stdout and JSON in $PERF_OUT (default <tmp>/loadout-perf).

import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "vitest";
import { App, createQueryClient } from "@/app/App";
import { t } from "@/i18n/en";
import { createMockTransport, type MockTransport } from "@/test/mockTransport";
import { buildPerfDataset } from "@/test/perfData";
import type { RenderLog } from "./renderHook";

const renderLog = (globalThis as unknown as { __perfRenderLog: RenderLog }).__perfRenderLog;

type Sample = {
  name: string;
  wallMs: number;
  renderMs: number;
  commits: number;
  componentsRendered: number;
  ipcCalls: number;
  ipc: Record<string, number>;
  topComponents: [string, number][];
};

const samples: Sample[] = [];

function sleep(ms: number): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  setTimeout(resolve, ms);
  return promise;
}

/** Waits until no IPC call and no commit happened for four 25 ms ticks. */
async function settle(transport: MockTransport) {
  let stable = 0;
  let last = -1;
  for (let i = 0; i < 400 && stable < 4; i++) {
    await act(() => sleep(25));
    const mark = transport.calls.length * 100_000 + renderLog.commits.length;
    stable = mark === last ? stable + 1 : 0;
    last = mark;
  }
}

async function measure(name: string, transport: MockTransport, action: () => Promise<void> | void) {
  const callsBefore = transport.calls.length;
  renderLog.reset();
  const start = performance.now();
  await action();
  await settle(transport);
  // The four quiet ticks are not part of the interaction.
  const wallMs = performance.now() - start - 100;
  const ipc: Record<string, number> = {};
  for (const call of transport.calls.slice(callsBefore)) {
    ipc[call] = (ipc[call] ?? 0) + 1;
  }
  const components = new Map<string, number>();
  for (const commit of renderLog.commits) {
    for (const [component, count] of commit.rendered) {
      components.set(component, (components.get(component) ?? 0) + count);
    }
  }
  const sample: Sample = {
    name,
    wallMs: Math.round(wallMs),
    renderMs: Math.round(renderLog.commits.reduce((sum, c) => sum + c.duration, 0)),
    commits: renderLog.commits.length,
    componentsRendered: [...components.values()].reduce((a, b) => a + b, 0),
    ipcCalls: transport.calls.length - callsBefore,
    ipc,
    topComponents: [...components.entries()].sort((a, b) => b[1] - a[1]).slice(0, 8),
  };
  samples.push(sample);
  return sample;
}

function nav(label: string) {
  const primary = screen.getByRole("navigation", { name: t("app.nav.primary") });
  return within(primary).getByRole("button", { name: new RegExp(`^${label}`) });
}

test("GUI baseline over the perf dataset", async () => {
  const transport = createMockTransport({ appInfo: { version: "0.0.0-perf", platform: "windows" }, ...buildPerfDataset() });
  const user = userEvent.setup({ delay: null });
  // Pages after Skills are lazy routes. Vitest transforms a module on its
  // first import, which takes longer than `settle` waits; a WebView reads the
  // built chunk from disk in milliseconds. Load them up front, so each page
  // switch measures the page, not the transform.
  await Promise.all([
    import("@/app/routes/history/HistoryPage"),
    import("@/app/routes/install/InstallPage"),
    import("@/app/routes/mcp/McpPage"),
    import("@/app/routes/projects/ProjectsPage"),
    import("@/app/routes/tools/ToolsPage"),
  ]);

  await measure("startup (mount to settled)", transport, () => {
    render(<App transport={transport} queryClient={createQueryClient()} />);
  });

  const pages: [string, string][] = [
    ["MCP", t("app.nav.mcp.label")],
    ["Skills", t("app.nav.skills.label")],
    ["Tools", t("app.nav.tools.label")],
    ["History", t("app.nav.history.label")],
    ["Projects", t("app.nav.projects.label")],
  ];
  for (const [name, label] of pages) {
    await measure(`page switch: ${name} (first visit)`, transport, () => user.click(nav(label)));
    await measure(`window focus on ${name}`, transport, () => act(() => transport.focus()));
  }
  for (const [name, label] of pages) {
    await measure(`page switch: ${name} (cached)`, transport, () => user.click(nav(label)));
  }

  await user.click(nav(t("app.nav.mcp.label")));
  await settle(transport);
  await measure("drift event (watcher) on MCP", transport, () => act(() => transport.changeTargets()));

  await measure("MCP expand one card", transport, () =>
    user.click(screen.getAllByRole("button", { name: /^Expand / })[1]),
  );
  const pill = screen.getAllByRole("button", { name: /: enabled$/, pressed: true })[0];
  await measure("MCP single pill toggle", transport, () => user.click(pill));

  const mcpSearch = screen.getByRole("searchbox", { name: t("mcp.toolbar.search") });
  await measure("MCP search typing 'server-12' (9 keys)", transport, () => user.type(mcpSearch, "server-12"));
  await user.clear(mcpSearch);
  await settle(transport);

  const pending = screen.getByRole("button", { name: /pending changes\. Open Pending changes/ });
  await measure("apply drawer open", transport, () => user.click(pending));
  await measure("window focus with apply drawer open", transport, () => act(() => transport.focus()));
  await user.keyboard("{Escape}");
  await settle(transport);

  await user.click(nav(t("app.nav.skills.label")));
  await settle(transport);
  const skillSearch = screen.getByRole("searchbox", { name: t("skills.searchAria") });
  await measure("Skills search typing 'skill-12' (8 keys)", transport, () => user.type(skillSearch, "skill-12"));
  await user.clear(skillSearch);
  await settle(transport);

  await measure("scope switch User -> project-3 on Skills", transport, async () => {
    await user.click(screen.getByRole("combobox", { name: t("scope.switcher.aria") }));
    await user.click(await screen.findByRole("option", { name: /project-3(?!\d)/ }));
  });
  await measure("window focus at project scope on Skills", transport, () => act(() => transport.focus()));

  const outDir = process.env.PERF_OUT ?? join(tmpdir(), "loadout-perf");
  mkdirSync(outDir, { recursive: true });
  writeFileSync(join(outDir, "gui.json"), JSON.stringify(samples, null, 2));
  const rows = samples.map(
    (s) =>
      `| ${s.name} | ${s.wallMs} | ${s.renderMs} | ${s.commits} | ${s.componentsRendered} | ${s.ipcCalls} | ${Object.entries(s.ipc)
        .map(([k, v]) => `${k}${v > 1 ? ` x${v}` : ""}`)
        .join(", ")} |`,
  );
  console.log(
    ["| Interaction | wall ms | render ms | commits | components rendered | IPC calls | IPC detail |", "|---|---|---|---|---|---|---|", ...rows].join("\n"),
  );
  console.log(samples.map((s) => `${s.name}: ${s.topComponents.map(([n, c]) => `${n} ${c}`).join(", ")}`).join("\n"));
  expect(samples.length).toBeGreaterThan(10);
});
