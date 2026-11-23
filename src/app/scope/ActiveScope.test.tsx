import { screen, waitFor } from "@testing-library/react";
import { useQueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { useActiveScope } from "@/app/scope/ActiveScope";
import type { ProjectRow } from "@/ipc/bindings";
import { ok } from "@/test/mockTransport";
import { renderWithProviders } from "@/test/renderWithProviders";

const APP: ProjectRow = {
  id: "app-a1b2",
  name: "app",
  path: "~/code/app",
  layout: "private",
  trust: "untrusted",
  error: null,
  servers: 0,
  clients: 0,
  pending: 0,
};

function Probe() {
  const { scope, setScope, project } = useActiveScope();
  const queryClient = useQueryClient();
  return (
    <div>
      <output data-testid="scope">{scope.kind === "user" ? "user" : `project:${scope.id}`}</output>
      <output data-testid="project">{project?.name ?? "none"}</output>
      <button type="button" onClick={() => setScope({ kind: "project", id: APP.id })}>
        pick app
      </button>
      <button type="button" onClick={() => setScope({ kind: "user" })}>
        pick user
      </button>
      <button
        type="button"
        onClick={() => void queryClient.invalidateQueries({ queryKey: storeQueryKeys.projects })}
      >
        reload
      </button>
    </div>
  );
}

describe("ActiveScope", () => {
  it("starts at User and doesn't load projects for it", () => {
    const { transport } = renderWithProviders(<Probe />);
    expect(screen.getByTestId("scope")).toHaveTextContent("user");
    expect(screen.getByTestId("project")).toHaveTextContent("none");
    expect(transport.calls).not.toContain("listProjects");
  });

  it("switches to a project, even an untrusted one, and resolves its row", async () => {
    const { user } = renderWithProviders(<Probe />, { projects: [APP] });

    await user.click(screen.getByRole("button", { name: "pick app" }));
    expect(screen.getByTestId("scope")).toHaveTextContent(`project:${APP.id}`);
    await waitFor(() => expect(screen.getByTestId("project")).toHaveTextContent("app"));

    await user.click(screen.getByRole("button", { name: "pick user" }));
    expect(screen.getByTestId("scope")).toHaveTextContent("user");
    expect(screen.getByTestId("project")).toHaveTextContent("none");
  });

  it("falls back to User when the selected project leaves the list", async () => {
    let rows: ProjectRow[] = [APP];
    const { user } = renderWithProviders(<Probe />, {
      scope: { kind: "project", id: APP.id },
      commands: { listProjects: async () => ok(rows) },
    });
    await waitFor(() => expect(screen.getByTestId("project")).toHaveTextContent("app"));

    rows = [];
    await user.click(screen.getByRole("button", { name: "reload" }));
    await waitFor(() => expect(screen.getByTestId("scope")).toHaveTextContent("user"));
    expect(screen.getByTestId("project")).toHaveTextContent("none");

    // Coming back doesn't reselect it: the fallback is a real switch.
    rows = [APP];
    await user.click(screen.getByRole("button", { name: "reload" }));
    expect(screen.getByTestId("scope")).toHaveTextContent("user");
  });
});
