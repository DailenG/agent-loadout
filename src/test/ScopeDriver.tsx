// Stands in for the sidebar switcher in page tests: buttons that set the
// shell's active scope (C1), so a page can be switched while it is shown.

import { useActiveScope } from "@/app/scope/ActiveScope";

export const DRIVE_USER = "Drive scope: user";

export function driveProject(id: string) {
  return `Drive scope: project ${id}`;
}

export function ScopeDriver({ projectIds }: { projectIds: string[] }) {
  const { setScope } = useActiveScope();
  return (
    <div>
      <button type="button" onClick={() => setScope({ kind: "user" })}>
        {DRIVE_USER}
      </button>
      {projectIds.map((id) => (
        <button key={id} type="button" onClick={() => setScope({ kind: "project", id })}>
          {driveProject(id)}
        </button>
      ))}
    </div>
  );
}
