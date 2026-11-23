// The active scope of the shell (SPEC 13.1): User or one project, picked in
// the sidebar switcher. Pages read it through `useActiveScope`. It lives in
// the shell only and isn't persisted: a restart opens User.

import { useQuery } from "@tanstack/react-query";
import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import type { ProjectRow, SkillScope } from "@/ipc/bindings";
import { unwrap, useTransport, type Commands } from "@/ipc/transport";

/** Same shape as the IPC `SkillScope`. */
export type ActiveScope = SkillScope;

export const USER_ACTIVE_SCOPE: ActiveScope = { kind: "user" };

type ActiveScopeValue = {
  scope: ActiveScope;
  setScope(next: ActiveScope): void;
  /** The selected project's row, once the project list has it. */
  project: ProjectRow | null;
};

const ActiveScopeContext = createContext<ActiveScopeValue | null>(null);

/** The projects this machine knows (`list_projects`), shared with the Projects page. */
export function projectListQuery(commands: Commands) {
  return {
    queryKey: storeQueryKeys.projects,
    queryFn: async () => unwrap(await commands.listProjects()),
  };
}

export function useProjectList(enabled = true) {
  const { commands } = useTransport();
  return useQuery({ ...projectListQuery(commands), enabled });
}

export function ActiveScopeProvider(props: { initial?: ActiveScope; children: ReactNode }) {
  const [selected, setSelected] = useState<ActiveScope>(props.initial ?? USER_ACTIVE_SCOPE);
  // User needs no list; the sidebar loads it anyway to offer the projects.
  const projects = useProjectList(selected.kind === "project");
  const selectedId = selected.kind === "project" ? selected.id : null;
  const project =
    selectedId === null ? null : (projects.data?.find((row) => row.id === selectedId) ?? null);
  // A project that left the list falls back to User at once, not a render later.
  const gone = selectedId !== null && projects.data !== undefined && project === null;
  const scope = gone ? USER_ACTIVE_SCOPE : selected;

  useEffect(() => {
    if (gone) {
      setSelected(USER_ACTIVE_SCOPE);
    }
  }, [gone]);

  const value = useMemo<ActiveScopeValue>(
    () => ({ scope, setScope: setSelected, project }),
    [scope, project],
  );
  return <ActiveScopeContext.Provider value={value}>{props.children}</ActiveScopeContext.Provider>;
}

export function useActiveScope(): ActiveScopeValue {
  const value = useContext(ActiveScopeContext);
  if (!value) {
    throw new Error("useActiveScope needs an ActiveScopeProvider");
  }
  return value;
}
